package drive

// Storage audit: walks the entire share — including non-active children
// (drafts, trashed, deleted) — and per-file revision history, so callers
// can pinpoint *what* is consuming Proton Drive quota when the active
// tree alone doesn't account for the reported usage.
//
// This is read-only: it only issues GET requests against the API. The
// companion `PurgeTrashedAndDrafts` / `PurgeObsoleteRevisions` methods
// (below) are the destructive counterparts.

import (
	"context"
	"errors"
	"log"
	"sync"

	"celeste/go/proton-ext"

	"github.com/ProtonMail/go-proton-api"
)

// AuditEntry is a single non-active link we encountered during an audit
// walk. The Path is the slash-joined chain of decrypted folder names
// from the share root down to the entry.
type AuditEntry struct {
	LinkID       string
	ParentLinkID string
	Path         string // human-readable path, decrypted
	Name         string
	IsDir        bool
	State        proton.LinkState
	Size         int64
	MIMEType     string
}

// AuditRevision is a non-active revision discovered while inspecting an
// active file's revision list.
type AuditRevision struct {
	FileLinkID string
	FilePath   string
	RevisionID string
	State      proton.RevisionState
	Size       int64
	CreateTime int64
}

// AuditReport summarises everything we found.
type AuditReport struct {
	UserUsedSpace  uint64
	UserMaxSpace   uint64
	DriveUsedSpace uint64

	VolumeUsedSpace int64
	VolumeMaxSpace  *int64

	// Tree walk: counts/sizes by link state (active is included so the
	// caller can sanity-check the totals).
	StateCounts map[proton.LinkState]int
	StateSizes  map[proton.LinkState]int64

	// Non-active links (trashed, draft, deleted) discovered while
	// walking. We don't list every active file — that explodes for big
	// trees and isn't what an audit is for.
	Trashed []AuditEntry
	Drafts  []AuditEntry
	Deleted []AuditEntry

	// Revision bloat: per-file non-active revisions. Populated only
	// when AuditOptions.IncludeRevisions is set, since this requires
	// one extra API call per active file.
	ObsoleteRevisions []AuditRevision

	// Per-folder ListChildren returned this many total links across the
	// whole walk (including non-active). Useful sanity-check.
	TotalLinksWalked int
}

// AuditOptions tweaks the walk's behaviour.
type AuditOptions struct {
	// IncludeRevisions adds one ListRevisions call per *active* file.
	// Slow on large shares (one round-trip per file); off by default.
	IncludeRevisions bool

	// MaxRevisionConcurrency caps parallel ListRevisions calls when
	// IncludeRevisions is true. Zero falls back to a small default.
	MaxRevisionConcurrency int
}

