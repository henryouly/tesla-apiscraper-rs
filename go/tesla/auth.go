// Package tesla ports the Rust Tesla auth client (tesla_auth.rs): OAuth
// refresh-token flow with retry, JWT region decode, and the encrypted
// on-disk token file.
package tesla

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// MaxRetries mirrors Rust MAX_RETRIES; backoff doubles from 1s (120s cap is
// unreachable at 3 attempts but kept for parity).
const MaxRetries = 3

// TokenResponse mirrors the Tesla token endpoint JSON.
type TokenResponse struct {
	AccessToken  string `json:"access_token"`
	RefreshToken string `json:"refresh_token"`
	ExpiresIn    int64  `json:"expires_in"`
	TokenType    string `json:"token_type"`
}

// ExpiresAt mirrors Rust TokenResponse::expires_at (now + expires_in).
func (r TokenResponse) ExpiresAt(now time.Time) int64 {
	return now.Unix() + r.ExpiresIn
}

// StoredTokens is the on-disk form: encrypted tokens, plaintext expiry
// (mirrors tokens.yml).
type StoredTokens struct {
	AccessToken  string `json:"access_token"`
	RefreshToken string `json:"refresh_token"`
	ExpiresAt    int64  `json:"expires_at"`
}

// APIError mirrors Rust AuthError::Api{status, body}.
type APIError struct {
	Status int
	Body   string
}

func (e *APIError) Error() string {
	return fmt.Sprintf("tesla api error %d: %s", e.Status, e.Body)
}

// AuthClient mirrors Rust TeslaAuthClient.
type AuthClient struct {
	ClientID string
	AuthURL  string // e.g. https://auth.tesla.com
	APIURL   string // default owner API, region fallback
	HTTP     *http.Client
}

// NewAuthClient builds a client with a 30s HTTP timeout.
func NewAuthClient(clientID, authURL, apiURL string) *AuthClient {
	return &AuthClient{
		ClientID: clientID,
		AuthURL:  strings.TrimSuffix(authURL, "/"),
		APIURL:   strings.TrimSuffix(apiURL, "/"),
		HTTP:     &http.Client{Timeout: 30 * time.Second},
	}
}

// SignIn signs in with an existing refresh token (delegates to RefreshTokens;
// there is no password flow — mirrors Rust sign_in).
func (c *AuthClient) SignIn(ctx context.Context, refreshToken string) (TokenResponse, error) {
	return c.RefreshTokens(ctx, refreshToken)
}

// RefreshTokens runs the grant_type=refresh_token flow, retrying transport
// errors and 5xx up to MaxRetries with doubling backoff.
func (c *AuthClient) RefreshTokens(ctx context.Context, refreshToken string) (TokenResponse, error) {
	form := url.Values{}
	form.Set("grant_type", "refresh_token")
	form.Set("client_id", c.ClientID)
	form.Set("refresh_token", refreshToken)
	form.Set("scope", "openid email offline_access")

	var lastErr error
	backoff := time.Second
	for attempt := 0; attempt <= MaxRetries; attempt++ {
		if attempt > 0 {
			select {
			case <-ctx.Done():
				return TokenResponse{}, ctx.Err()
			case <-time.After(backoff):
			}
			backoff *= 2
			if backoff > 120*time.Second {
				backoff = 120 * time.Second
			}
		}
		tokens, retryable, err := c.refreshOnce(ctx, form)
		if err == nil {
			return tokens, nil
		}
		lastErr = err
		if !retryable {
			return TokenResponse{}, err
		}
		slog.Warn("token refresh failed, retrying", "attempt", attempt, "error", err)
	}
	return TokenResponse{}, lastErr
}

