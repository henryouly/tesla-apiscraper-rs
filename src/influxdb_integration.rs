//! Real InfluxDB 1.8 integration tests covering all database operations.
//!
//! Every test is marked `#[ignore]` so `cargo test` stays hermetic without a
//! running InfluxDB instance. In CI (and locally with Docker), run:
//!
//! ```bash
//! INFLUXDB_TEST_URL=http://127.0.0.1:8086 cargo test influxdb_integration -- --ignored
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::api::{create_router, test_helpers};
use crate::influxdb::InfluxDb;

#[allow(dead_code)]
static DB_COUNTER: AtomicU64 = AtomicU64::new(0);

#[allow(dead_code)]
fn test_influxdb_url() -> String {
    std::env::var("INFLUXDB_TEST_URL")
        .or_else(|_| std::env::var("INFLUXDB_URL"))
        .unwrap_or_else(|_| "http://127.0.0.1:8086".to_string())
}

/// Isolated InfluxDB 1.8 database created per test and dropped via `cleanup`.
#[allow(dead_code)]
struct TestDb {
    url: String,
    name: String,
    db: Arc<InfluxDb>,
}

#[allow(dead_code)]
impl TestDb {
    async fn new(prefix: &str) -> Self {
        let url = test_influxdb_url();
        let seq = DB_COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!("it_{prefix}_{}_{}", std::process::id(), seq);
        let db = Arc::new(InfluxDb::new(&url, "", "", &name).expect("valid InfluxDb config"));
        db.ensure_database()
            .await
            .expect("CREATE DATABASE failed against InfluxDB 1.8");
        Self { url, name, db }
    }

    fn authed_app(&self) -> Router {
        let mut state = test_helpers::test_state_authed();
        state.db = Arc::clone(&self.db);
        create_router(state)
    }

    async fn cleanup(self) {
        let q = format!("DROP DATABASE \"{}\"", self.name.replace('"', "\\\""));
        let _ = reqwest::Client::new()
            .post(format!("{}/query", self.url.trim_end_matches('/')))
            .form(&[("q", q)])
            .send()
            .await;
    }
}

#[allow(dead_code)]
async fn get_json(app: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or_else(|e| {
        panic!(
            "expected JSON from {uri} (status {status}), got error {e}: {}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, json)
}

#[allow(dead_code)]
async fn put_json(
    app: &Router,
    uri: &str,
    payload: &serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or_else(|e| {
        panic!(
            "expected JSON from PUT {uri} (status {status}), got error {e}: {}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, json)
}
