use super::{TestDb, get_json};
use axum::http::StatusCode;

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_updates_against_real_influxdb() {
    use crate::influxdb::{Precision, Update};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_updates").await;
    let app = tdb.authed_app();

    let rows = [
        (
            1_699_956_000u128,
            "upd-a",
            "2023.32.9",
            "2023.38.6",
            "2023-11-14T10:00:00Z",
            "2023-11-14T10:25:00Z",
            "installed",
        ),
        (
            1_699_966_000u128,
            "upd-b",
            "2023.38.6",
            "2023.44.1",
            "2023-11-14T12:46:40Z",
            "2023-11-14T13:15:00Z",
            "installed",
        ),
    ];
    for (ts, id, before, after, start, end, st) in rows {
        let u = Update {
            time: Timestamp::Seconds(ts),
            vin: "VINUPD2".into(),
            update_id: id.into(),
            version_before: Some(before.into()),
            version_after: Some(after.into()),
            install_start: Some(start.into()),
            install_end: Some(end.into()),
            status: Some(st.into()),
            abandoned: Some(false),
        };
        tdb.db
            .write_query(u.into_query("updates"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, json) = get_json(
        &app,
        "/api/dashboards/updates?vin=VINUPD2&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "updates response: {json}");
    assert_eq!(json["count"], 2);
    let list = json["updates"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    // Ordered newest first (DESC across update_id series).
    assert_eq!(list[0]["version_after"], "2023.44.1");
    assert_eq!(list[1]["version_after"], "2023.38.6");

    tdb.cleanup().await;
}
