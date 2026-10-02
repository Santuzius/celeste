package drive

// Destructive Drive ops: trash, untrash, permanent delete.
//
// Upstream go-proton-api exposes `Client.TrashChildren(shareID,
// parentLinkID, linkIDs...)` and `Client.DeleteChildren(...)` with
// the same (parent, linkIDs...) shape — the same surgical primitive
// Bridge's `moveToTrash` wraps. Our cascade-bug hypothesis is that
// something above this level (rclone's protondrive glue) was passing
// the wrong linkIDs slice; by calling TrashChildren directly with
// exactly the one link ID the caller asks for, we eliminate that
// failure mode.
//
// Ported from Proton-API-Bridge/delete.go, minus the cache
// invalidation (we don't have a cache yet) and the EmptyTrash /
// EmptyRootFolder paths (not on Celeste's sync engine's critical
// path; can come back if we want them).

import (
	"context"
	"errors"

	"github.com/ProtonMail/go-proton-api"
)

// ErrLinkHasNoParent surfaces when a caller tries to trash the root
// link — it's the share's own entry point and can't be trashed.
var ErrLinkHasNoParent = errors.New("link has no parent (is the share root)")

// TrashLink moves the named link into Proton's Trash. Works for both
// files and folders; for folders the server cascade-trashes the
// contents. Exactly one `linkID` per call — we look up the parent
// so the `TrashChildren` request body carries precisely one ID.
func (s *Session) TrashLink(ctx context.Context, linkID string) error {
	if linkID == "" {
		return errors.New("link ID is empty")
	}
	defer s.invalidateListings()
	link, err := s.c.GetLink(ctx, s.mainShare.ShareID, linkID)
	if err != nil {
		return err
	}
	if link.ParentLinkID == "" {
		return ErrLinkHasNoParent
	}
	// Only active links can be trashed — already-trashed links would
	// need UntrashLink first; deleted links are gone. Surface the
	// state with a generic error rather than a soft no-op so callers
	// see the race.
	if link.State != proton.LinkStateActive {
		return errors.New("link is not active")
	}
	return s.c.TrashChildren(ctx, s.mainShare.ShareID, link.ParentLinkID, linkID)
}

// PermanentDeleteLink removes a trashed link outright. Celeste's
// sync engine shouldn't call this — mirror-deletes go through
// TrashLink so the user has a recovery window — but it's here for
// completeness (integration code may expose it as an explicit
// "empty trash for this file" action later).
func (s *Session) PermanentDeleteLink(ctx context.Context, linkID string) error {
	if linkID == "" {
		return errors.New("link ID is empty")
	}
	defer s.invalidateListings()
	link, err := s.c.GetLink(ctx, s.mainShare.ShareID, linkID)
	if err != nil {
		return err
	}
	if link.ParentLinkID == "" {
		return ErrLinkHasNoParent
	}
	return s.c.DeleteChildren(ctx, s.mainShare.ShareID, link.ParentLinkID, linkID)
}
