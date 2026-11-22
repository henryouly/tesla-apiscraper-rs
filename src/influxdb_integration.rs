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
