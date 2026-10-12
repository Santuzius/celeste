package drive

// Pure gopenpgp wrappers for the Proton Drive encryption envelope.
// Ported from Proton-API-Bridge/crypto.go (MIT, henrybear327) — these
// helpers don't depend on any Bridge-specific state, so the port is
// a straight copy with package rename. Upload/download hooks in the
// session grow on top of this file.

import (
	"crypto/sha256"
	"encoding/base64"
	"errors"
	"io"

	"github.com/ProtonMail/gopenpgp/v2/crypto"
	"github.com/ProtonMail/gopenpgp/v2/helper"
)

// ErrDownloadedBlockHashVerificationFailed mirrors Bridge's sentinel —
// surfaces through Phase 3's download path once we wire it.
var ErrDownloadedBlockHashVerificationFailed = errors.New(
	"the hash of the downloaded block doesn't match the original hash",
)

// generatePassphrase returns a base64-encoded 32-byte random token,
// used as the passphrase for freshly-generated node keys.
func generatePassphrase() (string, error) {
	token, err := crypto.RandomToken(32)
	if err != nil {
		return "", err
	}
	return base64.StdEncoding.EncodeToString(token), nil
}

// generateCryptoKey returns (passphrase, armoredKey, error) for a new
// x25519 key — same parameters the Proton iOS Drive client uses.
func generateCryptoKey() (string, string, error) {
	passphrase, err := generatePassphrase()
	if err != nil {
		return "", "", err
	}
	key, err := helper.GenerateKey("Drive key", "noreply@protonmail.com", []byte(passphrase), "x25519", 0)
	if err != nil {
		return "", "", err
	}
	return passphrase, key, nil
}

// encryptWithSignature encrypts `b` with `kr` and detached-signs it with
// `addrKR`. Returns (armoredEnc, armoredSig, error).
func encryptWithSignature(kr, addrKR *crypto.KeyRing, b []byte) (string, string, error) {
	enc, err := kr.Encrypt(crypto.NewPlainMessage(b), nil)
	if err != nil {
		return "", "", err
	}
	encArm, err := enc.GetArmored()
	if err != nil {
		return "", "", err
	}
	sig, err := addrKR.SignDetached(crypto.NewPlainMessage(b))
	if err != nil {
		return "", "", err
	}
	sigArm, err := sig.GetArmored()
	if err != nil {
		return "", "", err
	}
	return encArm, sigArm, nil
}

// generateNodeKeys produces a new (key, passphraseEnc, passphraseSig)
// triple for a new Drive node (file or folder). Passphrase is encrypted
// to `kr` and signed by `addrKR`.
func generateNodeKeys(kr, addrKR *crypto.KeyRing) (string, string, string, error) {
	passphrase, key, err := generateCryptoKey()
	if err != nil {
		return "", "", "", err
	}
	passphraseEnc, passphraseSig, err := encryptWithSignature(kr, addrKR, []byte(passphrase))
	if err != nil {
		return "", "", "", err
	}
	return key, passphraseEnc, passphraseSig, nil
}

// reencryptKeyPacket decrypts `passphrase`'s session key with `srcKR`
// and re-encrypts it to `dstKR`. Used during node moves.
func reencryptKeyPacket(srcKR, dstKR, addrKR *crypto.KeyRing, passphrase string) (string, error) {
	_ = addrKR // kept to match Bridge's signature; not used here
	oldSplit, err := crypto.NewPGPSplitMessageFromArmored(passphrase)
	if err != nil {
		return "", err
	}
	sessionKey, err := srcKR.DecryptSessionKey(oldSplit.KeyPacket)
	if err != nil {
		return "", err
	}
	newKeyPacket, err := dstKR.EncryptSessionKey(sessionKey)
	if err != nil {
		return "", err
	}
	newSplit := crypto.NewPGPSplitMessage(newKeyPacket, oldSplit.DataPacket)
	return newSplit.GetArmored()
}

// getKeyRing unlocks an armoured node key using passphrase decrypted
// through the parent keyring. `addrKR` verifies the signature.
func getKeyRing(kr, addrKR *crypto.KeyRing, key, passphrase, passphraseSignature string) (*crypto.KeyRing, error) {
	enc, err := crypto.NewPGPMessageFromArmored(passphrase)
	if err != nil {
		return nil, err
	}
	dec, err := kr.Decrypt(enc, nil, crypto.GetUnixTime())
	if err != nil {
		return nil, err
	}
	sig, err := crypto.NewPGPSignatureFromArmored(passphraseSignature)
	if err != nil {
		return nil, err
	}
	if err := addrKR.VerifyDetached(dec, sig, crypto.GetUnixTime()); err != nil {
		return nil, err
	}
	lockedKey, err := crypto.NewKeyFromArmored(key)
	if err != nil {
		return nil, err
	}
	unlockedKey, err := lockedKey.Unlock(dec.GetBinary())
	if err != nil {
		return nil, err
	}
	return crypto.NewKeyRing(unlockedKey)
}

// decryptBlockIntoBuffer decrypts a downloaded block with its session
// key, verifies the detached encrypted signature against the node
// keyring, writes the plaintext into `buffer`, and checks the SHA-256
// of the ciphertext against `originalHash`.
func decryptBlockIntoBuffer(
	sessionKey *crypto.SessionKey,
	addrKR, nodeKR *crypto.KeyRing,
	originalHash, encSignature string,
	buffer io.ReaderFrom,
	data []byte,
) error {
	plainMessage, err := sessionKey.Decrypt(data)
	if err != nil {
		return err
	}
	encSignatureArm, err := crypto.NewPGPMessageFromArmored(encSignature)
	if err != nil {
		return err
	}
	if err := addrKR.VerifyDetachedEncrypted(plainMessage, encSignatureArm, nodeKR, crypto.GetUnixTime()); err != nil {
		return err
	}
	if _, err := buffer.ReadFrom(plainMessage.NewReader()); err != nil {
		return err
	}
	h := sha256.New()
	h.Write(data)
	if base64.StdEncoding.EncodeToString(h.Sum(nil)) != originalHash {
		return ErrDownloadedBlockHashVerificationFailed
	}
	return nil
}
