// Command tesla-port is the Go port's P0 daemon: authenticate, discover
// vehicles, poll each one on its state cadence, and write positions to
// InfluxDB v2. Sessions (drives/charges/updates), enrichment, and streaming
// arrive in later phases; the process already runs them as no-ops.
package main

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"os/signal"
	"path/filepath"
	"sync"
	"syscall"
	"time"

	"github.com/henryouly/tesla-apiscraper-rs/go/config"
	"github.com/henryouly/tesla-apiscraper-rs/go/store"
	"github.com/henryouly/tesla-apiscraper-rs/go/tesla"
	"github.com/henryouly/tesla-apiscraper-rs/go/vehicles"
)

func main() {
	if err := run(); err != nil {
		slog.Error("fatal", "error", err)
		os.Exit(1)
	}
}

func run() error {
	cfg, err := config.Load()
	if err != nil {
		return err
	}
	setupLogging(cfg.LogLevel)

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	// InfluxDB v2: ping/ready, no database creation step (bucket pre-exists).
	db := store.New(cfg.InfluxURL, cfg.InfluxToken, cfg.InfluxOrg, cfg.InfluxBucket)
	defer db.Close()
	if err := db.Ping(ctx); err != nil {
		return err
	}
	slog.Info("InfluxDB connection OK", "bucket", cfg.InfluxBucket)

	key, err := tesla.KeyFromHex(cfg.DataEncryptionKey)
	if err != nil {
		return err
	}
	tokenPath := filepath.Join(cfg.ConfigDir, "tokens.json")
	auth := tesla.NewAuthClient(cfg.TeslaAPIClientID, cfg.TeslaAuthURL, cfg.TeslaAPIURL)
	api := tesla.NewAPIClient()

	// Stored tokens: use if healthy, else refresh at startup (mirrors Rust
	// try_use_stored_tokens with the 3600s threshold).
	access, refresh := "", ""
	if stored, err := tesla.LoadTokens(tokenPath, key); err != nil {
		return fmt.Errorf("token file: %w", err)
	} else if stored != nil {
		access, refresh = stored.AccessToken, stored.RefreshToken
		if tesla.ShouldRefresh(stored.ExpiresAt, time.Now().Unix()) {
			slog.Info("stored tokens stale, refreshing at startup")
			access, refresh = "", ""
		}
	}
	if access == "" {
		if refresh == "" {
			return fmt.Errorf("no stored tokens — sign in via the Rust app first (P0 has no sign-in flow)")
		}
		tokens, err := auth.RefreshTokens(ctx, refresh)
		if err != nil {
			return fmt.Errorf("startup refresh: %w", err)
		}
		access, refresh = tokens.AccessToken, tokens.RefreshToken
		if err := tesla.SaveTokens(tokenPath, key, access, refresh, tokens.ExpiresAt(time.Now())); err != nil {
			return fmt.Errorf("persist tokens: %w", err)
		}
		slog.Info("stored tokens refreshed successfully at startup")
	}

	// Region-aware discovery (mirrors discover_vehicles).
	region, err := auth.DecodeRegion(access)
	if err != nil {
		slog.Warn("region decode failed, using default API URL", "error", err)
		region = tesla.Region{APIURL: cfg.TeslaAPIURL}
	}
	products, err := api.ListProducts(ctx, access, region.APIURL)
	if err != nil {
		return fmt.Errorf("vehicle discovery: %w", err)
	}
	slog.Info("vehicle discovery complete", "vehicle_count", len(products))

	// Shared current-token holder (watch-channel equivalent).
	var mu sync.RWMutex
	current := access
	tokenOf := func() string {
		mu.RLock()
		defer mu.RUnlock()
		return current
	}

	vm := vehicles.NewSupervisor(region.APIURL, api, db, cfg.PollInterval)
	vm.SpawnAll(ctx, products, tokenOf)
	slog.Info("vehicle state machines started", "vehicle_count", len(products))

	// Background auto-refresh every 60s when within 3600s of expiry
	// (mirrors token_auto_refresh_loop).
	go func() {
		ticker := time.NewTicker(60 * time.Second)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				stored, err := tesla.LoadTokens(tokenPath, key)
				if err != nil || stored == nil {
					continue
				}
				if !tesla.ShouldRefresh(stored.ExpiresAt, time.Now().Unix()) {
					continue
				}
				tokens, err := auth.RefreshTokens(ctx, stored.RefreshToken)
				if err != nil {
					slog.Warn("auto-refresh failed", "error", err)
					continue
				}
				if err := tesla.SaveTokens(tokenPath, key, tokens.AccessToken, tokens.RefreshToken, tokens.ExpiresAt(time.Now())); err != nil {
					slog.Warn("persist refreshed tokens failed", "error", err)
					continue
				}
				mu.Lock()
				current = tokens.AccessToken
				mu.Unlock()
				slog.Info("auto-refresh: tokens refreshed successfully")
			}
		}
	}()

	<-ctx.Done()
	slog.Info("shutting down vehicle state machines")
	vm.ShutdownAll()
	db.Flush()
	slog.Info("shutdown complete")
	return nil
}

func setupLogging(level string) {
	var l slog.Level
	switch level {
	case "debug":
		l = slog.LevelDebug
	case "warn":
		l = slog.LevelWarn
	case "error":
		l = slog.LevelError
	default:
		l = slog.LevelInfo
	}
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: l})))
}
