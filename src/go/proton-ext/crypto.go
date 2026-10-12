package protonext

// Crypto helpers Syncative needs around the upstream go-proton-api types.
// Kept out-of-tree so the proton-api submodule can stay pristine. All
// helpers here are thin wrappers over gopenpgp + crypto/hmac that the
// upstream package intentionally doesn't expose.

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"

	"github.com/ProtonMail/gopenpgp/v2/crypto"
)

// EncryptName encrypts a plaintext name with the parent folder's node
// keyring and signs it with the address keyring. Returns the armored
// PGP string the Proton API expects.
func EncryptName(name string, addrKR, nodeKR *crypto.KeyRing) (string, error) {
	plain := crypto.NewPlainMessageFromString(name)
	enc, err := nodeKR.Encrypt(plain, addrKR)
	if err != nil {
		return "", err
	}
	return enc.GetArmored()
}

// NameHash returns hex(HMAC-SHA256(name, hashKey)). Proton uses this to
// detect filename conflicts in a folder without revealing the plaintext
// name.
func NameHash(name string, hashKey []byte) (string, error) {
	mac := hmac.New(sha256.New, hashKey)
	if _, err := mac.Write([]byte(name)); err != nil {
		return "", err
	}
	return hex.EncodeToString(mac.Sum(nil)), nil
}

// EncryptNodeHashKey generates a fresh 32-byte token, encrypts it to the
// parent's node keyring, and returns the armored result. The token is
// later used to hash names of children created inside this folder.
func EncryptNodeHashKey(parentNodeKR *crypto.KeyRing) (string, error) {
	token, err := crypto.RandomToken(32)
	if err != nil {
		return "", err
	}
	enc, err := parentNodeKR.Encrypt(crypto.NewPlainMessage(token), parentNodeKR)
	if err != nil {
		return "", err
	}
	return enc.GetArmored()
}

// GenerateFileSessionKey produces a new session key, encrypts it to the
// node keyring, and detached-signs the raw key bytes with that same
// keyring. Returns the session key (so the caller can use it to encrypt
// the file's blocks), the base64-encoded encrypted key packet, and the
// armored signature.
func GenerateFileSessionKey(nodeKR *crypto.KeyRing) (sessionKey *crypto.SessionKey, contentKeyPacketB64, signatureArmored string, err error) {
	sk, err := crypto.GenerateSessionKey()
	if err != nil {
		return nil, "", "", err
	}
	encKey, err := nodeKR.EncryptSessionKey(sk)
	if err != nil {
		return nil, "", "", err
	}
	sig, err := nodeKR.SignDetached(crypto.NewPlainMessage(sk.Key))
	if err != nil {
		return nil, "", "", err
	}
	sigArm, err := sig.GetArmored()
	if err != nil {
		return nil, "", "", err
	}
	return sk, base64.StdEncoding.EncodeToString(encKey), sigArm, nil
}
