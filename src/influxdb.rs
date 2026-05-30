#![allow(dead_code)]

use anyhow::{Context, Result};
use influxdb::{InfluxDbWriteable, Query, Timestamp, WriteQuery};

pub struct InfluxDb {
    url: String,
    client: reqwest::Client,
    pub database: String,
}

impl InfluxDb {
    pub fn new(url: &str, token: &str, database: &str) -> Result<Self> {
        let url = url.trim_end_matches('/').to_string();

        let mut headers = reqwest::header::HeaderMap::new();
        let mut auth = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
            .context("INFLUXDB_TOKEN cannot be encoded into an HTTP Authorization header")?;
        auth.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, auth);

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("reqwest::Client builder is infallible with these options");

        Ok(Self {
            url,
            client,
            database: database.to_string(),
        })
    }

    pub async fn ping(&self) -> Result<()> {
        let resp = self
            .client
            .get(format!("{}/ping", self.url))
            .send()
            .await
            .context("InfluxDB ping failed — is the server running?")?;

        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }

        anyhow::bail!("InfluxDB ping failed (HTTP {status})");
    }

    pub async fn ensure_database(&self) -> Result<()> {
        let resp = self
            .client
            .post(format!("{}/api/v3/configure/database", self.url))
            .json(&serde_json::json!({ "db": self.database }))
            .send()
            .await
            .context("failed to send database creation request")?;

        let status = resp.status();
        if status.is_success() || status.as_u16() == 409 {
            return Ok(());
        }

        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("failed to create InfluxDB database (HTTP {status}): {body}");
    }

    pub async fn write_lp(&self, line_protocol: &str) -> Result<()> {
        let url = format!("{}/api/v3/write?db={}&precision=s", self.url, self.database);
        let resp = self
            .client
            .post(&url)
            .header("content-type", "text/plain; charset=utf-8")
            .body(line_protocol.to_string())
            .send()
            .await
            .context("failed to send write request to InfluxDB")?;

        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }

        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("InfluxDB write failed (HTTP {status}): {body}");
    }

    pub async fn write_query(&self, query: influxdb::WriteQuery) -> Result<()> {
        let lp = query
            .build()
            .context("failed to build InfluxDB line protocol")?;
        self.write_lp(&lp.get()).await
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
    pub rated_battery_range_km: Option<f64>,
    pub outside_temp: Option<f64>,
    pub inside_temp: Option<f64>,
    pub heading: Option<i64>,
    pub elevation: Option<f64>,
    pub shift_state: Option<String>,
    pub tpms_pressure_fl: Option<f64>,
    pub tpms_pressure_fr: Option<f64>,
    pub tpms_pressure_rl: Option<f64>,
    pub tpms_pressure_rr: Option<f64>,
    pub fan_status: Option<i64>,
    pub is_front_defroster_on: Option<bool>,
    pub is_rear_defroster_on: Option<bool>,
    pub ideal_battery_range_km: Option<f64>,
    pub est_battery_range_km: Option<f64>,
    pub usable_battery_level: Option<i64>,
    pub is_climate_on: Option<bool>,
    pub driver_temp_setting: Option<f64>,
    pub passenger_temp_setting: Option<f64>,
    pub battery_heater: Option<bool>,
    pub battery_heater_on: Option<bool>,
    pub battery_heater_no_power: Option<bool>,
}

/// Live charge reading sampled during a charging session.
#[derive(Debug, InfluxDbWriteable)]
pub struct ChargeReading {
    pub time: Timestamp,
    #[influxdb(tag)]
    pub vin: String,
    #[influxdb(tag)]
    pub charge_id: String,
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
    pub outside_temp: Option<f64>,
    pub fast_charger_brand: Option<String>,
    pub fast_charger_type: Option<String>,
    pub conn_charge_cable: Option<String>,
    pub usable_battery_level: Option<i64>,
    pub charger_pilot_current: Option<i64>,
    pub fast_charger_present: Option<bool>,
    pub battery_heater_on: Option<bool>,
    pub not_enough_power_to_heat: Option<bool>,
    pub ideal_battery_range: Option<f64>,
    pub rated_battery_range: Option<f64>,
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
    pub outside_temp_avg: Option<f64>,
    pub inside_temp_avg: Option<f64>,
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use influxdb::Query;

