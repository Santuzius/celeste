package drive

// Create: mkdir + file uploads.
//
// Ported and trimmed from Proton-API-Bridge:
//   - folder.go::CreateNewFolder → CreateFolder
//   - file_upload.go::createFileUploadDraft + uploadAndCollectBlockData
//     + commitNewRevision → UploadFile
//
// CreateFolder handles the "already exists" conflict (Proton error
// code 2500, HTTP 422) by looking up the existing folder and returning
// its link ID, making the operation idempotent.
//
// UploadFile / createFileDraft now also handles the same 422 conflict
// for files: when a file with the same name already exists, the code
// locates the existing link, cleans up any stale draft revision,
// creates a new revision on the existing file, and reuses the
// existing file's node keyring + session key for block encryption.

import (
	"context"
	"crypto/sha1"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"log"
	"mime"
	"os"
	"path/filepath"
	"sort"
	"time"

	"celeste/native-go/proton-ext"

	"github.com/ProtonMail/go-proton-api"
	"github.com/ProtonMail/gopenpgp/v2/crypto"
	"github.com/go-resty/resty/v2"
)

// Block / batch sizes lifted from Bridge. 4 MB blocks match Proton's
// on-disk chunking; 8 blocks per batch matches their concurrency
// sweet spot.
const (
	uploadBlockSize      = 4 * 1024 * 1024
	uploadBatchBlockSize = 8
)

// codeAlreadyExists is Proton's server-side error for "a file or
// folder with that name already exists" (HTTP 422).
const codeAlreadyExists = proton.Code(2500)

// ErrParentNotFolder surfaces when a mkdir / upload target parent
// turns out to be a file. The sync engine enforces this pre-flight
// anyway; this is belt-and-braces.
var ErrParentNotFolder = errors.New("parent link is not a folder")

// CreateFolder creates a new subfolder named `name` under
// `parentLinkID`. Empty parent = session root. Returns the new
// folder's link ID. Idempotent: if a folder with the same name
// already exists, its link ID is returned without hitting the
// create endpoint (avoids noisy 422 / Code=2500 from the server).
func (s *Session) CreateFolder(ctx context.Context, parentLinkID, name string) (string, error) {
	if parentLinkID == "" {
		parentLinkID = s.RootLinkID()
	}

	// Fast path: if a folder with this name already exists under the
	// parent, return its link ID immediately. This avoids the 422
	// round-trip (and resty's WARN/ERROR log noise) plus the wasted
	// crypto work of generating throwaway node keys.
	if existing, err := s.findChildByName(ctx, parentLinkID, name, true); err == nil && existing != "" {
		return existing, nil
	}
	defer s.invalidateListings()

	parentLink, err := s.getLink(ctx, parentLinkID)
	if err != nil {
		return "", err
	}
	if parentLink.Type != proton.LinkTypeFolder {
		return "", ErrParentNotFolder
	}
	parentNodeKR, err := s.linkKR(ctx, &parentLink)
	if err != nil {
		return "", err
	}
	parentHashKey, err := parentLink.GetHashKey(parentNodeKR)
	if err != nil {
		return "", err
	}

	nodeKey, nodePassphraseEnc, nodePassphraseSig, err := generateNodeKeys(parentNodeKR, s.defaultAddrKR)
	if err != nil {
		return "", err
	}

	req := proton.CreateFolderReq{
		ParentLinkID: parentLink.LinkID,

		NodeKey:                 nodeKey,
		NodePassphrase:          nodePassphraseEnc,
		NodePassphraseSignature: nodePassphraseSig,

		SignatureAddress: s.signatureAddress,
	}
	if err := protonext.SetCreateFolderName(&req, name, s.defaultAddrKR, parentNodeKR); err != nil {
		return "", err
	}
	if err := protonext.SetCreateFolderHash(&req, name, parentHashKey); err != nil {
		return "", err
	}
	// The new folder's own hash key is independent of its parent's;
	// it's used to hash the names of children we create inside it.
	newFolderKR, err := getKeyRing(parentNodeKR, s.defaultAddrKR, nodeKey, nodePassphraseEnc, nodePassphraseSig)
	if err != nil {
		return "", err
	}
	_ = newFolderKR // only needed once we want the hash key back locally
	if err := protonext.SetCreateFolderNodeHashKey(&req, newFolderKR); err != nil {
		return "", err
	}

	res, err := s.c.CreateFolder(ctx, s.mainShare.ShareID, req)
	if err != nil {
		// Handle "already exists" — look up the existing folder and
		// return its link ID, making CreateFolder idempotent.
		var apiErr *proton.APIError
		if errors.As(err, &apiErr) && apiErr.Code == codeAlreadyExists {
			existing, lookupErr := s.findChildByName(ctx, parentLink.LinkID, name, true)
			if lookupErr == nil && existing != "" {
				return existing, nil
			}
		}
		return "", err
	}
	return res.ID, nil
}

