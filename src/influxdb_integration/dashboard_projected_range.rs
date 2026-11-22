use super::{TestDb, get_json};
use axum::http::StatusCode;

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
