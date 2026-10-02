package drive

// Drive read surface: listing children + statting a link.
//
// Ported and simplified from Proton-API-Bridge/folder.go. Two
// deliberate deviations from Bridge:
//
//   1. Bridge threads a `signatureVerificationKeyring` composed from
//      the signer's email address through every GetName / GetKeyRing
//      call. Upstream Link doesn't expose `SignatureEmail` /
//      `NameSignatureEmail` — those fields only exist on
//      RevisionMetadata. For single-account usage (our case) the
//      owner's default address keyring is always the correct
//      verifier, which is what we pass.
//   2. Child links fetched during listings are cached on the Session's
//      linkCache so that recursive listings and subsequent getLink /
//      linkKR calls hit the cache instead of re-fetching each link
//      individually.
//
// Bridge's original `folder.go::ListDirectory` filters out
// non-active children (drafts, trashed, deleted). We do the same.

import (
	"context"
	"log"
	"sync"
	"time"

	"github.com/ProtonMail/go-proton-api"
)

// Entry is the per-child record returned by ListDirectory / Stat.
// Shapes to match Celeste's `RemoteItem` domain type across the FFI
// boundary without the domain crate pulling in anything Proton-
// specific.
type Entry struct {
	LinkID       string `json:"link_id"`
	ParentLinkID string `json:"parent_link_id"`
	Name         string `json:"name"`
	IsDir        bool   `json:"is_dir"`
	Size         int64  `json:"size"`
	ModTimeUnix  int64  `json:"mod_time_unix"`
	MIMEType     string `json:"mime_type,omitempty"`
}

// DumpEntry is like Entry but includes the link State for diagnostic
// purposes (debugging invisible drafts / trashed children).
type DumpEntry struct {
	LinkID       string `json:"link_id"`
	ParentLinkID string `json:"parent_link_id"`
	Name         string `json:"name"`
	IsDir        bool   `json:"is_dir"`
	Size         int64  `json:"size"`
	ModTimeUnix  int64  `json:"mod_time_unix"`
	MIMEType     string `json:"mime_type,omitempty"`
	State        int    `json:"state"`
}

// entryFromLink picks a stable mtime for the sync engine to compare
// against. proton.Link.ModifyTime is documented (link_types.go:27 in
// the upstream proton-api) as the *API-side* modification time — it
// gets bumped server-side after the commit returns (we measured
// +10..+100 s of post-commit drift), which made the engine flag
// `BothMoreCurrent` whenever a file was both locally edited and
// caught mid-drift. The active revision's CreateTime is stamped at
// revision creation and only changes when a new revision replaces
// the active one — i.e. when content actually changes — so it's the
// right "remote content version" signal. Folders have no
// FileProperties; fall back to the link timestamps for them.
func entryFromLink(link *proton.Link, name string) *Entry {
	modTime := stableModTime(link)
	return &Entry{
		LinkID:       link.LinkID,
		ParentLinkID: link.ParentLinkID,
		Name:         name,
		IsDir:        link.Type == proton.LinkTypeFolder,
		Size:         link.Size,
		ModTimeUnix:  modTime,
		MIMEType:     link.MIMEType,
	}
}

func stableModTime(link *proton.Link) int64 {
	if link.FileProperties != nil {
		if t := link.FileProperties.ActiveRevision.CreateTime; t != 0 {
			return t
		}
	}
	if t := link.ModifyTime; t != 0 {
		return t
	}
	if t := link.CreateTime; t != 0 {
		return t
	}
	return time.Now().Unix()
}

// ListDirectory returns the active children of `folderLinkID`. The
// caller can pass the empty string to list the root.
func (s *Session) ListDirectory(ctx context.Context, folderLinkID string) ([]*Entry, error) {
	if folderLinkID == "" {
		folderLinkID = s.RootLinkID()
	}
	if out, ok := s.cachedListing(ctx, false, folderLinkID); ok {
		return out, nil
	}
	out, err := s.listDirectory(ctx, folderLinkID)
	if err == nil {
		s.storeListing(false, folderLinkID, out)
	}
	return out, err
}

