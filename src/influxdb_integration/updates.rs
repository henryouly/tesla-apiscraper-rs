use super::TestDb;

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn updates_write_and_upsert_on_close() {
    use crate::influxdb::{Precision, Update};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("updates").await;
    let start_ts: u128 = 1_700_003_000;

    let open = Update {
        time: Timestamp::Seconds(start_ts),
        vin: "VINUPD01".into(),
        update_id: "upd-1".into(),
        version_before: Some("2023.38.6".into()),
        version_after: None,
        install_start: Some("2023-11-14T23:03:20Z".into()),
        install_end: None,
        status: Some("installing".into()),
        abandoned: Some(false),
    };
    tdb.db
        .write_query(open.into_query("updates"), Precision::Seconds)
        .await
        .expect("open update write should succeed");

    let closed = Update {
        time: Timestamp::Seconds(start_ts),
        vin: "VINUPD01".into(),
        update_id: "upd-1".into(),
        version_before: Some("2023.38.6".into()),
        version_after: Some("2023.44.1".into()),
        install_start: Some("2023-11-14T23:03:20Z".into()),
        install_end: Some("2023-11-14T23:35:00Z".into()),
        status: Some("installed".into()),
        abandoned: Some(false),
    };
    tdb.db
        .write_query(closed.into_query("updates"), Precision::Seconds)
        .await
        .expect("closed update upsert should succeed");

    let res = tdb
        .db
        .query("SELECT * FROM updates WHERE update_id='upd-1'", "s")
        .await
        .expect("query updates should succeed");
    let series = &res["results"][0]["series"][0];
    let rows = series["values"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "upsert must keep a single update point");
    let cols = series["columns"].as_array().unwrap();
    let col = |name: &str| {
        let idx = cols.iter().position(|c| c == name).unwrap();
        rows[0][idx].clone()
    };
    assert_eq!(col("version_before"), serde_json::json!("2023.38.6"));
    assert_eq!(col("version_after"), serde_json::json!("2023.44.1"));
    assert_eq!(col("status"), serde_json::json!("installed"));
    assert_eq!(col("abandoned"), serde_json::json!(false));

    tdb.cleanup().await;
}
