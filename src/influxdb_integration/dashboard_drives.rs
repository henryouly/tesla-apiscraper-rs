use super::{TestDb, get_json};
use axum::http::StatusCode;

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