func (s *Session) listDirectory(ctx context.Context, folderLinkID string) ([]*Entry, error) {
	folderLink, err := s.getLink(ctx, folderLinkID)
	if err != nil {
		return nil, err
	}
	if folderLink.State != proton.LinkStateActive {
		return nil, nil
	}

	childrenLinks, err := s.c.ListChildren(ctx, s.mainShare.ShareID, folderLink.LinkID, true)
	if err != nil {
		return nil, err
	}

	// The children's names are encrypted to the *folder's* node
	// keyring, so we unlock that once and reuse.
	folderKR, err := s.linkKR(ctx, &folderLink)
	if err != nil {
		return nil, err
	}

	out := make([]*Entry, 0, len(childrenLinks))
	for i := range childrenLinks {
		child := &childrenLinks[i]
		// Cache every child's link metadata so recursive listings
		// hit the cache instead of re-fetching each link individually.
		s.linkCache[child.LinkID] = *child
		if child.State != proton.LinkStateActive {
			continue
		}
		name, err := child.GetName(folderKR, s.defaultAddrKR)
		if err != nil {
			return nil, err
		}
		out = append(out, entryFromLink(child, name))
	}
	return out, nil
}

// Stat returns the metadata for a single link, with the name decrypted
// via the parent's keyring. Returns nil without error when the link
// exists but is not in the active state — mirrors the semantics the
// sync engine expects from its `stat` port.
func (s *Session) Stat(ctx context.Context, linkID string) (*Entry, error) {
	if linkID == "" {
		linkID = s.RootLinkID()
	}
	link, err := s.getLink(ctx, linkID)
	if err != nil {
		return nil, err
	}
	if link.State != proton.LinkStateActive {
		return nil, nil
	}
	var name string
	if link.ParentLinkID == "" {
		// Root link has no encrypted name — it's named by the share,
		// not by the parent's keyring. Return a stable placeholder.
		name = ""
	} else {
		parentKR, err := s.linkKRByID(ctx, link.ParentLinkID)
		if err != nil {
			return nil, err
		}
		name, err = link.GetName(parentKR, s.defaultAddrKR)
		if err != nil {
			return nil, err
		}
	}
	return entryFromLink(&link, name), nil
}

// ListAllChildren is a diagnostic variant of ListDirectory that returns
// ALL children of a folder — including drafts, trashed, deleted, and
// restoring links — so callers can inspect ghost entries that cause
// "file already exists" conflicts on the server side.
func (s *Session) ListAllChildren(ctx context.Context, folderLinkID string) ([]*DumpEntry, error) {
	if folderLinkID == "" {
		folderLinkID = s.RootLinkID()
	}
	folderLink, err := s.getLink(ctx, folderLinkID)
	if err != nil {
		return nil, err
	}

	childrenLinks, err := s.c.ListChildren(ctx, s.mainShare.ShareID, folderLink.LinkID, true)
	if err != nil {
		return nil, err
	}

	folderKR, err := s.linkKR(ctx, &folderLink)
	if err != nil {
		return nil, err
	}

	out := make([]*DumpEntry, 0, len(childrenLinks))
	for i := range childrenLinks {
		child := &childrenLinks[i]
		s.linkCache[child.LinkID] = *child

		name, err := child.GetName(folderKR, s.defaultAddrKR)
		if err != nil {
			// Decryption may fail for very old trashed/deleted links;
			// log and use a placeholder so the dump is still useful.
			log.Printf("[proton-native] child %s: name decrypt failed: %v", child.LinkID, err)
			name = "<decrypt-error>"
		}

		modTime := stableModTime(child)

		out = append(out, &DumpEntry{
			LinkID:       child.LinkID,
			ParentLinkID: child.ParentLinkID,
			Name:         name,
			IsDir:        child.Type == proton.LinkTypeFolder,
			Size:         child.Size,
			ModTimeUnix:  modTime,
			MIMEType:     child.MIMEType,
			State:        int(child.State),
		})
	}
	return out, nil
}