// findChildByName lists the active children of parentLinkID and returns
// the link ID of the first child matching `name` with the given type
// (isDir=true for folders, false for files). Returns ("", nil) when no
// match is found.
func (s *Session) findChildByName(ctx context.Context, parentLinkID, name string, isDir bool) (string, error) {
	entries, err := s.ListDirectory(ctx, parentLinkID)
	if err != nil {
		return "", err
	}
	for _, e := range entries {
		if e.Name == name && e.IsDir == isDir {
			return e.LinkID, nil
		}
	}
	return "", nil
}

// findChildLinkByName is like findChildByName but returns the full
// proton.Link for the matched child, which callers need when they
// must inspect FileProperties / State / keyrings.
func (s *Session) findChildLinkByName(ctx context.Context, parentLinkID, name string, wantFile bool) (*proton.Link, error) {
	entries, err := s.ListDirectory(ctx, parentLinkID)
	if err != nil {
		return nil, err
	}
	wantType := proton.LinkTypeFolder
	if wantFile {
		wantType = proton.LinkTypeFile
	}
	for _, e := range entries {
		if e.Name == name {
			// Fetch the full link metadata (ListDirectory caches it).
			link, err := s.getLink(ctx, e.LinkID)
			if err != nil {
				continue
			}
			if link.Type == wantType {
				return &link, nil
			}
		}
	}
	return nil, nil
}

// UploadFile uploads `srcPath`'s contents as a new file named `name`
// under `parentLinkID`. Uses the file's mtime for the revision's
// XAttr ModificationTime. Returns the new file's link ID.
//
// Flow (mirrors Bridge's uploadFile):
//   1. Open source file, get size + mtime.
//   2. Create draft on the server (CreateFile with encrypted name /
//      hash / session-key packet), OR find the existing file and
//      create a new revision on it if a name conflict occurs.
//   3. Read the file in 4 MB chunks: encrypt + detached-sign each
//      chunk, batch 8 blocks, RequestBlockUpload + UploadBlock for
//      the batch. Accumulate manifest-hash (SHA-256 of encrypted
//      blocks), XAttr block sizes, and SHA-1 of plaintext.
//   4. Commit the revision: sign the manifest, encrypt the XAttr
//      payload, send the CommitRevisionReq.
//
// Hash-first short-circuit: when step 2 finds the parent already has
// an active file with the same name, the existing active revision's
// XAttr SHA-1 is compared to the local file's SHA-1. On match, the
// upload + commit are skipped entirely and the existing link ID is
// returned — no blocks are re-encrypted, no bytes re-uploaded. Keeps
// first-sync against a non-empty remote tree from re-pushing the
// content of every byte-identical file.
func (s *Session) UploadFile(ctx context.Context, parentLinkID, name, srcPath string) (string, error) {
	if parentLinkID == "" {
		parentLinkID = s.RootLinkID()
	}
	file, err := os.Open(srcPath)
	if err != nil {
		return "", err
	}
	defer file.Close()
	stat, err := file.Stat()
	if err != nil {
		return "", err
	}
	modTime := stat.ModTime()

	linkID, revisionID, sessionKey, nodeKR, err := s.createFileDraft(ctx, parentLinkID, name, srcPath)
	if err != nil || revisionID != "" {
		// The tree changes from here on (or may have, on error); only the hash-match short-circuit leaves it untouched.
		defer s.invalidateListings()
	}
	if err != nil {
		return "", err
	}
	if revisionID == "" {
		// Hash-first short-circuit: the existing active revision
		// already holds content byte-identical to srcPath.
		return linkID, nil
	}

	manifest, fileSize, blockSizes, sha1Hex, err := s.uploadBlocks(ctx, sessionKey, nodeKR, file, linkID, revisionID)
	if err != nil {
		// Best-effort draft cleanup. If this fails we swallow — the
		// CreateRevision path will retry the user's next upload and
		// the stale draft will be collected server-side eventually.
		_ = protonext.DeleteRevision(ctx, s.protonextAuth(), s.mainShare.ShareID, linkID, revisionID)
		return "", err
	}

	if err := s.commitRevision(ctx, nodeKR, linkID, revisionID, manifest, fileSize, blockSizes, sha1Hex, modTime); err != nil {
		_ = protonext.DeleteRevision(ctx, s.protonextAuth(), s.mainShare.ShareID, linkID, revisionID)
		return "", err
	}
	// Best-effort: drop any non-active revisions left behind by prior
	// uploads of this file. Proton retains the previous active revision
	// as "Obsolete" on every commit, so without this every re-upload
	// permanently grows the user's quota — which is exactly the bloat
	// we discovered (~520 MiB across 227 files in one account).
	s.cleanupNonActiveRevisions(ctx, linkID, revisionID)
	return linkID, nil
}

