use super::TestDb;

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