// refreshOnce performs a single token request. retryable reports whether the
// caller should retry (transport error or 5xx).
func (c *AuthClient) refreshOnce(ctx context.Context, form url.Values) (TokenResponse, bool, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost,
		c.AuthURL+"/oauth2/v3/token", strings.NewReader(form.Encode()))
	if err != nil {
		return TokenResponse{}, false, err
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")

	resp, err := c.HTTP.Do(req)
	if err != nil {
		return TokenResponse{}, true, err
	}
	defer resp.Body.Close()

	if resp.StatusCode >= 500 {
		return TokenResponse{}, true, &APIError{Status: resp.StatusCode, Body: "token endpoint unavailable"}
	}
	if resp.StatusCode != http.StatusOK {
		var body struct {
			Error            string `json:"error"`
			ErrorDescription string `json:"error_description"`
		}
		_ = json.NewDecoder(resp.Body).Decode(&body)
		detail := body.Error
		if body.ErrorDescription != "" {
			detail += ": " + body.ErrorDescription
		}
		return TokenResponse{}, false, &APIError{Status: resp.StatusCode, Body: detail}
	}
	var tokens TokenResponse
	if err := json.NewDecoder(resp.Body).Decode(&tokens); err != nil {
		return TokenResponse{}, false, fmt.Errorf("invalid token response: %w", err)
	}
	return tokens, false, nil
}

// Region mirrors Rust Region: the owner-API base URL for a JWT audience.
type Region struct {
	APIURL string
}

// DecodeRegion extracts the region from an access-token JWT (manual base64
// decode of the payload segment — no JWT library, mirrors Rust).
func (c *AuthClient) DecodeRegion(accessToken string) (Region, error) {
	parts := strings.Split(accessToken, ".")
	if len(parts) != 3 {
		return Region{}, fmt.Errorf("not-a-jwt: expected 3 segments")
	}
	raw, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err != nil {
		return Region{}, fmt.Errorf("invalid base64url payload: %w", err)
	}
	var payload struct {
		Aud string `json:"aud"`
	}
	// An empty/non-JSON payload falls back to the default URL (mirrors Rust
	// from_jwt_payload defaulting aud to "").
	aud := ""
	if json.Unmarshal(raw, &payload) == nil {
		aud = payload.Aud
	}
	var apiURL string
	switch {
	case strings.HasSuffix(aud, ".cn") || strings.Contains(aud, ".cn/"):
		apiURL = "https://owner-api.vn.cloud.tesla.cn"
	case strings.HasSuffix(aud, ".eu") || strings.Contains(aud, ".eu/"):
		apiURL = "https://owner-api.vn.cloud.tesla.eu"
	case strings.Contains(aud, "owner-api"):
		apiURL = "https://owner-api.teslamotors.com"
	default:
		apiURL = c.APIURL
	}
	return Region{APIURL: apiURL}, nil
}

// ShouldRefresh mirrors the Rust startup/loop rule: refresh when expired or
// within 3600s of expiry.
func ShouldRefresh(expiresAt, nowUnix int64) bool {
	return expiresAt-nowUnix <= 3600
}

// LoadTokens reads and decrypts the token file. Missing file returns
// (nil, nil) — tokens.yml is optional until sign-in.
func LoadTokens(path string, key [32]byte) (*StoredTokens, error) {
	raw, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	var stored StoredTokens
	if err := json.Unmarshal(raw, &stored); err != nil {
		return nil, fmt.Errorf("invalid token file: %w", err)
	}
	access, err := DecryptToken(key, stored.AccessToken)
	if err != nil {
		return nil, fmt.Errorf("decrypt access token: %w", err)
	}
	refresh, err := DecryptToken(key, stored.RefreshToken)
	if err != nil {
		return nil, fmt.Errorf("decrypt refresh token: %w", err)
	}
	return &StoredTokens{AccessToken: access, RefreshToken: refresh, ExpiresAt: stored.ExpiresAt}, nil
}

// SaveTokens encrypts and atomically writes the token file (temp + rename,
// mirrors Rust save_tokens).
func SaveTokens(path string, key [32]byte, access, refresh string, expiresAt int64) error {
	encAccess, err := EncryptToken(key, access)
	if err != nil {
		return err
	}
	encRefresh, err := EncryptToken(key, refresh)
	if err != nil {
		return err
	}
	raw, err := json.Marshal(StoredTokens{AccessToken: encAccess, RefreshToken: encRefresh, ExpiresAt: expiresAt})
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, raw, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}