// AuditStorage walks the entire share — visiting every folder regardless
// of state — and assembles an AuditReport. Read-only.
func (s *Session) AuditStorage(ctx context.Context, opts AuditOptions) (*AuditReport, error) {
	report := &AuditReport{
		StateCounts: map[proton.LinkState]int{},
		StateSizes:  map[proton.LinkState]int64{},
	}

	// --- account-level numbers ---------------------------------------
	user, err := s.c.GetUser(ctx)
	if err != nil {
		return nil, err
	}
	report.UserUsedSpace = user.UsedSpace
	report.UserMaxSpace = user.MaxSpace
	report.DriveUsedSpace = user.ProductUsedSpace.Drive

	volumes, err := s.c.ListVolumes(ctx)
	if err != nil {
		return nil, err
	}
	for _, v := range volumes {
		if v.Share.ShareID == s.mainShare.ShareID {
			report.VolumeUsedSpace = v.UsedSpace
			report.VolumeMaxSpace = v.MaxSpace
			break
		}
	}

	// --- tree walk ---------------------------------------------------
	rootID := s.RootLinkID()

	type folderJob struct {
		linkID         string
		path           string
		ancestorActive bool // true iff every ancestor (incl. this folder) is in the Active state
	}
	queue := []folderJob{{linkID: rootID, path: "", ancestorActive: true}}

	var activeFiles []struct {
		linkID string
		path   string
	}

	for len(queue) > 0 {
		cur := queue[0]
		queue = queue[1:]

		// ListAllChildren caches links and decrypts names for us.
		entries, err := s.ListAllChildren(ctx, cur.linkID)
		if err != nil {
			return nil, err
		}

		for _, e := range entries {
			report.TotalLinksWalked++
			state := proton.LinkState(e.State)
			report.StateCounts[state]++
			report.StateSizes[state] += e.Size

			fullPath := e.Name
			if cur.path != "" {
				fullPath = cur.path + "/" + e.Name
			}

			ae := AuditEntry{
				LinkID:       e.LinkID,
				ParentLinkID: e.ParentLinkID,
				Path:         fullPath,
				Name:         e.Name,
				IsDir:        e.IsDir,
				State:        state,
				Size:         e.Size,
				MIMEType:     e.MIMEType,
			}

			switch state {
			case proton.LinkStateTrashed:
				report.Trashed = append(report.Trashed, ae)
			case proton.LinkStateDraft:
				report.Drafts = append(report.Drafts, ae)
			case proton.LinkStateDeleted:
				report.Deleted = append(report.Deleted, ae)
			}

			if e.IsDir {
				// Recurse into folders regardless of state — trashed
				// folders cascade-trash their contents but the children
				// still occupy space until permanently deleted.
				queue = append(queue, folderJob{linkID: e.LinkID, path: fullPath, ancestorActive: cur.ancestorActive && state == proton.LinkStateActive})
			} else if state == proton.LinkStateActive && opts.IncludeRevisions && cur.ancestorActive {
				// Only consider files whose entire ancestor chain is
				// active. Children of trashed folders keep their own
				// state=Active but the API rejects revision lookups
				// for them with 2501 ("File or folder not found").
				activeFiles = append(activeFiles, struct {
					linkID string
					path   string
				}{linkID: e.LinkID, path: fullPath})
			}
		}
	}

	// --- per-file revision walk (optional) --------------------------
	if opts.IncludeRevisions && len(activeFiles) > 0 {
		concurrency := opts.MaxRevisionConcurrency
		if concurrency <= 0 {
			concurrency = 8
		}
		sem := make(chan struct{}, concurrency)

		var (
			mu       sync.Mutex
			wg       sync.WaitGroup
			firstErr error
		)
		recordErr := func(err error) {
			mu.Lock()
			if firstErr == nil {
				firstErr = err
			}
			mu.Unlock()
		}

		for _, f := range activeFiles {
			f := f
			wg.Add(1)
			go func() {
				defer wg.Done()
				sem <- struct{}{}
				defer func() { <-sem }()

				revs, err := s.c.ListRevisions(ctx, s.mainShare.ShareID, f.linkID)
				if err != nil {
					var apiErr *proton.APIError
					if errors.As(err, &apiErr) && apiErr.Code == proton.Code(2501) {
						// Link disappeared between tree walk and
						// revision lookup (race), or it's an active
						// link under a trashed parent the upstream
						// filter missed. Log and skip.
						log.Printf("[audit] skipping revisions for %s: %v", f.path, err)
						return
					}
					recordErr(err)
					return
				}
				for _, r := range revs {
					if r.State == proton.RevisionStateActive {
						continue
					}
					mu.Lock()
					report.ObsoleteRevisions = append(report.ObsoleteRevisions, AuditRevision{
						FileLinkID: f.linkID,
						FilePath:   f.path,
						RevisionID: r.ID,
						State:      r.State,
						Size:       r.Size,
						CreateTime: r.CreateTime,
					})
					mu.Unlock()
				}
			}()
		}
		wg.Wait()
		if firstErr != nil {
			return nil, firstErr
		}
	}

	return report, nil
}

// PurgeLinks permanently deletes the given link IDs. They must all share
// the same parent — the Proton API takes (parentID, childIDs...). The
// caller is responsible for grouping by parent and confirming intent;
// this method does no extra safety checks beyond what the API enforces.
func (s *Session) PurgeLinks(ctx context.Context, parentLinkID string, linkIDs []string) error {
	if len(linkIDs) == 0 {
		return nil
	}
	if err := s.c.DeleteChildren(ctx, s.mainShare.ShareID, parentLinkID, linkIDs...); err != nil {
		return err
	}
	for _, id := range linkIDs {
		delete(s.linkCache, id)
		delete(s.krCache, id)
	}
	return nil
}

// PurgeRevision permanently deletes a single revision (e.g. an obsolete
// historical version). The active revision can never be deleted via this
// path — Proton's API rejects it.
func (s *Session) PurgeRevision(ctx context.Context, fileLinkID, revisionID string) error {
	return protonext.DeleteRevision(ctx, s.protonextAuth(), s.mainShare.ShareID, fileLinkID, revisionID)
}
