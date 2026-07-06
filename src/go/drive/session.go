package drive

// Session lifecycle for the native Proton Drive client. Handles the
// login flow (username + password [+ TOTP]), keyring unlock, and
// token persistence in a small on-disk file Celeste owns. Ported and
// trimmed from Proton-API-Bridge/common/user.go + keyring.go.
//
// Phase 2 scope: in-memory session + opaque UID handle exposed over
// cgo, plus save/load of the reusable credential blob. Subsequent
// phases attach Drive operations (list, upload, download, trash) to
// the Session type.

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"net"
	"net/http"
	"os"
	"sync"
	"time"

	"celeste/native-go/proton-ext"

	"github.com/ProtonMail/go-proton-api"
	"github.com/ProtonMail/gopenpgp/v2/crypto"
)

// newProtonHTTPTransport returns the http.Transport we hand to the
// upstream proton.Manager via WithTransport. Defaults from
// http.DefaultTransport leave ResponseHeaderTimeout at 0 and rely on
// the kernel's TCP keepalive (~5–12 min) to notice a dead peer, which
// surfaces as a multi-minute Celeste "Listing remote…" hang after
// suspend/resume — TCP is still alive from our side until the OS
// finally returns ENETUNREACH. The HTTP/2 ping pair forces the
// transport to actively probe an idle conn so a dead one is recycled
// before the next listing batch fans out across it.
func newProtonHTTPTransport() *http.Transport {
	t := http.DefaultTransport.(*http.Transport).Clone()
	t.DialContext = (&net.Dialer{
		Timeout:   30 * time.Second,
		KeepAlive: 15 * time.Second,
	}).DialContext
	t.IdleConnTimeout = 30 * time.Second
	t.ResponseHeaderTimeout = 30 * time.Second
	t.HTTP2 = &http.HTTP2Config{
		SendPingTimeout: 15 * time.Second,
		PingTimeout:     15 * time.Second,
	}
	return t
}

// AppVersion sent in the `AppVersion` header for every Proton API
// call. Proton's server validates the platform prefix against a
// whitelist; we follow rclone's proven format and piggyback on the
// `macos-drive` platform slot with our own version + tag so the
// upstream-side rate-limit telemetry still distinguishes Celeste
// from rclone.
const AppVersion = "macos-drive@1.0.0-alpha.1+rclone"

// Errors that can surface through the FFI boundary. Message text is
// the only thing the Rust side sees — keep them short and actionable.
var (
	ErrUsernamePasswordRequired = errors.New("username and password are required")
	ErrMailboxPasswordRequired  = errors.New("this account uses two-password mode; a mailbox password is required")
	ErrTwoFARequired            = errors.New("this account requires a 2FA code")
	ErrFailedToUnlockUserKeys   = errors.New("failed to unlock user keys with the provided password")
	ErrSessionNotFound          = errors.New("no active session for the given UID")
	ErrInvalidSessionFile       = errors.New("session file is missing or malformed")
)

// Session owns everything we need to make authenticated calls to
// Proton Drive. The Manager holds the HTTP client + rate limit state;
// the Client holds the authenticated session. Keyrings are unlocked
// and cached in memory.
type Session struct {
	m *proton.Manager
	c *proton.Client

	// Reusable credential blob. Safe to persist — the saltedKeyPass
	// is the only crypto-sensitive piece and base64 of that was the
	// storage format Bridge used.
	//
	// AccessToken / RefreshToken rotate underneath us: on a 401 the
	// upstream client silently refreshes and Proton hands back a NEW
	// (one-time-use) refresh token. The AuthHandler registered in
	// Login/Resume writes the rotated pair back here and flips
	// `rotated`, so a later SaveIfRotated re-persists them. Guarded by
	// authMu because refreshes fire from arbitrary API-call goroutines
	// (including the concurrent listings in proton-ext) while
	// protonextAuth / AsCredential read the same fields.
	authMu        sync.RWMutex
	rotated       bool
	UID           string
	AccessToken   string
	RefreshToken  string
	SaltedKeyPass string // base64(saltedKeyPass bytes)

	// In-memory unlocked keyrings. Cleared on Close().
	userKR  *crypto.KeyRing
	addrKRs map[string]*crypto.KeyRing
	addrs   map[string]proton.Address

	// Drive state populated by bootstrapDrive (see drive.go). All
	// later Drive operations read these fields; they're nil only
	// between Login()/Resume() and the bootstrap's completion.
	mainShare        *proton.Share
	mainShareKR      *crypto.KeyRing
	defaultAddrKR    *crypto.KeyRing
	rootLink         *proton.Link
	signatureAddress string

	// Caches — link metadata and decrypted keyrings. Populated
	// lazily by getLink / linkKR / ListDirectory. Avoids redundant
	// API round-trips during recursive listings where each folder
	// requires walking the parent keyring chain. Single-threaded per
	// session so no locking needed.
	linkCache map[string]proton.Link
	krCache   map[string]*crypto.KeyRing

	// Set of ghost link IDs we've already warned about this session.
	// Trashed / deleted links that block an upload can't be removed
	// via the folder's delete_multiple endpoint, so the upload fails
	// every sync cycle until the user empties their Proton trash.
	// Dedupe the warning so the log stays readable.
	warnedGhosts map[string]struct{}
}