// cleanupNonActiveRevisions deletes every revision on `fileLinkID`
// except `keepRevisionID`. Best-effort: errors are logged and swallowed
// so a slow/failed cleanup never breaks the upload's caller.
func (s *Session) cleanupNonActiveRevisions(ctx context.Context, fileLinkID, keepRevisionID string) {
	revs, err := protonext.ListRevisions(ctx, s.protonextAuth(), s.mainShare.ShareID, fileLinkID)
	if err != nil {
		log.Printf("[proton-native] revision-cleanup: ListRevisions(%s) failed: %v", fileLinkID, err)
		return
	}
	for _, r := range revs {
		if r.ID == keepRevisionID {
			continue
		}
		if proton.RevisionState(r.State) == proton.RevisionStateActive {
			// Defensive: never delete the active revision. If we
			// somehow saw two Active revisions, that's a server-side
			// invariant violation; let the user investigate.
			log.Printf("[proton-native] revision-cleanup: refusing to delete Active revision %s on %s", r.ID, fileLinkID)
			continue
		}
		if err := protonext.DeleteRevision(ctx, s.protonextAuth(), s.mainShare.ShareID, fileLinkID, r.ID); err != nil {
			log.Printf("[proton-native] revision-cleanup: DeleteRevision(%s/%s) failed: %v", fileLinkID, r.ID, err)
		}
	}
}

// createFileDraft posts a `CreateFile` request for a new file under
// `parentLinkID`. If the file already exists on the server (422 /
// Code=2500), the existing file link is located, any stale draft
// revision is cleaned up, a fresh revision is created on the
// existing link, and the existing file's keyring + session key are
// returned so the caller can encrypt blocks with the right key
// material. Returns (linkID, revisionID, fileSessionKey, fileNodeKR).
//
// `srcPath` is consulted only for the active-conflict hash-first
// short-circuit: when the existing active revision's XAttr SHA-1
// matches the local file's SHA-1, createFileDraft returns
// `(existingLinkID, "", nil, nil, nil)` to signal "no upload needed".
// Any failure to read XAttr / hash falls through to the normal
// new-revision path, so the short-circuit is always opportunistic.
func (s *Session) createFileDraft(
	ctx context.Context,
	parentLinkID, name, srcPath string,
) (string, string, *crypto.SessionKey, *crypto.KeyRing, error) {
	parentLink, err := s.getLink(ctx, parentLinkID)
	if err != nil {
		return "", "", nil, nil, err
	}
	if parentLink.Type != proton.LinkTypeFolder {
		return "", "", nil, nil, ErrParentNotFolder
	}
	parentNodeKR, err := s.linkKR(ctx, &parentLink)
	if err != nil {
		return "", "", nil, nil, err
	}
	parentHashKey, err := parentLink.GetHashKey(parentNodeKR)
	if err != nil {
		return "", "", nil, nil, err
	}

	// Fast path: avoid the 422 round-trip when an ACTIVE file with
	// this name already exists under the parent, and keep the pick
	// deterministic across sync cycles when duplicates accumulate
	// (the cascade the user saw on 2026-04-22 where each CreateFile
	// attempt logged a different conflicting linkID).
	//
	// Trashed / deleted / draft ghosts are intentionally excluded
	// here — trashed links don't always consume the name-hash slot,
	// so pre-empting CreateFile on them forces the reconcile path
	// for names the server would gladly accept. If a ghost really
	// does occupy the slot, CreateFile 422s below and the post-422
	// fallback (`findAnyChildFileByName`) handles it.
	if preExisting, _ := s.findActiveChildFileByName(ctx, parentLink.LinkID, name); preExisting != nil {
		log.Printf("[proton-native] pre-flight found active link for %q: linkID=%s — skipping CreateFile", name, preExisting.LinkID)
		return s.reconcileExistingFileLink(ctx, &parentLink, parentNodeKR, parentHashKey, preExisting, name, srcPath)
	}

	nodeKey, nodePassphraseEnc, nodePassphraseSig, err := generateNodeKeys(parentNodeKR, s.defaultAddrKR)
	if err != nil {
		return "", "", nil, nil, err
	}

	mimeType := mime.TypeByExtension(filepath.Ext(name))
	if mimeType == "" {
		mimeType = "application/octet-stream"
	}

	req := proton.CreateFileReq{
		ParentLinkID:            parentLink.LinkID,
		MIMEType:                mimeType,
		NodeKey:                 nodeKey,
		NodePassphrase:          nodePassphraseEnc,
		NodePassphraseSignature: nodePassphraseSig,
		SignatureAddress:        s.signatureAddress,
	}
	if err := protonext.SetCreateFileName(&req, name, s.defaultAddrKR, parentNodeKR); err != nil {
		return "", "", nil, nil, err
	}
	if err := protonext.SetCreateFileHash(&req, name, parentHashKey); err != nil {
		return "", "", nil, nil, err
	}

	// Build the file's own node keyring so we can generate its
	// content session key and sign it.
	nodeKR, err := getKeyRing(parentNodeKR, s.defaultAddrKR, nodeKey, nodePassphraseEnc, nodePassphraseSig)
	if err != nil {
		return "", "", nil, nil, err
	}
	sessionKey, err := protonext.SetCreateFileContentKey(&req, nodeKR)
	if err != nil {
		return "", "", nil, nil, err
	}

	res, err := s.c.CreateFile(ctx, s.mainShare.ShareID, req)
	if err == nil {
		// Happy path: brand-new file draft created.
		return res.ID, res.RevisionID, sessionKey, nodeKR, nil
	}

	// -----------------------------------------------------------
	// Conflict handling: file with the same name already exists.
	// Mirror Proton-API-Bridge's createFileUploadDraft logic.
	// This branch is only reached when the pre-flight above missed
	// the existing link — e.g. another client created the file
	// between our lookup and CreateFile, or the link's name was
	// indexed but not yet visible via ListChildren.
	// -----------------------------------------------------------
	var apiErr *proton.APIError
	if !errors.As(err, &apiErr) || apiErr.Code != codeAlreadyExists {
		// Not a name-conflict — genuine error.
		return "", "", nil, nil, err
	}

	log.Printf("[proton-native] CreateFile conflict for %q — searching ALL children (incl. trashed/draft)", name)

	existingLink, findErr := s.findAnyChildFileByName(ctx, parentLink.LinkID, name)
	if findErr != nil {
		log.Printf("[proton-native] error searching for conflicting file: %v", findErr)
		return "", "", nil, nil, err
	}
	if existingLink == nil {
		log.Printf("[proton-native] could not locate conflicting file %q in any state — returning original error", name)
		return "", "", nil, nil, err
	}

	log.Printf("[proton-native] found conflicting file: linkID=%s state=%d type=%d", existingLink.LinkID, existingLink.State, existingLink.Type)

	return s.reconcileExistingFileLink(ctx, &parentLink, parentNodeKR, parentHashKey, existingLink, name, srcPath)
}

