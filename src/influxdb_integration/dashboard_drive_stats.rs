use super::{TestDb, get_json};
use axum::http::StatusCode;

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
