use anyhow::{Context, Result};
use figment::{Figment, providers::Env};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
#[expect(dead_code)]
pub struct Config {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_config_dir")]
    pub config_dir: PathBuf,

    pub influxdb_url: String,
    pub influxdb_token: String,
    #[serde(default = "default_influxdb_database")]
    pub influxdb_database: String,

    pub tesla_api_client_id: String,
    pub tesla_api_client_secret: String,
    #[serde(default = "default_tesla_auth_url")]
    pub tesla_auth_url: String,
    #[serde(default = "default_tesla_api_url")]
    pub tesla_api_url: String,

    pub data_encryption_key: String,

    #[serde(default = "default_rust_log")]
    pub rust_log: String,
    #[serde(default = "default_log_format")]
    pub log_format: String,

    pub mqtt_host: Option<String>,
    #[serde(default = "default_mqtt_port")]
    pub mqtt_port: u16,

    #[serde(default = "default_poll_interval_seconds")]
    pub poll_interval_seconds: u64,

    #[serde(default)]
    pub streaming_enabled: bool,

    pub grafana_url: Option<String>,
}

fn default_host() -> String {
    "0.0.0.0".into()
}
fn default_port() -> u16 {
    4000
}
fn default_config_dir() -> PathBuf {
    PathBuf::from("config")
}
fn default_influxdb_database() -> String {
    "tesla".into()
}
fn default_tesla_auth_url() -> String {
    "https://auth.tesla.com".into()
}
fn default_tesla_api_url() -> String {
    "https://fleet-api.prd.na.vn.cloud.tesla.com".into()
}
fn default_rust_log() -> String {
    "info".into()
}
fn default_log_format() -> String {
    "text".into()
}
fn default_mqtt_port() -> u16 {
    1883
}
fn default_poll_interval_seconds() -> u64 {
    30
}

/// Checks whether `s` is a valid hex string of exactly `byte_len * 2` characters.
fn is_valid_hex(s: &str, byte_len: usize) -> bool {
    s.len() == byte_len * 2 && s.chars().all(|c| c.is_ascii_hexdigit())
}

impl Config {
    pub fn load() -> Result<Self> {
        let _ = dotenvy::dotenv();

        let config: Config = Figment::new()
            .merge(Env::raw())
            .extract()
            .context("failed to parse configuration from environment variables")?;

        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        let mut errors: Vec<String> = Vec::new();

        if self.influxdb_url.is_empty() {
            errors.push("INFLUXDB_URL is required".into());
        } else if !self.influxdb_url.starts_with("http://")
            && !self.influxdb_url.starts_with("https://")
        {
            errors.push("INFLUXDB_URL must start with http:// or https://".into());
        }
        if self.influxdb_token.is_empty() {
            errors.push("INFLUXDB_TOKEN is required".into());
        }
        if self.tesla_api_client_id.is_empty() {
            errors.push("TESLA_API_CLIENT_ID is required".into());
        }
        if self.tesla_api_client_secret.is_empty() {
            errors.push("TESLA_API_CLIENT_SECRET is required".into());
        }
        if self.data_encryption_key.is_empty() {
            errors.push("DATA_ENCRYPTION_KEY is required".into());
        } else if !is_valid_hex(&self.data_encryption_key, 32) {
            errors.push(
                "DATA_ENCRYPTION_KEY must be 64 hexadecimal characters (32 bytes for AES-256-GCM)"
                    .into(),
            );
        }

        if self.port == 0 {
            errors.push("PORT must be between 1 and 65535".into());
        }
        if self.poll_interval_seconds == 0 {
            errors.push("POLL_INTERVAL_SECONDS must be >= 1".into());
        }
        if let Some(ref host) = self.mqtt_host
            && host.is_empty()
        {
            errors.push("MQTT_HOST must not be empty if set".into());
        }
        if let Some(ref url) = self.grafana_url
            && !url.starts_with("http://")
            && !url.starts_with("https://")
        {
            errors.push("GRAFANA_URL must start with http:// or https://".into());
        }

        if !errors.is_empty() {
            anyhow::bail!(
                "configuration validation failed:\n  {}\n\n\
                 See .env.example for all available options.",
                errors.join("\n  ")
            );
        }

        Ok(())
    }

    pub fn listen_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> Config {
        Config {
            host: default_host(),
            port: default_port(),
            config_dir: default_config_dir(),
            influxdb_url: "http://localhost:8086".into(),
            influxdb_token: "my-token".into(),
            influxdb_database: default_influxdb_database(),
            tesla_api_client_id: "client-id".into(),
            tesla_api_client_secret: "client-secret".into(),
            tesla_auth_url: default_tesla_auth_url(),
            tesla_api_url: default_tesla_api_url(),
            data_encryption_key: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .into(),
            rust_log: default_rust_log(),
            log_format: default_log_format(),
            mqtt_host: None,
            mqtt_port: default_mqtt_port(),
            poll_interval_seconds: default_poll_interval_seconds(),
            streaming_enabled: false,
            grafana_url: None,
        }
    }

