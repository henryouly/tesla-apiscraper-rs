//! Dashboard data endpoints for the native in-app dashboards
//! (`web/src/pages`, Stage 0+ of `docs/dashboards-app/`).
//!
//! Thin aggregation layer over InfluxDB: browsers never query the database
//! directly. One endpoint per dashboard; shared helpers below. Error shape
//! mirrors the other API modules: 4xx for bad params, 502 with an `error`
//! body when InfluxDB fails.

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};

use super::AppState;

pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/overview", get(overview))
        .route("/states", get(states))
}

fn escape_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

type ApiError = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: impl Into<String>) -> ApiError {
    (status, Json(serde_json::json!({ "error": msg.into() })))
}

/// Shared range params: VIN + RFC3339 window + display unit. Stored
/// distances/ranges are km-based, speeds mph-based (Owner API units); the
/// `unit` factor converts km figures (`km` → 1, `mi` → 0.621371, matching
/// the Grafana `length_unit` variable).
#[derive(Deserialize)]
pub struct RangeParams {
    pub vin: String,
    pub from: String,
    pub to: String,
    pub unit: Option<String>,
}

fn parse_time(s: &str) -> Result<chrono::DateTime<chrono::FixedOffset>, ApiError> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map_err(|_| err(StatusCode::BAD_REQUEST, format!("invalid time: {s}")))
}

/// Maximum accepted query window: 90 days. At the fixed 5-minute bucket
/// that is ~26k points per series; wider windows would materialize
/// unbounded result sets (a multi-year window is millions of buckets
/// across two queries). Later endpoints with legitimate longer horizons
/// (e.g. Mileage) set their own bound.
const MAX_WINDOW_SECS: i64 = 90 * 24 * 60 * 60;

fn unit_factor(unit: Option<&str>) -> Result<f64, ApiError> {
    match unit.unwrap_or("km") {
        "km" => Ok(1.0),
        "mi" => Ok(0.621371),
        other => Err(err(
            StatusCode::BAD_REQUEST,
            format!("invalid unit: {other}"),
        )),
    }
}

fn num(v: &serde_json::Value) -> Option<f64> {
    if v.is_null() {
        None
    } else {
        v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))
    }
}

/// One InfluxQL row: timestamp plus cell values in column order.
pub type SeriesRow = (i64, Vec<serde_json::Value>);

/// Rows of the first series of an InfluxQL envelope: column names plus
/// rows. No series (e.g. empty window) is `Ok` with no rows — not an
/// error. A result-level `error` is upstream failure.
fn series_rows(json: &serde_json::Value) -> Result<(Vec<String>, Vec<SeriesRow>), anyhow::Error> {
    let results = json
        .get("results")
        .and_then(|r| r.as_array())
        .ok_or_else(|| anyhow::anyhow!("malformed InfluxDB response"))?;
    let first = results
        .first()
        .ok_or_else(|| anyhow::anyhow!("malformed InfluxDB response"))?;
    if let Some(e) = first.get("error").and_then(|e| e.as_str()) {
        anyhow::bail!("InfluxDB query error: {e}");
    }
    let Some(series) = first
        .get("series")
        .and_then(|s| s.as_array())
        .and_then(|a| a.first())
    else {
        return Ok((Vec::new(), Vec::new()));
    };
    let columns: Vec<String> = series
        .get("columns")
        .and_then(|c| c.as_array())
        .map(|a| {
            a.iter()
                .map(|c| c.as_str().unwrap_or("").to_string())
                .collect()
        })
        .ok_or_else(|| anyhow::anyhow!("malformed InfluxDB response"))?;
    let time_idx = columns
        .iter()
        .position(|c| c == "time")
        .ok_or_else(|| anyhow::anyhow!("malformed InfluxDB response"))?;
    let mut rows = Vec::new();
    if let Some(values) = series.get("values").and_then(|v| v.as_array()) {
        for row in values {
            let row = row.as_array().cloned().unwrap_or_default();
            if row.len() != columns.len() {
                anyhow::bail!("malformed InfluxDB response");
            }
            let t = row
                .get(time_idx)
                .and_then(|t| t.as_i64())
                .ok_or_else(|| anyhow::anyhow!("malformed InfluxDB response"))?;
            rows.push((t, row));
        }
    }
    Ok((columns, rows))
}

