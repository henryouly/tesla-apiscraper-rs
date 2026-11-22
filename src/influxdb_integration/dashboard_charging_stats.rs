use super::{TestDb, get_json};
use axum::http::StatusCode;

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
