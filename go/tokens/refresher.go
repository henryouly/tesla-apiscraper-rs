// Package tokens owns the Tesla token lifecycle: startup validation and
// background refresh with persistence (mirrors Rust try_use_stored_tokens +
// token_auto_refresh_loop with the shared 3600s staleness rule).
package tokens

import (
	"context"
	"fmt"
	"log/slog"
	"time"

	"github.com/henryouly/tesla-apiscraper-rs/go/tesla"
)

// AuthRefresher is the refresh half of the auth client
// (*tesla.AuthClient satisfies it; fakes stand in for tests).
type AuthRefresher interface {
	RefreshTokens(ctx context.Context, refreshToken string) (tesla.TokenResponse, error)
}

// Store persists token pairs (file-backed in production).
type Store interface {
	Load() (*tesla.StoredTokens, error)
	Save(access, refresh string, expiresAt int64) error
}

// fileStore delegates to the encrypted on-disk token file.
type fileStore struct {
	path string
	key  [32]byte
}

// FileStore builds the production Store over path.
func FileStore(path string, key [32]byte) Store {
	return fileStore{path: path, key: key}
}

func (s fileStore) Load() (*tesla.StoredTokens, error) {
	return tesla.LoadTokens(s.path, s.key)
}

func (s fileStore) Save(access, refresh string, expiresAt int64) error {
	return tesla.SaveTokens(s.path, s.key, access, refresh, expiresAt)
}

// Refresher owns token validity. now is injectable for tests.
type Refresher struct {
	auth     AuthRefresher
	store    Store
	now      func() time.Time
	onUpdate func(string) // invoked with each fresh access token; may be nil
}

// NewRefresher builds a Refresher; a nil now means time.Now.
func NewRefresher(auth AuthRefresher, store Store, now func() time.Time, onUpdate func(string)) *Refresher {
	if now == nil {
		now = time.Now
	}
	return &Refresher{auth: auth, store: store, now: now, onUpdate: onUpdate}
}

// EnsureValid implements the startup rule: use stored tokens when healthy,
// refresh (and persist) when stale, fail when there is nothing to refresh.
func (r *Refresher) EnsureValid(ctx context.Context) (string, error) {
	stored, err := r.store.Load()
	if err != nil {
		return "", fmt.Errorf("token file: %w", err)
	}
	if stored == nil {
		return "", fmt.Errorf("no stored tokens — sign in via the Rust app first (P0 has no sign-in flow)")
	}
	if !tesla.ShouldRefresh(stored.ExpiresAt, r.now().Unix()) {
		return stored.AccessToken, nil
	}
	slog.Info("stored tokens stale, refreshing at startup")
	tokens, err := r.auth.RefreshTokens(ctx, stored.RefreshToken)
	if err != nil {
		return "", fmt.Errorf("startup refresh: %w", err)
	}
	access := tokens.AccessToken
	if err := r.store.Save(access, tokens.RefreshToken, tokens.ExpiresAt(r.now())); err != nil {
		return "", fmt.Errorf("persist tokens: %w", err)
	}
	r.updated(access)
	slog.Info("stored tokens refreshed successfully at startup")
	return access, nil
}

// PollOnce implements one background-loop iteration: skip silently when
// there is nothing to do (missing/corrupt file, healthy tokens); otherwise
// refresh, persist, and broadcast. A refresh failure is returned and leaves
// the current token untouched.
func (r *Refresher) PollOnce(ctx context.Context) error {
	stored, err := r.store.Load()
	if err != nil || stored == nil {
		return nil
	}
	if !tesla.ShouldRefresh(stored.ExpiresAt, r.now().Unix()) {
		return nil
	}
	tokens, err := r.auth.RefreshTokens(ctx, stored.RefreshToken)
	if err != nil {
		return fmt.Errorf("auto-refresh failed: %w", err)
	}
	access := tokens.AccessToken
	if err := r.store.Save(access, tokens.RefreshToken, tokens.ExpiresAt(r.now())); err != nil {
		return fmt.Errorf("persist refreshed tokens failed: %w", err)
	}
	r.updated(access)
	slog.Info("auto-refresh: tokens refreshed successfully")
	return nil
}

func (r *Refresher) updated(access string) {
	if r.onUpdate != nil {
		r.onUpdate(access)
	}
}
