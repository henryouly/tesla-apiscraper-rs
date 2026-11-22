use super::{TestDb, get_json};
use axum::http::StatusCode;

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
