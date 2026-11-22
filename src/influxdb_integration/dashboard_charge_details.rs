use super::{TestDb, get_json};
use axum::http::StatusCode;

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
