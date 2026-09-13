package main

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/henryouly/tesla-apiscraper-rs/go/tesla"
)

type fakeRefresher struct {
	tokens tesla.TokenResponse
	err    error
	calls  int
	lastRT string
}

func (f *fakeRefresher) RefreshTokens(_ context.Context, rt string) (tesla.TokenResponse, error) {
	f.calls++
	f.lastRT = rt
	return f.tokens, f.err
}

type fakeStore struct {
	stored  *tesla.StoredTokens
	loadErr error
	saved   *tesla.StoredTokens
}

func (f *fakeStore) Load() (*tesla.StoredTokens, error) {
	if f.loadErr != nil {
		return nil, f.loadErr
	}
	return f.stored, nil
}

func (f *fakeStore) Save(access, refresh string, expiresAt int64) error {
	f.saved = &tesla.StoredTokens{AccessToken: access, RefreshToken: refresh, ExpiresAt: expiresAt}
	if f.stored != nil {
		f.stored = &tesla.StoredTokens{AccessToken: access, RefreshToken: refresh, ExpiresAt: expiresAt}
	}
	return nil
}

func testRefresher(store *fakeStore, auth *fakeRefresher) (*Refresher, *string) {
	var updated string
	return &Refresher{
		auth:     auth,
		store:    store,
		now:      func() time.Time { return time.Unix(1700000000, 0) },
		onUpdate: func(a string) { updated = a },
	}, &updated
}

func TestEnsureValidMissing(t *testing.T) {
	r, _ := testRefresher(&fakeStore{}, &fakeRefresher{})
	_, err := r.EnsureValid(context.Background())
	if err == nil {
		t.Fatal("expected error with no stored tokens")
	}
}

func TestEnsureValidHealthy(t *testing.T) {
	auth := &fakeRefresher{}
	store := &fakeStore{stored: &tesla.StoredTokens{AccessToken: "at", RefreshToken: "rt", ExpiresAt: 1700000000 + 7200}}
	r, _ := testRefresher(store, auth)
	got, err := r.EnsureValid(context.Background())
	if err != nil {
		t.Fatalf("EnsureValid: %v", err)
	}
	if got != "at" {
		t.Errorf("got %q", got)
	}
	if auth.calls != 0 {
		t.Errorf("healthy tokens must not refresh (%d calls)", auth.calls)
	}
}

func TestEnsureValidStaleRefreshes(t *testing.T) {
	auth := &fakeRefresher{tokens: tesla.TokenResponse{AccessToken: "new-at", RefreshToken: "new-rt", ExpiresIn: 3600}}
	store := &fakeStore{stored: &tesla.StoredTokens{AccessToken: "old-at", RefreshToken: "old-rt", ExpiresAt: 1700000000 - 10}}
	r, updated := testRefresher(store, auth)
	got, err := r.EnsureValid(context.Background())
	if err != nil {
		t.Fatalf("EnsureValid: %v", err)
	}
	if got != "new-at" {
		t.Errorf("got %q", got)
	}
	if auth.calls != 1 || auth.lastRT != "old-rt" {
		t.Errorf("expected one refresh with old-rt, got %+v", auth)
	}
	if store.saved == nil || store.saved.AccessToken != "new-at" || store.saved.ExpiresAt != 1700000000+3600 {
		t.Errorf("not persisted: %+v", store.saved)
	}
	if *updated != "new-at" {
		t.Errorf("onUpdate not called: %q", *updated)
	}
}

func TestEnsureValidCorrupt(t *testing.T) {
	auth := &fakeRefresher{}
	r, _ := testRefresher(&fakeStore{loadErr: errors.New("bad file")}, auth)
	if _, err := r.EnsureValid(context.Background()); err == nil {
		t.Error("expected error for corrupt store")
	}
	if auth.calls != 0 {
		t.Error("must not refresh on load error")
	}
}

func TestPollOnceHealthySkips(t *testing.T) {
	auth := &fakeRefresher{}
	store := &fakeStore{stored: &tesla.StoredTokens{AccessToken: "at", RefreshToken: "rt", ExpiresAt: 1700000000 + 7200}}
	r, updated := testRefresher(store, auth)
	if err := r.PollOnce(context.Background()); err != nil {
		t.Fatalf("PollOnce: %v", err)
	}
	if auth.calls != 0 || store.saved != nil || *updated != "" {
		t.Errorf("healthy poll must be a no-op: %+v %+v %q", auth, store.saved, *updated)
	}
}

func TestPollOnceMissingSkips(t *testing.T) {
	auth := &fakeRefresher{}
	r, _ := testRefresher(&fakeStore{}, auth)
	if err := r.PollOnce(context.Background()); err != nil {
		t.Fatalf("missing tokens must skip silently, got %v", err)
	}
	if auth.calls != 0 {
		t.Error("must not refresh without stored tokens")
	}
}

func TestPollOnceStaleRefreshes(t *testing.T) {
	auth := &fakeRefresher{tokens: tesla.TokenResponse{AccessToken: "new-at", RefreshToken: "new-rt", ExpiresIn: 3600}}
	store := &fakeStore{stored: &tesla.StoredTokens{AccessToken: "old-at", RefreshToken: "old-rt", ExpiresAt: 1700000000 + 100}}
	r, updated := testRefresher(store, auth)
	if err := r.PollOnce(context.Background()); err != nil {
		t.Fatalf("PollOnce: %v", err)
	}
	if auth.calls != 1 || store.saved == nil || *updated != "new-at" {
		t.Errorf("stale poll must refresh+save+broadcast: %+v %+v %q", auth, store.saved, *updated)
	}
}

func TestPollOnceFailureKeepsOld(t *testing.T) {
	auth := &fakeRefresher{err: errors.New("server down")}
	store := &fakeStore{stored: &tesla.StoredTokens{AccessToken: "old-at", RefreshToken: "old-rt", ExpiresAt: 0}}
	r, updated := testRefresher(store, auth)
	if err := r.PollOnce(context.Background()); err == nil {
		t.Fatal("expected refresh error")
	}
	if store.saved != nil || *updated != "" {
		t.Errorf("failure must not save or broadcast: %+v %q", store.saved, *updated)
	}
}