// reconcileExistingFileLink produces the `(linkID, revisionID,
// sessionKey, nodeKR)` tuple `createFileDraft` returns when a link
// named `name` is already present under the parent. Shared by the
// pre-flight fast path (no CreateFile issued) and the post-422
// fallback (CreateFile 422'd between our lookup and the retry).
//
// Paths:
//   - Draft ghost → delete via DeleteChildren (the folder's
//     `/delete_multiple` endpoint permanently removes drafts) and
//     submit a fresh CreateFile. Name-hash slot releases.
//   - Trashed / Deleted ghost → `/delete_multiple` only handles
//     drafts, returning 2501 "Draft file not found" for these
//     states; we have no bound endpoint that can permanently
//     remove a trashed link. Surface an actionable error so the
//     user can clear the block by emptying their Proton trash,
//     and dedupe the warning so the sync log doesn't spam.
//   - Active with matching XAttr SHA-1 → short-circuit: return the
//     existing link ID with an empty revision ID so UploadFile skips
//     blocks + commit entirely.
//   - Active with differing content → call handleRevisionConflict
//     (deletes any stale draft, creates a new revision on the link).
//     Returns the existing link's keyring / session key so blocks
//     encrypt with the same content key the server expects.
func (s *Session) reconcileExistingFileLink(
	ctx context.Context,
	parentLink *proton.Link,
	parentNodeKR *crypto.KeyRing,
	parentHashKey []byte,
	existingLink *proton.Link,
	name, srcPath string,
) (string, string, *crypto.SessionKey, *crypto.KeyRing, error) {
	// ---- Draft ghost: DeleteChildren works for this state ----
	if existingLink.State == proton.LinkStateDraft {
		log.Printf("[proton-native] permanently deleting draft ghost %s", existingLink.LinkID)
		if delErr := s.c.DeleteChildren(ctx, s.mainShare.ShareID, existingLink.ParentLinkID, existingLink.LinkID); delErr != nil {
			return "", "", nil, nil, delErr
		}
		delete(s.linkCache, existingLink.LinkID)
		delete(s.krCache, existingLink.LinkID)
		return s.createFreshFileDraft(ctx, parentLink, parentNodeKR, parentHashKey, name)
	}

	// ---- Trashed / Deleted ghost: actionable failure, no retry ----
	if existingLink.State == proton.LinkStateTrashed || existingLink.State == proton.LinkStateDeleted {
		s.warnGhostBlocker(existingLink.LinkID, name, existingLink.State)
		return "", "", nil, nil, fmt.Errorf(
			"upload blocked: %q has a trashed copy on Proton (linkID=%s) occupying the name slot; empty your Proton Drive trash at https://drive.proton.me and retry",
			name, existingLink.LinkID,
		)
	}

	// ---- Active: hash-first short-circuit, otherwise new revision ----
	//
	// Hash-first: see whether the active revision's XAttr SHA-1
	// matches the local file. If it does, there's nothing to
	// upload — return the existing link ID with an empty revision
	// ID so UploadFile can bail out. Any error (XAttr missing,
	// decrypt failure, hash I/O error) falls through to the normal
	// new-revision handling.
	if existingLink.State == proton.LinkStateActive {
		if skip := s.contentAlreadyMatches(ctx, existingLink, parentNodeKR, srcPath); skip {
			log.Printf("[proton-native] content-hash match for %q — skipping upload, reusing linkID=%s", name, existingLink.LinkID)
			return existingLink.LinkID, "", nil, nil, nil
		}
	}

	revisionID, needRetry, revErr := s.handleRevisionConflict(ctx, existingLink)
	if revErr != nil {
		return "", "", nil, nil, revErr
	}

	if needRetry {
		// The link only had a draft (no active revision), so we
		// deleted the link and need to re-issue CreateFile.
		return s.createFreshFileDraft(ctx, parentLink, parentNodeKR, parentHashKey, name)
	}

	// Use the *existing* file's keyring and session key, not the
	// freshly-generated ones. The server expects blocks encrypted
	// with the original file's content key.
	existNodeKR, err := existingLink.GetKeyRing(parentNodeKR, s.defaultAddrKR)
	if err != nil {
		return "", "", nil, nil, err
	}
	existSessionKey, err := existingLink.GetSessionKey(existNodeKR)
	if err != nil {
		return "", "", nil, nil, err
	}

	log.Printf("[proton-native] reusing existing file link %s, new revision %s", existingLink.LinkID, revisionID)
	return existingLink.LinkID, revisionID, existSessionKey, existNodeKR, nil
}