    #[test]
    fn valid_config_passes() {
        let config = valid_config();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn requires_influxdb_url() {
        let mut c = valid_config();
        c.influxdb_url = "".into();
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("INFLUXDB_URL"));
    }

    #[test]
    fn influxdb_url_must_have_scheme() {
        let mut c = valid_config();
        c.influxdb_url = "localhost:8086".into();
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("http:// or https://"));
    }

    #[test]
    fn influxdb_url_accepts_https() {
        let mut c = valid_config();
        c.influxdb_url = "https://influxdb.example.com".into();
        assert!(c.validate().is_ok());
    }

    #[test]
    fn requires_influxdb_token() {
        let mut c = valid_config();
        c.influxdb_token = "".into();
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("INFLUXDB_TOKEN"));
    }

    #[test]
    fn requires_tesla_api_client_id() {
        let mut c = valid_config();
        c.tesla_api_client_id = "".into();
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("TESLA_API_CLIENT_ID"));
    }

    #[test]
    fn requires_tesla_api_client_secret() {
        let mut c = valid_config();
        c.tesla_api_client_secret = "".into();
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("TESLA_API_CLIENT_SECRET"));
    }

    #[test]
    fn requires_data_encryption_key() {
        let mut c = valid_config();
        c.data_encryption_key = "".into();
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("DATA_ENCRYPTION_KEY"));
    }

    #[test]
    fn encryption_key_must_be_64_hex_chars() {
        let mut c = valid_config();
        c.data_encryption_key = "not-a-hex-key".into();
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("DATA_ENCRYPTION_KEY"));
        assert!(err.contains("64 hexadecimal"));
    }

    #[test]
    fn encryption_key_rejects_invalid_hex() {
        let mut c = valid_config();
        c.data_encryption_key =
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".into();
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("DATA_ENCRYPTION_KEY"));
    }

    #[test]
    fn port_zero_rejected() {
        let mut c = valid_config();
        c.port = 0;
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("PORT"));
    }

    #[test]
    fn poll_interval_zero_rejected() {
        let mut c = valid_config();
        c.poll_interval_seconds = 0;
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("POLL_INTERVAL_SECONDS"));
    }

    #[test]
    fn mqtt_host_empty_rejected() {
        let mut c = valid_config();
        c.mqtt_host = Some("".into());
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("MQTT_HOST"));
    }

    #[test]
    fn empty_grafana_url_rejected() {
        let mut c = valid_config();
        c.grafana_url = Some("".into());
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("GRAFANA_URL"));
    }

    #[test]
    fn grafana_url_needs_scheme() {
        let mut c = valid_config();
        c.grafana_url = Some("localhost:3000".into());
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("http:// or https://"));
    }

    #[test]
    fn valid_grafana_url_passes() {
        let mut c = valid_config();
        c.grafana_url = Some("https://grafana.example.com".into());
        assert!(c.validate().is_ok());
    }

    #[test]
    fn valid_mqtt_host_passes() {
        let mut c = valid_config();
        c.mqtt_host = Some("mqtt.example.com".into());
        assert!(c.validate().is_ok());
    }

    #[test]
    fn listen_addr_format() {
        let c = valid_config();
        assert_eq!(c.listen_addr(), "0.0.0.0:4000");
    }

    #[test]
    fn hex_validation_rejects_short() {
        assert!(!is_valid_hex("abcdef", 32));
    }

    #[test]
    fn hex_validation_accepts_correct() {
        assert!(is_valid_hex(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            32
        ));
    }

    #[test]
    fn hex_validation_rejects_non_hex() {
        assert!(!is_valid_hex(
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
            32
        ));
    }

    #[test]
    fn all_errors_reported_at_once() {
        let c = Config {
            influxdb_url: "bad-url".into(),
            influxdb_token: "".into(),
            tesla_api_client_id: "".into(),
            tesla_api_client_secret: "".into(),
            data_encryption_key: "short".into(),
            port: 0,
            poll_interval_seconds: 0,
            ..valid_config()
        };
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("INFLUXDB_URL"));
        assert!(err.contains("INFLUXDB_TOKEN"));
        assert!(err.contains("TESLA_API_CLIENT_ID"));
        assert!(err.contains("TESLA_API_CLIENT_SECRET"));
        assert!(err.contains("DATA_ENCRYPTION_KEY"));
        assert!(err.contains("PORT"));
        assert!(err.contains("POLL_INTERVAL_SECONDS"));
    }

    #[test]
    fn config_dir_defaults_to_config() {
        assert_eq!(default_config_dir(), std::path::PathBuf::from("config"));
    }
}
