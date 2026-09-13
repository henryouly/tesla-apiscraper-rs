// Token encryption helpers mirroring Rust encryption.rs: AES-256-GCM with a
// random 12-byte nonce per encryption; wire format is base64(nonce ‖
// ciphertext+16-byte GCM tag). Only the token strings are encrypted;
// expiry stays plaintext in the token file.
package tesla

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/base64"
	"encoding/hex"
	"fmt"
)

// KeyFromHex decodes the 64-char hex DATA_ENCRYPTION_KEY into 32 bytes.
func KeyFromHex(s string) ([32]byte, error) {
	var key [32]byte
	b, err := hex.DecodeString(s)
	if err != nil || len(b) != 32 {
		return key, fmt.Errorf("encryption key must be 64 hex chars")
	}
	copy(key[:], b)
	return key, nil
}

// EncryptToken encrypts one token string with key.
func EncryptToken(key [32]byte, plaintext string) (string, error) {
	block, err := aes.NewCipher(key[:])
	if err != nil {
		return "", err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", err
	}
	nonce := make([]byte, gcm.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		return "", err
	}
	out := gcm.Seal(nonce, nonce, []byte(plaintext), nil)
	return base64.StdEncoding.EncodeToString(out), nil
}

// DecryptToken reverses EncryptToken.
func DecryptToken(key [32]byte, encoded string) (string, error) {
	raw, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		return "", fmt.Errorf("invalid base64 token: %w", err)
	}
	block, err := aes.NewCipher(key[:])
	if err != nil {
		return "", err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", err
	}
	if len(raw) < gcm.NonceSize() {
		return "", fmt.Errorf("token too short")
	}
	nonce, ct := raw[:gcm.NonceSize()], raw[gcm.NonceSize():]
	plain, err := gcm.Open(nil, nonce, ct, nil)
	if err != nil {
		return "", fmt.Errorf("token decrypt failed: %w", err)
	}
	return string(plain), nil
}