// createFreshFileDraft builds a CreateFile request from scratch —
// fresh node keys, encrypted name, hashed name — and submits it.
// Used by the reconcile paths after a ghost-link deletion or a
// draft-only link removal; the name-hash index slot the existing
// link was holding is now free, so the server accepts the new draft.
func (s *Session) createFreshFileDraft(
	ctx context.Context,
	parentLink *proton.Link,
	parentNodeKR *crypto.KeyRing,
	parentHashKey []byte,
	name string,
) (string, string, *crypto.SessionKey, *crypto.KeyRing, error) {
	nodeKey, nodePassphraseEnc, nodePassphraseSig, err := generateNodeKeys(parentNodeKR, s.defaultAddrKR)
	if err != nil {
		return "", "", nil, nil, err
	}
	mimeType := mime.TypeByExtension(filepath.Ext(name))
	if mimeType == "" {
		mimeType = "application/octet-stream"
	}
	req := proton.CreateFileReq{
		ParentLinkID:            parentLink.LinkID,
		MIMEType:                mimeType,
		NodeKey:                 nodeKey,
		NodePassphrase:          nodePassphraseEnc,
		NodePassphraseSignature: nodePassphraseSig,
		SignatureAddress:        s.signatureAddress,
	}
	if err := protonext.SetCreateFileName(&req, name, s.defaultAddrKR, parentNodeKR); err != nil {
		return "", "", nil, nil, err
	}
	if err := protonext.SetCreateFileHash(&req, name, parentHashKey); err != nil {
		return "", "", nil, nil, err
	}
	nodeKR, err := getKeyRing(parentNodeKR, s.defaultAddrKR, nodeKey, nodePassphraseEnc, nodePassphraseSig)
	if err != nil {
		return "", "", nil, nil, err
	}
	sessionKey, err := protonext.SetCreateFileContentKey(&req, nodeKR)
	if err != nil {
		return "", "", nil, nil, err
	}
	res, err := s.c.CreateFile(ctx, s.mainShare.ShareID, req)
	if err != nil {
		return "", "", nil, nil, err
	}
	return res.ID, res.RevisionID, sessionKey, nodeKR, nil
}

