package tesla

import (
	"strings"
	"testing"
)

func testKey(t *testing.T) [32]byte {
	t.Helper()
	key, err := KeyFromHex("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
	if err != nil {
		t.Fatalf("KeyFromHex: %v", err)
	}
	return key
}

func TestKeyFromHexRejects(t *testing.T) {
	for _, s := range []string{"", "zzzz", "abcd", strings.Repeat("ab", 31), strings.Repeat("ab", 33)} {
		if _, err := KeyFromHex(s); err == nil {
			t.Errorf("expected error for %q", s)
		}
	}
}

func TestEncryptDecryptRoundTrip(t *testing.T) {
	key := testKey(t)
	enc1, err := EncryptToken(key, "secret-token")
	if err != nil {
		t.Fatalf("EncryptToken: %v", err)
	}
	enc2, err := EncryptToken(key, "secret-token")
	if err != nil {
		t.Fatalf("EncryptToken: %v", err)
	}
	if enc1 == enc2 {
		t.Error("expected random nonce to differ between encryptions")
	}
	dec, err := DecryptToken(key, enc1)
	if err != nil {
		t.Fatalf("DecryptToken: %v", err)
	}
	if dec != "secret-token" {
		t.Errorf("round trip = %q", dec)
	}
}

func TestDecryptWrongKeyFails(t *testing.T) {
	key := testKey(t)
	enc, err := EncryptToken(key, "secret-token")
	if err != nil {
		t.Fatal(err)
	}
	var wrong [32]byte
	if _, err := DecryptToken(wrong, enc); err == nil {
		t.Error("expected auth failure with wrong key")
	}
	if _, err := DecryptToken(key, "!!!not-base64!!!"); err == nil {
		t.Error("expected error for invalid base64")
	}
}
