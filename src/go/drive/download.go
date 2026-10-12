package drive

// File download, ported from Proton-API-Bridge/file_download.go with
// three trims:
//
//   1. `FileDownloadReader` becomes a private helper — Syncative downloads
//      whole files to local paths, not partial reads, so we don't need
//      to plumb an `io.ReadCloser` across the FFI boundary.
//   2. Bridge's offset/seek logic is gone for the same reason. If the
//      sync engine ever wants ranged fetches, reintroduce from Bridge.
//   3. `getSignatureVerificationKeyring` replaced with the default
//      address keyring — same single-account assumption folder.go
//      uses. Proton's block signatures are owner-signed.
//
// The new upstream helper `getRevisionAllBlocks` pages through
// `Client.GetRevision` to collect every block of a revision —
// Bridge relied on a flattened view that upstream doesn't expose.

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sync/atomic"
	"time"

	"github.com/ProtonMail/go-proton-api"
	"github.com/ProtonMail/gopenpgp/v2/crypto"

	"syncative/go/proton-ext"
)

// Errors specific to the download path.
var (
	ErrLinkIsNotFile      = errors.New("link is not a file")
	ErrNoActiveRevision   = errors.New("file has no active revision")
	ErrInvalidDestination = errors.New("destination path is empty")
)

// getRevisionPageSize is how many blocks we request per `GetRevision`
// call. Proton's API doesn't document a maximum explicitly; Bridge's
// fork pins 500. Leave generous headroom and we'll tune if we hit
// server-side limits.
const getRevisionPageSize = 500

// blockStallTimeout and blockRetryDelays bound how a block download rides out a network change; see fetchBlock. Right after the change the new network may refuse connections for a few seconds, e.g. while a VPN app rebuilds its tunnel, so the attempts spread over about a minute.
var (
	blockStallTimeout = 30 * time.Second
	blockRetryDelays  = []time.Duration{time.Second, 5 * time.Second, 15 * time.Second, 30 * time.Second}
)

// getRevisionAllBlocks returns a Revision with every block populated,
// paginated by `GetRevision`. Upstream go-proton-api only exposes
// paged access — this is the helper Bridge had built into its own
// API surface that we have to reconstruct.
func (s *Session) getRevisionAllBlocks(
	ctx context.Context,
	linkID, revisionID string,
) (proton.Revision, error) {
	var full proton.Revision
	fromBlock := 1
	for {
		page, err := s.c.GetRevision(ctx, s.mainShare.ShareID, linkID, revisionID, fromBlock, getRevisionPageSize)
		if err != nil {
			return proton.Revision{}, err
		}
		if fromBlock == 1 {
			full = page
			full.Blocks = append([]proton.Block(nil), page.Blocks...)
		} else {
			full.Blocks = append(full.Blocks, page.Blocks...)
		}
		if len(page.Blocks) < getRevisionPageSize {
			break
		}
		fromBlock += len(page.Blocks)
	}
	return full, nil
}