// findActiveChildFileByName searches the ACTIVE children of
// parentLinkID for a file matching `name`. Used by the pre-flight
// optimization in createFileDraft: when we spot an active duplicate
// we skip CreateFile and reconcile immediately. Trashed / draft /
// deleted ghosts are deliberately ignored here — they don't
// reliably occupy the name-hash slot, so pre-empting on them sends
// uploads down the reconcile path unnecessarily. Any actual
// ghost-slot conflict surfaces when CreateFile 422s and the caller
// falls back to `findAnyChildFileByName`.
//
// Deterministic among active duplicates: lowest LinkID wins so
// consecutive sync passes don't oscillate between copies.
func (s *Session) findActiveChildFileByName(ctx context.Context, parentLinkID, name string) (*proton.Link, error) {
	allEntries, err := s.ListAllChildren(ctx, parentLinkID)
	if err != nil {
		return nil, err
	}
	var matches []*DumpEntry
	for _, e := range allEntries {
		if e.Name == name && !e.IsDir && e.State == int(proton.LinkStateActive) {
			matches = append(matches, e)
		}
	}
	if len(matches) == 0 {
		return nil, nil
	}
	sort.SliceStable(matches, func(i, j int) bool {
		return matches[i].LinkID < matches[j].LinkID
	})
	if len(matches) > 1 {
		log.Printf("[proton-native] found %d active duplicate links named %q under %s — picking %s", len(matches), name, parentLinkID, matches[0].LinkID)
	}
	for _, m := range matches {
		link, err := s.getLink(ctx, m.LinkID)
		if err != nil {
			log.Printf("[proton-native] getLink(%s) failed: %v — skipping", m.LinkID, err)
			continue
		}
		return &link, nil
	}
	return nil, nil
}

// warnGhostBlocker logs a one-liner the first time a trashed/deleted
// ghost blocks an upload for a given linkID. Subsequent cycles (same
// session, same ghost) stay silent so the sync log doesn't fill with
// the same warning every pass while the user resolves the block.
func (s *Session) warnGhostBlocker(linkID, name string, state proton.LinkState) {
	if s.warnedGhosts == nil {
		s.warnedGhosts = make(map[string]struct{})
	}
	if _, seen := s.warnedGhosts[linkID]; seen {
		return
	}
	s.warnedGhosts[linkID] = struct{}{}
	log.Printf("[proton-native] upload blocked for %q — %s ghost linkID=%s still holds the name slot; empty your Proton trash (https://drive.proton.me) to clear it", name, ghostStateLabel(state), linkID)
}

// ghostStateLabel formats a non-active LinkState for human-facing logs.
func ghostStateLabel(s proton.LinkState) string {
	switch s {
	case proton.LinkStateTrashed:
		return "trashed"
	case proton.LinkStateDeleted:
		return "deleted"
	case proton.LinkStateDraft:
		return "draft"
	default:
		return fmt.Sprintf("state=%d", int(s))
	}
}

// findAnyChildFileByName searches ALL children of parentLinkID —
// including trashed, deleted, and draft links — for a file matching
// `name`. This is needed because Proton's name-hash index retains
// entries for trashed/deleted files, so CreateFile can return 422
// even when no *active* file with that name exists.
//
// Selection is deterministic when multiple links share the same
// plaintext name (which can happen when duplicates accumulate from
// prior races / failed cleanups):
//
//  1. Prefer Active links over ghosts (Draft / Trashed / Deleted) —
//     handleRevisionConflict creates a new revision on the Active
//     one instead of deleting it.
//  2. Within a state bucket, pick the lowest LinkID lexically —
//     stable across calls so consecutive passes don't oscillate
//     between different duplicates (the cascade pattern the user
//     saw where each CreateFile attempt logged a different
//     conflicting linkID).
func (s *Session) findAnyChildFileByName(ctx context.Context, parentLinkID, name string) (*proton.Link, error) {
	// ListAllChildren fetches with showAll=true and does NOT filter
	// by state, so trashed/draft/deleted links are included.
	allEntries, err := s.ListAllChildren(ctx, parentLinkID)
	if err != nil {
		return nil, err
	}
	var matches []*DumpEntry
	for _, e := range allEntries {
		if e.Name == name && !e.IsDir {
			matches = append(matches, e)
		}
	}
	if len(matches) == 0 {
		return nil, nil
	}
	// Sort: Active (state=1) first, then by LinkID for stability.
	sort.SliceStable(matches, func(i, j int) bool {
		ai := matches[i].State == int(proton.LinkStateActive)
		aj := matches[j].State == int(proton.LinkStateActive)
		if ai != aj {
			return ai
		}
		return matches[i].LinkID < matches[j].LinkID
	})
	if len(matches) > 1 {
		log.Printf("[proton-native] found %d duplicate links named %q under %s — picking %s (state=%d)", len(matches), name, parentLinkID, matches[0].LinkID, matches[0].State)
	}
	for _, m := range matches {
		link, err := s.getLink(ctx, m.LinkID)
		if err != nil {
			log.Printf("[proton-native] getLink(%s) failed: %v — skipping", m.LinkID, err)
			continue
		}
		return &link, nil
	}
	return nil, nil
}

