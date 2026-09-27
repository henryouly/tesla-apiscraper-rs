//! Individual charge-session cost editing.
//!
//! InfluxDB overwrites whole points, so editing `cost` is a careful
//! read-modify-write: the latest row for `charge_id` is reread (all
//! columns, nanosecond timestamp) and rewritten with only `cost` changed.
//! Costs apply to the edited session only — other sessions are untouched.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};

use super::AppState;
use crate::influxdb::Precision;

#[derive(Serialize)]
pub struct ChargeSessionResponse {
    pub charge_id: String,
    #[serde(flatten)]
    pub fields: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
pub struct CostRequest {
    /// `per_kwh` or `per_minute`.
    pub mode: String,
    pub cost_per_unit: f64,
    #[serde(default)]
    pub session_fee: f64,
}

#[derive(Serialize)]
pub struct CostResponse {
    pub charge_id: String,
    pub cost: f64,
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/{id}", get(get_session))
        .route("/{id}/cost", axum::routing::put(set_cost))
}

fn escape_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

fn escape_tag(s: &str) -> String {
    sanitize(s)
        .replace(' ', "\\ ")
        .replace(',', "\\,")
        .replace('=', "\\=")
}

/// Line protocol has no escape for newlines inside string fields or tags —
/// a raw one splits the body into extra points. Strip them (names are
/// validated control-free at the CRUD layer; this covers Tesla- and
/// geocode-sourced strings).
fn sanitize(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Latest `charging_sessions` row for `charge_id`: (timestamp ns, fields by
/// column, tags). Distinguishes "no such session" from query/parse
/// failures so handlers don't report an unhealthy database as 404.
async fn read_session(
    db: &crate::influxdb::InfluxDb,
    charge_id: &str,
) -> Result<(i64, Vec<(String, serde_json::Value)>, Vec<(String, String)>), SessionReadError> {
    let q = format!(
        "SELECT * FROM charging_sessions WHERE charge_id='{}' ORDER BY time DESC LIMIT 1",
        escape_literal(charge_id)
    );
    let json = db
        .query(&q, "ns")
        .await
        .map_err(SessionReadError::Upstream)?;
    let malformed = || SessionReadError::Upstream(anyhow::anyhow!("malformed InfluxDB response"));
    let results = json
        .get("results")
        .and_then(|r| r.as_array())
        .ok_or_else(malformed)?;
    let first = results.first().ok_or(SessionReadError::NotFound)?;
    // InfluxDB v1 reports statement failures as HTTP 200 with a
    // result-level error and no series — that is upstream, not absent.
    if let Some(e) = first.get("error").and_then(|e| e.as_str()) {
        return Err(SessionReadError::Upstream(anyhow::anyhow!(
            "InfluxDB query error: {e}"
        )));
    }
    let series = first
        .get("series")
        .and_then(|s| s.as_array())
        .and_then(|a| a.first())
        .ok_or(SessionReadError::NotFound)?;
    let columns: Vec<&str> = series
        .get("columns")
        .and_then(|c| c.as_array())
        .map(|a| a.iter().filter_map(|c| c.as_str()).collect())
        .ok_or_else(malformed)?;
    let row = series
        .get("values")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|r| r.as_array())
        .ok_or_else(malformed)?;
    let time = columns
        .iter()
        .position(|c| *c == "time")
        .and_then(|i| row.get(i))
        .and_then(|t| t.as_i64())
        .ok_or_else(malformed)?;
    let mut tags = Vec::new();
    let mut fields = Vec::new();
    for (i, name) in columns.iter().enumerate() {
        if *name == "time" {
            continue;
        }
        let Some(v) = row.get(i) else { continue };
        if *name == "vin" || *name == "charge_id" {
            if let Some(s) = v.as_str() {
                tags.push((name.to_string(), s.to_string()));
            }
        } else if !v.is_null() {
            fields.push((name.to_string(), v.clone()));
        }
    }
    Ok((time, fields, tags))
}

#[derive(Debug)]
enum SessionReadError {
    NotFound,
    Upstream(anyhow::Error),
}

impl SessionReadError {
    fn status(&self) -> StatusCode {
        match self {
            SessionReadError::NotFound => StatusCode::NOT_FOUND,
            SessionReadError::Upstream(_) => StatusCode::BAD_GATEWAY,
        }
    }

    fn message(&self) -> String {
        match self {
            SessionReadError::NotFound => "charge session not found".into(),
            SessionReadError::Upstream(e) => format!("failed to read charge session: {e}"),
        }
    }
}

/// Numeric field types of `charging_sessions`, from the `ChargingSession`
/// struct. InfluxDB v1 JSON drops the `.0` of whole floats, so types cannot
/// be inferred from the response — an integral `energy_added_wh` rewritten
/// as `11000i` would conflict with the stored float field.
const FLOAT_FIELDS: &[&str] = &[
    "start_lat",
    "start_lng",
    "end_lat",
    "end_lng",
    "start_range",
    "end_range",
    "start_rated_range",
    "end_rated_range",
    "energy_added_wh",
    "cost",
    "charge_energy_used",
    "outside_temp_avg",
    "inside_temp_avg",
];

const INT_FIELDS: &[&str] = &[
    "start_battery_level",
    "end_battery_level",
    "duration_seconds",
];

fn line_protocol_value(name: &str, v: &serde_json::Value) -> Option<String> {
    if FLOAT_FIELDS.contains(&name) {
        let f = v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))?;
        return Some(format!("{f:?}"));
    }
    if INT_FIELDS.contains(&name) {
        let i = v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))?;
        return Some(format!("{i}i"));
    }
    // Unknown future columns keep arrival representation.
    if let Some(i) = v.as_i64() {
        Some(format!("{i}i"))
    } else if let Some(f) = v.as_f64() {
        Some(format!("{f:?}"))
    } else if let Some(s) = v.as_str() {
        Some(format!(
            "\"{}\"",
            sanitize(s).replace('\\', "\\\\").replace('"', "\\\"")
        ))
    } else {
        v.as_bool()
            .map(|b| if b { "true".into() } else { "false".into() })
    }
}

