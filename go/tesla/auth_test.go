package tesla

import (
	"context"
	"encoding/base64"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func makeJWT(t *testing.T, payload string) string {
	t.Helper()
	enc := base64.RawURLEncoding.EncodeToString([]byte(payload))
	return "header." + enc + ".signature"
}

func TestDecodeRegion(t *testing.T) {
	c := NewAuthClient("ownerapi", "https://auth.tesla.com", "https://default.api")
	cases := []struct {
		name string
		aud  string
		want string
	}{
		{"na", `{"aud":"https://owner-api.teslamotors.com"}`, "https://owner-api.teslamotors.com"},
		{"cn", `{"aud":"https://owner-api.vn.cloud.tesla.cn"}`, "https://owner-api.vn.cloud.tesla.cn"},
		{"eu", `{"aud":"https://owner-api.vn.cloud.tesla.eu"}`, "https://owner-api.vn.cloud.tesla.eu"},
		{"default", `{"sub":"abc"}`, "https://default.api"},
		{"unknown", `{"aud":"https://strange.example.com"}`, "https://default.api"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			region, err := c.DecodeRegion(makeJWT(t, tc.aud))
			if err != nil {
				t.Fatalf("DecodeRegion: %v", err)
			}
			if region.APIURL != tc.want {
				t.Errorf("got %q want %q", region.APIURL, tc.want)
			}
		})
	}
}

func TestDecodeRegionInvalid(t *testing.T) {
	c := NewAuthClient("ownerapi", "https://auth.tesla.com", "https://default.api")
	if _, err := c.DecodeRegion("not-a-jwt"); err == nil {
		t.Error("expected error for malformed JWT")
	}
	if _, err := c.DecodeRegion("a.!!!.c"); err == nil {
		t.Error("expected error for invalid base64")
	}
}

func TestRefreshRetriesThenSucceeds(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if n := calls.Add(1); n < 3 {
			w.WriteHeader(http.StatusBadGateway)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		fmt.Fprint(w, `{"access_token":"at","refresh_token":"rt","expires_in":3600,"token_type":"Bearer"}`)
	}))
	defer server.Close()

	c := NewAuthClient("ownerapi", server.URL, "https://default.api")
	tokens, err := c.RefreshTokens(context.Background(), "old-rt")
	if err != nil {
		t.Fatalf("RefreshTokens: %v", err)
	}
	if tokens.AccessToken != "at" || tokens.ExpiresIn != 3600 {
		t.Errorf("unexpected tokens: %+v", tokens)
	}
	if calls.Load() != 3 {
		t.Errorf("expected 3 calls, got %d", calls.Load())
	}
}

func TestRefreshClientErrorNoRetry(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		w.WriteHeader(http.StatusBadRequest)
		fmt.Fprint(w, `{"error":"invalid_grant","error_description":"expired"}`)
	}))
	defer server.Close()

	c := NewAuthClient("ownerapi", server.URL, "https://default.api")
	_, err := c.RefreshTokens(context.Background(), "bad-rt")
	apiErr, ok := err.(*APIError)
	if !ok || apiErr.Status != 400 {
		t.Fatalf("expected Api 400 error, got %v", err)
	}
	if calls.Load() != 1 {
		t.Errorf("must not retry 4xx, got %d calls", calls.Load())
	}
}

func TestShouldRefresh(t *testing.T) {
	now := time.Now().Unix()
	if !ShouldRefresh(now-10, now) {
		t.Error("expired should refresh")
	}
	if !ShouldRefresh(now+3599, now) {
		t.Error("within 1h should refresh")
	}
	if ShouldRefresh(now+3601, now) {
		t.Error("healthy token should not refresh")
	}
}

func TestTokenFileRoundTrip(t *testing.T) {
	key := testKey(t)
	path := filepath.Join(t.TempDir(), "tokens.json")
	got, err := LoadTokens(path, key)
	if err != nil || got != nil {
		t.Fatalf("missing file should return nil,nil: %v %+v", got, err)
	}
	exp := time.Now().Add(time.Hour).Unix()
	if err := SaveTokens(path, key, "at", "rt", exp); err != nil {
		t.Fatalf("SaveTokens: %v", err)
	}
	// Ciphertext must not contain plaintext.
	raw, _ := os.ReadFile(path)
	for _, s := range []string{`"at"`, `"rt"`} {
		if strings.Contains(string(raw), s) {
			t.Errorf("token file leaks plaintext %s", s)
		}
	}
	got, err = LoadTokens(path, key)
	if err != nil {
		t.Fatalf("LoadTokens: %v", err)
	}
	if got.AccessToken != "at" || got.RefreshToken != "rt" || got.ExpiresAt != exp {
		t.Errorf("round trip mismatch: %+v", got)
	}
}
