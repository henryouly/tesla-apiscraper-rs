// Package config loads process configuration from environment variables,
// mirroring the Rust implementation's Config surface field-for-field.
// InfluxDB settings are v2-only: URL + token + org + bucket.
//
// NOTE: unlike the Rust app this process does not auto-load a .env file;
// export variables first, e.g. `set -a; source .env; set +a`.
package config

import (
	"fmt"
	"os"
	"strconv"
	"strings"
	"time"
)

// Config mirrors Rust `Config` (src/config.rs). Durations replace the Rust
// *_seconds integers; validation rules match (see Validate).
type Config struct {
	Host string
	Port uint16

	ConfigDir string

	// InfluxDB v2 (replaces the Rust INFLUXDB_USERNAME/PASSWORD/DATABASE).
	InfluxURL    string
	InfluxToken  string
	InfluxOrg    string
	InfluxBucket string

	TeslaAPIClientID string
	TeslaAuthURL     string
	TeslaAPIURL      string

	DataEncryptionKey string // 64 hex chars = 32-byte AES-256 key

	LogLevel string

	PollInterval     time.Duration
	StreamingEnabled bool

	// Retained for surface parity; unused by the P0 telemetry pipeline.
	MQTTPort   uint16
	MQTTHost   string
	GrafanaURL string
	LogFile    string
	LogFormat  string
}

// Load reads the environment, applies defaults, and validates.
func Load() (Config, error) {
	c := Config{
		Host:              envOr("HOST", "0.0.0.0"),
		Port:              uint16(envUint("PORT", 4000)),
		ConfigDir:         envOr("CONFIG_DIR", "config"),
		InfluxURL:         os.Getenv("INFLUXDB_URL"),
		InfluxToken:       os.Getenv("INFLUXDB_TOKEN"),
		InfluxOrg:         os.Getenv("INFLUXDB_ORG"),
		InfluxBucket:      envOr("INFLUXDB_BUCKET", "tesla"),
		TeslaAPIClientID:  envOr("TESLA_API_CLIENT_ID", "ownerapi"),
		TeslaAuthURL:      envOr("TESLA_AUTH_URL", "https://auth.tesla.com"),
		TeslaAPIURL:       envOr("TESLA_API_URL", "https://owner-api.teslamotors.com"),
		DataEncryptionKey: os.Getenv("DATA_ENCRYPTION_KEY"),
		LogLevel:          envOr("LOG_LEVEL", envOr("RUST_LOG", "info")),
		PollInterval:      time.Duration(envUint("POLL_INTERVAL_SECONDS", 60)) * time.Second,
		StreamingEnabled:  envBool("STREAMING_ENABLED"),
		MQTTPort:          uint16(envUint("MQTT_PORT", 1883)),
		MQTTHost:          os.Getenv("MQTT_HOST"),
		GrafanaURL:        os.Getenv("GRAFANA_URL"),
		LogFile:           os.Getenv("LOG_FILE"),
		LogFormat:         envOr("LOG_FORMAT", "text"),
	}
	if err := c.Validate(); err != nil {
		return Config{}, err
	}
	return c, nil
}

// Validate mirrors Rust Config::validate plus the v2 credential requirements
// (v2 has no anonymous access, so token and org are mandatory).
func (c Config) Validate() error {
	var errs []string
	if c.InfluxURL == "" {
		errs = append(errs, "INFLUXDB_URL is required")
	} else if !strings.HasPrefix(c.InfluxURL, "http://") && !strings.HasPrefix(c.InfluxURL, "https://") {
		errs = append(errs, "INFLUXDB_URL must start with http:// or https://")
	}
	if c.InfluxToken == "" {
		errs = append(errs, "INFLUXDB_TOKEN is required (InfluxDB v2 has no anonymous access)")
	}
	if c.InfluxOrg == "" {
		errs = append(errs, "INFLUXDB_ORG is required")
	}
	if c.TeslaAPIClientID == "" {
		errs = append(errs, "TESLA_API_CLIENT_ID is required")
	}
	if len(c.DataEncryptionKey) != 64 || !isHex(c.DataEncryptionKey) {
		errs = append(errs, "DATA_ENCRYPTION_KEY must be 64 hex chars (32-byte AES-256 key)")
	}
	if c.Port == 0 {
		errs = append(errs, "PORT must be non-zero")
	}
	if c.PollInterval < time.Second {
		errs = append(errs, "POLL_INTERVAL_SECONDS must be >= 1")
	}
	if len(errs) > 0 {
		return fmt.Errorf("invalid configuration: %s", strings.Join(errs, "; "))
	}
	return nil
}

func envOr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

func envUint(key string, def uint64) uint64 {
	v := os.Getenv(key)
	if v == "" {
		return def
	}
	n, err := strconv.ParseUint(v, 10, 64)
	if err != nil {
		return def
	}
	return n
}

func envBool(key string) bool {
	v := strings.ToLower(strings.TrimSpace(os.Getenv(key)))
	return v == "true" || v == "1" || v == "yes"
}

func isHex(s string) bool {
	for _, c := range s {
		if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f' || c >= 'A' && c <= 'F') {
			return false
		}
	}
	return true
}