// RootLinkID returns the main share's root folder ID. Callers use this
// as the starting point for directory listings.
func (s *Session) RootLinkID() string {
	if s.rootLink == nil {
		return ""
	}
	return s.rootLink.LinkID
}

// LoginParams is the input to [Login] / FFI entry point.
type LoginParams struct {
	Username        string `json:"username"`
	Password        string `json:"password"`
	MailboxPassword string `json:"mailbox_password,omitempty"`
	TwoFA           string `json:"two_fa,omitempty"`
}

// Login performs a fresh username+password (+ optional TOTP / mailbox
// password) authentication and decrypts the user / address keyrings.
// Caller may immediately Save() the returned session to persist
// tokens.
func Login(ctx context.Context, p LoginParams) (*Session, error) {
	if p.Username == "" || p.Password == "" {
		return nil, ErrUsernamePasswordRequired
	}
	m := proton.New(
		proton.WithAppVersion(AppVersion),
		proton.WithTransport(newProtonHTTPTransport()),
	)
	c, auth, err := m.NewClientWithLogin(ctx, p.Username, []byte(p.Password))
	if err != nil {
		m.Close()
		return nil, err
	}
	if auth.TwoFA.Enabled&proton.HasTOTP != 0 {
		if p.TwoFA == "" {
			c.Close()
			m.Close()
			return nil, ErrTwoFARequired
		}
		if err := c.Auth2FA(ctx, proton.Auth2FAReq{TwoFactorCode: p.TwoFA}); err != nil {
			c.Close()
			m.Close()
			return nil, err
		}
	}

	var keyPass []byte
	if auth.PasswordMode == proton.TwoPasswordMode {
		if p.MailboxPassword == "" {
			c.Close()
			m.Close()
			return nil, ErrMailboxPasswordRequired
		}
		keyPass = []byte(p.MailboxPassword)
	} else {
		keyPass = []byte(p.Password)
	}

	userKR, addrKRs, addrs, saltedKeyPass, err := unlockAccount(ctx, c, keyPass, nil)
	if err != nil {
		c.Close()
		m.Close()
		return nil, err
	}

	sess := &Session{
		m:             m,
		c:             c,
		UID:           auth.UID,
		AccessToken:   auth.AccessToken,
		RefreshToken:  auth.RefreshToken,
		SaltedKeyPass: base64.StdEncoding.EncodeToString(saltedKeyPass),
		userKR:        userKR,
		addrKRs:       addrKRs,
		addrs:         addrs,
		linkCache:     make(map[string]proton.Link),
		krCache:       make(map[string]*crypto.KeyRing),
		warnedGhosts:  make(map[string]struct{}),
	}
	sess.registerAuthHandler()
	if err := sess.bootstrapDrive(ctx); err != nil {
		sess.Close()
		return nil, err
	}
	return sess, nil
}

