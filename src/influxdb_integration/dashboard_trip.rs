use super::{TestDb, get_json};
use axum::http::StatusCode;

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_trip_against_real_influxdb() {
    use crate::influxdb::{ChargingSession, Drive, Position, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_trip").await;
    let app = tdb.authed_app();

    // Window: 2023-11-14T10:00:00Z (1699956000) .. 12:00:00Z (1699963200)
    let drive = Drive {
        time: Timestamp::Seconds(1_699_956_000),
        vin: "VINTRP1".into(),
        drive_id: "trp-d1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: Some(37.3382),
        end_lng: Some(-121.8863),
        start_address: Some("San Francisco".into()),
        end_address: Some("San Jose".into()),
        start_time: Some("2023-11-14T10:00:00Z".into()),
        end_time: Some("2023-11-14T11:00:00Z".into()),
        distance_meters: Some(80_000.0),
        duration_seconds: Some(3600),
        energy_used_wh: Some(14_400.0),
        max_speed: Some(70.0),
        average_speed: Some(50.0),
        outside_temp_avg: Some(20.0),
        inside_temp_avg: Some(21.0),
        geofence_enter: Some("Home".into()),
        geofence_exit: None,
        is_merged: Some(false),
    };
    tdb.db
        .write_query(drive.into_query("drives"), Precision::Seconds)
        .await
        .unwrap();

    let charge = ChargingSession {
        time: Timestamp::Seconds(1_699_959_600),
        vin: "VINTRP1".into(),
        charge_id: "trp-c1".into(),
        start_lat: Some(37.3382),
        start_lng: Some(-121.8863),
        end_lat: Some(37.3382),
        end_lng: Some(-121.8863),
        start_address: None,
        start_range: None,
        end_range: None,
        start_rated_range: None,
        end_rated_range: None,
        start_battery_level: Some(40),
        end_battery_level: Some(80),
        energy_added_wh: Some(30_000.0),
        duration_seconds: Some(1200),
        cost: Some(12.5),
        geofence_id: None,
        geofence_name: Some("San Jose SC".into()),
        charge_energy_used: None,
        connector_type: None,
        outside_temp_avg: None,
        inside_temp_avg: None,
    };
    tdb.db
        .write_query(charge.into_query("charging_sessions"), Precision::Seconds)
        .await
        .unwrap();

    for (ts, lat, lng, soc, ideal) in [
        (1_699_956_060u128, 37.7749, -122.4194, 80, 220.0),
        (1_699_959_500, 37.3382, -121.8863, 40, 110.0),
    ] {
        let pos = Position {
            time: Timestamp::Seconds(ts),
            vin: "VINTRP1".into(),
            car_id: 1,
            latitude: Some(lat),
            longitude: Some(lng),
            speed: Some(50.0),
            power: Some(20),
            odometer: Some(10_000.0),
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

    let (status, json) = get_json(
        &app,
        "/api/dashboards/trip?vin=VINTRP1&from=2023-11-14T10:00:00Z&to=2023-11-14T12:00:00Z&unit=km",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "trip: {json}");
    assert!((json["distance"].as_f64().unwrap() - 80.0).abs() < 1e-6);
    assert_eq!(json["driving_s"], 3600);
    assert_eq!(json["charging_s"], 1200);
    assert!((json["avg_speed"].as_f64().unwrap() - 80.0).abs() < 1e-6);
    assert!((json["consumption_wh_per_km"].as_f64().unwrap() - 180.0).abs() < 1e-6);
    assert!((json["charging_cost"].as_f64().unwrap() - 12.5).abs() < 1e-6);
    assert_eq!(json["drives"].as_array().unwrap().len(), 1);
    assert_eq!(json["drives"][0]["drive_id"], "trp-d1");
    assert_eq!(json["charges"].as_array().unwrap().len(), 1);
    assert_eq!(json["charges"][0]["charge_id"], "trp-c1");
    assert!(!json["battery"].as_array().unwrap().is_empty());
    assert_eq!(json["route"].as_array().unwrap().len(), 2);

    tdb.cleanup().await;
}
