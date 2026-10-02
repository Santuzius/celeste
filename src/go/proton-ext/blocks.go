package protonext

// Block upload with per-block verification. Proton rejects block uploads without a verifier token ("You are using an outdated version of the app", Code=2000) for every client except a few legacy app versions, and upstream's BlockUploadInfo has no field for it. Mirrors the official Drive SDK (client/js/src/internal/upload).

import (
	"context"
	"encoding/base64"

	"github.com/ProtonMail/go-proton-api"
)

// VerificationData is what the server expects block uploads of one draft revision to be checked against.
type VerificationData struct {
	VerificationCode []byte
	ContentKeyPacket []byte
}

// GetVerificationData fetches the verification code and content key packet of a draft revision.
func GetVerificationData(ctx context.Context, auth Auth, volumeID, linkID, revisionID string) (VerificationData, error) {
	var res struct {
		VerificationCode string
		ContentKeyPacket string
	}
	path := "/drive/v2/volumes/" + volumeID + "/links/" + linkID + "/revisions/" + revisionID + "/verification"
	if err := do(ctx, auth, "GET", path, nil, &res); err != nil {
		return VerificationData{}, err
	}
	code, err := base64.StdEncoding.DecodeString(res.VerificationCode)
	if err != nil {
		return VerificationData{}, err
	}
	ckp, err := base64.StdEncoding.DecodeString(res.ContentKeyPacket)
	if err != nil {
		return VerificationData{}, err
	}
	return VerificationData{VerificationCode: code, ContentKeyPacket: ckp}, nil
}

// VerificationToken is the verification code XOR the first bytes of the encrypted block (zero-padded when the block is shorter).
func VerificationToken(code, encryptedBlock []byte) []byte {
	token := make([]byte, len(code))
	for i := range code {
		var b byte
		if i < len(encryptedBlock) {
			b = encryptedBlock[i]
		}
		token[i] = code[i] ^ b
	}
	return token
}

// BlockUploadInfo is upstream's proton.BlockUploadInfo plus the verifier.
type BlockUploadInfo struct {
	Index        int
	Size         int64
	EncSignature string
	Hash         string
	Verifier     BlockVerifier
}

type BlockVerifier struct {
	Token string
}

type BlockUploadReq struct {
	AddressID  string
	ShareID    string
	LinkID     string
	RevisionID string
	BlockList  []BlockUploadInfo
}

// RequestBlockUpload asks for upload URLs for a batch of blocks; the blocks themselves still go through upstream's UploadBlock.
func RequestBlockUpload(ctx context.Context, auth Auth, req BlockUploadReq) ([]proton.BlockUploadLink, error) {
	var res struct {
		UploadLinks []proton.BlockUploadLink
	}
	if err := do(ctx, auth, "POST", "/drive/blocks", req, &res); err != nil {
		return nil, err
	}
	return res.UploadLinks, nil
}