fn cell(columns: &[String], row: &[serde_json::Value], name: &str) -> Option<f64> {
    columns
        .iter()
        .position(|c| c == name)
        .and_then(|i| row.get(i))
        .and_then(num)
}

fn str_cell(columns: &[String], row: &[serde_json::Value], name: &str) -> Option<String> {
    columns
        .iter()
        .position(|c| c == name)
        .and_then(|i| row.get(i))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

#[derive(Serialize, PartialEq, Debug)]
pub struct BatteryPoint {
    pub t: i64,
    pub level: Option<f64>,
    pub usable: Option<f64>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct LevelPoint {
    pub t: i64,
    pub value: Option<f64>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct OverviewResponse {
    pub vin: String,
    pub battery: Vec<BatteryPoint>,
    pub ideal_range: Vec<LevelPoint>,
}

/// Battery + ideal range series for the Overview page. Fixed 5-minute
/// buckets: the window is bounded by `from`/`to`, so a range-derived
/// interval would collapse short windows into one point (never use
/// `$__interval`-style grouping over a bounded window).
async fn overview(
    State(state): State<AppState>,
    Query(p): Query<RangeParams>,
) -> Result<Json<OverviewResponse>, ApiError> {
    if p.vin.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "vin is required"));
    }
    let from = parse_time(&p.from)?;
    let to = parse_time(&p.to)?;
    if from > to {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "invalid time range: from is after to",
        ));
    }
    if (to - from).num_seconds() > MAX_WINDOW_SECS {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "time range exceeds the 90-day maximum",
        ));
    }
    let from = from.to_rfc3339();
    let to = to.to_rfc3339();
    let factor = unit_factor(p.unit.as_deref())?;
    let vin = escape_literal(&p.vin);

    let bq = format!(
        "SELECT mean(\"battery_level\"), mean(\"usable_battery_level\") FROM \"positions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' GROUP BY time(5m) fill(previous)"
    );
    let bjson = state
        .db
        .query(&bq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (bcols, brows) = series_rows(&bjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let rq = format!(
        "SELECT mean(\"ideal_battery_range_km\")*{factor} FROM \"positions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' GROUP BY time(5m) fill(previous)"
    );
    let rjson = state
        .db
        .query(&rq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (rcols, rrows) = series_rows(&rjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    // InfluxQL names single-aggregate columns `mean`, `mean_1`, … in order.
    let battery = brows
        .iter()
        .map(|(t, row)| BatteryPoint {
            t: *t,
            level: cell(&bcols, row, "mean"),
            usable: cell(&bcols, row, "mean_1"),
        })
        .collect();
    let ideal_range = rrows
        .iter()
        .map(|(t, row)| LevelPoint {
            t: *t,
            value: cell(&rcols, row, "mean"),
        })
        .collect();
    Ok(Json(OverviewResponse {
        vin: p.vin,
        battery,
        ideal_range,
    }))
}

#[derive(Serialize, PartialEq, Debug)]
pub struct StatePoint {
    pub t: i64,
    pub state: String,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct StatesResponse {
    pub vin: String,
    pub current: Option<String>,
    pub timeline: Vec<StatePoint>,
    pub changes: i64,
}

/// Vin + RFC3339 window without a display unit (states carry no distances).
#[derive(Deserialize)]
pub struct StatesParams {
    pub vin: String,
    pub from: String,
    pub to: String,
}

/// Current state + raw state timeline + transition count for the States
/// page. Mirrors the three `states.json` panels: `last("state")`, raw
/// `"state"`, and `count("state")`. The raw timeline is capped with
/// `LIMIT 10000` to bound the response on busy/long windows.
async fn states(
    State(state): State<AppState>,
    Query(p): Query<StatesParams>,
) -> Result<Json<StatesResponse>, ApiError> {
    if p.vin.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "vin is required"));
    }
    let from = parse_time(&p.from)?;
    let to = parse_time(&p.to)?;
    if from > to {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "invalid time range: from is after to",
        ));
    }
    if (to - from).num_seconds() > MAX_WINDOW_SECS {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "time range exceeds the 90-day maximum",
        ));
    }
    let from = from.to_rfc3339();
    let to = to.to_rfc3339();
    let vin = escape_literal(&p.vin);

    let cq = format!(
        "SELECT last(\"state\") FROM \"states\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}'"
    );
    let cjson = state
        .db
        .query(&cq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (ccols, crows) = series_rows(&cjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let tq = format!(
        "SELECT \"state\" FROM \"states\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' LIMIT 10000"
    );
    let tjson = state
        .db
        .query(&tq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (tcols, trows) = series_rows(&tjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let nq = format!(
        "SELECT count(\"state\") FROM \"states\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}'"
    );
    let njson = state
        .db
        .query(&nq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (ncols, nrows) = series_rows(&njson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let current = crows
        .first()
        .and_then(|(_, row)| str_cell(&ccols, row, "last"));
    let timeline = trows
        .iter()
        .filter_map(|(t, row)| {
            str_cell(&tcols, row, "state").map(|state| StatePoint { t: *t, state })
        })
        .collect();
    let changes = nrows
        .first()
        .and_then(|(_, row)| cell(&ncols, row, "count"))
        .map(|v| v as i64)
        .unwrap_or(0);
    Ok(Json(StatesResponse {
        vin: p.vin,
        current,
        timeline,
        changes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;

    const BATTERY_ROWS: &str = r#"{"results": [{"series": [{
        "name": "positions",
        "columns": ["time", "mean", "mean_1"],
        "values": [[1700000000000, 82, 80], [1700000300000, null, 79]]
    }]}]}"#;

    const RANGE_ROWS: &str = r#"{"results": [{"series": [{
        "name": "positions",
        "columns": ["time", "mean"],
        "values": [[1700000000000, 270.5]]
    }]}]}"#;

    fn state_with_mock(db_url: &str) -> AppState {
        let mut state = crate::api::test_helpers::test_state();
        state.db = Arc::new(crate::influxdb::InfluxDb::new(db_url, "", "", "tesla").unwrap());
        state
    }

    async fn mock_db() -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(|req: &wiremock::Request| {
                // Route to the fixture matching the query: the battery
                // query selects usable_battery_level, the range one does not.
                let body = if req
                    .url
                    .query()
                    .unwrap_or("")
                    .contains("usable_battery_level")
                {
                    BATTERY_ROWS
                } else {
                    RANGE_ROWS
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        server
    }

    async fn get_overview(query: &str) -> (StatusCode, serde_json::Value) {
        let server = mock_db().await;
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/overview{query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (status, json)
    }

    #[tokio::test]
    async fn overview_returns_series() {
        let (status, json) =
            get_overview("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=km")
                .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["vin"], "VIN1");
        assert_eq!(json["battery"][0]["t"], 1700000000000i64);
        assert_eq!(json["battery"][0]["level"], 82.0);
        assert_eq!(json["battery"][0]["usable"], 80.0);
        assert!(json["battery"][1]["level"].is_null());
        assert_eq!(json["battery"][1]["usable"], 79.0);
        assert_eq!(json["ideal_range"][0]["value"], 270.5);
    }

    #[tokio::test]
    async fn overview_rejects_inverted_range() {
        let (status, json) =
            get_overview("?vin=VIN1&from=2023-11-15T00:00:00Z&to=2023-11-14T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("from is after to"));
    }

    #[tokio::test]
    async fn overview_rejects_oversized_range() {
        let (status, json) =
            get_overview("?vin=VIN1&from=2020-01-01T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("90-day maximum"));
    }

    #[tokio::test]
    async fn overview_rejects_bad_time() {
        let (status, json) =
            get_overview("?vin=VIN1&from=not-a-time&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid time"));
    }

    #[tokio::test]
    async fn overview_rejects_bad_unit() {
        let (status, json) = get_overview(
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=parsecs",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid unit"));
    }

    #[tokio::test]
    async fn overview_rejects_empty_vin() {
        let (status, _) =
            get_overview("?vin=&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn overview_upstream_failure_is_502() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/overview?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn series_rows_empty_window_is_ok() {
        let json: serde_json::Value = serde_json::json!({"results": [{}]});
        let (cols, rows) = series_rows(&json).unwrap();
        assert!(cols.is_empty() && rows.is_empty());
    }

    #[test]
    fn series_rows_result_error_is_err() {
        let json: serde_json::Value = serde_json::json!({"results": [{"error": "boom"}]});
        assert!(series_rows(&json).is_err());
    }

    const STATES_CURRENT_ROWS: &str = r#"{"results": [{"series": [{
        "name": "states",
        "columns": ["time", "last"],
        "values": [[1700000300000, "driving"]]
    }]}]}"#;

    const STATES_TIMELINE_ROWS: &str = r#"{"results": [{"series": [{
        "name": "states",
        "columns": ["time", "state"],
        "values": [[1700000000000, "asleep"], [1700000100000, "online"], [1700000300000, "driving"]]
    }]}]}"#;

    const STATES_CHANGES_ROWS: &str = r#"{"results": [{"series": [{
        "name": "states",
        "columns": ["time", "count"],
        "values": [[1700000000000, 3]]
    }]}]}"#;

    async fn mock_states_db() -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(|req: &wiremock::Request| {
                // Route by query content. Note: reqwest percent-encodes
                // the query text, so `last(` arrives as `last%28` —
                // match on the bare function names instead.
                let q = req.url.query().unwrap_or("");
                let body = if q.contains("last") {
                    STATES_CURRENT_ROWS
                } else if q.contains("count") {
                    STATES_CHANGES_ROWS
                } else {
                    STATES_TIMELINE_ROWS
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        server
    }

    async fn get_states(query: &str) -> (StatusCode, serde_json::Value) {
        let server = mock_states_db().await;
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/states{query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (status, json)
    }

    #[tokio::test]
    async fn states_returns_series() {
        let (status, json) =
            get_states("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["vin"], "VIN1");
        assert_eq!(json["current"], "driving");
        assert_eq!(json["timeline"].as_array().unwrap().len(), 3);
        assert_eq!(json["timeline"][0]["t"], 1700000000000i64);
        assert_eq!(json["timeline"][0]["state"], "asleep");
        assert_eq!(json["timeline"][2]["state"], "driving");
        assert_eq!(json["changes"], 3);
    }

    #[tokio::test]
    async fn states_empty_window_is_empty_not_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(r#"{"results": [{}]}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/states?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["current"].is_null());
        assert_eq!(json["timeline"].as_array().unwrap().len(), 0);
        assert_eq!(json["changes"], 0);
    }

    #[tokio::test]
    async fn states_rejects_inverted_range() {
        let (status, json) =
            get_states("?vin=VIN1&from=2023-11-15T00:00:00Z&to=2023-11-14T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("from is after to"));
    }

    #[tokio::test]
    async fn states_rejects_oversized_range() {
        let (status, json) =
            get_states("?vin=VIN1&from=2020-01-01T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("90-day maximum"));
    }

    #[tokio::test]
    async fn states_rejects_bad_time() {
        let (status, json) = get_states("?vin=VIN1&from=not-a-time&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid time"));
    }

    #[tokio::test]
    async fn states_rejects_empty_vin() {
        let (status, _) =
            get_states("?vin=&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn states_upstream_failure_is_502() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/states?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn str_cell_extracts_strings() {
        let cols = vec!["time".to_string(), "state".to_string()];
        let row = vec![
            serde_json::json!(1700000000000i64),
            serde_json::json!("online"),
        ];
        assert_eq!(str_cell(&cols, &row, "state").as_deref(), Some("online"));
        assert_eq!(str_cell(&cols, &row, "missing"), None);
    }
}
