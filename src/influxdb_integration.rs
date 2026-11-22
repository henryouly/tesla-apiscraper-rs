//! Real InfluxDB 1.8 integration tests covering all database operations.
//!
//! Every test is marked `#[ignore]` so `cargo test` stays hermetic without a
//! running InfluxDB instance. In CI (and locally with Docker), run:
//!
//! ```bash
//! INFLUXDB_TEST_URL=http://127.0.0.1:8086 cargo test influxdb_integration -- --ignored
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::api::{create_router, test_helpers};
use crate::influxdb::InfluxDb;

#[allow(dead_code)]
static DB_COUNTER: AtomicU64 = AtomicU64::new(0);

#[allow(dead_code)]
fn test_influxdb_url() -> String {
    std::env::var("INFLUXDB_TEST_URL")
        .or_else(|_| std::env::var("INFLUXDB_URL"))
        .unwrap_or_else(|_| "http://127.0.0.1:8086".to_string())
}

/// Isolated InfluxDB 1.8 database created per test and dropped via `cleanup`.
#[allow(dead_code)]
struct TestDb {
    url: String,
    name: String,
    db: Arc<InfluxDb>,
}

#[allow(dead_code)]
impl TestDb {
    async fn new(prefix: &str) -> Self {
        let url = test_influxdb_url();
        let seq = DB_COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!("it_{prefix}_{}_{}", std::process::id(), seq);
        let db = Arc::new(InfluxDb::new(&url, "", "", &name).expect("valid InfluxDb config"));
        db.ensure_database()
            .await
            .expect("CREATE DATABASE failed against InfluxDB 1.8");
        Self { url, name, db }
    }

    fn authed_app(&self) -> Router {
        let mut state = test_helpers::test_state_authed();
        state.db = Arc::clone(&self.db);
        create_router(state)
    }

    async fn cleanup(self) {
        let q = format!("DROP DATABASE \"{}\"", self.name.replace('"', "\\\""));
        let _ = reqwest::Client::new()
            .post(format!("{}/query", self.url.trim_end_matches('/')))
            .form(&[("q", q)])
            .send()
            .await;
    }
}

