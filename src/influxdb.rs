#![allow(dead_code)]

use anyhow::{Context, Result};
use influxdb::{Client, InfluxDbWriteable, Timestamp, WriteQuery};

// ---------------------------------------------------------------------------
// Client wrapper
// ---------------------------------------------------------------------------

pub struct InfluxDb {
    client: Client,
    url: String,
    token: String,
    pub bucket: String,
}

impl InfluxDb {
    pub fn new(url: &str, token: &str, _org: &str, bucket: &str) -> Self {
        let client = Client::new(url, token);
        Self {
            client,
            url: url.to_string(),
            token: token.to_string(),
            bucket: bucket.to_string(),
        }
    }

    pub async fn ping(&self) -> Result<()> {
        let (_version, _body) = self
            .client
            .ping()
            .await
            .context("InfluxDB ping failed — is the server running?")?;
        Ok(())
    }

    /// Create the configured bucket if it does not already exist.
    pub async fn ensure_bucket(&self) -> Result<()> {
        let http = reqwest::Client::new();
        let resp = http
            .post(format!("{}/api/v2/buckets", self.url))
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&serde_json::json!({
                "name": self.bucket,
                "orgID": "",
                "retentionRules": []
            }))
            .send()
            .await
            .context("failed to send bucket creation request")?;

        let status = resp.status();
        if status.is_success() || status.as_u16() == 422 {
            return Ok(());
        }

        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("failed to create InfluxDB bucket (HTTP {status}): {body}");
    }
}

// ---------------------------------------------------------------------------
// Measurement schemas
// ---------------------------------------------------------------------------

/// Per-second GPS position and vehicle state snapshot.
#[derive(Debug, InfluxDbWriteable)]
pub struct Position {
    pub time: Timestamp,
    #[influxdb(tag)]
    pub vin: String,
    #[influxdb(tag)]
    pub car_id: i64,
    pub latitude: f64,
    pub longitude: f64,
    pub speed: Option<f64>,
    pub power: Option<i64>,
    pub odometer: Option<f64>,
    pub battery_level: Option<i64>,
    pub battery_range: Option<f64>,
    pub outside_temp: Option<f64>,
    pub inside_temp: Option<f64>,
    pub heading: Option<i64>,
    pub elevation: Option<f64>,
    pub shift_state: Option<String>,
}

/// Live charge reading sampled during a charging session.
#[derive(Debug, InfluxDbWriteable)]
pub struct ChargeReading {
    pub time: Timestamp,
    #[influxdb(tag)]
    pub vin: String,
    pub voltage: Option<f64>,
    pub current: Option<f64>,
    pub power: Option<f64>,
    pub phases: Option<i64>,
    pub energy_added: Option<f64>,
    pub battery_level: Option<i64>,
    pub battery_range: Option<f64>,
    pub charger_power: Option<i64>,
    pub charger_voltage: Option<i64>,
    pub charger_phases: Option<i64>,
}

/// Drive event (upserted on close — see update-on-close pattern).
#[derive(Debug, InfluxDbWriteable)]
pub struct Drive {
    pub time: Timestamp,
    #[influxdb(tag)]
    pub vin: String,
    #[influxdb(tag)]
    pub drive_id: String,
    pub start_lat: f64,
    pub start_lng: f64,
    pub end_lat: Option<f64>,
    pub end_lng: Option<f64>,
    pub start_address: Option<String>,
    pub end_address: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub distance_meters: Option<f64>,
    pub duration_seconds: Option<i64>,
    pub energy_used_wh: Option<f64>,
    pub max_speed: Option<f64>,
    pub average_speed: Option<f64>,
    pub outside_temp_avg: Option<f64>,
    pub inside_temp_avg: Option<f64>,
    pub geofence_enter: Option<String>,
    pub geofence_exit: Option<String>,
    pub is_merged: Option<bool>,
}

/// Charging session (upserted on close — same pattern as [`Drive`]).
#[derive(Debug, InfluxDbWriteable)]
pub struct ChargingSession {
    pub time: Timestamp,
    #[influxdb(tag)]
    pub vin: String,
    #[influxdb(tag)]
    pub charge_id: String,
    pub start_lat: f64,
    pub start_lng: f64,
    pub end_lat: Option<f64>,
    pub end_lng: Option<f64>,
    pub start_range: Option<f64>,
    pub end_range: Option<f64>,
    pub start_battery_level: Option<i64>,
    pub end_battery_level: Option<i64>,
    pub energy_added_wh: Option<f64>,
    pub duration_seconds: Option<i64>,
    pub cost: Option<f64>,
    pub geofence_id: Option<String>,
    pub geofence_name: Option<String>,
    pub charge_energy_used: Option<f64>,
    pub connector_type: Option<String>,
}

/// Vehicle online/offline/asleep state transitions.
#[derive(Debug, InfluxDbWriteable)]
pub struct VehicleState {
    pub time: Timestamp,
    #[influxdb(tag)]
    pub vin: String,
    pub state: String,
    pub inside_temp: Option<f64>,
    pub outside_temp: Option<f64>,
    pub battery_level: Option<i64>,
    pub locked: Option<bool>,
    pub sentry_mode: Option<bool>,
    pub dog_mode: Option<bool>,
    pub cabin_overheat_protection: Option<bool>,
}

/// Software update tracking.
#[derive(Debug, InfluxDbWriteable)]
pub struct Update {
    pub time: Timestamp,
    #[influxdb(tag)]
    pub vin: String,
    #[influxdb(tag)]
    pub update_id: String,
    pub version_before: Option<String>,
    pub version_after: Option<String>,
    pub install_start: Option<String>,
    pub install_end: Option<String>,
    pub status: Option<String>,
    pub abandoned: Option<bool>,
}

/// Build a [`WriteQuery`] for a measurement (used by update-on-close pattern).
pub fn write_query(measurement: &str, timestamp: Timestamp) -> WriteQuery {
    WriteQuery::new(timestamp, measurement)
}
