use super::{TestDb, get_json};
use axum::http::StatusCode;

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_overview_against_real_influxdb() {
    use crate::influxdb::{Position, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_overview").await;
    let app = tdb.authed_app();

    // Window: 2023-11-14T10:00:00Z .. 2023-11-14T10:15:00Z (3 five-minute buckets)
    // t0 = 1699956000 (10:00:00Z), t2 = 1699956600 (10:10:00Z) — middle bucket tests fill(previous).
    let base: u128 = 1_699_956_000;
    for (offset, soc, usable, ideal_mi) in [(60u128, 80i64, 78i64, 200.0f64), (660, 76, 74, 190.0)]
    {
        let pos = Position {
            time: Timestamp::Seconds(base + offset),
            vin: "VINOVR1".into(),
            car_id: 1,
            latitude: None,
            longitude: None,
            speed: None,
            power: None,
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
            ideal_battery_range_km: Some(ideal_mi),
            est_battery_range_km: None,
            usable_battery_level: Some(usable),
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

    let (status, km) = get_json(
        &app,
        "/api/dashboards/overview?vin=VINOVR1&from=2023-11-14T10:00:00Z&to=2023-11-14T10:14:59Z&unit=km",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "km response: {km}");
    assert_eq!(km["vin"], "VINOVR1");
    let battery = km["battery"].as_array().unwrap();
    assert_eq!(battery.len(), 3, "expected 3 five-minute buckets: {km}");
    assert_eq!(battery[0]["level"].as_f64(), Some(80.0));
    assert_eq!(battery[0]["usable"].as_f64(), Some(78.0));
    // Middle bucket filled via fill(previous) in InfluxDB 1.8.
    assert_eq!(battery[1]["level"].as_f64(), Some(80.0));
    assert_eq!(battery[2]["level"].as_f64(), Some(76.0));

    let range_km = km["ideal_range"][0]["value"].as_f64().unwrap();
    assert!(
        (range_km - 200.0 * 1.60934).abs() < 0.01,
        "expected km-converted range, got {range_km}"
    );

    let (status, mi) = get_json(
        &app,
        "/api/dashboards/overview?vin=VINOVR1&from=2023-11-14T10:00:00Z&to=2023-11-14T10:14:59Z&unit=mi",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let range_mi = mi["ideal_range"][0]["value"].as_f64().unwrap();
    assert!(
        (range_mi - 200.0).abs() < 0.1,
        "expected miles range ~200, got {range_mi}"
    );

    tdb.cleanup().await;
}