// Resume rebuilds a Session from a previously-saved credential blob
// without going through NewClientWithLogin. The saltedKeyPass is
// reused to skip the password/salt dance.
func Resume(ctx context.Context, cred ReusableCredential) (*Session, error) {
	saltedKeyPass, err := base64.StdEncoding.DecodeString(cred.SaltedKeyPass)
	if err != nil {
		return nil, err
	}
	m := proton.New(
		proton.WithAppVersion(AppVersion),
		proton.WithTransport(newProtonHTTPTransport()),
	)
	c := m.NewClient(cred.UID, cred.AccessToken, cred.RefreshToken)
	sess := &Session{
		m:             m,
		c:             c,
		UID:           cred.UID,
		AccessToken:   cred.AccessToken,
		RefreshToken:  cred.RefreshToken,
		SaltedKeyPass: cred.SaltedKeyPass,
		linkCache:     make(map[string]proton.Link),
		krCache:       make(map[string]*crypto.KeyRing),
		warnedGhosts:  make(map[string]struct{}),
	}
	// Attach the refresh callback before the first API call. On resume
	// the stored access token is frequently already expired, so
	// unlockAccount's GetUser triggers an immediate refresh — and that
	// rotation is exactly the one we must capture. Registering it after
	// unlockAccount would drop the newly-issued refresh token and
	// re-invalidate the persisted blob on the very first resume.
	sess.registerAuthHandler()

	userKR, addrKRs, addrs, _, err := unlockAccount(ctx, c, nil, saltedKeyPass)
	if err != nil {
		sess.Close()
		return nil, err
	}
	sess.userKR = userKR
	sess.addrKRs = addrKRs
	sess.addrs = addrs

	if err := sess.bootstrapDrive(ctx); err != nil {
		sess.Close()
		return nil, err
	}
	return sess, nil
}

// unlockAccount mirrors Bridge's getAccountKRs: fetches user +
// addresses, derives saltedKeyPass from keyPass when needed, and
// unlocks the full keyring set. Exactly one of keyPass /
// saltedKeyPass must be non-nil.
func unlockAccount(
	ctx context.Context,
	c *proton.Client,
	keyPass, saltedKeyPass []byte,
) (*crypto.KeyRing, map[string]*crypto.KeyRing, map[string]proton.Address, []byte, error) {
	user, err := c.GetUser(ctx)
	if err != nil {
		return nil, nil, nil, nil, err
	}
	addrsArr, err := c.GetAddresses(ctx)
	if err != nil {
		return nil, nil, nil, nil, err
	}
	if saltedKeyPass == nil {
		if keyPass == nil {
			return nil, nil, nil, nil, errors.New("either keyPass or saltedKeyPass must be set")
		}
		salts, err := c.GetSalts(ctx)
		if err != nil {
			return nil, nil, nil, nil, err
		}
		saltedKeyPass, err = salts.SaltForKey(keyPass, user.Keys.Primary().ID)
		if err != nil {
			return nil, nil, nil, nil, err
		}
	}
	userKR, addrKRs, err := proton.Unlock(user, addrsArr, saltedKeyPass, nil)
	if err != nil {
		return nil, nil, nil, nil, err
	}
	if userKR.CountDecryptionEntities() == 0 {
		return nil, nil, nil, nil, ErrFailedToUnlockUserKeys
	}
	addrs := make(map[string]proton.Address, len(addrsArr))
	for _, a := range addrsArr {
		addrs[a.Email] = a
	}
	return userKR, addrKRs, addrs, saltedKeyPass, nil
}

// ReusableCredential is the persisted session blob. JSON-encoded on
// disk. Contains the fields needed to call Resume() later without
// re-prompting the user.
type ReusableCredential struct {
	UID           string `json:"uid"`
	AccessToken   string `json:"access_token"`
	RefreshToken  string `json:"refresh_token"`
	SaltedKeyPass string `json:"salted_key_pass"`
}

// AsCredential returns the persistable subset of the session. Read
// under authMu so a rotation in flight can't tear the token pair.
func (s *Session) AsCredential() ReusableCredential {
	s.authMu.RLock()
	defer s.authMu.RUnlock()
	return ReusableCredential{
		UID:           s.UID,
		AccessToken:   s.AccessToken,
		RefreshToken:  s.RefreshToken,
		SaltedKeyPass: s.SaltedKeyPass,
	}
}

// protonextAuth bundles the credential headers protonext needs to make
// HTTP calls against the Proton API. Read under authMu so a concurrent
// AuthHandler-driven token rotation is picked up cleanly by the next
// request rather than racing the field write.
func (s *Session) protonextAuth() protonext.Auth {
	s.authMu.RLock()
	defer s.authMu.RUnlock()
	return protonext.Auth{
		UID:         s.UID,
		AccessToken: s.AccessToken,
		AppVersion:  AppVersion,
	}
}

// registerAuthHandler wires the upstream client's post-refresh callback
// to mirror rotated tokens back onto the Session and mark it dirty. The
// upstream client rotates its own c.acc/c.ref internally; without this
// the persisted blob keeps the original refresh token, which Proton
// invalidates on first use — the "must re-enter 2FA every day" bug.
// Called once from Login and Resume before any Drive API traffic.
func (s *Session) registerAuthHandler() {
	s.c.AddAuthHandler(func(auth proton.Auth) {
		s.authMu.Lock()
		defer s.authMu.Unlock()
		s.AccessToken = auth.AccessToken
		s.RefreshToken = auth.RefreshToken
		s.rotated = true
	})
}