// handleRevisionConflict mirrors Proton-API-Bridge's
// handleRevisionConflict. Given an existing file link that caused a
// name conflict:
//
//   - If the file has a stale draft revision AND an active revision,
//     the draft is deleted and a fresh revision is created.
//   - If the file has a stale draft but NO active revision (the link
//     itself is in draft state), the link is deleted entirely and
//     (needRetry=true) signals the caller to re-issue CreateFile.
//   - If there is no draft, a new revision is created directly.
//
// Returns (revisionID, needRetry, error).
func (s *Session) handleRevisionConflict(ctx context.Context, link *proton.Link) (string, bool, error) {
	linkID := link.LinkID

	revisions, err := s.c.ListRevisions(ctx, s.mainShare.ShareID, linkID)
	if err != nil {
		return "", false, err
	}

	// Find any draft revision.
	var draftRevID string
	for i := range revisions {
		if revisions[i].State == proton.RevisionStateDraft {
			draftRevID = revisions[i].ID
			break
		}
	}

	if draftRevID != "" {
		// There's an existing draft. Two sub-cases:
		if link.State == proton.LinkStateDraft {
			// The entire link is draft-only (no active revision).
			// Delete the link and tell caller to re-create.
			log.Printf("[proton-native] deleting draft-only link %s", linkID)
			err := s.c.DeleteChildren(ctx, s.mainShare.ShareID, link.ParentLinkID, linkID)
			if err != nil {
				return "", false, err
			}
			// Invalidate cache for this link.
			delete(s.linkCache, linkID)
			delete(s.krCache, linkID)
			return "", true, nil
		}

		// Link has an active revision plus a stale draft — delete
		// the draft so we can create a fresh one.
		log.Printf("[proton-native] deleting stale draft revision %s on link %s", draftRevID, linkID)
		if err := protonext.DeleteRevision(ctx, s.protonextAuth(), s.mainShare.ShareID, linkID, draftRevID); err != nil {
			return "", false, err
		}
	}

	// Create a new revision on the existing file.
	newRev, err := protonext.CreateRevision(ctx, s.protonextAuth(), s.mainShare.ShareID, linkID)
	if err != nil {
		return "", false, err
	}

	log.Printf("[proton-native] created new revision %s on existing file %s", newRev.ID, linkID)
	return newRev.ID, false, nil
}

// uploadBlocks reads `file` in 4 MB chunks, encrypts + signs each,
// batches `uploadBatchBlockSize` at a time, and uploads in parallel
// within each batch. Returns (manifestData, totalFileSize, blockSizes,
// sha1Hex).
func (s *Session) uploadBlocks(
	ctx context.Context,
	sessionKey *crypto.SessionKey,
	nodeKR *crypto.KeyRing,
	file io.Reader,
	linkID, revisionID string,
) ([]byte, int64, []int64, string, error) {
	type pendingBlock struct {
		info proton.BlockUploadInfo
		data []byte
	}

	var (
		totalSize  int64
		manifest   []byte
		blockSizes []int64
		sha1Sum    = sha1.New()
		pending    []pendingBlock
		blockIdx   = 1
	)

	flushBatch := func() error {
		if len(pending) == 0 {
			return nil
		}
		list := make([]proton.BlockUploadInfo, len(pending))
		for i := range pending {
			list[i] = pending[i].info
		}
		uploadReq := proton.BlockUploadReq{
			AddressID:  s.mainShare.AddressID,
			ShareID:    s.mainShare.ShareID,
			LinkID:     linkID,
			RevisionID: revisionID,
			BlockList:  list,
		}
		uploadLinks, err := s.c.RequestBlockUpload(ctx, uploadReq)
		if err != nil {
			return err
		}
		if len(uploadLinks) != len(pending) {
			return errors.New("block upload: server returned wrong number of upload links")
		}
		errCh := make(chan error, len(uploadLinks))
		for i := range uploadLinks {
			go func(i int) {
				stream := resty.NewByteMultipartStream(pending[i].data)
				errCh <- s.c.UploadBlock(ctx, uploadLinks[i].BareURL, uploadLinks[i].Token, stream)
			}(i)
		}
		for range uploadLinks {
			if err := <-errCh; err != nil {
				return err
			}
		}
		pending = pending[:0]
		return nil
	}

	buf := make([]byte, uploadBlockSize)
	for {
		n, readErr := io.ReadFull(file, buf)
		if n == 0 {
			if readErr == io.EOF || readErr == io.ErrUnexpectedEOF {
				break
			}
			if readErr != nil {
				return nil, 0, nil, "", readErr
			}
		}
		chunk := make([]byte, n)
		copy(chunk, buf[:n])
		totalSize += int64(n)
		blockSizes = append(blockSizes, int64(n))
		sha1Sum.Write(chunk)

		plain := crypto.NewPlainMessage(chunk)
		encData, err := sessionKey.Encrypt(plain)
		if err != nil {
			return nil, 0, nil, "", err
		}
		encSig, err := s.defaultAddrKR.SignDetachedEncrypted(plain, nodeKR)
		if err != nil {
			return nil, 0, nil, "", err
		}
		encSigArm, err := encSig.GetArmored()
		if err != nil {
			return nil, 0, nil, "", err
		}

		hash := sha256.Sum256(encData)
		manifest = append(manifest, hash[:]...)

		pending = append(pending, pendingBlock{
			info: proton.BlockUploadInfo{
				Index:        blockIdx,
				Size:         int64(len(encData)),
				EncSignature: encSigArm,
				Hash:         base64.StdEncoding.EncodeToString(hash[:]),
			},
			data: encData,
		})
		blockIdx++
		if len(pending) == uploadBatchBlockSize {
			if err := flushBatch(); err != nil {
				return nil, 0, nil, "", err
			}
		}
		if readErr == io.EOF || readErr == io.ErrUnexpectedEOF {
			break
		}
		if readErr != nil {
			return nil, 0, nil, "", readErr
		}
	}
	if err := flushBatch(); err != nil {
		return nil, 0, nil, "", err
	}
	return manifest, totalSize, blockSizes, hex.EncodeToString(sha1Sum.Sum(nil)), nil
}