// DownloadFile downloads the active revision of `linkID` to
// `destPath`, creating parent directories as needed. The content goes
// to a hidden `.part` file next to it first (the sync walk skips those)
// and replaces `destPath` only once complete: a download cut off by a
// network change must not leave an empty or half file behind, which
// the next pass would take for a newer local copy and upload.
func (s *Session) DownloadFile(ctx context.Context, linkID, destPath string) error {
	if destPath == "" {
		return ErrInvalidDestination
	}
	link, err := s.getLink(ctx, linkID)
	if err != nil {
		return err
	}
	if link.Type != proton.LinkTypeFile {
		return ErrLinkIsNotFile
	}
	if link.State != proton.LinkStateActive {
		return fmt.Errorf("link state %d is not active", link.State)
	}
	if link.FileProperties == nil {
		return errors.New("link has no file properties")
	}

	parentKR, err := s.linkKRByID(ctx, link.ParentLinkID)
	if err != nil {
		return err
	}
	nodeKR, err := link.GetKeyRing(parentKR, s.defaultAddrKR)
	if err != nil {
		return err
	}
	sessionKey, err := link.GetSessionKey(nodeKR)
	if err != nil {
		return err
	}

	revisionID := link.FileProperties.ActiveRevision.ID
	if revisionID == "" {
		return ErrNoActiveRevision
	}
	revision, err := s.getRevisionAllBlocks(ctx, link.LinkID, revisionID)
	if err != nil {
		return err
	}

	if err := os.MkdirAll(filepath.Dir(destPath), 0o755); err != nil {
		return err
	}
	out, err := os.CreateTemp(filepath.Dir(destPath), ".syncative-*.part")
	if err != nil {
		return err
	}
	partPath := out.Name()
	done := false
	defer func() {
		if !done {
			out.Close()
			os.Remove(partPath)
		}
	}()

	// Decrypt blocks in order, streaming plaintext into `out` via a
	// reusable buffer. gopenpgp takes `io.ReaderFrom` — bytes.Buffer
	// satisfies that; we then copy its contents to the file.
	buf := &bytes.Buffer{}
	var written int64
	for i := range revision.Blocks {
		block := &revision.Blocks[i]
		blockData, err := s.fetchBlock(ctx, block)
		if err != nil {
			return err
		}
		buf.Reset()
		err = decryptBlockIntoBuffer(
			sessionKey,
			s.defaultAddrKR,
			nodeKR,
			block.Hash,
			block.EncSignature,
			buf,
			blockData,
		)
		if err != nil {
			return err
		}
		n, err := buf.WriteTo(out)
		written += n
		if err != nil {
			return err
		}
	}
	// The uploader records the plaintext size; a revision whose blocks don't add up to it is incomplete.
	if size, ok := s.plainSize(ctx, link, revisionID, nodeKR); ok && size != written {
		return fmt.Errorf("download incomplete: got %d of %d bytes", written, size)
	}
	// CreateTemp makes it private; downloads get the usual mode. Android's shared storage ignores modes, so a failure doesn't matter.
	_ = out.Chmod(0o644)
	if err := out.Sync(); err != nil {
		return err
	}
	if err := out.Close(); err != nil {
		return err
	}
	if err := os.Rename(partPath, destPath); err != nil {
		os.Remove(partPath)
		done = true
		return err
	}
	done = true
	return nil
}

// fetchBlock downloads one encrypted block. A network change cuts the connection mid-block ("unexpected EOF") or leaves it open without data, so a block gets several attempts with growing pauses, each given up after blockStallTimeout without a byte.
func (s *Session) fetchBlock(ctx context.Context, block *proton.Block) ([]byte, error) {
	attempts := len(blockRetryDelays) + 1
	for attempt := 1; ; attempt++ {
		data, err := s.fetchBlockOnce(ctx, block)
		if err == nil || ctx.Err() != nil {
			return data, err
		}
		logf("drive: block %d, attempt %d of %d: %v", block.Index, attempt, attempts, err)
		if attempt == attempts {
			return nil, err
		}
		select {
		case <-time.After(blockRetryDelays[attempt-1]):
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}
}

func (s *Session) fetchBlockOnce(ctx context.Context, block *proton.Block) ([]byte, error) {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	body, err := s.c.GetBlock(ctx, block.BareURL, block.Token)
	if err != nil {
		return nil, err
	}
	defer body.Close()
	return readAllStalling(body, blockStallTimeout, cancel)
}

// readAllStalling reads r to the end like io.ReadAll, but calls cancel, which must abort the read, once timeout passes without a byte.
func readAllStalling(r io.Reader, timeout time.Duration, cancel func()) ([]byte, error) {
	var stalled atomic.Bool
	timer := time.AfterFunc(timeout, func() {
		stalled.Store(true)
		cancel()
	})
	defer timer.Stop()
	data, err := io.ReadAll(&progressReader{r: r, progress: func() { timer.Reset(timeout) }})
	if err != nil && stalled.Load() {
		return nil, fmt.Errorf("no data for %v", timeout)
	}
	return data, err
}

type progressReader struct {
	r        io.Reader
	progress func()
}

func (p *progressReader) Read(b []byte) (int, error) {
	n, err := p.r.Read(b)
	if n > 0 {
		p.progress()
	}
	return n, err
}

// plainSize is the plaintext size the uploader recorded in the revision's extended attributes; false when the revision has none.
func (s *Session) plainSize(ctx context.Context, link proton.Link, revisionID string, nodeKR *crypto.KeyRing) (int64, bool) {
	rev, err := protonext.GetRevisionXAttr(ctx, s.protonextAuth(), s.mainShare.ShareID, link.LinkID, revisionID)
	if err != nil || rev.XAttr == "" {
		return 0, false
	}
	xa, err := protonext.DecryptRevisionXAttr(rev.XAttr, s.defaultAddrKR, nodeKR)
	if err != nil || xa == nil {
		return 0, false
	}
	return xa.Size, true
}
