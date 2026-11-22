use super::TestDb;
use std::sync::Arc;

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn states_write_and_db_writer_flush() {
    use crate::influxdb::{Precision, VehicleState as StatePoint};
    use crate::vehicles::db_writer::DbWriter;
    use influxdb::{InfluxDbWriteable, Query, Timestamp};

    let tdb = TestDb::new("states_writer").await;
    let writer = DbWriter::new(Arc::clone(&tdb.db), 16);

    let s1 = StatePoint {
        time: Timestamp::Seconds(1_700_000_100),
        vin: "VINSTATE1".into(),
        state: "online".into(),
        inside_temp: Some(21.0),
        outside_temp: Some(18.5),
        battery_level: Some(82),
        locked: Some(true),
        sentry_mode: Some(false),
        dog_mode: Some(false),
        cabin_overheat_protection: Some(true),
    };
    let s2 = StatePoint {
        time: Timestamp::Seconds(1_700_000_200),
        vin: "VINSTATE1".into(),
        state: "driving".into(),
        inside_temp: Some(22.0),
        outside_temp: Some(18.0),
        battery_level: Some(81),
        locked: Some(true),
        sentry_mode: Some(false),
        dog_mode: Some(false),
        cabin_overheat_protection: Some(false),
    };
    let s3 = StatePoint {
        time: Timestamp::Seconds(1_700_000_300),
        vin: "VINSTATE1".into(),
        state: "asleep".into(),
        inside_temp: None,
        outside_temp: None,
        battery_level: Some(80),
        locked: Some(true),
        sentry_mode: Some(false),
        dog_mode: None,
        cabin_overheat_protection: None,
    };

    assert!(writer.send_telemetry(
        s1.into_query("states").build().unwrap().get(),
        Precision::Seconds,
    ));
    assert!(
        writer
            .send_session(
                s2.into_query("states").build().unwrap().get(),
                Precision::Seconds,
            )
            .await
    );
    writer.flush().await;

    // Session records must also survive shutdown().
    assert!(
        writer
            .send_session(
                s3.into_query("states").build().unwrap().get(),
                Precision::Seconds,
            )
            .await
    );
    writer.shutdown().await;

    let res = tdb
        .db
        .query(
            "SELECT state, battery_level, locked, cabin_overheat_protection FROM states WHERE vin='VINSTATE1' ORDER BY time ASC",
            "s",
        )
        .await
        .expect("query states should succeed");
    let rows = res["results"][0]["series"][0]["values"]
        .as_array()
        .expect("states rows");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][1], serde_json::json!("online"));
    assert_eq!(rows[0][4], serde_json::json!(true));
    assert_eq!(rows[1][1], serde_json::json!("driving"));
    assert_eq!(rows[1][2], serde_json::json!(81));
    assert_eq!(rows[2][1], serde_json::json!("asleep"));
    assert_eq!(rows[2][2], serde_json::json!(80));

    tdb.cleanup().await;
}