// commitRevision signs the manifest, builds the XAttr payload,
// encrypts it, and PUTs the CommitRevisionReq.
func (s *Session) commitRevision(
	ctx context.Context,
	nodeKR *crypto.KeyRing,
	linkID, revisionID string,
	manifestData []byte,
	fileSize int64,
	blockSizes []int64,
	sha1Hex string,
	modTime time.Time,
) error {
	manifestSig, err := s.defaultAddrKR.SignDetached(crypto.NewPlainMessage(manifestData))
	if err != nil {
		return err
	}
	manifestSigArm, err := manifestSig.GetArmored()
	if err != nil {
		return err
	}
	req := protonext.CommitRevisionReq{
		ManifestSignature: manifestSigArm,
		SignatureAddress:  s.signatureAddress,
	}
	xAttr := &protonext.RevisionXAttrCommon{
		ModificationTime: modTime.UTC().Format("2006-01-02T15:04:05-0700"),
		Size:             fileSize,
		BlockSizes:       blockSizes,
		Digests:          map[string]string{"SHA1": sha1Hex},
	}
	if err := protonext.SetCommitRevisionXAttr(&req, s.defaultAddrKR, nodeKR, xAttr); err != nil {
		return err
	}
	return protonext.CommitRevision(ctx, s.protonextAuth(), s.mainShare.ShareID, linkID, revisionID, req)
}

// contentAlreadyMatches returns true when the active revision of
// `link` carries a decrypted XAttr SHA-1 equal to the SHA-1 of
// `srcPath`'s contents. Returns false on any error — the caller
// treats that as "no signal", so a failed hash check never blocks
// the normal upload path.
func (s *Session) contentAlreadyMatches(
	ctx context.Context,
	link *proton.Link,
	parentNodeKR *crypto.KeyRing,
	srcPath string,
) bool {
	if link == nil || link.FileProperties == nil {
		return false
	}
	revID := link.FileProperties.ActiveRevision.ID
	if revID == "" {
		return false
	}
	nodeKR, err := link.GetKeyRing(parentNodeKR, s.defaultAddrKR)
	if err != nil {
		return false
	}
	// PageSize=1 — we're only after the decrypted XAttr metadata, not
	// the block list. Upstream's GetRevision drops the XAttr field on
	// the floor, so we go through protonext to capture it.
	rev, err := protonext.GetRevisionXAttr(ctx, s.protonextAuth(), s.mainShare.ShareID, link.LinkID, revID)
	if err != nil || rev.XAttr == "" {
		return false
	}
	xa, err := protonext.DecryptRevisionXAttr(rev.XAttr, s.defaultAddrKR, nodeKR)
	if err != nil || xa == nil {
		return false
	}
	remoteSHA1 := xa.Digests["SHA1"]
	if remoteSHA1 == "" {
		return false
	}
	localSHA1, err := sha1OfFile(srcPath)
	if err != nil {
		return false
	}
	return localSHA1 == remoteSHA1
}

// sha1OfFile streams `path`'s contents through SHA-1 and returns the
// hex-encoded digest. Matches the digest commitRevision writes into
// the revision's XAttr on upload, so equality implies byte-identical
// content.
func sha1OfFile(path string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	h := sha1.New()
	if _, err := io.Copy(h, f); err != nil {
		return "", err
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}