    #[test]
    fn position_all_fields_and_tags() {
        let pos = Position {
            time: Timestamp::Hours(42),
            vin: "5YJSA1".into(),
            car_id: 1,
            latitude: 37.7749,
            longitude: -122.4194,
            speed: Some(65.0),
            power: Some(12000),
            odometer: Some(50000.5),
            battery_level: Some(85),
            rated_battery_range_km: Some(270.0),
            outside_temp: Some(22.5),
            inside_temp: Some(24.0),
            heading: Some(180),
            elevation: Some(10.0),
            shift_state: Some("D".into()),
            tpms_pressure_fl: Some(42.0),
            tpms_pressure_fr: Some(41.5),
            tpms_pressure_rl: Some(40.0),
            tpms_pressure_rr: Some(40.5),
            fan_status: Some(5),
            is_front_defroster_on: Some(false),
            is_rear_defroster_on: Some(false),
            ideal_battery_range_km: Some(300.0),
            est_battery_range_km: Some(260.0),
            usable_battery_level: Some(82),
            is_climate_on: Some(true),
            driver_temp_setting: Some(22.0),
            passenger_temp_setting: Some(22.0),
            battery_heater: Some(false),
            battery_heater_on: Some(false),
            battery_heater_no_power: Some(false),
        };

        let lp = pos.into_query("positions").build().unwrap();
        let s = lp.get();
        assert!(s.starts_with("positions,vin=5YJSA1,car_id=1 "));
        assert!(s.contains("latitude=37.7749"));
        assert!(s.contains("speed=65"));
        assert!(s.contains("heading=180i"));
        assert!(s.contains(r#"shift_state="D""#));
        assert!(s.contains("tpms_pressure_fl=42"));
        assert!(s.contains("fan_status=5i"));
        assert!(s.contains("is_front_defroster_on=false"));
        assert!(s.contains("ideal_battery_range_km=300"));
        assert!(s.contains("est_battery_range_km=260"));
        assert!(s.contains("usable_battery_level=82i"));
        assert!(s.contains("is_climate_on=true"));
        assert!(s.ends_with(" 42"), "expected timestamp 42, got: {s:?}");
    }

    #[test]
    fn position_optional_fields_omitted_when_none() {
        let pos = Position {
            time: Timestamp::Hours(1),
            vin: "TEST".into(),
            car_id: 0,
            latitude: 0.0,
            longitude: 0.0,
            speed: None,
            power: None,
            odometer: None,
            battery_level: None,
            rated_battery_range_km: None,
            outside_temp: None,
            inside_temp: None,
            heading: None,
            elevation: None,
            shift_state: None,
            tpms_pressure_fl: None,
            tpms_pressure_fr: None,
            tpms_pressure_rl: None,
            tpms_pressure_rr: None,
            fan_status: None,
            is_front_defroster_on: None,
            is_rear_defroster_on: None,
            ideal_battery_range_km: None,
            est_battery_range_km: None,
            usable_battery_level: None,
            is_climate_on: None,
            driver_temp_setting: None,
            passenger_temp_setting: None,
            battery_heater: None,
            battery_heater_on: None,
            battery_heater_no_power: None,
        };

        let lp = pos.into_query("positions").build().unwrap();
        let s = lp.get();
        assert!(s.starts_with("positions,vin=TEST,car_id=0 "));
        assert!(s.contains("latitude=0"));
        assert!(s.contains("longitude=0"));
        assert!(!s.contains("speed="), "unexpected speed: {s:?}");
        assert!(!s.contains("heading="), "unexpected heading: {s:?}");
        assert!(!s.contains("tpms_pressure_"), "unexpected tpms: {s:?}");
        assert!(!s.contains("fan_status="), "unexpected fan_status: {s:?}");
        assert!(!s.contains("defroster"), "unexpected defroster: {s:?}");
    }

    #[test]
    fn drive_start_has_required_fields() {
        let drive = Drive {
            time: Timestamp::Hours(100),
            vin: "VIN1".into(),
            drive_id: "drive-001".into(),
            start_lat: 37.77,
            start_lng: -122.42,
            end_lat: None,
            end_lng: None,
            start_address: None,
            end_address: None,
            start_time: None,
            end_time: None,
            distance_meters: None,
            duration_seconds: None,
            energy_used_wh: None,
            max_speed: None,
            average_speed: None,
            outside_temp_avg: None,
            inside_temp_avg: None,
            geofence_enter: None,
            geofence_exit: None,
            is_merged: None,
        };

        let lp = drive.into_query("drives").build().unwrap();
        let s = lp.get();
        assert!(s.starts_with("drives,vin=VIN1,drive_id=drive-001 "));
        assert!(s.contains("start_lat="));
        assert!(s.contains("start_lng="));
        assert!(!s.contains("end_lat="));
        assert!(!s.contains("end_lng="));
    }

    #[test]
    fn charge_reading_serializes() {
        let cr = ChargeReading {
            time: Timestamp::Hours(5),
            vin: "VIN1".into(),
            charge_id: "ch-1".into(),
            voltage: Some(230.0),
            current: Some(16.0),
            power: Some(3680.0),
            phases: Some(1),
            energy_added: Some(5.2),
            battery_level: Some(50),
            battery_range: Some(150.0),
            charger_power: Some(7000),
            charger_voltage: Some(230),
            charger_phases: Some(1),
            outside_temp: Some(22.5),
            fast_charger_brand: Some("Tesla".into()),
            fast_charger_type: Some("Supercharger".into()),
            conn_charge_cable: Some("CCS".into()),
            usable_battery_level: Some(48),
            charger_pilot_current: Some(32),
            fast_charger_present: Some(true),
            battery_heater_on: Some(false),
            not_enough_power_to_heat: Some(false),
            ideal_battery_range: Some(300.0),
            rated_battery_range: Some(270.0),
        };

        let lp = cr.into_query("charge_readings").build().unwrap();
        let s = lp.get();
        assert!(s.starts_with("charge_readings,vin=VIN1,charge_id=ch-1 "));
        assert!(s.contains("voltage=230"));
        assert!(s.contains("phases=1i"));
        assert!(s.contains("outside_temp=22.5"));
        assert!(s.contains(r#"fast_charger_brand="Tesla""#));
        assert!(s.contains("usable_battery_level=48i"));
        assert!(s.contains("battery_heater_on=false"));
    }

    #[test]
    fn vehicle_state_serializes() {
        let vs = VehicleState {
            time: Timestamp::Seconds(12345),
            vin: "VIN1".into(),
            state: "online".into(),
            inside_temp: Some(23.0),
            outside_temp: Some(15.0),
            battery_level: Some(80),
            locked: Some(true),
            sentry_mode: Some(false),
            dog_mode: None,
            cabin_overheat_protection: None,
        };

        let lp = vs.into_query("states").build().unwrap();
        let s = lp.get();
        assert!(s.starts_with("states,vin=VIN1 "));
        assert!(s.contains("state="));
        assert!(s.contains("locked=true"));
        assert!(!s.contains("dog_mode="));
    }

    #[test]
    fn update_serializes() {
        let up = Update {
            time: Timestamp::Hours(200),
            vin: "VIN1".into(),
            update_id: "up-1".into(),
            version_before: Some("2024.8".into()),
            version_after: None,
            install_start: Some("2024-03-01T00:00:00Z".into()),
            install_end: None,
            status: Some("pending".into()),
            abandoned: None,
        };

        let lp = up.into_query("updates").build().unwrap();
        let s = lp.get();
        assert!(s.starts_with("updates,vin=VIN1,update_id=up-1 "));
        assert!(s.contains(r#"version_before="2024.8""#));
        assert!(!s.contains("version_after="));
        assert!(s.contains(r#"install_start="2024-03-01T00:00:00Z""#));
    }

    #[test]
    fn write_query_helper_creates_valid_query() {
        let q = write_query("test", Timestamp::Hours(1))
            .add_field("value", 42i64)
            .add_tag("tag1", "val1")
            .build()
            .unwrap();
        assert_eq!(q.get(), "test,tag1=val1 value=42i 1");
    }

    #[test]
    fn charging_session_serializes() {
        let cs = ChargingSession {
            time: Timestamp::Hours(10),
            vin: "VIN1".into(),
            charge_id: "ch-1".into(),
            start_lat: 37.77,
            start_lng: -122.42,
            end_lat: Some(37.77),
            end_lng: Some(-122.42),
            start_range: Some(50.0),
            end_range: Some(250.0),
            start_battery_level: Some(10),
            end_battery_level: Some(90),
            energy_added_wh: Some(50000.0),
            duration_seconds: Some(3600),
            cost: Some(6.50),
            geofence_id: Some("gf-home".into()),
            geofence_name: Some("Home".into()),
            charge_energy_used: Some(55000.0),
            connector_type: Some("CCS".into()),
            outside_temp_avg: Some(22.5),
            inside_temp_avg: Some(24.0),
        };

        let lp = cs.into_query("charging_sessions").build().unwrap();
        let s = lp.get();
        assert!(s.starts_with("charging_sessions,vin=VIN1,charge_id=ch-1 "));
        assert!(s.contains("start_lat=37.77"));
        assert!(s.contains("cost=6.5"));
        assert!(s.contains("outside_temp_avg=22.5"));
    }

    #[tokio::test]
    async fn ping_success() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/ping"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let db = InfluxDb::new(&server.uri(), "token", "test_db").unwrap();
        assert!(db.ping().await.is_ok());
    }

    #[tokio::test]
    async fn ping_failure() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/ping"))
            .respond_with(wiremock::ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let db = InfluxDb::new(&server.uri(), "token", "test_db").unwrap();
        assert!(db.ping().await.is_err());
    }

    #[tokio::test]
    async fn ensure_database_success() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/configure/database"))
            .and(wiremock::matchers::body_json(
                serde_json::json!({ "db": "my_db" }),
            ))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let db = InfluxDb::new(&server.uri(), "token", "my_db").unwrap();
        assert!(db.ensure_database().await.is_ok());
    }

    #[tokio::test]
    async fn ensure_database_already_exists() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/configure/database"))
            .respond_with(wiremock::ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let db = InfluxDb::new(&server.uri(), "token", "my_db").unwrap();
        assert!(db.ensure_database().await.is_ok());
    }

    #[tokio::test]
    async fn ensure_database_failure() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/configure/database"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let db = InfluxDb::new(&server.uri(), "token", "my_db").unwrap();
        assert!(db.ensure_database().await.is_err());
    }

    #[tokio::test]
    async fn write_lp_success() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .and(wiremock::matchers::query_param("db", "my_db"))
            .and(wiremock::matchers::query_param("precision", "s"))
            .and(wiremock::matchers::header(
                "content-type",
                "text/plain; charset=utf-8",
            ))
            .and(wiremock::matchers::body_string("test,tag=a value=1i 100"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let db = InfluxDb::new(&server.uri(), "token", "my_db").unwrap();
        db.write_lp("test,tag=a value=1i 100").await.unwrap();
    }

    #[tokio::test]
    async fn write_lp_server_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let db = InfluxDb::new(&server.uri(), "token", "my_db").unwrap();
        assert!(db.write_lp("test value=1 0").await.is_err());
    }
}