#[allow(dead_code)]
async fn get_json(app: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or_else(|e| {
        panic!(
            "expected JSON from {uri} (status {status}), got error {e}: {}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, json)
}

#[allow(dead_code)]
async fn put_json(
    app: &Router,
    uri: &str,
    payload: &serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or_else(|e| {
        panic!(
            "expected JSON from PUT {uri} (status {status}), got error {e}: {}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, json)
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn ping_ensure_database_and_health_ready() {
    let tdb = TestDb::new("ping_health").await;

    tdb.db.ping().await.expect("ping should succeed");
    // Second ensure_database call must be idempotent on InfluxDB 1.8.
    tdb.db
        .ensure_database()
        .await
        .expect("idempotent CREATE DATABASE should succeed");

    // Verify database actually exists in InfluxDB 1.8.
    let show = tdb
        .db
        .query("SHOW DATABASES", "ms")
        .await
        .expect("SHOW DATABASES should succeed");
    let names: Vec<&str> = show["results"][0]["series"][0]["values"]
        .as_array()
        .expect("SHOW DATABASES values")
        .iter()
        .filter_map(|row| row[0].as_str())
        .collect();
    assert!(
        names.contains(&tdb.name.as_str()),
        "database {} not found in {names:?}",
        tdb.name
    );

    // GET /health/ready hits db.ping() and returns 200 OK.
    let app = tdb.authed_app();
    let (status, json) = get_json(&app, "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert!(json.get("error").is_none());

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn positions_write_and_last_known_summary() {
    use crate::influxdb::{Position, Precision};
    use crate::tesla_api::Vehicle;
    use crate::vehicle_summary::last_known_summary;
    use crate::vehicles::VehicleState;
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("positions").await;

    // 1. Poll-style position at Precision::Seconds (car_id = 1) with all fields populated.
    let ts_sec: u128 = 1_700_000_000;
    let poll_pos = Position {
        time: Timestamp::Seconds(ts_sec),
        vin: "VINPOS01".into(),
        car_id: 1,
        latitude: Some(37.7749),
        longitude: Some(-122.4194),
        speed: Some(45.5),
        power: Some(15),
        odometer: Some(12345.6),
        battery_level: Some(80),
        rated_battery_range_km: Some(310.0),
        outside_temp: Some(21.5),
        inside_temp: Some(22.0),
        heading: Some(180),
        elevation: Some(12.5),
        shift_state: Some("D".into()),
        tpms_pressure_fl: Some(2.9),
        tpms_pressure_fr: Some(2.9),
        tpms_pressure_rl: Some(2.8),
        tpms_pressure_rr: Some(2.8),
        fan_status: Some(3),
        is_front_defroster_on: Some(false),
        is_rear_defroster_on: Some(true),
        ideal_battery_range_km: Some(340.0),
        est_battery_range_km: Some(295.0),
        usable_battery_level: Some(78),
        is_climate_on: Some(true),
        driver_temp_setting: Some(21.0),
        passenger_temp_setting: Some(21.5),
        battery_heater: Some(false),
        battery_heater_on: Some(false),
        battery_heater_no_power: Some(false),
        is_preconditioning: Some(false),
        climate_keeper_mode: Some("off".into()),
        locked: Some(true),
        is_user_present: Some(true),
        sentry_mode: Some(false),
    };
    tdb.db
        .write_query(poll_pos.into_query("positions"), Precision::Seconds)
        .await
        .expect("seconds position write should succeed");

    // 2. Streaming-style position at Precision::Milliseconds (same second + 500ms, different car_id series).
    let stream_pos = Position {
        time: Timestamp::Milliseconds(ts_sec * 1000 + 500),
        vin: "VINPOS01".into(),
        car_id: 2,
        latitude: Some(37.7800),
        longitude: Some(-122.4100),
        speed: Some(62.0),
        power: Some(22),
        odometer: Some(12346.2),
        battery_level: Some(79),
        rated_battery_range_km: None,
        outside_temp: None,
        inside_temp: None,
        heading: Some(90),
        elevation: Some(18.0),
        shift_state: Some("D".into()),
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
        is_preconditioning: None,
        climate_keeper_mode: None,
        locked: None,
        is_user_present: None,
        sentry_mode: None,
    };
    tdb.db
        .write_query(stream_pos.into_query("positions"), Precision::Milliseconds)
        .await
        .expect("milliseconds position write should succeed");

    // Verify full round-trip on the seconds row.
    let res = tdb
        .db
        .query(
            "SELECT * FROM positions WHERE vin='VINPOS01' AND car_id='1'",
            "ms",
        )
        .await
        .expect("query positions should succeed");
    let series = &res["results"][0]["series"][0];
    let cols = series["columns"].as_array().unwrap();
    let row = &series["values"][0];
    let col = |name: &str| {
        let idx = cols.iter().position(|c| c == name).unwrap();
        row[idx].clone()
    };
    assert_eq!(col("battery_level"), serde_json::json!(80));
    assert_eq!(col("shift_state"), serde_json::json!("D"));
    assert_eq!(col("is_rear_defroster_on"), serde_json::json!(true));
    assert_eq!(col("sentry_mode"), serde_json::json!(false));
    assert_eq!(col("climate_keeper_mode"), serde_json::json!("off"));

    // Verify last_known_summary picks the newer ms-precision row across car_id series.
    let vehicle = Vehicle {
        id: 1,
        vehicle_id: 100,
        vin: "VINPOS01".into(),
        display_name: Some("Model 3".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let summary = last_known_summary(&tdb.db, &vehicle, VehicleState::Online)
        .await
        .expect("last_known_summary should find latest position");
    assert_eq!(summary.battery_level, Some(79));
    assert_eq!(summary.speed, Some(62.0));
    assert_eq!(summary.odometer, Some(12346.2));
    assert_eq!(summary.last_updated_at, ts_sec as i64);

    // Unknown VIN returns None.
    let missing = Vehicle {
        vin: "UNKNOWN_VIN".into(),
        ..vehicle
    };
    assert!(
        last_known_summary(&tdb.db, &missing, VehicleState::Offline)
            .await
            .is_none()
    );

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn states_write_and_db_writer_flush() {
    use crate::influxdb::{Precision, VehicleState as StatePoint};
    use crate::vehicles::db_writer::DbWriter;
    use influxdb::{InfluxDbWriteable, Query, Timestamp};

    let tdb = TestDb::new("states_writer").await;
    let writer = DbWriter::new(Arc::clone(&tdb.db), 16);

    let s1 = StatePoint {
        time: Timestamp::Seconds(1_700_000_100),
        vin: "VINSTATE1".into(),
        state: "online".into(),
        inside_temp: Some(21.0),
        outside_temp: Some(18.5),
        battery_level: Some(82),
        locked: Some(true),
        sentry_mode: Some(false),
        dog_mode: Some(false),
        cabin_overheat_protection: Some(true),
    };
    let s2 = StatePoint {
        time: Timestamp::Seconds(1_700_000_200),
        vin: "VINSTATE1".into(),
        state: "driving".into(),
        inside_temp: Some(22.0),
        outside_temp: Some(18.0),
        battery_level: Some(81),
        locked: Some(true),
        sentry_mode: Some(false),
        dog_mode: Some(false),
        cabin_overheat_protection: Some(false),
    };
    let s3 = StatePoint {
        time: Timestamp::Seconds(1_700_000_300),
        vin: "VINSTATE1".into(),
        state: "asleep".into(),
        inside_temp: None,
        outside_temp: None,
        battery_level: Some(80),
        locked: Some(true),
        sentry_mode: Some(false),
        dog_mode: None,
        cabin_overheat_protection: None,
    };

    assert!(writer.send_telemetry(
        s1.into_query("states").build().unwrap().get(),
        Precision::Seconds,
    ));
    assert!(
        writer
            .send_session(
                s2.into_query("states").build().unwrap().get(),
                Precision::Seconds,
            )
            .await
    );
    writer.flush().await;

    // Session records must also survive shutdown().
    assert!(
        writer
            .send_session(
                s3.into_query("states").build().unwrap().get(),
                Precision::Seconds,
            )
            .await
    );
    writer.shutdown().await;

    let res = tdb
        .db
        .query(
            "SELECT state, battery_level, locked, cabin_overheat_protection FROM states WHERE vin='VINSTATE1' ORDER BY time ASC",
            "s",
        )
        .await
        .expect("query states should succeed");
    let rows = res["results"][0]["series"][0]["values"]
        .as_array()
        .expect("states rows");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][1], serde_json::json!("online"));
    assert_eq!(rows[0][4], serde_json::json!(true));
    assert_eq!(rows[1][1], serde_json::json!("driving"));
    assert_eq!(rows[1][2], serde_json::json!(81));
    assert_eq!(rows[2][1], serde_json::json!("asleep"));
    assert_eq!(rows[2][2], serde_json::json!(80));

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn drives_write_and_upsert_on_close() {
    use crate::influxdb::{Drive, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("drives").await;
    let start_ts: u128 = 1_700_001_000;

    // Open drive point (initial write on drive start).
    let open = Drive {
        time: Timestamp::Seconds(start_ts),
        vin: "VINDRV01".into(),
        drive_id: "drv-1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: None,
        end_lng: None,
        start_address: Some("San Francisco, CA".into()),
        end_address: None,
        start_time: Some("2023-11-14T22:30:00Z".into()),
        end_time: None,
        distance_meters: None,
        duration_seconds: None,
        energy_used_wh: None,
        max_speed: None,
        average_speed: None,
        outside_temp_avg: None,
        inside_temp_avg: None,
        geofence_enter: Some("Home".into()),
        geofence_exit: None,
        is_merged: Some(false),
    };
    tdb.db
        .write_query(open.into_query("drives"), Precision::Seconds)
        .await
        .expect("open drive write should succeed");

    // Close drive point (same timestamp + tags -> InfluxDB 1.8 upserts in place).
    let closed = Drive {
        time: Timestamp::Seconds(start_ts),
        vin: "VINDRV01".into(),
        drive_id: "drv-1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: Some(37.8044),
        end_lng: Some(-122.2712),
        start_address: Some("San Francisco, CA".into()),
        end_address: Some("Oakland, CA".into()),
        start_time: Some("2023-11-14T22:30:00Z".into()),
        end_time: Some("2023-11-14T23:00:00Z".into()),
        distance_meters: Some(18500.0),
        duration_seconds: Some(1800),
        energy_used_wh: Some(3120.5),
        max_speed: Some(72.0),
        average_speed: Some(37.0),
        outside_temp_avg: Some(19.5),
        inside_temp_avg: Some(21.5),
        geofence_enter: Some("Home".into()),
        geofence_exit: Some("Office".into()),
        is_merged: Some(false),
    };
    tdb.db
        .write_query(closed.into_query("drives"), Precision::Seconds)
        .await
        .expect("closed drive upsert should succeed");

    let res = tdb
        .db
        .query("SELECT * FROM drives WHERE drive_id='drv-1'", "s")
        .await
        .expect("query drives should succeed");
    let series = &res["results"][0]["series"][0];
    let rows = series["values"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "upsert must keep a single point");

    let cols = series["columns"].as_array().unwrap();
    let col = |name: &str| {
        let idx = cols.iter().position(|c| c == name).unwrap();
        rows[0][idx].clone()
    };
    assert_eq!(col("distance_meters").as_f64(), Some(18500.0));
    assert_eq!(col("duration_seconds"), serde_json::json!(1800));
    assert_eq!(col("energy_used_wh").as_f64(), Some(3120.5));
    assert_eq!(col("geofence_enter"), serde_json::json!("Home"));
    assert_eq!(col("geofence_exit"), serde_json::json!("Office"));
    assert_eq!(col("end_address"), serde_json::json!("Oakland, CA"));
    assert_eq!(col("is_merged"), serde_json::json!(false));

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn charging_sessions_and_readings_upsert() {
    use crate::influxdb::{ChargeReading, ChargingSession, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("charging").await;
    let start_ts: u128 = 1_700_002_000;

    // Open charging session point.
    let open = ChargingSession {
        time: Timestamp::Seconds(start_ts),
        vin: "VINCHG01".into(),
        charge_id: "chg-1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: None,
        end_lng: None,
        start_address: Some("Home".into()),
        start_range: Some(120.0),
        end_range: None,
        start_rated_range: Some(115.0),
        end_rated_range: None,
        start_battery_level: Some(30),
        end_battery_level: None,
        energy_added_wh: None,
        duration_seconds: None,
        cost: None,
        geofence_id: Some("home".into()),
        geofence_name: Some("Home".into()),
        charge_energy_used: None,
        connector_type: Some("SAE".into()),
        outside_temp_avg: None,
        inside_temp_avg: None,
    };
    tdb.db
        .write_query(open.into_query("charging_sessions"), Precision::Seconds)
        .await
        .expect("open charging session write should succeed");

    // Live charge readings during session.
    for (offset, soc, kw, added) in [(60u128, 45i64, 11.2f64, 10.5f64), (120, 60, 11.0, 21.0)] {
        let reading = ChargeReading {
            time: Timestamp::Seconds(start_ts + offset),
            vin: "VINCHG01".into(),
            charge_id: "chg-1".into(),
            voltage: Some(240.0),
            current: Some(48.0),
            power: Some(kw),
            phases: Some(1),
            energy_added: Some(added),
            battery_level: Some(soc),
            battery_range: Some(180.0),
            charger_power: Some(11),
            charger_voltage: Some(240),
            charger_phases: Some(1),
            outside_temp: Some(18.0),
            fast_charger_brand: Some("<invalid>".into()),
            fast_charger_type: Some("ACSingleWireCAN".into()),
            conn_charge_cable: Some("SAE".into()),
            usable_battery_level: Some(soc - 1),
            charger_pilot_current: Some(48),
            fast_charger_present: Some(false),
            battery_heater_on: Some(false),
            not_enough_power_to_heat: Some(false),
            ideal_battery_range: Some(190.0),
            rated_battery_range: Some(180.0),
        };
        tdb.db
            .write_query(reading.into_query("charge_readings"), Precision::Seconds)
            .await
            .expect("charge_readings write should succeed");
    }

    // Close charging session (upsert same timestamp + tags).
    let closed = ChargingSession {
        time: Timestamp::Seconds(start_ts),
        vin: "VINCHG01".into(),
        charge_id: "chg-1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: Some(37.7749),
        end_lng: Some(-122.4194),
        start_address: Some("Home".into()),
        start_range: Some(120.0),
        end_range: Some(260.0),
        start_rated_range: Some(115.0),
        end_rated_range: Some(250.0),
        start_battery_level: Some(30),
        end_battery_level: Some(80),
        energy_added_wh: Some(35000.0),
        duration_seconds: Some(3600),
        cost: Some(8.75),
        geofence_id: Some("home".into()),
        geofence_name: Some("Home".into()),
        charge_energy_used: Some(37500.0),
        connector_type: Some("SAE".into()),
        outside_temp_avg: Some(18.0),
        inside_temp_avg: Some(20.0),
    };
    tdb.db
        .write_query(closed.into_query("charging_sessions"), Precision::Seconds)
        .await
        .expect("closed charging session upsert should succeed");

    // Verify single upserted session point.
    let sess = tdb
        .db
        .query(
            "SELECT * FROM charging_sessions WHERE charge_id='chg-1'",
            "s",
        )
        .await
        .expect("query charging_sessions should succeed");
    let series = &sess["results"][0]["series"][0];
    let rows = series["values"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "upsert must produce a single point");
    let cols = series["columns"].as_array().unwrap();
    let col = |name: &str| {
        let idx = cols.iter().position(|c| c == name).unwrap();
        rows[0][idx].clone()
    };
    assert_eq!(col("start_battery_level"), serde_json::json!(30));
    assert_eq!(col("end_battery_level"), serde_json::json!(80));
    assert_eq!(col("energy_added_wh").as_f64(), Some(35000.0));
    assert_eq!(col("duration_seconds"), serde_json::json!(3600));
    assert_eq!(col("cost").as_f64(), Some(8.75));

    // Verify both charge readings persisted.
    let readings = tdb
        .db
        .query(
            "SELECT battery_level, power, fast_charger_present FROM charge_readings WHERE charge_id='chg-1' ORDER BY time ASC",
            "s",
        )
        .await
        .expect("query charge_readings should succeed");
    let r_rows = readings["results"][0]["series"][0]["values"]
        .as_array()
        .unwrap();
    assert_eq!(r_rows.len(), 2);
    assert_eq!(r_rows[0][1], serde_json::json!(45));
    assert_eq!(r_rows[1][1], serde_json::json!(60));
    assert_eq!(r_rows[0][3], serde_json::json!(false));

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn updates_write_and_upsert_on_close() {
    use crate::influxdb::{Precision, Update};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("updates").await;
    let start_ts: u128 = 1_700_003_000;

    let open = Update {
        time: Timestamp::Seconds(start_ts),
        vin: "VINUPD01".into(),
        update_id: "upd-1".into(),
        version_before: Some("2023.38.6".into()),
        version_after: None,
        install_start: Some("2023-11-14T23:03:20Z".into()),
        install_end: None,
        status: Some("installing".into()),
        abandoned: Some(false),
    };
    tdb.db
        .write_query(open.into_query("updates"), Precision::Seconds)
        .await
        .expect("open update write should succeed");

    let closed = Update {
        time: Timestamp::Seconds(start_ts),
        vin: "VINUPD01".into(),
        update_id: "upd-1".into(),
        version_before: Some("2023.38.6".into()),
        version_after: Some("2023.44.1".into()),
        install_start: Some("2023-11-14T23:03:20Z".into()),
        install_end: Some("2023-11-14T23:35:00Z".into()),
        status: Some("installed".into()),
        abandoned: Some(false),
    };
    tdb.db
        .write_query(closed.into_query("updates"), Precision::Seconds)
        .await
        .expect("closed update upsert should succeed");

    let res = tdb
        .db
        .query("SELECT * FROM updates WHERE update_id='upd-1'", "s")
        .await
        .expect("query updates should succeed");
    let series = &res["results"][0]["series"][0];
    let rows = series["values"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "upsert must keep a single update point");
    let cols = series["columns"].as_array().unwrap();
    let col = |name: &str| {
        let idx = cols.iter().position(|c| c == name).unwrap();
        rows[0][idx].clone()
    };
    assert_eq!(col("version_before"), serde_json::json!("2023.38.6"));
    assert_eq!(col("version_after"), serde_json::json!("2023.44.1"));
    assert_eq!(col("status"), serde_json::json!("installed"));
    assert_eq!(col("abandoned"), serde_json::json!(false));

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn charges_api_get_and_set_cost_roundtrip() {
    use crate::influxdb::{ChargingSession, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("charges_api").await;
    let app = tdb.authed_app();

    // Seed a closed session with whole-number float fields (InfluxDB JSON omits `.0`)
    // to prove read-modify-write preserves wire field types on real InfluxDB 1.8.
    let closed = ChargingSession {
        time: Timestamp::Seconds(1_700_004_000),
        vin: "VINCOST1".into(),
        charge_id: "chg-cost-1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: Some(37.7749),
        end_lng: Some(-122.4194),
        start_address: Some("Home".into()),
        start_range: Some(100.0),
        end_range: Some(250.0),
        start_rated_range: Some(95.0),
        end_rated_range: Some(240.0),
        start_battery_level: Some(25),
        end_battery_level: Some(75),
        energy_added_wh: Some(40000.0),
        duration_seconds: Some(3600),
        cost: Some(0.0),
        geofence_id: Some("home".into()),
        geofence_name: Some("Home".into()),
        charge_energy_used: Some(42000.0),
        connector_type: Some("SAE".into()),
        outside_temp_avg: Some(20.0),
        inside_temp_avg: Some(22.0),
    };
    tdb.db
        .write_query(closed.into_query("charging_sessions"), Precision::Seconds)
        .await
        .expect("closed session write should succeed");

    // Seed an open session (duration_seconds = None).
    let open = ChargingSession {
        time: Timestamp::Seconds(1_700_005_000),
        vin: "VINCOST1".into(),
        charge_id: "chg-open-1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: None,
        end_lng: None,
        start_address: None,
        start_range: Some(100.0),
        end_range: None,
        start_rated_range: None,
        end_rated_range: None,
        start_battery_level: Some(40),
        end_battery_level: None,
        energy_added_wh: None,
        duration_seconds: None,
        cost: None,
        geofence_id: None,
        geofence_name: None,
        charge_energy_used: None,
        connector_type: None,
        outside_temp_avg: None,
        inside_temp_avg: None,
    };
    tdb.db
        .write_query(open.into_query("charging_sessions"), Precision::Seconds)
        .await
        .expect("open session write should succeed");

    // 1. GET /api/charges/{id}
    let (status, got) = get_json(&app, "/api/charges/chg-cost-1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["charge_id"], "chg-cost-1");
    assert_eq!(got["energy_added_wh"].as_f64(), Some(40000.0));
    assert_eq!(got["duration_seconds"], serde_json::json!(3600));

    // 2. PUT /api/charges/{id}/cost (per_kwh + session_fee: 40 kWh * $0.30 + $1.50 = $13.50)
    let (status, updated) = put_json(
        &app,
        "/api/charges/chg-cost-1/cost",
        &serde_json::json!({
            "mode": "per_kwh",
            "cost_per_unit": 0.30,
            "session_fee": 1.50
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "response: {updated}");
    assert!((updated["cost"].as_f64().unwrap() - 13.50).abs() < 1e-6);

    // Re-read via API: cost updated, other fields intact.
    let (status, after) = get_json(&app, "/api/charges/chg-cost-1").await;
    assert_eq!(status, StatusCode::OK);
    assert!((after["cost"].as_f64().unwrap() - 13.50).abs() < 1e-6);
    assert_eq!(after["start_battery_level"], serde_json::json!(25));
    assert_eq!(after["end_battery_level"], serde_json::json!(75));
    assert_eq!(after["geofence_name"], serde_json::json!("Home"));

    // Verify InfluxDB still has exactly 1 point (no duplicate created at different timestamp).
    let raw = tdb
        .db
        .query(
            "SELECT cost FROM charging_sessions WHERE charge_id='chg-cost-1'",
            "ns",
        )
        .await
        .unwrap();
    let rows = raw["results"][0]["series"][0]["values"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "cost rewrite must upsert the exact point");

    // 3. PUT /api/charges/{id}/cost (per_minute: 60 min * $0.25 + $0 = $15.00)
    let (status, updated_min) = put_json(
        &app,
        "/api/charges/chg-cost-1/cost",
        &serde_json::json!({
            "mode": "per_minute",
            "cost_per_unit": 0.25,
            "session_fee": 0.0
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "response: {updated_min}");
    assert!((updated_min["cost"].as_f64().unwrap() - 15.0).abs() < 1e-6);

    // 4. Open session returns 409 Conflict.
    let (status, _) = put_json(
        &app,
        "/api/charges/chg-open-1/cost",
        &serde_json::json!({
            "mode": "per_kwh",
            "cost_per_unit": 0.20,
            "session_fee": 0.0
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // 5. Missing session returns 404 Not Found.
    let (status, _) = get_json(&app, "/api/charges/no-such-charge").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_overview_against_real_influxdb() {
    use crate::influxdb::{Position, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_overview").await;
    let app = tdb.authed_app();

    // Window: 2023-11-14T10:00:00Z .. 2023-11-14T10:15:00Z (3 five-minute buckets)
    // t0 = 1699956000 (10:00:00Z), t2 = 1699956600 (10:10:00Z) — middle bucket tests fill(previous).
    let base: u128 = 1_699_956_000;
    for (offset, soc, usable, ideal_mi) in [(60u128, 80i64, 78i64, 200.0f64), (660, 76, 74, 190.0)]
    {
        let pos = Position {
            time: Timestamp::Seconds(base + offset),
            vin: "VINOVR1".into(),
            car_id: 1,
            latitude: None,
            longitude: None,
            speed: None,
            power: None,
            odometer: None,
            battery_level: Some(soc),
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
            ideal_battery_range_km: Some(ideal_mi),
            est_battery_range_km: None,
            usable_battery_level: Some(usable),
            is_climate_on: None,
            driver_temp_setting: None,
            passenger_temp_setting: None,
            battery_heater: None,
            battery_heater_on: None,
            battery_heater_no_power: None,
            is_preconditioning: None,
            climate_keeper_mode: None,
            locked: None,
            is_user_present: None,
            sentry_mode: None,
        };
        tdb.db
            .write_query(pos.into_query("positions"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, km) = get_json(
        &app,
        "/api/dashboards/overview?vin=VINOVR1&from=2023-11-14T10:00:00Z&to=2023-11-14T10:14:59Z&unit=km",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "km response: {km}");
    assert_eq!(km["vin"], "VINOVR1");
    let battery = km["battery"].as_array().unwrap();
    assert_eq!(battery.len(), 3, "expected 3 five-minute buckets: {km}");
    assert_eq!(battery[0]["level"].as_f64(), Some(80.0));
    assert_eq!(battery[0]["usable"].as_f64(), Some(78.0));
    // Middle bucket filled via fill(previous) in InfluxDB 1.8.
    assert_eq!(battery[1]["level"].as_f64(), Some(80.0));
    assert_eq!(battery[2]["level"].as_f64(), Some(76.0));

    let range_km = km["ideal_range"][0]["value"].as_f64().unwrap();
    assert!(
        (range_km - 200.0 * 1.60934).abs() < 0.01,
        "expected km-converted range, got {range_km}"
    );

    let (status, mi) = get_json(
        &app,
        "/api/dashboards/overview?vin=VINOVR1&from=2023-11-14T10:00:00Z&to=2023-11-14T10:14:59Z&unit=mi",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let range_mi = mi["ideal_range"][0]["value"].as_f64().unwrap();
    assert!(
        (range_mi - 200.0).abs() < 0.1,
        "expected miles range ~200, got {range_mi}"
    );

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_states_against_real_influxdb() {
    use crate::influxdb::{Precision, VehicleState as StatePoint};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_states").await;
    let app = tdb.authed_app();

    // Window: 2023-11-14T10:00:00Z (1699956000) .. 2023-11-14T11:00:00Z (1699959600)
    // Seed one state before `from`, two inside window, one after `to`.
    let points = [
        (1_699_955_000u128, "asleep"),
        (1_699_956_600, "online"),
        (1_699_957_200, "driving"),
        (1_699_960_000, "charging"),
    ];
    for (ts, st) in points {
        let pt = StatePoint {
            time: Timestamp::Seconds(ts),
            vin: "VINST1".into(),
            state: st.into(),
            inside_temp: None,
            outside_temp: None,
            battery_level: Some(75),
            locked: Some(true),
            sentry_mode: Some(false),
            dog_mode: None,
            cabin_overheat_protection: None,
        };
        tdb.db
            .write_query(pt.into_query("states"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, json) = get_json(
        &app,
        "/api/dashboards/states?vin=VINST1&from=2023-11-14T10:00:00Z&to=2023-11-14T11:00:00Z",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "states response: {json}");
    assert_eq!(json["current"], "driving");
    assert_eq!(json["changes"], 2);

    let timeline = json["timeline"].as_array().unwrap();
    assert_eq!(
        timeline.len(),
        3,
        "prior state + 2 in-window transitions: {json}"
    );
    assert_eq!(timeline[0]["t"], 1_699_956_000_000i64);
    assert_eq!(timeline[0]["state"], "asleep");
    assert_eq!(timeline[1]["state"], "online");
    assert_eq!(timeline[2]["state"], "driving");

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_updates_against_real_influxdb() {
    use crate::influxdb::{Precision, Update};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_updates").await;
    let app = tdb.authed_app();

    let rows = [
        (
            1_699_956_000u128,
            "upd-a",
            "2023.32.9",
            "2023.38.6",
            "2023-11-14T10:00:00Z",
            "2023-11-14T10:25:00Z",
            "installed",
        ),
        (
            1_699_966_000u128,
            "upd-b",
            "2023.38.6",
            "2023.44.1",
            "2023-11-14T12:46:40Z",
            "2023-11-14T13:15:00Z",
            "installed",
        ),
    ];
    for (ts, id, before, after, start, end, st) in rows {
        let u = Update {
            time: Timestamp::Seconds(ts),
            vin: "VINUPD2".into(),
            update_id: id.into(),
            version_before: Some(before.into()),
            version_after: Some(after.into()),
            install_start: Some(start.into()),
            install_end: Some(end.into()),
            status: Some(st.into()),
            abandoned: Some(false),
        };
        tdb.db
            .write_query(u.into_query("updates"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, json) = get_json(
        &app,
        "/api/dashboards/updates?vin=VINUPD2&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "updates response: {json}");
    assert_eq!(json["count"], 2);
    let list = json["updates"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    // Ordered newest first (DESC across update_id series).
    assert_eq!(list[0]["version_after"], "2023.44.1");
    assert_eq!(list[1]["version_after"], "2023.38.6");

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_mileage_against_real_influxdb() {
    use crate::influxdb::{Position, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_mileage").await;
    let app = tdb.authed_app();

    // 2023-11-14T12:00:00Z = 1699963200, 2023-11-16T12:00:00Z = 1700136000
    // Also write an odometer=0 row that must be excluded by `odometer > 0`.
    let samples = [
        (1_699_963_200u128, 10000.0f64),
        (1_699_966_800, 10050.0),
        (1_699_970_400, 0.0),
        (1_700_136_000, 10200.0),
    ];
    for (ts, odo) in samples {
        let pos = Position {
            time: Timestamp::Seconds(ts),
            vin: "VINMIL1".into(),
            car_id: 1,
            latitude: None,
            longitude: None,
            speed: None,
            power: None,
            odometer: Some(odo),
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
            is_preconditioning: None,
            climate_keeper_mode: None,
            locked: None,
            is_user_present: None,
            sentry_mode: None,
        };
        tdb.db
            .write_query(pos.into_query("positions"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, km) = get_json(
        &app,
        "/api/dashboards/mileage?vin=VINMIL1&from=2023-11-14T00:00:00Z&to=2023-11-16T23:59:59Z&unit=km",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "mileage km: {km}");
    let pts = km["points"].as_array().unwrap();
    assert_eq!(pts.len(), 3, "3 daily buckets: {km}");
    let day0 = pts[0]["value"].as_f64().unwrap();
    let day1 = pts[1]["value"].as_f64().unwrap();
    let day2 = pts[2]["value"].as_f64().unwrap();
    assert!((day0 - 10050.0 * 1.60934).abs() < 0.1);
    // Day 1 has no samples -> fill(previous) carries day 0.
    assert!((day1 - day0).abs() < 0.01);
    assert!((day2 - 10200.0 * 1.60934).abs() < 0.1);

    let (status, mi) = get_json(
        &app,
        "/api/dashboards/mileage?vin=VINMIL1&from=2023-11-14T00:00:00Z&to=2023-11-16T23:59:59Z&unit=mi",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mi_day2 = mi["points"][2]["value"].as_f64().unwrap();
    assert!((mi_day2 - 10200.0).abs() < 1.0);

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_projected_range_against_real_influxdb() {
    use crate::influxdb::{Position, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_proj_range").await;
    let app = tdb.authed_app();

    // 2023-11-14T10:01:00Z = 1699956060
    // ideal=160 mi at 50% -> projected 100% = 320 mi * 1.60934 km
    // rated=150 mi at 50% -> projected 100% = 300 mi * 1.60934 km
    let pos = Position {
        time: Timestamp::Seconds(1_699_956_060),
        vin: "VINPRJ1".into(),
        car_id: 1,
        latitude: None,
        longitude: None,
        speed: None,
        power: None,
        odometer: None,
        battery_level: Some(50),
        rated_battery_range_km: Some(150.0),
        outside_temp: Some(16.5),
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
        ideal_battery_range_km: Some(160.0),
        est_battery_range_km: None,
        usable_battery_level: None,
        is_climate_on: None,
        driver_temp_setting: None,
        passenger_temp_setting: None,
        battery_heater: None,
        battery_heater_on: None,
        battery_heater_no_power: None,
        is_preconditioning: None,
        climate_keeper_mode: None,
        locked: None,
        is_user_present: None,
        sentry_mode: None,
    };
    tdb.db
        .write_query(pos.into_query("positions"), Precision::Seconds)
        .await
        .unwrap();

    let (status, json) = get_json(
        &app,
        "/api/dashboards/projected-range?vin=VINPRJ1&from=2023-11-14T10:00:00Z&to=2023-11-14T10:04:59Z&unit=km",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "projected-range: {json}");
    let projected = json["projected"].as_array().unwrap();
    assert_eq!(projected.len(), 1);
    let ideal_km = projected[0]["ideal"].as_f64().unwrap();
    let rated_km = projected[0]["rated"].as_f64().unwrap();
    assert!(
        (ideal_km - 320.0 * 1.60934).abs() < 0.05,
        "ideal={ideal_km}"
    );
    assert!(
        (rated_km - 300.0 * 1.60934).abs() < 0.05,
        "rated={rated_km}"
    );
    assert_eq!(json["outside_temp"][0]["value"].as_f64(), Some(16.5));

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_efficiency_against_real_influxdb() {
    use crate::influxdb::{Drive, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_eff").await;
    let app = tdb.authed_app();

    let drives = [
        (1_699_956_000u128, "eff-1", 10_000.0, 1_500.0, 600),
        (1_699_960_000u128, "eff-2", 20_000.0, 4_500.0, 1200),
        // Short drive (<= 100m): counts in logged_distance, excluded from consumption & per_drive.
        (1_699_964_000u128, "eff-short", 50.0, 500.0, 60),
    ];
    for (ts, id, dist_m, energy_wh, dur_s) in drives {
        let d = Drive {
            time: Timestamp::Seconds(ts),
            vin: "VINEFF1".into(),
            drive_id: id.into(),
            start_lat: None,
            start_lng: None,
            end_lat: None,
            end_lng: None,
            start_address: None,
            end_address: None,
            start_time: None,
            end_time: None,
            distance_meters: Some(dist_m),
            duration_seconds: Some(dur_s),
            energy_used_wh: Some(energy_wh),
            max_speed: None,
            average_speed: None,
            outside_temp_avg: None,
            inside_temp_avg: None,
            geofence_enter: None,
            geofence_exit: None,
            is_merged: Some(false),
        };
        tdb.db
            .write_query(d.into_query("drives"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, json) = get_json(
        &app,
        "/api/dashboards/efficiency?vin=VINEFF1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=km",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "efficiency: {json}");
    let avg = json["avg_consumption_wh_per_km"].as_f64().unwrap();
    assert!((avg - 200.0).abs() < 1e-6, "expected 200 Wh/km, got {avg}");
    let logged = json["logged_distance"].as_f64().unwrap();
    assert!(
        (logged - 30.05).abs() < 1e-6,
        "expected 30.05 km, got {logged}"
    );
    let per_drive = json["per_drive"].as_array().unwrap();
    assert_eq!(per_drive.len(), 2);
    assert!((per_drive[0]["value"].as_f64().unwrap() - 150.0).abs() < 1e-6);
    assert!((per_drive[1]["value"].as_f64().unwrap() - 225.0).abs() < 1e-6);

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_locations_against_real_influxdb() {
    use crate::influxdb::{Drive, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_loc").await;
    let app = tdb.authed_app();

    let drives = [
        (
            1_699_956_000u128,
            "loc-1",
            Some("Office"),
            Some("123 Market St"),
            "2023-11-14T10:30:00Z",
            12_000.0,
        ),
        (
            1_699_960_000u128,
            "loc-2",
            Some("Office"),
            Some("123 Market St"),
            "2023-11-14T11:30:00Z",
            10_000.0,
        ),
        (
            1_699_964_000u128,
            "loc-3",
            None,
            Some("Golden Gate Park"),
            "2023-11-14T12:30:00Z",
            8_000.0,
        ),
    ];
    for (ts, id, gf, addr, end_time, dist) in drives {
        let d = Drive {
            time: Timestamp::Seconds(ts),
            vin: "VINLOC1".into(),
            drive_id: id.into(),
            start_lat: None,
            start_lng: None,
            end_lat: None,
            end_lng: None,
            start_address: None,
            end_address: addr.map(String::from),
            start_time: None,
            end_time: Some(end_time.into()),
            distance_meters: Some(dist),
            duration_seconds: Some(900),
            energy_used_wh: Some(1500.0),
            max_speed: None,
            average_speed: None,
            outside_temp_avg: None,
            inside_temp_avg: None,
            geofence_enter: None,
            geofence_exit: gf.map(String::from),
            is_merged: Some(false),
        };
        tdb.db
            .write_query(d.into_query("drives"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, json) = get_json(
        &app,
        "/api/dashboards/locations?vin=VINLOC1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=km",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "locations: {json}");
    assert_eq!(json["drives_logged"], 3);

    let top = json["top"].as_array().unwrap();
    assert_eq!(top.len(), 2);
    assert_eq!(top[0]["place"], "Office");
    assert_eq!(top[0]["visits"], 2);
    assert_eq!(top[1]["place"], "Golden Gate Park");
    assert_eq!(top[1]["visits"], 1);

    let visited = json["last_visited"].as_array().unwrap();
    assert_eq!(visited.len(), 3);
    // Most recent end_time first.
    assert_eq!(visited[0]["address"], "Golden Gate Park");
    assert!((visited[0]["distance"].as_f64().unwrap() - 8.0).abs() < 1e-6);

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_drive_stats_against_real_influxdb() {
    use crate::influxdb::{Drive, Position, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_drive_stats").await;
    let app = tdb.authed_app();

    for (ts, id, dist_m, energy_wh, max_mph, dest) in [
        (1_699_956_000u128, "ds-1", 10_000.0, 2_000.0, 60.0, "Work"),
        (1_699_960_000u128, "ds-2", 20_000.0, 3_500.0, 75.0, "Work"),
    ] {
        let d = Drive {
            time: Timestamp::Seconds(ts),
            vin: "VINDS1".into(),
            drive_id: id.into(),
            start_lat: None,
            start_lng: None,
            end_lat: None,
            end_lng: None,
            start_address: None,
            end_address: None,
            start_time: None,
            end_time: None,
            distance_meters: Some(dist_m),
            duration_seconds: Some(1200),
            energy_used_wh: Some(energy_wh),
            max_speed: Some(max_mph),
            average_speed: Some(40.0),
            outside_temp_avg: None,
            inside_temp_avg: None,
            geofence_enter: None,
            geofence_exit: Some(dest.into()),
            is_merged: Some(false),
        };
        tdb.db
            .write_query(d.into_query("drives"), Precision::Seconds)
            .await
            .unwrap();
    }

    // Seed positions with speeds > 1 mph (plus a 0 mph point excluded by `speed > 1`).
    for (ts, spd) in [
        (1_699_956_060u128, 0.0),
        (1_699_956_120, 50.0),
        (1_699_956_180, 70.0),
    ] {
        let pos = Position {
            time: Timestamp::Seconds(ts),
            vin: "VINDS1".into(),
            car_id: 1,
            latitude: None,
            longitude: None,
            speed: Some(spd),
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
            is_preconditioning: None,
            climate_keeper_mode: None,
            locked: None,
            is_user_present: None,
            sentry_mode: None,
        };
        tdb.db
            .write_query(pos.into_query("positions"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, json) = get_json(
        &app,
        "/api/dashboards/drive-stats?vin=VINDS1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=km",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "drive-stats: {json}");
    assert_eq!(json["drives"], 2);
    assert!((json["distance"].as_f64().unwrap() - 30.0).abs() < 1e-6);
    assert!((json["energy_kwh"].as_f64().unwrap() - 5.5).abs() < 1e-6);
    assert!(json["median_drive"].as_f64().is_some());
    assert!((json["max_speed"].as_f64().unwrap() - 75.0 * 1.60934).abs() < 0.05);
    assert_eq!(json["speeds"].as_array().unwrap().len(), 2);
    assert_eq!(json["top_destinations"][0]["place"], "Work");
    assert_eq!(json["top_destinations"][0]["visits"], 2);

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_charges_against_real_influxdb() {
    use crate::influxdb::{ChargeReading, ChargingSession, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_charges").await;
    let app = tdb.authed_app();

    for (ts, id, loc, energy_wh, dur_s, cost, s_soc, e_soc) in [
        (
            1_699_956_000u128,
            "c-1",
            "Home",
            30_000.0,
            3600,
            6.0,
            30,
            70,
        ),
        (
            1_699_970_000u128,
            "c-2",
            "Supercharger",
            20_000.0,
            1200,
            9.0,
            20,
            60,
        ),
    ] {
        let s = ChargingSession {
            time: Timestamp::Seconds(ts),
            vin: "VINCHGD1".into(),
            charge_id: id.into(),
            start_lat: Some(37.77),
            start_lng: Some(-122.41),
            end_lat: Some(37.77),
            end_lng: Some(-122.41),
            start_address: None,
            start_range: None,
            end_range: None,
            start_rated_range: None,
            end_rated_range: None,
            start_battery_level: Some(s_soc),
            end_battery_level: Some(e_soc),
            energy_added_wh: Some(energy_wh),
            duration_seconds: Some(dur_s),
            cost: Some(cost),
            geofence_id: None,
            geofence_name: Some(loc.into()),
            charge_energy_used: None,
            connector_type: None,
            outside_temp_avg: None,
            inside_temp_avg: None,
        };
        tdb.db
            .write_query(s.into_query("charging_sessions"), Precision::Seconds)
            .await
            .unwrap();
    }

    let reading = ChargeReading {
        time: Timestamp::Seconds(1_699_956_300),
        vin: "VINCHGD1".into(),
        charge_id: "c-1".into(),
        voltage: Some(240.0),
        current: Some(40.0),
        power: Some(9600.0),
        phases: Some(1),
        energy_added: Some(5.0),
        battery_level: Some(40),
        battery_range: None,
        charger_power: Some(10),
        charger_voltage: Some(240),
        charger_phases: Some(1),
        outside_temp: None,
        fast_charger_brand: None,
        fast_charger_type: None,
        conn_charge_cable: None,
        usable_battery_level: None,
        charger_pilot_current: None,
        fast_charger_present: None,
        battery_heater_on: None,
        not_enough_power_to_heat: None,
        ideal_battery_range: None,
        rated_battery_range: None,
    };
    tdb.db
        .write_query(reading.into_query("charge_readings"), Precision::Seconds)
        .await
        .unwrap();

    let (status, json) = get_json(
        &app,
        "/api/dashboards/charges?vin=VINCHGD1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "charges: {json}");
    assert_eq!(json["count"], 2);
    assert!((json["energy_kwh"].as_f64().unwrap() - 50.0).abs() < 1e-6);
    assert!((json["total_cost"].as_f64().unwrap() - 15.0).abs() < 1e-6);
    assert!((json["avg_cost_per_kwh"].as_f64().unwrap() - 0.30).abs() < 1e-6);
    assert_eq!(json["sessions"].as_array().unwrap().len(), 2);
    assert!(!json["energy_per_day"].as_array().unwrap().is_empty());
    let power_series = json["power_by_session"].as_array().unwrap();
    assert_eq!(power_series.len(), 1);
    assert_eq!(power_series[0]["charge_id"], "c-1");

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_charging_stats_against_real_influxdb() {
    use crate::influxdb::{ChargingSession, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_chg_stats").await;
    let app = tdb.authed_app();

    for (ts, id, loc, energy_wh, cost, s_soc, e_soc) in [
        (1_699_956_000u128, "cs-1", "Home", 30_000.0, 6.0, 20, 80),
        (1_700_042_400u128, "cs-2", "Home", 20_000.0, 4.0, 40, 80),
        (
            1_700_046_000u128,
            "cs-3",
            "Supercharger",
            40_000.0,
            16.0,
            15,
            85,
        ),
    ] {
        let s = ChargingSession {
            time: Timestamp::Seconds(ts),
            vin: "VINCS1".into(),
            charge_id: id.into(),
            start_lat: None,
            start_lng: None,
            end_lat: None,
            end_lng: None,
            start_address: None,
            start_range: None,
            end_range: None,
            start_rated_range: None,
            end_rated_range: None,
            start_battery_level: Some(s_soc),
            end_battery_level: Some(e_soc),
            energy_added_wh: Some(energy_wh),
            duration_seconds: Some(1800),
            cost: Some(cost),
            geofence_id: None,
            geofence_name: Some(loc.into()),
            charge_energy_used: None,
            connector_type: None,
            outside_temp_avg: None,
            inside_temp_avg: None,
        };
        tdb.db
            .write_query(s.into_query("charging_sessions"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, json) = get_json(
        &app,
        "/api/dashboards/charging-stats?vin=VINCS1&from=2023-11-14T00:00:00Z&to=2023-11-15T23:59:59Z",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "charging-stats: {json}");
    assert_eq!(json["count"], 3);
    assert!((json["energy_kwh"].as_f64().unwrap() - 90.0).abs() < 1e-6);
    assert!((json["total_cost"].as_f64().unwrap() - 26.0).abs() < 1e-6);
    assert!((json["avg_cost_per_kwh"].as_f64().unwrap() - (26.0 / 90.0)).abs() < 1e-6);
    assert_eq!(json["soc_delta"].as_array().unwrap().len(), 2);

    // Note: top_stations groups by tag `geofence_name` (if stored as field rather than tag,
    // InfluxDB 1.8 collapses into the empty tag set -> "Unknown"). Assert real InfluxDB behavior.
    let stations = json["top_stations"].as_array().unwrap();
    assert!(
        !stations.is_empty(),
        "top_stations should not be empty: {json}"
    );
    let total_station_energy: f64 = stations
        .iter()
        .filter_map(|s| s["energy_kwh"].as_f64())
        .sum();
    let total_station_sessions: i64 = stations.iter().filter_map(|s| s["sessions"].as_i64()).sum();
    assert!(
        (total_station_energy - 90.0).abs() < 1e-6,
        "stations: {stations:?}"
    );
    assert_eq!(total_station_sessions, 3, "stations: {stations:?}");

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_charge_details_against_real_influxdb() {
    use crate::influxdb::{ChargeReading, ChargingSession, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_chg_det").await;
    let app = tdb.authed_app();

    let start_ts: u128 = 1_699_956_000;
    let session = ChargingSession {
        time: Timestamp::Seconds(start_ts),
        vin: "VINCD1".into(),
        charge_id: "cd-1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: Some(37.7749),
        end_lng: Some(-122.4194),
        start_address: None,
        start_range: None,
        end_range: None,
        start_rated_range: None,
        end_rated_range: None,
        start_battery_level: Some(25),
        end_battery_level: Some(80),
        energy_added_wh: Some(42_000.0),
        duration_seconds: Some(1800),
        cost: Some(10.5),
        geofence_id: None,
        geofence_name: Some("Home".into()),
        charge_energy_used: None,
        connector_type: None,
        outside_temp_avg: None,
        inside_temp_avg: None,
    };
    tdb.db
        .write_query(session.into_query("charging_sessions"), Precision::Seconds)
        .await
        .unwrap();

    for (offset, soc, watts, added) in [
        (60u128, 30i64, 11_000.0f64, 5.0f64),
        (180, 35, 11_500.0, 10.0),
    ] {
        let r = ChargeReading {
            time: Timestamp::Seconds(start_ts + offset),
            vin: "VINCD1".into(),
            charge_id: "cd-1".into(),
            voltage: Some(240.0),
            current: Some(48.0),
            power: Some(watts),
            phases: Some(1),
            energy_added: Some(added),
            battery_level: Some(soc),
            battery_range: None,
            charger_power: Some(11),
            charger_voltage: Some(240),
            charger_phases: Some(1),
            outside_temp: None,
            fast_charger_brand: None,
            fast_charger_type: None,
            conn_charge_cable: None,
            usable_battery_level: None,
            charger_pilot_current: None,
            fast_charger_present: None,
            battery_heater_on: None,
            not_enough_power_to_heat: None,
            ideal_battery_range: None,
            rated_battery_range: None,
        };
        tdb.db
            .write_query(r.into_query("charge_readings"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, json) = get_json(&app, "/api/dashboards/charge-details?charge_id=cd-1").await;
    assert_eq!(status, StatusCode::OK, "charge-details: {json}");
    assert_eq!(json["charge_id"], "cd-1");
    assert!((json["cost"].as_f64().unwrap() - 10.5).abs() < 1e-6);
    assert_eq!(json["duration_s"].as_f64(), Some(1800.0));
    assert_eq!(json["geofence"], "Home");
    assert!((json["energy_kwh"].as_f64().unwrap() - 42.0).abs() < 1e-6);
    assert_eq!(json["soc_added"].as_f64(), Some(55.0));
    assert_eq!(json["start_t"], (start_ts * 1000) as i64);
    assert_eq!(json["end_t"], ((start_ts + 1800) * 1000) as i64);

    let curve = json["curve"].as_array().unwrap();
    assert!(
        curve.len() >= 3 && curve.len() <= 31,
        "bounded 30m session span should yield <= 31 one-minute buckets, got {}: {json}",
        curve.len()
    );
    // Bucket at +60s has 11 kW, +120s is filled with 30%, +180s has 35%.
    let at_60 = curve
        .iter()
        .find(|p| p["t"].as_i64() == Some(((start_ts + 60) * 1000) as i64))
        .expect("bucket at +60s");
    let at_120 = curve
        .iter()
        .find(|p| p["t"].as_i64() == Some(((start_ts + 120) * 1000) as i64))
        .expect("bucket at +120s");
    let at_180 = curve
        .iter()
        .find(|p| p["t"].as_i64() == Some(((start_ts + 180) * 1000) as i64))
        .expect("bucket at +180s");
    assert!((at_60["power_kw"].as_f64().unwrap() - 11.0).abs() < 1e-6);
    assert_eq!(at_120["battery"].as_f64(), Some(30.0));
    assert_eq!(at_180["battery"].as_f64(), Some(35.0));

    let (status, _) = get_json(&app, "/api/dashboards/charge-details?charge_id=missing").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_drives_against_real_influxdb() {
    use crate::influxdb::{Drive, Position, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_drives").await;
    let app = tdb.authed_app();

    // Drive window: 2023-11-14T10:00:00Z (1699956000) .. 10:30:00Z (1699957800)
    let drive = Drive {
        time: Timestamp::Seconds(1_699_956_000),
        vin: "VINDRVS1".into(),
        drive_id: "d-100".into(),
        start_lat: Some(37.77),
        start_lng: Some(-122.41),
        end_lat: Some(37.80),
        end_lng: Some(-122.27),
        start_address: Some("SF".into()),
        end_address: Some("Oakland".into()),
        start_time: Some("2023-11-14T10:00:00Z".into()),
        end_time: Some("2023-11-14T10:30:00Z".into()),
        distance_meters: Some(25_000.0),
        duration_seconds: Some(1800),
        energy_used_wh: Some(4_500.0),
        max_speed: Some(65.0),
        average_speed: Some(31.0),
        outside_temp_avg: Some(19.0),
        inside_temp_avg: Some(21.0),
        geofence_enter: Some("Home".into()),
        geofence_exit: Some("Office".into()),
        is_merged: Some(false),
    };
    tdb.db
        .write_query(drive.into_query("drives"), Precision::Seconds)
        .await
        .unwrap();

    // Positions inside drive window for start_battery (80) -> end_battery (72) and power_max (95 kW).
    for (ts, soc, pwr) in [
        (1_699_956_060u128, 80i64, 40i64),
        (1_699_956_900, 76, 95),
        (1_699_957_740, 72, 20),
    ] {
        let pos = Position {
            time: Timestamp::Seconds(ts),
            vin: "VINDRVS1".into(),
            car_id: 1,
            latitude: None,
            longitude: None,
            speed: None,
            power: Some(pwr),
            odometer: None,
            battery_level: Some(soc),
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
            is_preconditioning: None,
            climate_keeper_mode: None,
            locked: None,
            is_user_present: None,
            sentry_mode: None,
        };
        tdb.db
            .write_query(pos.into_query("positions"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, json) = get_json(
        &app,
        "/api/dashboards/drives?vin=VINDRVS1&from=2023-11-14T00:00:00Z&to=2023-11-15T23:59:59Z&unit=km",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "drives: {json}");
    assert_eq!(json["count"], 1);
    assert!((json["distance"].as_f64().unwrap() - 25.0).abs() < 1e-6);
    assert_eq!(json["duration_s"], 1800);
    assert!((json["energy_kwh"].as_f64().unwrap() - 4.5).abs() < 1e-6);

    let row = &json["drives"][0];
    assert_eq!(row["drive_id"], "d-100");
    assert_eq!(row["from_gf"], "Home");
    assert_eq!(row["to_gf"], "Office");
    assert_eq!(row["start_battery"].as_f64(), Some(80.0));
    assert_eq!(row["end_battery"].as_f64(), Some(72.0));
    assert_eq!(row["power_max"].as_f64(), Some(95.0));

    let per_day = json["per_day"].as_array().unwrap();
    assert_eq!(per_day.len(), 2, "2 daily buckets with fill(0): {json}");
    assert!((per_day[0]["value"].as_f64().unwrap() - 25.0).abs() < 1e-6);
    assert_eq!(per_day[1]["value"].as_f64(), Some(0.0));

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_drive_details_against_real_influxdb() {
    use crate::influxdb::{Drive, Position, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_drv_det").await;
    let app = tdb.authed_app();

    // 1-minute drive: 2023-11-14T10:00:00Z (1699956000) .. 10:01:00Z (1699956060)
    let drive = Drive {
        time: Timestamp::Seconds(1_699_956_000),
        vin: "VINDD1".into(),
        drive_id: "dd-1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: Some(37.7850),
        end_lng: Some(-122.4050),
        start_address: None,
        end_address: None,
        start_time: Some("2023-11-14T10:00:00Z".into()),
        end_time: Some("2023-11-14T10:01:00Z".into()),
        distance_meters: Some(1_500.0),
        duration_seconds: Some(60),
        energy_used_wh: Some(300.0),
        max_speed: Some(55.0),
        average_speed: Some(40.0),
        outside_temp_avg: Some(18.0),
        inside_temp_avg: Some(21.0),
        geofence_enter: None,
        geofence_exit: None,
        is_merged: Some(false),
    };
    tdb.db
        .write_query(drive.into_query("drives"), Precision::Seconds)
        .await
        .unwrap();

    for (ts, lat, lng, spd, pwr, soc, ideal, elev, out_t, in_t, odo) in [
        (
            1_699_956_005u128,
            37.7749,
            -122.4194,
            30.0,
            25,
            80,
            200.0,
            10.0,
            18.0,
            21.0,
            10_000.0,
        ),
        (
            1_699_956_035,
            37.7800,
            -122.4120,
            55.0,
            45,
            79,
            198.0,
            25.0,
            18.5,
            21.5,
            10_000.5,
        ),
        (
            1_699_956_055,
            37.7850,
            -122.4050,
            20.0,
            -10,
            79,
            197.0,
            15.0,
            18.0,
            21.0,
            10_001.0,
        ),
    ] {
        let pos = Position {
            time: Timestamp::Seconds(ts),
            vin: "VINDD1".into(),
            car_id: 1,
            latitude: Some(lat),
            longitude: Some(lng),
            speed: Some(spd),
            power: Some(pwr),
            odometer: Some(odo),
            battery_level: Some(soc),
            rated_battery_range_km: None,
            outside_temp: Some(out_t),
            inside_temp: Some(in_t),
            heading: None,
            elevation: Some(elev),
            shift_state: Some("D".into()),
            tpms_pressure_fl: None,
            tpms_pressure_fr: None,
            tpms_pressure_rl: None,
            tpms_pressure_rr: None,
            fan_status: None,
            is_front_defroster_on: None,
            is_rear_defroster_on: None,
            ideal_battery_range_km: Some(ideal),
            est_battery_range_km: None,
            usable_battery_level: None,
            is_climate_on: None,
            driver_temp_setting: None,
            passenger_temp_setting: None,
            battery_heater: None,
            battery_heater_on: None,
            battery_heater_no_power: None,
            is_preconditioning: None,
            climate_keeper_mode: None,
            locked: None,
            is_user_present: None,
            sentry_mode: None,
        };
        tdb.db
            .write_query(pos.into_query("positions"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, json) =
        get_json(&app, "/api/dashboards/drive-details?drive_id=dd-1&unit=km").await;
    assert_eq!(status, StatusCode::OK, "drive-details: {json}");
    assert_eq!(json["drive_id"], "dd-1");
    assert_eq!(json["route"].as_array().unwrap().len(), 3);
    assert!(!json["series"].as_array().unwrap().is_empty());
    assert!(!json["elevation"].as_array().unwrap().is_empty());
    assert!(!json["temps"].as_array().unwrap().is_empty());
    assert_eq!(json["stats"]["duration_s"].as_f64(), Some(60.0));
    assert!((json["stats"]["distance"].as_f64().unwrap() - 1.5).abs() < 1e-6);
    assert!((json["stats"]["energy_kwh"].as_f64().unwrap() - 0.3).abs() < 1e-6);
    assert!((json["odometer"]["from"].as_f64().unwrap() - 10_000.0 * 1.60934).abs() < 0.1);
    assert!((json["odometer"]["to"].as_f64().unwrap() - 10_001.0 * 1.60934).abs() < 0.1);

    let (status, _) = get_json(&app, "/api/dashboards/drive-details?drive_id=missing").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    tdb.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_trip_against_real_influxdb() {
    use crate::influxdb::{ChargingSession, Drive, Position, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_trip").await;
    let app = tdb.authed_app();

    // Window: 2023-11-14T10:00:00Z (1699956000) .. 12:00:00Z (1699963200)
    let drive = Drive {
        time: Timestamp::Seconds(1_699_956_000),
        vin: "VINTRP1".into(),
        drive_id: "trp-d1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: Some(37.3382),
        end_lng: Some(-121.8863),
        start_address: Some("San Francisco".into()),
        end_address: Some("San Jose".into()),
        start_time: Some("2023-11-14T10:00:00Z".into()),
        end_time: Some("2023-11-14T11:00:00Z".into()),
        distance_meters: Some(80_000.0),
        duration_seconds: Some(3600),
        energy_used_wh: Some(14_400.0),
        max_speed: Some(70.0),
        average_speed: Some(50.0),
        outside_temp_avg: Some(20.0),
        inside_temp_avg: Some(21.0),
        geofence_enter: Some("Home".into()),
        geofence_exit: None,
        is_merged: Some(false),
    };
    tdb.db
        .write_query(drive.into_query("drives"), Precision::Seconds)
        .await
        .unwrap();

    let charge = ChargingSession {
        time: Timestamp::Seconds(1_699_959_600),
        vin: "VINTRP1".into(),
        charge_id: "trp-c1".into(),
        start_lat: Some(37.3382),
        start_lng: Some(-121.8863),
        end_lat: Some(37.3382),
        end_lng: Some(-121.8863),
        start_address: None,
        start_range: None,
        end_range: None,
        start_rated_range: None,
        end_rated_range: None,
        start_battery_level: Some(40),
        end_battery_level: Some(80),
        energy_added_wh: Some(30_000.0),
        duration_seconds: Some(1200),
        cost: Some(12.5),
        geofence_id: None,
        geofence_name: Some("San Jose SC".into()),
        charge_energy_used: None,
        connector_type: None,
        outside_temp_avg: None,
        inside_temp_avg: None,
    };
    tdb.db
        .write_query(charge.into_query("charging_sessions"), Precision::Seconds)
        .await
        .unwrap();

    for (ts, lat, lng, soc, ideal) in [
        (1_699_956_060u128, 37.7749, -122.4194, 80, 220.0),
        (1_699_959_500, 37.3382, -121.8863, 40, 110.0),
    ] {
        let pos = Position {
            time: Timestamp::Seconds(ts),
            vin: "VINTRP1".into(),
            car_id: 1,
            latitude: Some(lat),
            longitude: Some(lng),
            speed: Some(50.0),
            power: Some(20),
            odometer: Some(10_000.0),
            battery_level: Some(soc),
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
            ideal_battery_range_km: Some(ideal),
            est_battery_range_km: None,
            usable_battery_level: None,
            is_climate_on: None,
            driver_temp_setting: None,
            passenger_temp_setting: None,
            battery_heater: None,
            battery_heater_on: None,
            battery_heater_no_power: None,
            is_preconditioning: None,
            climate_keeper_mode: None,
            locked: None,
            is_user_present: None,
            sentry_mode: None,
        };
        tdb.db
            .write_query(pos.into_query("positions"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, json) = get_json(
        &app,
        "/api/dashboards/trip?vin=VINTRP1&from=2023-11-14T10:00:00Z&to=2023-11-14T12:00:00Z&unit=km",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "trip: {json}");
    assert!((json["distance"].as_f64().unwrap() - 80.0).abs() < 1e-6);
    assert_eq!(json["driving_s"], 3600);
    assert_eq!(json["charging_s"], 1200);
    assert!((json["avg_speed"].as_f64().unwrap() - 80.0).abs() < 1e-6);
    assert!((json["consumption_wh_per_km"].as_f64().unwrap() - 180.0).abs() < 1e-6);
    assert!((json["charging_cost"].as_f64().unwrap() - 12.5).abs() < 1e-6);
    assert_eq!(json["drives"].as_array().unwrap().len(), 1);
    assert_eq!(json["drives"][0]["drive_id"], "trp-d1");
    assert_eq!(json["charges"].as_array().unwrap().len(), 1);
    assert_eq!(json["charges"][0]["charge_id"], "trp-c1");
    assert!(!json["battery"].as_array().unwrap().is_empty());
    assert_eq!(json["route"].as_array().unwrap().len(), 2);

    tdb.cleanup().await;
}
