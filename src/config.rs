use anyhow::{Context, Result};
use figment::{Figment, providers::Env};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[expect(dead_code)]
pub struct Config {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,

    pub database_url: String,
    pub influxdb_url: String,
    pub influxdb_token: String,
    #[serde(default = "default_influxdb_org")]
    pub influxdb_org: String,
    #[serde(default = "default_influxdb_bucket")]
    pub influxdb_bucket: String,

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
fn default_influxdb_org() -> String {
    "tesla".into()
}
fn default_influxdb_bucket() -> String {
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
        let mut missing: Vec<&str> = Vec::new();

        if self.database_url.is_empty() {
            missing.push("DATABASE_URL");
        }
        if self.influxdb_url.is_empty() {
            missing.push("INFLUXDB_URL");
        }
        if self.influxdb_token.is_empty() {
            missing.push("INFLUXDB_TOKEN");
        }
        if self.tesla_api_client_id.is_empty() {
            missing.push("TESLA_API_CLIENT_ID");
        }
        if self.tesla_api_client_secret.is_empty() {
            missing.push("TESLA_API_CLIENT_SECRET");
        }
        if self.data_encryption_key.is_empty() {
            missing.push("DATA_ENCRYPTION_KEY");
        }

        if !missing.is_empty() {
            anyhow::bail!(
                "missing required environment variables:\n  {}\n\n\
                 See .env.example for all available options.",
                missing.join("\n  ")
            );
        }

        Ok(())
    }

    pub fn listen_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}