// Save writes the session's reusable credential to `path`. Permissions
// are 0600 — only Celeste's user should be able to read the file.
func (s *Session) Save(path string) error {
	data, err := json.MarshalIndent(s.AsCredential(), "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(path, data, 0600)
}

// SaveIfRotated writes the credential to `path` only when the tokens
// have rotated since the last save, clearing the dirty flag on a
// successful write. Returns whether a write happened so the caller can
// skip re-storing an unchanged blob in the OS keyring every sync pass.
// The flag is cleared only after the file is written; if the caller's
// downstream persistence fails the in-memory tokens still carry the
// running session, and the next rotation re-arms the flag.
func (s *Session) SaveIfRotated(path string) (bool, error) {
	s.authMu.Lock()
	if !s.rotated {
		s.authMu.Unlock()
		return false, nil
	}
	cred := ReusableCredential{
		UID:           s.UID,
		AccessToken:   s.AccessToken,
		RefreshToken:  s.RefreshToken,
		SaltedKeyPass: s.SaltedKeyPass,
	}
	s.authMu.Unlock()

	data, err := json.MarshalIndent(cred, "", "  ")
	if err != nil {
		return false, err
	}
	if err := os.WriteFile(path, data, 0600); err != nil {
		return false, err
	}

	// Clear the flag only if no newer rotation slipped in while we were
	// writing — otherwise the just-landed (newer) token pair would be
	// dropped until the next rotation. Leaving `rotated` set makes the
	// next SaveIfRotated re-persist the current pair.
	s.authMu.Lock()
	if s.AccessToken == cred.AccessToken && s.RefreshToken == cred.RefreshToken {
		s.rotated = false
	}
	s.authMu.Unlock()
	return true, nil
}

// LoadCredential reads a credential blob from disk. Caller passes it
// to Resume() to rehydrate a session.
func LoadCredential(path string) (ReusableCredential, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return ReusableCredential{}, err
	}
	var cred ReusableCredential
	if err := json.Unmarshal(data, &cred); err != nil {
		return ReusableCredential{}, ErrInvalidSessionFile
	}
	if cred.UID == "" || cred.AccessToken == "" || cred.RefreshToken == "" || cred.SaltedKeyPass == "" {
		return ReusableCredential{}, ErrInvalidSessionFile
	}
	return cred, nil
}

// Close tears down the session's clients and wipes unlocked keyrings
// from memory. Safe to call multiple times.
func (s *Session) Close() {
	if s.userKR != nil {
		s.userKR.ClearPrivateParams()
		s.userKR = nil
	}
	for _, kr := range s.addrKRs {
		kr.ClearPrivateParams()
	}
	s.addrKRs = nil
	s.addrs = nil
	if s.mainShareKR != nil {
		s.mainShareKR.ClearPrivateParams()
		s.mainShareKR = nil
	}
	s.defaultAddrKR = nil
	s.mainShare = nil
	s.rootLink = nil
	s.linkCache = nil
	s.krCache = nil
	if s.c != nil {
		s.c.Close()
		s.c = nil
	}
	if s.m != nil {
		s.m.Close()
		s.m = nil
	}
}

// Logout revokes the session on Proton's side (invalidating the
// refresh token) and then Close()s local state.
func (s *Session) Logout(ctx context.Context) error {
	if s.c == nil {
		return nil
	}
	err := s.c.AuthDelete(ctx)
	s.Close()
	return err
}

// -----------------------------------------------------------------
// Session registry — keyed by UID so cgo callers can hand a short
// string handle across the FFI boundary instead of a raw pointer.

var (
	registryMu sync.RWMutex
	registry   = make(map[string]*Session)
)

// Register stores a session and returns its UID.
func Register(s *Session) string {
	registryMu.Lock()
	defer registryMu.Unlock()
	registry[s.UID] = s
	return s.UID
}

// Lookup retrieves a session by UID. Returns (nil, ErrSessionNotFound)
// if the session isn't registered.
func Lookup(uid string) (*Session, error) {
	registryMu.RLock()
	defer registryMu.RUnlock()
	s, ok := registry[uid]
	if !ok {
		return nil, ErrSessionNotFound
	}
	return s, nil
}

// Unregister removes a session from the registry. Does NOT call
// Close() — caller is responsible for cleanup.
func Unregister(uid string) {
	registryMu.Lock()
	defer registryMu.Unlock()
	delete(registry, uid)
}
