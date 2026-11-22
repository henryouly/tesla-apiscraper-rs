use super::{TestDb, get_json};
use axum::http::StatusCode;

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
