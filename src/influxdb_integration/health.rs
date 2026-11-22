use super::{TestDb, get_json};
use axum::http::StatusCode;

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn ping_ensure_database_and_health_ready() {
    let tdb = TestDb::new("ping_health").await;

    tdb.db.ping().await.expect("ping should succeed");
    // Second ensure_database call must be idempotent on InfluxDB 1.8.
    tdb.db
        .ensure_database()
        .await
        .expect("idempotent CREATE DATABASE should succeed");

    // Verify database actually exists in InfluxDB 1.8.
    let show = tdb
        .db
        .query("SHOW DATABASES", "ms")
        .await
        .expect("SHOW DATABASES should succeed");
    let names: Vec<&str> = show["results"][0]["series"][0]["values"]
        .as_array()
        .expect("SHOW DATABASES values")
        .iter()
        .filter_map(|row| row[0].as_str())
        .collect();
    assert!(
        names.contains(&tdb.name.as_str()),
        "database {} not found in {names:?}",
        tdb.name
    );

    // GET /health/ready hits db.ping() and returns 200 OK.
    let app = tdb.authed_app();
    let (status, json) = get_json(&app, "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert!(json.get("error").is_none());

    tdb.cleanup().await;
}
