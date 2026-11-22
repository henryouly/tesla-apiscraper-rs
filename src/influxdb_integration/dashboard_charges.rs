use super::{TestDb, get_json};
use axum::http::StatusCode;

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_charges_against_real_influxdb() {
    use crate::influxdb::{ChargeReading, ChargingSession, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_charges").await;
    let app = tdb.authed_app();

    for (ts, id, loc, energy_wh, dur_s, cost, s_soc, e_soc) in [
        (
            1_699_956_000u128,
            "c-1",
            "Home",
            30_000.0,
            3600,
            6.0,
            30,
            70,
        ),
        (
            1_699_970_000u128,
            "c-2",
            "Supercharger",
            20_000.0,
            1200,
            9.0,
            20,
            60,
        ),
    ] {
        let s = ChargingSession {
            time: Timestamp::Seconds(ts),
            vin: "VINCHGD1".into(),
            charge_id: id.into(),
            start_lat: Some(37.77),
            start_lng: Some(-122.41),
            end_lat: Some(37.77),
            end_lng: Some(-122.41),
            start_address: None,
            start_range: None,
            end_range: None,
            start_rated_range: None,
            end_rated_range: None,
            start_battery_level: Some(s_soc),
            end_battery_level: Some(e_soc),
            energy_added_wh: Some(energy_wh),
            duration_seconds: Some(dur_s),
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

    let reading = ChargeReading {
        time: Timestamp::Seconds(1_699_956_300),
        vin: "VINCHGD1".into(),
        charge_id: "c-1".into(),
        voltage: Some(240.0),
        current: Some(40.0),
        power: Some(9600.0),
        phases: Some(1),
        energy_added: Some(5.0),
        battery_level: Some(40),
        battery_range: None,
        charger_power: Some(10),
        charger_voltage: Some(240),
        charger_phases: Some(1),
        outside_temp: None,
        fast_charger_brand: None,
        fast_charger_type: None,
        conn_charge_cable: None,
        usable_battery_level: None,
        charger_pilot_current: None,
        fast_charger_present: None,
        battery_heater_on: None,
        not_enough_power_to_heat: None,
        ideal_battery_range: None,
        rated_battery_range: None,
    };
    tdb.db
        .write_query(reading.into_query("charge_readings"), Precision::Seconds)
        .await
        .unwrap();

    let (status, json) = get_json(
        &app,
        "/api/dashboards/charges?vin=VINCHGD1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "charges: {json}");
    assert_eq!(json["count"], 2);
    assert!((json["energy_kwh"].as_f64().unwrap() - 50.0).abs() < 1e-6);
    assert!((json["total_cost"].as_f64().unwrap() - 15.0).abs() < 1e-6);
    assert!((json["avg_cost_per_kwh"].as_f64().unwrap() - 0.30).abs() < 1e-6);
    assert_eq!(json["sessions"].as_array().unwrap().len(), 2);
    assert!(!json["energy_per_day"].as_array().unwrap().is_empty());
    let power_series = json["power_by_session"].as_array().unwrap();
    assert_eq!(power_series.len(), 1);
    assert_eq!(power_series[0]["charge_id"], "c-1");

    tdb.cleanup().await;
}
