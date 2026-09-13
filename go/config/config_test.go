package config

import (
	"testing"
	"time"
)

func validEnv(t *testing.T) {
	t.Helper()
	t.Setenv("INFLUXDB_URL", "http://localhost:8086")
	t.Setenv("INFLUXDB_TOKEN", "token")
	t.Setenv("INFLUXDB_ORG", "org")
	t.Setenv("DATA_ENCRYPTION_KEY", "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
}

func TestLoadDefaults(t *testing.T) {
	validEnv(t)
	c, err := Load()
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	if c.Host != "0.0.0.0" || c.Port != 4000 {
		t.Errorf("listen defaults: %+v", c)
	}
	if c.InfluxBucket != "tesla" {
		t.Errorf("bucket default: %q", c.InfluxBucket)
	}
	if c.PollInterval != 60*time.Second {
		t.Errorf("poll default: %v", c.PollInterval)
	}
	if c.TeslaAPIClientID != "ownerapi" {
		t.Errorf("client id default: %q", c.TeslaAPIClientID)
	}
	if c.StreamingEnabled {
		t.Errorf("streaming should default off")
	}
}

func TestLoadMissingRequired(t *testing.T) {
	for _, key := range []string{"INFLUXDB_URL", "INFLUXDB_TOKEN", "INFLUXDB_ORG", "DATA_ENCRYPTION_KEY"} {
		t.Run(key, func(t *testing.T) {
			validEnv(t)
			t.Setenv(key, "")
			if _, err := Load(); err == nil {
				t.Errorf("expected error with %s unset", key)
			}
		})
	}
}

func TestLoadValidation(t *testing.T) {
	validEnv(t)
	t.Setenv("INFLUXDB_URL", "localhost:8086")
	if _, err := Load(); err == nil {
		t.Error("expected error for URL without scheme")
	}

	validEnv(t)
	t.Setenv("DATA_ENCRYPTION_KEY", "zzzz")
	if _, err := Load(); err == nil {
		t.Error("expected error for non-hex key")
	}

	validEnv(t)
	t.Setenv("POLL_INTERVAL_SECONDS", "0")
	if _, err := Load(); err == nil {
		t.Error("expected error for zero poll interval")
	}
}