fn field_num(fields: &[(String, serde_json::Value)], name: &str) -> Option<f64> {
    fields
        .iter()
        .find(|(n, _)| n == name)
        .and_then(|(_, v)| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
}

/// Cost in whole cents-rounded units. Preview formula (mirrored client-side).
fn calculate_cost(
    mode: &str,
    energy_added_wh: f64,
    duration_seconds: f64,
    rate: f64,
    fee: f64,
) -> f64 {
    let base = if mode == "per_kwh" {
        energy_added_wh / 1000.0 * rate
    } else {
        duration_seconds / 60.0 * rate
    };
    ((base + fee) * 100.0).round() / 100.0
}

type ApiError = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: impl Into<String>) -> ApiError {
    (status, Json(serde_json::json!({ "error": msg.into() })))
}

async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ChargeSessionResponse>, ApiError> {
    let (_time, fields, _tags) = read_session(&state.db, &id)
        .await
        .map_err(|e| err(e.status(), e.message()))?;
    Ok(Json(ChargeSessionResponse {
        charge_id: id,
        fields: fields.into_iter().collect(),
    }))
}

async fn set_cost(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CostRequest>,
) -> Result<Json<CostResponse>, ApiError> {
    if req.mode != "per_kwh" && req.mode != "per_minute" {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "mode must be per_kwh or per_minute",
        ));
    }
    for (name, v) in [
        ("cost_per_unit", req.cost_per_unit),
        ("session_fee", req.session_fee),
    ] {
        if !v.is_finite() || v < 0.0 {
            return Err(err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("{name} must not be negative"),
            ));
        }
    }
    let (time_ns, mut fields, tags) = read_session(&state.db, &id)
        .await
        .map_err(|e| err(e.status(), e.message()))?;

    let (source, source_val) = if req.mode == "per_kwh" {
        ("energy_added_wh", field_num(&fields, "energy_added_wh"))
    } else {
        ("duration_seconds", field_num(&fields, "duration_seconds"))
    };
    let Some(_) = source_val else {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("session has no {source} to bill from"),
        ));
    };
    let energy = field_num(&fields, "energy_added_wh").unwrap_or(0.0);
    let duration = field_num(&fields, "duration_seconds").unwrap_or(0.0);
    let cost = calculate_cost(
        &req.mode,
        energy,
        duration,
        req.cost_per_unit,
        req.session_fee,
    );

    // Rewrite the whole point with only `cost` changed.
    if let Some(slot) = fields.iter_mut().find(|(n, _)| n == "cost") {
        slot.1 = serde_json::json!(cost);
    } else {
        fields.push(("cost".into(), serde_json::json!(cost)));
    }
    let mut fieldset = String::new();
    for (n, v) in &fields {
        if let Some(lp) = line_protocol_value(n, v) {
            if !fieldset.is_empty() {
                fieldset.push(',');
            }
            fieldset.push_str(&format!("{n}={lp}"));
        }
    }
    let tagset: Vec<String> = tags
        .iter()
        .map(|(k, v)| format!("{k}={}", escape_tag(v)))
        .collect();
    let lp = format!(
        "charging_sessions,{} {fieldset} {time_ns}",
        tagset.join(",")
    );
    if let Err(e) = state.db.write_lp(&lp, Precision::Nanoseconds).await {
        return Err(err(
            StatusCode::BAD_GATEWAY,
            format!("failed to persist cost: {e}"),
        ));
    }
    Ok(Json(CostResponse {
        charge_id: id,
        cost,
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

    // energy_added_wh is integral JSON on purpose: real InfluxDB drops
    // the `.0` of whole floats, and the rewrite must still emit a float.
    const ROW: &str = r#"{"results": [{"series": [{
        "name": "charging_sessions",
        "columns": ["time", "vin", "charge_id", "energy_added_wh", "duration_seconds", "cost", "geofence_name"],
        "values": [[1700000000000000000, "VIN1", "VIN1_1700000000", 11000, 3600, 2.5, "Home"]]
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
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_raw(ROW, "application/json"),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;
        server
    }

    #[test]
    fn calculate_cost_modes() {
        assert_eq!(calculate_cost("per_kwh", 11000.0, 3600.0, 0.3, 1.0), 4.3);
        assert_eq!(calculate_cost("per_minute", 11000.0, 3600.0, 0.1, 0.0), 6.0);
    }

    #[tokio::test]
    async fn get_session_returns_row() {
        let server = mock_db().await;
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/VIN1_1700000000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["charge_id"], "VIN1_1700000000");
        assert_eq!(json["energy_added_wh"], 11000);
        assert_eq!(json["cost"], 2.5);
    }

    #[tokio::test]
    async fn set_cost_rewrites_point_preserving_fields() {
        let server = mock_db().await;
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/VIN1_1700000000/cost")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "mode": "per_kwh",
                            "cost_per_unit": 0.3,
                            "session_fee": 1.0
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["cost"], 4.3);

        // The rewrite kept every field, the timestamp, and only changed cost.
        let writes: Vec<_> = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == "/write")
            .collect();
        assert_eq!(writes.len(), 1);
        let lp = String::from_utf8_lossy(&writes[0].body).into_owned();
        assert!(
            lp.starts_with("charging_sessions,vin=VIN1,charge_id=VIN1_1700000000 "),
            "{lp}"
        );
        assert!(lp.ends_with(" 1700000000000000000"), "{lp}");
        assert!(lp.contains("cost=4.3"), "{lp}");
        assert!(lp.contains("energy_added_wh=11000.0"), "{lp}");
        assert!(lp.contains("duration_seconds=3600i"), "{lp}");
        assert!(lp.contains("geofence_name=\"Home\""), "{lp}");
    }

    #[tokio::test]
    async fn set_cost_sanitizes_newlines_to_single_line() {
        // A pre-existing fence name with an embedded newline (predates the
        // CRUD control-char rejection) must not split the LP body.
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "results": [{"series": [{
                        "columns": ["time", "vin", "charge_id", "energy_added_wh", "geofence_name"],
                        "values": [[1700000000000000000i64, "VIN1", "NL1", 5000, "Ho\nme"]],
                    }]}]
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/NL1/cost")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "mode": "per_kwh",
                            "cost_per_unit": 0.2,
                            "session_fee": 0.0
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let writes: Vec<_> = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == "/write")
            .collect();
        assert_eq!(writes.len(), 1);
        let lp = String::from_utf8_lossy(&writes[0].body).into_owned();
        assert!(!lp.contains('\n'), "{lp:?}");
        assert!(lp.contains("geofence_name=\"Ho me\""), "{lp}");
    }

    #[tokio::test]
    async fn set_cost_rejects_bad_mode_and_missing_source() {
        let server = mock_db().await;
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/VIN1_1700000000/cost")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "mode": "flat",
                            "cost_per_unit": 1.0
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn db_failure_returns_502_not_404() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(wiremock::ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/ANYTHING")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn result_level_error_returns_502_not_404() {
        // InfluxDB v1 reports statement failures as HTTP 200 with a
        // result-level error object instead of a series.
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "results": [{"statement_id": 0, "error": "database not found: tesla"}]
                })),
            )
            .mount(&server)
            .await;
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/ANYTHING")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn unknown_session_returns_404() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "results": [{}]
                })),
            )
            .mount(&server)
            .await;
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/NOPE/cost")
                    .method("PUT")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "mode": "per_kwh",
                            "cost_per_unit": 0.3
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
