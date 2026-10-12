package protonext

// Revision-lifecycle endpoints upstream go-proton-api doesn't expose:
// CreateRevision, CommitRevision, DeleteRevision, plus a GetRevision
// variant that captures the XAttr field (upstream's RevisionMetadata
// drops it on the floor).

import (
	"context"
)

// CreateRevision posts an empty new draft revision on an existing file
// link. Used when replacing the contents of an already-uploaded file.
func CreateRevision(ctx context.Context, auth Auth, shareID, linkID string) (CreateRevisionRes, error) {
	var env struct {
		Revision CreateRevisionRes
	}
	path := "/drive/shares/" + shareID + "/files/" + linkID + "/revisions"
	if err := do(ctx, auth, "POST", path, nil, &env); err != nil {
		return CreateRevisionRes{}, err
	}
	return env.Revision, nil
}

// CommitRevision marks a draft revision as active once all its blocks
// have been uploaded.
func CommitRevision(ctx context.Context, auth Auth, shareID, linkID, revisionID string, req CommitRevisionReq) error {
	path := "/drive/shares/" + shareID + "/files/" + linkID + "/revisions/" + revisionID
	return do(ctx, auth, "PUT", path, req, nil)
}

// DeleteRevision permanently removes a revision (draft, obsolete, or
// otherwise non-active). Proton's API rejects deletion of the active
// revision.
func DeleteRevision(ctx context.Context, auth Auth, shareID, linkID, revisionID string) error {
	path := "/drive/shares/" + shareID + "/files/" + linkID + "/revisions/" + revisionID
	return do(ctx, auth, "DELETE", path, nil, nil)
}

// RevisionWithXAttr is the subset of the GetRevision response Syncative
// actually consumes — currently just the encrypted XAttr blob. Upstream
// returns the same JSON, but its `RevisionMetadata` doesn't decode the
// XAttr field, so we re-decode here to capture it.
type RevisionWithXAttr struct {
	ID    string `json:"ID"`
	XAttr string `json:"XAttr"`
}

// GetRevisionXAttr fetches a single revision and returns its XAttr
// blob (still armored — caller decrypts with DecryptRevisionXAttr).
// Pages of blocks are skipped (FromBlockIndex=1, PageSize=1) since we
// only care about the metadata.
func GetRevisionXAttr(ctx context.Context, auth Auth, shareID, linkID, revisionID string) (RevisionWithXAttr, error) {
	var env struct {
		Revision RevisionWithXAttr
	}
	path := "/drive/shares/" + shareID + "/files/" + linkID +
		"/revisions/" + revisionID + "?FromBlockIndex=1&PageSize=1"
	if err := do(ctx, auth, "GET", path, nil, &env); err != nil {
		return RevisionWithXAttr{}, err
	}
	return env.Revision, nil
}

// ListRevisionMeta is the metadata Syncative needs from /revisions when
// it's looking for non-active revisions to clean up. Mirrors the parts
// of upstream's RevisionMetadata we use plus nothing else.
type ListRevisionMeta struct {
	ID    string `json:"ID"`
	State int    `json:"State"`
	Size  int64  `json:"Size"`
}

// ListRevisions enumerates every revision on a file (active + obsolete
// + draft + deleted). Used by the upload pipeline's post-commit cleanup
// to delete revisions left behind by older uploads.
func ListRevisions(ctx context.Context, auth Auth, shareID, linkID string) ([]ListRevisionMeta, error) {
	var env struct {
		Revisions []ListRevisionMeta
	}
	path := "/drive/shares/" + shareID + "/files/" + linkID + "/revisions"
	if err := do(ctx, auth, "GET", path, nil, &env); err != nil {
		return nil, err
	}
	return env.Revisions, nil
}