// ListRecursive returns all active entries under rootLinkID by walking
// the folder tree with concurrent API calls. Entries have their Name
// field set to the full relative path (e.g. "Foo/Bar/baz.txt").
//
// Design: two-phase approach to avoid concurrent access to Session caches.
//
//	Phase 1: BFS with goroutine pool — only calls s.c.ListChildren()
//	         (thread-safe). Collects raw proton.Link slices per folder.
//	Phase 2: Sequential walk — caches links, resolves keyrings,
//	         decrypts names, builds Entry list with full paths.
func (s *Session) ListRecursive(ctx context.Context, rootLinkID string) ([]*Entry, error) {
	if rootLinkID == "" {
		rootLinkID = s.RootLinkID()
	}
	if out, ok := s.cachedListing(ctx, true, rootLinkID); ok {
		return out, nil
	}
	out, err := s.listRecursive(ctx, rootLinkID)
	if err == nil {
		s.storeListing(true, rootLinkID, out)
	}
	return out, err
}

func (s *Session) listRecursive(ctx context.Context, rootLinkID string) ([]*Entry, error) {

	// ── Phase 1: concurrent ListChildren calls ──────────────────

	const maxConcurrency = 10
	sem := make(chan struct{}, maxConcurrency)

	var (
		mu       sync.Mutex
		wg       sync.WaitGroup
		firstErr error
		results  = make(map[string][]proton.Link) // folderLinkID → children
	)

	// errSeen returns true if an error has already been recorded.
	errSeen := func() bool {
		mu.Lock()
		defer mu.Unlock()
		return firstErr != nil
	}

	// recordErr stores the first error encountered.
	recordErr := func(err error) {
		mu.Lock()
		defer mu.Unlock()
		if firstErr == nil {
			firstErr = err
		}
	}

	// storeResult saves the children slice and returns the sub-folders
	// that need further traversal.
	storeResult := func(folderID string, children []proton.Link) []string {
		mu.Lock()
		defer mu.Unlock()
		results[folderID] = children
		var subFolders []string
		for i := range children {
			c := &children[i]
			if c.Type == proton.LinkTypeFolder && c.State == proton.LinkStateActive {
				if _, already := results[c.LinkID]; !already {
					subFolders = append(subFolders, c.LinkID)
				}
			}
		}
		return subFolders
	}

	// Recursive fetch function. Each call handles one folder.
	var fetch func(linkID string)
	fetch = func(linkID string) {
		defer wg.Done()
		if errSeen() {
			return
		}

		sem <- struct{}{}
		children, err := s.c.ListChildren(ctx, s.mainShare.ShareID, linkID, true)
		<-sem

		if err != nil {
			recordErr(err)
			return
		}

		subFolders := storeResult(linkID, children)
		for _, sf := range subFolders {
			if errSeen() {
				break
			}
			wg.Add(1)
			go fetch(sf)
		}
	}

	wg.Add(1)
	go fetch(rootLinkID)
	wg.Wait()

	if firstErr != nil {
		return nil, firstErr
	}

	// ── Phase 2: sequential decrypt + path assembly ─────────────

	// Cache all fetched links.
	for _, children := range results {
		for i := range children {
			s.linkCache[children[i].LinkID] = children[i]
		}
	}

	type bfsItem struct {
		linkID string
		prefix string // parent's full path (empty for root's children)
	}

	queue := []bfsItem{{linkID: rootLinkID, prefix: ""}}
	var out []*Entry

	for len(queue) > 0 {
		item := queue[0]
		queue = queue[1:]

		children, ok := results[item.linkID]
		if !ok {
			continue
		}

		folderLink := s.linkCache[item.linkID]
		folderKR, err := s.linkKR(ctx, &folderLink)
		if err != nil {
			return nil, err
		}

		for i := range children {
			child := &children[i]
			if child.State != proton.LinkStateActive {
				continue
			}
			name, err := child.GetName(folderKR, s.defaultAddrKR)
			if err != nil {
				return nil, err
			}

			fullPath := name
			if item.prefix != "" {
				fullPath = item.prefix + "/" + name
			}

			out = append(out, entryFromLink(child, fullPath))

			if child.Type == proton.LinkTypeFolder {
				queue = append(queue, bfsItem{linkID: child.LinkID, prefix: fullPath})
			}
		}
	}

	return out, nil
}
