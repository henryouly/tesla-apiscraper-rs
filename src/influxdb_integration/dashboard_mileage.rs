use super::{TestDb, get_json};
use axum::http::StatusCode;

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_mileage_against_real_influxdb() {
    use crate::influxdb::{Position, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_mileage").await;
    let app = tdb.authed_app();

    // 2023-11-14T12:00:00Z = 1699963200, 2023-11-16T12:00:00Z = 1700136000
    // Also write an odometer=0 row that must be excluded by `odometer > 0`.
    let samples = [
        (1_699_963_200u128, 10000.0f64),
        (1_699_966_800, 10050.0),
        (1_699_970_400, 0.0),
        (1_700_136_000, 10200.0),
    ];
    for (ts, odo) in samples {
        let pos = Position {
            time: Timestamp::Seconds(ts),
            vin: "VINMIL1".into(),
            car_id: 1,
            latitude: None,
            longitude: None,
            speed: None,
            power: None,
            odometer: Some(odo),
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

    let (status, km) = get_json(
        &app,
        "/api/dashboards/mileage?vin=VINMIL1&from=2023-11-14T00:00:00Z&to=2023-11-16T23:59:59Z&unit=km",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "mileage km: {km}");
    let pts = km["points"].as_array().unwrap();
    assert_eq!(pts.len(), 3, "3 daily buckets: {km}");
    let day0 = pts[0]["value"].as_f64().unwrap();
    let day1 = pts[1]["value"].as_f64().unwrap();
    let day2 = pts[2]["value"].as_f64().unwrap();
    assert!((day0 - 10050.0 * 1.60934).abs() < 0.1);
    // Day 1 has no samples -> fill(previous) carries day 0.
    assert!((day1 - day0).abs() < 0.01);
    assert!((day2 - 10200.0 * 1.60934).abs() < 0.1);

    let (status, mi) = get_json(
        &app,
        "/api/dashboards/mileage?vin=VINMIL1&from=2023-11-14T00:00:00Z&to=2023-11-16T23:59:59Z&unit=mi",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mi_day2 = mi["points"][2]["value"].as_f64().unwrap();
    assert!((mi_day2 - 10200.0).abs() < 1.0);

    tdb.cleanup().await;
}
