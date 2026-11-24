use super::{TestDb, get_json, put_json};
use axum::http::StatusCode;

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
async fn charges_api_special_chars_in_metadata_roundtrip() {
    use crate::influxdb::{ChargingSession, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("charges_api_esc").await;
    let app = tdb.authed_app();

    let tricky_geo = r#"Joe's "Fast", Supercharger"#;
    let tricky_addr = "123 Main St, Suite 'A'";
    let session = ChargingSession {
        time: Timestamp::Seconds(1_700_006_000),
        vin: "VINESC1".into(),
        charge_id: "chg-esc-1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: Some(37.7749),
        end_lng: Some(-122.4194),
        start_address: Some(tricky_addr.into()),
        start_range: Some(120.0),
        end_range: Some(240.0),
        start_rated_range: Some(115.0),
        end_rated_range: Some(235.0),
        start_battery_level: Some(30),
        end_battery_level: Some(80),
        energy_added_wh: Some(50_000.0),
        duration_seconds: Some(1800),
        cost: Some(0.0),
        geofence_id: Some("joes-sc".into()),
        geofence_name: Some(tricky_geo.into()),
        charge_energy_used: Some(52_000.0),
        connector_type: Some("Tesla".into()),
        outside_temp_avg: Some(19.5),
        inside_temp_avg: Some(21.0),
    };
    tdb.db
        .write_query(session.into_query("charging_sessions"), Precision::Seconds)
        .await
        .expect("tricky metadata write should succeed");

    let (status, updated) = put_json(
        &app,
        "/api/charges/chg-esc-1/cost",
        &serde_json::json!({
            "mode": "per_kwh",
            "cost_per_unit": 0.40,
            "session_fee": 2.00
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "response: {updated}");
    assert!((updated["cost"].as_f64().unwrap() - 22.0).abs() < 1e-6);

    let (status, after) = get_json(&app, "/api/charges/chg-esc-1").await;
    assert_eq!(status, StatusCode::OK);
    assert!((after["cost"].as_f64().unwrap() - 22.0).abs() < 1e-6);
    assert_eq!(after["geofence_name"], serde_json::json!(tricky_geo));
    assert_eq!(after["start_address"], serde_json::json!(tricky_addr));

    tdb.cleanup().await;
}
