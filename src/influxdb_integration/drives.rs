use super::TestDb;

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn drives_write_and_upsert_on_close() {
    use crate::influxdb::{Drive, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("drives").await;
    let start_ts: u128 = 1_700_001_000;

    // Open drive point (initial write on drive start).
    let open = Drive {
        time: Timestamp::Seconds(start_ts),
        vin: "VINDRV01".into(),
        drive_id: "drv-1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: None,
        end_lng: None,
        start_address: Some("San Francisco, CA".into()),
        end_address: None,
        start_time: Some("2023-11-14T22:30:00Z".into()),
        end_time: None,
        distance_meters: None,
        duration_seconds: None,
        energy_used_wh: None,
        max_speed: None,
        average_speed: None,
        outside_temp_avg: None,
        inside_temp_avg: None,
        geofence_enter: Some("Home".into()),
        geofence_exit: None,
        is_merged: Some(false),
    };
    tdb.db
        .write_query(open.into_query("drives"), Precision::Seconds)
        .await
        .expect("open drive write should succeed");

    // Close drive point (same timestamp + tags -> InfluxDB 1.8 upserts in place).
    let closed = Drive {
        time: Timestamp::Seconds(start_ts),
        vin: "VINDRV01".into(),
        drive_id: "drv-1".into(),
        start_lat: Some(37.7749),
        start_lng: Some(-122.4194),
        end_lat: Some(37.8044),
        end_lng: Some(-122.2712),
        start_address: Some("San Francisco, CA".into()),
        end_address: Some("Oakland, CA".into()),
        start_time: Some("2023-11-14T22:30:00Z".into()),
        end_time: Some("2023-11-14T23:00:00Z".into()),
        distance_meters: Some(18500.0),
        duration_seconds: Some(1800),
        energy_used_wh: Some(3120.5),
        max_speed: Some(72.0),
        average_speed: Some(37.0),
        outside_temp_avg: Some(19.5),
        inside_temp_avg: Some(21.5),
        geofence_enter: Some("Home".into()),
        geofence_exit: Some("Office".into()),
        is_merged: Some(false),
    };
    tdb.db
        .write_query(closed.into_query("drives"), Precision::Seconds)
        .await
        .expect("closed drive upsert should succeed");

    let res = tdb
        .db
        .query("SELECT * FROM drives WHERE drive_id='drv-1'", "s")
        .await
        .expect("query drives should succeed");
    let series = &res["results"][0]["series"][0];
    let rows = series["values"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "upsert must keep a single point");

    let cols = series["columns"].as_array().unwrap();
    let col = |name: &str| {
        let idx = cols.iter().position(|c| c == name).unwrap();
        rows[0][idx].clone()
    };
    assert_eq!(col("distance_meters").as_f64(), Some(18500.0));
    assert_eq!(col("duration_seconds"), serde_json::json!(1800));
    assert_eq!(col("energy_used_wh").as_f64(), Some(3120.5));
    assert_eq!(col("geofence_enter"), serde_json::json!("Home"));
    assert_eq!(col("geofence_exit"), serde_json::json!("Office"));
    assert_eq!(col("end_address"), serde_json::json!("Oakland, CA"));
    assert_eq!(col("is_merged"), serde_json::json!(false));

    tdb.cleanup().await;
}
