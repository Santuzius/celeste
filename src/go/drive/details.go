package drive

import (
	"context"
	"time"

	"github.com/ProtonMail/go-proton-api"

	"celeste/native-go/proton-ext"
)

// FileDetails describes a file's active revision for the conflict dialog and the identical-content check: plaintext size, the modification time the uploader recorded and the SHA-1 of the content, all from the revision's encrypted extended attributes. Revisions without them only carry ModTimeUnix (from the link).
type FileDetails struct {
	Size        int64  `json:"size"`
	HasSize     bool   `json:"has_size"`
	ModTimeUnix int64  `json:"mod_time_unix"`
	SHA1        string `json:"sha1,omitempty"`
}

// FileDetails fetches the link fresh from the API (the conflict check must not see a cached revision) and decrypts its extended attributes. Returns nil without error for folders and inactive links.
func (s *Session) FileDetails(ctx context.Context, linkID string) (*FileDetails, error) {
	link, err := s.refreshLink(ctx, linkID)
	if err != nil {
		return nil, err
	}
	if link.State != proton.LinkStateActive || link.Type != proton.LinkTypeFile || link.FileProperties == nil {
		return nil, nil
	}
	out := &FileDetails{ModTimeUnix: stableModTime(&link)}

	parentKR, err := s.linkKRByID(ctx, link.ParentLinkID)
	if err != nil {
		return out, nil
	}
	nodeKR, err := link.GetKeyRing(parentKR, s.defaultAddrKR)
	if err != nil {
		return out, nil
	}
	rev, err := protonext.GetRevisionXAttr(ctx, s.protonextAuth(), s.mainShare.ShareID, link.LinkID, link.FileProperties.ActiveRevision.ID)
	if err != nil || rev.XAttr == "" {
		return out, nil
	}
	xa, err := protonext.DecryptRevisionXAttr(rev.XAttr, s.defaultAddrKR, nodeKR)
	if err != nil || xa == nil {
		return out, nil
	}
	out.Size, out.HasSize = xa.Size, true
	out.SHA1 = xa.Digests["SHA1"]
	if t, err := time.Parse("2006-01-02T15:04:05-0700", xa.ModificationTime); err == nil {
		out.ModTimeUnix = t.Unix()
	}
	return out, nil
}

// refreshLink fetches a link from the API, bypassing and updating the link cache.
func (s *Session) refreshLink(ctx context.Context, linkID string) (proton.Link, error) {
	link, err := s.c.GetLink(ctx, s.mainShare.ShareID, linkID)
	if err != nil {
		return link, err
	}
	s.linkCache[linkID] = link
	return link, nil
}
