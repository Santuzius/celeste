package drive

// Drive bootstrap: finds the user's active volume + primary share,
// decrypts the main share keyring, fetches the root link. Attached
// to Session at Login/Resume time so every later Drive call has the
// state it needs without re-querying Proton.
//
// Ported and trimmed from Proton-API-Bridge/drive.go + shares.go +
// volumes.go. Link metadata and keyring resolution results are cached
// on the Session (linkCache / krCache) to avoid redundant API
// round-trips during recursive listings.

import (
	"context"
	"errors"

	"github.com/ProtonMail/go-proton-api"
	"github.com/ProtonMail/gopenpgp/v2/crypto"
)

// Errors surfaced during Drive bootstrap.
var (
	ErrNoActiveVolume             = errors.New("account has no active Proton Drive volume")
	ErrMainSharePreconditionFail  = errors.New("main share integrity check failed (unexpected share layout)")
	ErrAddressKeyringMissing      = errors.New("address keyring for main share not present")
)

// bootstrapDrive populates the Drive-state fields on a Session: main
// share, root link, and the decrypted main-share keyring. Called
// once from Login() and Resume(); failure here tears the session
// back down.
func (s *Session) bootstrapDrive(ctx context.Context) error {
	volumes, err := s.c.ListVolumes(ctx)
	if err != nil {
		return err
	}
	var mainShareID, volumeID string
	for i := range volumes {
		if volumes[i].State == proton.VolumeStateActive {
			mainShareID = volumes[i].Share.ShareID
			volumeID = volumes[i].VolumeID
			break
		}
	}
	if mainShareID == "" {
		return ErrNoActiveVolume
	}

	mainShare, err := s.c.GetShare(ctx, mainShareID)
	if err != nil {
		return err
	}

	// Mirror Bridge's integrity check: the main share must be the
	// primary share of type "main" for our assumptions to hold.
	shares, err := s.c.ListShares(ctx, true)
	if err != nil {
		return err
	}
	mainShareOK := false
	for _, m := range shares {
		if m.ShareID == mainShare.ShareID &&
			m.LinkID == mainShare.LinkID &&
			m.Flags == proton.PrimaryShare &&
			m.Type == proton.ShareTypeMain {
			mainShareOK = true
			break
		}
	}
	if !mainShareOK {
		return ErrMainSharePreconditionFail
	}

	addrKR, ok := s.addrKRs[mainShare.AddressID]
	if !ok {
		return ErrAddressKeyringMissing
	}
	mainShareKR, err := mainShare.GetKeyRing(addrKR)
	if err != nil {
		return err
	}

	rootLink, err := s.c.GetLink(ctx, mainShare.ShareID, mainShare.LinkID)
	if err != nil {
		return err
	}

	s.mainShare = &mainShare
	s.mainShareKR = mainShareKR
	s.defaultAddrKR = addrKR
	s.rootLink = &rootLink
	s.signatureAddress = mainShare.Creator
	s.volumeID = volumeID
	// Seed the caches with the root link so the first listing
	// doesn't need to re-fetch it.
	s.linkCache[rootLink.LinkID] = rootLink
	return nil
}

// linkKR returns `link`'s own unlocked node keyring, caching the
// result. For the root link the parent KR is the main share KR;
// otherwise we walk the parent chain (also cached).
func (s *Session) linkKR(ctx context.Context, link *proton.Link) (*crypto.KeyRing, error) {
	if cached, ok := s.krCache[link.LinkID]; ok {
		return cached, nil
	}
	var parentKR *crypto.KeyRing
	if link.ParentLinkID == "" {
		parentKR = s.mainShareKR
	} else {
		pkr, err := s.linkKRByID(ctx, link.ParentLinkID)
		if err != nil {
			return nil, err
		}
		parentKR = pkr
	}
	kr, err := link.GetKeyRing(parentKR, s.defaultAddrKR)
	if err != nil {
		return nil, err
	}
	s.krCache[link.LinkID] = kr
	return kr, nil
}

// linkKRByID is linkKR's "I only have the ID" variant. Checks the
// keyring cache first; on miss fetches the link (also cached) and
// delegates to linkKR.
func (s *Session) linkKRByID(ctx context.Context, linkID string) (*crypto.KeyRing, error) {
	if linkID == "" {
		return s.mainShareKR, nil
	}
	if cached, ok := s.krCache[linkID]; ok {
		return cached, nil
	}
	link, err := s.getLink(ctx, linkID)
	if err != nil {
		return nil, err
	}
	return s.linkKR(ctx, &link)
}

// getLink returns link metadata by ID, using the session cache when
// available. Every link fetched from the API is cached so subsequent
// keyring-chain walks don't re-fetch the same link.
func (s *Session) getLink(ctx context.Context, linkID string) (proton.Link, error) {
	if cached, ok := s.linkCache[linkID]; ok {
		return cached, nil
	}
	link, err := s.c.GetLink(ctx, s.mainShare.ShareID, linkID)
	if err != nil {
		return link, err
	}
	s.linkCache[linkID] = link
	return link, nil
}
