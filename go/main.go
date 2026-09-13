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
	refresher := &Refresher{auth: auth, store: fileTokenStore{path: tokenPath, key: key}, now: time.Now}
	access, err := refresher.EnsureValid(ctx)
	if err != nil {
		return err
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

	// Shared current-token holder (watch-channel equivalent), updated by
	// every successful refresh.
	var mu sync.RWMutex
	current := access
	refresher.onUpdate = func(a string) {
		mu.Lock()
		current = a
		mu.Unlock()
	}
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
				if err := refresher.PollOnce(ctx); err != nil {
					slog.Warn("auto-refresh failed", "error", err)
				}
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
