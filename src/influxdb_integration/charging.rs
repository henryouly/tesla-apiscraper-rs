use super::TestDb;

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn charging_sessions_and_readings_upsert() {
    use crate::influxdb::{ChargeReading, ChargingSession, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("charging").await;
    let start_ts: u128 = 1_700_002_000;

    // Open charging session point.
    let open = ChargingSession {
        time: Timestamp::Seconds(start_ts),
        vin: "VINCHG01".into(),
        charge_id: "chg-1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: None,
        end_lng: None,
        start_address: Some("Home".into()),
        start_range: Some(120.0),
        end_range: None,
        start_rated_range: Some(115.0),
        end_rated_range: None,
        start_battery_level: Some(30),
        end_battery_level: None,
        energy_added_wh: None,
        duration_seconds: None,
        cost: None,
        geofence_id: Some("home".into()),
        geofence_name: Some("Home".into()),
        charge_energy_used: None,
        connector_type: Some("SAE".into()),
        outside_temp_avg: None,
        inside_temp_avg: None,
    };
    tdb.db
        .write_query(open.into_query("charging_sessions"), Precision::Seconds)
        .await
        .expect("open charging session write should succeed");

    // Live charge readings during session.
    for (offset, soc, kw, added) in [(60u128, 45i64, 11.2f64, 10.5f64), (120, 60, 11.0, 21.0)] {
        let reading = ChargeReading {
            time: Timestamp::Seconds(start_ts + offset),
            vin: "VINCHG01".into(),
            charge_id: "chg-1".into(),
            voltage: Some(240.0),
            current: Some(48.0),
            power: Some(kw),
            phases: Some(1),
            energy_added: Some(added),
            battery_level: Some(soc),
            battery_range: Some(180.0),
            charger_power: Some(11),
            charger_voltage: Some(240),
            charger_phases: Some(1),
            outside_temp: Some(18.0),
            fast_charger_brand: Some("<invalid>".into()),
            fast_charger_type: Some("ACSingleWireCAN".into()),
            conn_charge_cable: Some("SAE".into()),
            usable_battery_level: Some(soc - 1),
            charger_pilot_current: Some(48),
            fast_charger_present: Some(false),
            battery_heater_on: Some(false),
            not_enough_power_to_heat: Some(false),
            ideal_battery_range: Some(190.0),
            rated_battery_range: Some(180.0),
        };
        tdb.db
            .write_query(reading.into_query("charge_readings"), Precision::Seconds)
            .await
            .expect("charge_readings write should succeed");
    }

    // Close charging session (upsert same timestamp + tags).
    let closed = ChargingSession {
        time: Timestamp::Seconds(start_ts),
        vin: "VINCHG01".into(),
        charge_id: "chg-1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: Some(37.7749),
        end_lng: Some(-122.4194),
        start_address: Some("Home".into()),
        start_range: Some(120.0),
        end_range: Some(260.0),
        start_rated_range: Some(115.0),
        end_rated_range: Some(250.0),
        start_battery_level: Some(30),
        end_battery_level: Some(80),
        energy_added_wh: Some(35000.0),
        duration_seconds: Some(3600),
        cost: Some(8.75),
        geofence_id: Some("home".into()),
        geofence_name: Some("Home".into()),
        charge_energy_used: Some(37500.0),
        connector_type: Some("SAE".into()),
        outside_temp_avg: Some(18.0),
        inside_temp_avg: Some(20.0),
    };
    tdb.db
        .write_query(closed.into_query("charging_sessions"), Precision::Seconds)
        .await
        .expect("closed charging session upsert should succeed");

    // Verify single upserted session point.
    let sess = tdb
        .db
        .query(
            "SELECT * FROM charging_sessions WHERE charge_id='chg-1'",
            "s",
        )
        .await
        .expect("query charging_sessions should succeed");
    let series = &sess["results"][0]["series"][0];
    let rows = series["values"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "upsert must produce a single point");
    let cols = series["columns"].as_array().unwrap();
    let col = |name: &str| {
        let idx = cols.iter().position(|c| c == name).unwrap();
        rows[0][idx].clone()
    };
    assert_eq!(col("start_battery_level"), serde_json::json!(30));
    assert_eq!(col("end_battery_level"), serde_json::json!(80));
    assert_eq!(col("energy_added_wh").as_f64(), Some(35000.0));
    assert_eq!(col("duration_seconds"), serde_json::json!(3600));
    assert_eq!(col("cost").as_f64(), Some(8.75));

    // Verify both charge readings persisted.
    let readings = tdb
        .db
        .query(
            "SELECT battery_level, power, fast_charger_present FROM charge_readings WHERE charge_id='chg-1' ORDER BY time ASC",
            "s",
        )
        .await
        .expect("query charge_readings should succeed");
    let r_rows = readings["results"][0]["series"][0]["values"]
        .as_array()
        .unwrap();
    assert_eq!(r_rows.len(), 2);
    assert_eq!(r_rows[0][1], serde_json::json!(45));
    assert_eq!(r_rows[1][1], serde_json::json!(60));
    assert_eq!(r_rows[0][3], serde_json::json!(false));

    tdb.cleanup().await;
}
