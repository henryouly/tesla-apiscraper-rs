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
        .route("/updates", get(updates))
        .route("/mileage", get(mileage))
        .route("/projected-range", get(projected_range))
        .route("/efficiency", get(efficiency))
        .route("/locations", get(locations))
        .route("/drive-stats", get(drive_stats))
        .route("/charges", get(charges))
        .route("/charging-stats", get(charging_stats))
        .route("/charge-details", get(charge_details))
        .route("/drives", get(drives))
        .route("/drive-details", get(drive_details))
        .route("/trip", get(trip))
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

/// One InfluxQL series: column names, rows, plus the series tag set
/// (e.g. `{"drive_id": "d-1"}` or `{"geofence_exit": "Home"}`).
/// InfluxQL `GROUP BY <tag>` and multi-tag measurements fan out into one
/// series per tag combination, and the group/tag value only survives in
/// `tags` — never as a column — so callers needing it must read it here.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesTable {
    pub columns: Vec<String>,
    pub rows: Vec<SeriesRow>,
    pub tags: std::collections::HashMap<String, String>,
}

/// Parse the `tags` object of one InfluxQL series into a string map.
/// Missing/non-string entries map to their absence (never an error: a
/// series without tags is valid, e.g. single-tag measurements).
fn series_tags(s: &serde_json::Value) -> std::collections::HashMap<String, String> {
    s.get("tags")
        .and_then(|t| t.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

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

/// Every series in an InfluxQL envelope. InfluxQL returns one series per
/// tag set, so raw queries over multi-tag measurements (e.g. `updates`,
/// tagged by vin + update_id) fan out into one series per tag combination.
/// Callers needing complete data must consume all of these, not just the
/// first — `series_rows` above is only correct for single-series shapes
/// (aggregates, `GROUP BY time`, single-tag measurements).
fn all_series(json: &serde_json::Value) -> Result<Vec<SeriesTable>, anyhow::Error> {
    let results = json
        .get("results")
        .and_then(|r| r.as_array())
        .ok_or_else(|| anyhow::anyhow!("malformed InfluxDB response"))?;
    let mut out = Vec::new();
    for res in results {
        if let Some(e) = res.get("error").and_then(|e| e.as_str()) {
            anyhow::bail!("InfluxDB query error: {e}");
        }
        let series = res
            .get("series")
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default();
        for s in &series {
            let columns: Vec<String> = s
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
            if let Some(values) = s.get("values").and_then(|v| v.as_array()) {
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
            out.push(SeriesTable {
                columns,
                rows,
                tags: series_tags(s),
            });
        }
    }
    Ok(out)
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

/// Maximum accepted query window for the updates endpoint: 5 years.
/// Unlike the 90-day `MAX_WINDOW_SECS` bound for fixed-bucket timeseries
/// (millions of points), the updates table is sparse raw rows (a 1-year
/// Grafana default window is a handful of rows), so a 5-year window stays
/// bounded; the raw query is additionally capped with `LIMIT 1000`.
const MAX_UPDATES_WINDOW_SECS: i64 = 5 * 365 * 24 * 60 * 60;

#[derive(Serialize, PartialEq, Debug)]
pub struct SoftwareUpdate {
    pub t: i64,
    pub version_before: Option<String>,
    pub version_after: Option<String>,
    pub install_start: Option<String>,
    pub install_end: Option<String>,
    pub status: Option<String>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct UpdatesResponse {
    pub vin: String,
    pub count: i64,
    pub updates: Vec<SoftwareUpdate>,
}

/// Count + raw update rows for the Updates page. Mirrors the two
/// `updates.json` panels: `count("status")` and the raw
/// version_before/after + install start/end + status table
/// (`ORDER BY time DESC LIMIT 1000`). Empty window → count 0,
/// updates [] (not an error).
async fn updates(
    State(state): State<AppState>,
    Query(p): Query<StatesParams>,
) -> Result<Json<UpdatesResponse>, ApiError> {
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
    if (to - from).num_seconds() > MAX_UPDATES_WINDOW_SECS {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "time range exceeds the 5-year maximum",
        ));
    }
    let from = from.to_rfc3339();
    let to = to.to_rfc3339();
    let vin = escape_literal(&p.vin);

    let cq = format!(
        "SELECT count(\"status\") FROM \"updates\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}'"
    );
    let cjson = state
        .db
        .query(&cq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let cseries = all_series(&cjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let rq = format!(
        "SELECT \"version_before\", \"version_after\", \"install_start\", \"install_end\", \"status\" FROM \"updates\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' ORDER BY time DESC LIMIT 1000"
    );
    let rjson = state
        .db
        .query(&rq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let rseries = all_series(&rjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    // `count` fans out one row per update_id series — sum them.
    let count: i64 = cseries
        .iter()
        .flat_map(|t| {
            t.rows
                .iter()
                .filter_map(|(_, row)| cell(&t.columns, row, "count"))
        })
        .map(|v| v as i64)
        .sum();
    // Raw rows fan out one series per update_id — combine and keep the
    // Grafana `ORDER BY time DESC` contract across series.
    let mut updates: Vec<SoftwareUpdate> = rseries
        .iter()
        .flat_map(|t| {
            t.rows.iter().map(|(time, row)| SoftwareUpdate {
                t: *time,
                version_before: str_cell(&t.columns, row, "version_before"),
                version_after: str_cell(&t.columns, row, "version_after"),
                install_start: str_cell(&t.columns, row, "install_start"),
                install_end: str_cell(&t.columns, row, "install_end"),
                status: str_cell(&t.columns, row, "status"),
            })
        })
        .collect();
    updates.sort_by_key(|u| std::cmp::Reverse(u.t));
    Ok(Json(UpdatesResponse {
        vin: p.vin,
        count,
        updates,
    }))
}

/// Maximum accepted query window for the mileage endpoint: 10 years.
/// Unlike the 90-day `MAX_WINDOW_SECS` bound for fixed 5-minute-bucket
/// timeseries (millions of points), mileage uses fixed daily buckets
/// (~3650 points for 10 years) and odometer history legitimately spans a
/// car's life, so a decade-long window stays bounded.
const MAX_MILEAGE_WINDOW_SECS: i64 = 10 * 365 * 24 * 60 * 60;

/// Stored `odometer` is in miles; the Grafana query multiplies by
/// `1.60934 * $length_unit` (`length_unit` 1 for km, 0.621371 for mi).
/// The km factor below times the standard `unit_factor()` reproduces that:
/// km → 1.60934, mi → 1.60934 * 0.621371 ≈ 1.0 (identity, as expected).
const MILES_TO_KM: f64 = 1.60934;

#[derive(Serialize, PartialEq, Debug)]
pub struct MileageResponse {
    pub vin: String,
    pub points: Vec<LevelPoint>,
}

/// Daily odometer history for the Mileage page. Mirrors the `mileage.json`
/// panel: `max("odometer") * factor` from `positions` with
/// `odometer > 0` (excludes unset readings), fixed `GROUP BY time(1d)`
/// buckets (never range-derived), `fill(previous)`. Empty window →
/// points [] (not an error).
async fn mileage(
    State(state): State<AppState>,
    Query(p): Query<RangeParams>,
) -> Result<Json<MileageResponse>, ApiError> {
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
    if (to - from).num_seconds() > MAX_MILEAGE_WINDOW_SECS {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "time range exceeds the 10-year maximum",
        ));
    }
    let from = from.to_rfc3339();
    let to = to.to_rfc3339();
    let factor = MILES_TO_KM * unit_factor(p.unit.as_deref())?;
    let vin = escape_literal(&p.vin);

    let q = format!(
        "SELECT max(\"odometer\")*{factor} FROM \"positions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"odometer\" > 0 GROUP BY time(1d) fill(previous)"
    );
    let json = state
        .db
        .query(&q, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (cols, rows) = series_rows(&json)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    // InfluxQL names single-aggregate columns `max`.
    let points = rows
        .iter()
        .map(|(t, row)| LevelPoint {
            t: *t,
            value: cell(&cols, row, "max"),
        })
        .collect();
    Ok(Json(MileageResponse { vin: p.vin, points }))
}

#[derive(Serialize, PartialEq, Debug)]
pub struct ProjectedPoint {
    pub t: i64,
    pub ideal: Option<f64>,
    pub rated: Option<f64>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct ProjectedRangeResponse {
    pub vin: String,
    pub projected: Vec<ProjectedPoint>,
    pub outside_temp: Vec<LevelPoint>,
}

/// Projected 100% range + outside temperature for the Projected Range
/// page. Mirrors the two `projected-range.json` panels with fixed
/// `GROUP BY time(5m)` buckets (never range-derived; see `overview`).
///
/// The Grafana panel computes the ratio server-side
/// (`sum(ideal)/sum(battery)*100*length_unit`). InfluxQL names for
/// arithmetic-expression columns are uncertain (full expression text or
/// positional `sum` variants), so instead we select the raw sums —
/// `sum("ideal_battery_range_km"), sum("battery_level"),
/// sum("rated_battery_range_km")` (columns `sum`, `sum_1`, `sum_2`) —
/// and compute the ratio + unit factor in Rust. Buckets where the
/// battery sum is missing or zero map to null points. Both queries carry
/// `AND "ideal_battery_range_km" > 0`, mirroring Grafana (excludes
/// garbage polls). Temperature stays °C (Grafana shows celsius; no
/// conversion). Empty window → empty vecs (not an error).
async fn projected_range(
    State(state): State<AppState>,
    Query(p): Query<RangeParams>,
) -> Result<Json<ProjectedRangeResponse>, ApiError> {
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

    let sq = format!(
        "SELECT sum(\"ideal_battery_range_km\"), sum(\"battery_level\"), sum(\"rated_battery_range_km\") FROM \"positions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"ideal_battery_range_km\" > 0 GROUP BY time(5m) fill(previous)"
    );
    let sjson = state
        .db
        .query(&sq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (scols, srows) = series_rows(&sjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let tq = format!(
        "SELECT mean(\"outside_temp\") FROM \"positions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"ideal_battery_range_km\" > 0 GROUP BY time(5m) fill(previous)"
    );
    let tjson = state
        .db
        .query(&tq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (tcols, trows) = series_rows(&tjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    // InfluxQL names positional same-aggregate columns `sum`, `sum_1`,
    // `sum_2` in select order: ideal sum, battery sum, rated sum.
    let projected = srows
        .iter()
        .map(|(t, row)| {
            let ideal_sum = cell(&scols, row, "sum");
            let battery_sum = cell(&scols, row, "sum_1");
            let rated_sum = cell(&scols, row, "sum_2");
            let ratio = |num: Option<f64>| match (num, battery_sum) {
                (Some(n), Some(b)) if b != 0.0 => Some(n / b * 100.0 * factor),
                _ => None,
            };
            ProjectedPoint {
                t: *t,
                ideal: ratio(ideal_sum),
                rated: ratio(rated_sum),
            }
        })
        .collect();
    let outside_temp = trows
        .iter()
        .map(|(t, row)| LevelPoint {
            t: *t,
            value: cell(&tcols, row, "mean"),
        })
        .collect();
    Ok(Json(ProjectedRangeResponse {
        vin: p.vin,
        projected,
        outside_temp,
    }))
}

#[derive(Serialize, PartialEq, Debug)]
pub struct EfficiencyResponse {
    pub vin: String,
    pub avg_consumption_wh_per_km: Option<f64>,
    pub logged_distance: f64,
    pub per_drive: Vec<LevelPoint>,
}

/// Maximum accepted query window for the efficiency endpoint: 5 years —
/// same bound as `MAX_UPDATES_WINDOW_SECS` (kept as its own const so each
/// endpoint documents its own horizon). Drives are sparse per-drive points
/// (one row per drive, not fixed buckets), so a multi-year window stays
/// bounded; the raw per-drive query is additionally capped with
/// `LIMIT 10000`.
const MAX_EFFICIENCY_WINDOW_SECS: i64 = 5 * 365 * 24 * 60 * 60;

/// Average consumption + logged distance + per-drive consumption for the
/// Efficiency page. Mirrors the three `efficiency.json` panels.
///
/// Query nuances (the JSON governs where the scope-doc paraphrase differs):
/// - Avg Consumption selects the raw `sum("energy_used_wh")`,
///   `sum("distance_meters")` (columns `sum`, `sum_1`) with `AND
///   "duration_seconds" >= 0 AND "distance_meters" > 100`, and the
///   `*1000` ratio is computed in Rust (same raw-sums-then-ratio pattern
///   as `projected_range`; missing/zero distance sum → null). No unit
///   factor — always Wh/km.
/// - Logged Distance selects `sum("distance_meters")` with ONLY `AND
///   "duration_seconds" >= 0`: the JSON has no `distance_meters > 100`
///   filter here (the scope doc says "same filters"; the JSON governs).
///   The `/1000*$length_unit` scaling is applied in Rust via
///   `unit_factor()`. Empty window → 0.
/// - Per-drive selects raw `"energy_used_wh", "distance_meters"` with both
///   filters. `drives` is tagged vin + drive_id, so the raw query fans out
///   into one series per drive: consume `all_series`, compute the per-row
///   `energy/distance*1000` ratio in Rust (zero/missing distance → null),
///   combine across series and sort by time ASC.
/// - Known data caveat (issue #93): `energy_used_wh` is currently 1000x
///   under (drive power is logged as kW, not W). Both stats read the same
///   fields, so no rework is needed — the endpoint mirrors Grafana exactly.
async fn efficiency(
    State(state): State<AppState>,
    Query(p): Query<RangeParams>,
) -> Result<Json<EfficiencyResponse>, ApiError> {
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
    if (to - from).num_seconds() > MAX_EFFICIENCY_WINDOW_SECS {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "time range exceeds the 5-year maximum",
        ));
    }
    let from = from.to_rfc3339();
    let to = to.to_rfc3339();
    let factor = unit_factor(p.unit.as_deref())?;
    let vin = escape_literal(&p.vin);

    let aq = format!(
        "SELECT sum(\"energy_used_wh\"), sum(\"distance_meters\") FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0 AND \"distance_meters\" > 100"
    );
    let ajson = state
        .db
        .query(&aq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let aseries = all_series(&ajson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let dq = format!(
        "SELECT sum(\"distance_meters\") FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0"
    );
    let djson = state
        .db
        .query(&dq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let dseries = all_series(&djson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let pq = format!(
        "SELECT \"energy_used_wh\", \"distance_meters\" FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0 AND \"distance_meters\" > 100 LIMIT 10000"
    );
    let pjson = state
        .db
        .query(&pq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let pseries = all_series(&pjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    // Aggregate sums fan out one row per drive_id series, so add them up
    // across all series before taking the ratio (a single-series window
    // collapses to the same math). Null cells contribute nothing; a window
    // with no sum rows at all (or a zero distance total) maps to null.
    let mut energy_sum = 0.0;
    let mut distance_sum = 0.0;
    let mut sum_rows = 0usize;
    for t in &aseries {
        for (_, row) in &t.rows {
            let e = cell(&t.columns, row, "sum");
            let d = cell(&t.columns, row, "sum_1");
            if e.is_some() || d.is_some() {
                sum_rows += 1;
                energy_sum += e.unwrap_or(0.0);
                distance_sum += d.unwrap_or(0.0);
            }
        }
    }
    let avg_consumption_wh_per_km = if sum_rows > 0 && distance_sum != 0.0 {
        Some(energy_sum / distance_sum * 1000.0)
    } else {
        None
    };

    let logged_sum: f64 = dseries
        .iter()
        .flat_map(|t| {
            t.rows
                .iter()
                .filter_map(|(_, row)| cell(&t.columns, row, "sum"))
        })
        .sum();
    let logged_distance = logged_sum / 1000.0 * factor;

    // Raw per-drive rows fan out one series per drive_id — combine across
    // series and restore chronological order (single-series windows sort
    // to themselves).
    let mut per_drive: Vec<LevelPoint> = pseries
        .iter()
        .flat_map(|t| {
            t.rows.iter().map(|(time, row)| {
                let value = match (
                    cell(&t.columns, row, "energy_used_wh"),
                    cell(&t.columns, row, "distance_meters"),
                ) {
                    (Some(e), Some(d)) if d != 0.0 => Some(e / d * 1000.0),
                    _ => None,
                };
                LevelPoint { t: *time, value }
            })
        })
        .collect();
    per_drive.sort_by_key(|p| p.t);
    Ok(Json(EfficiencyResponse {
        vin: p.vin,
        avg_consumption_wh_per_km,
        logged_distance,
        per_drive,
    }))
}

#[derive(Serialize, PartialEq, Debug)]
pub struct TopDestination {
    /// Geofence name OR end address, whichever keyed the destination
    /// (see `locations` docs). Named `place` because it no longer holds
    /// a geofence exclusively.
    pub place: String,
    pub visits: i64,
}

/// Place key for one drive row: `geofence_exit` when non-empty, else
/// `end_address` when non-empty, else `"Unknown"`. Shared by the
/// locations and drive-stats endpoints so both pages group identically.
fn place_key(geofence: Option<String>, address: Option<String>) -> String {
    geofence
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| address.filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Group raw per-drive series (which must carry `geofence_exit` and
/// `end_address` columns) into top destinations: counts per place key,
/// sorted desc by visits (ties broken by place name for deterministic
/// output). Shared by the locations and drive-stats handlers.
fn top_destinations(rseries: &[SeriesTable]) -> Vec<TopDestination> {
    let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for t in rseries {
        for (_, row) in &t.rows {
            let geofence = str_cell(&t.columns, row, "geofence_exit");
            let address = str_cell(&t.columns, row, "end_address");
            *counts.entry(place_key(geofence, address)).or_insert(0) += 1;
        }
    }
    let mut top: Vec<TopDestination> = counts
        .into_iter()
        .map(|(place, visits)| TopDestination { place, visits })
        .collect();
    top.sort_by(|a, b| b.visits.cmp(&a.visits).then_with(|| a.place.cmp(&b.place)));
    top
}

#[derive(Serialize, PartialEq, Debug)]
pub struct VisitedRow {
    pub t: i64,
    pub address: Option<String>,
    pub geofence: Option<String>,
    pub distance: Option<f64>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct LocationsResponse {
    pub vin: String,
    pub top: Vec<TopDestination>,
    pub last_visited: Vec<VisitedRow>,
    pub drives_logged: i64,
}

/// Maximum accepted query window for the locations endpoint: 5 years —
/// same sparse-rows rationale as `MAX_EFFICIENCY_WINDOW_SECS` (kept as its
/// own const so each endpoint documents its own horizon). Drives are one
/// row per drive, so a multi-year window stays bounded; the raw per-drive
/// query is additionally capped with `LIMIT 10000` (one row per series, so
/// effectively complete — same cap rationale as the states endpoint) while
/// last-visited truncates the combined rows to 50.
const MAX_LOCATIONS_WINDOW_SECS: i64 = 5 * 365 * 24 * 60 * 60;

/// Top destinations + last-visited table + drives-logged stat for the
/// Locations page. Mirrors the three `locations.json` panels.
///
/// Query nuances (the JSON governs, except top destinations — see below):
/// - Top Destinations deliberately DIVERGES from Grafana: the JSON groups
///   `count("distance_meters")` by tag `geofence_exit`, which lumps every
///   drive ending outside a geofence into one giant "Unknown" bucket
///   (useless when most drives end outside any geofence). Instead, ONE raw
///   query serves both top + last_visited (no separate `GROUP BY` count
///   query): the combined per-drive rows are grouped in Rust by place key
///   = `geofence_exit` when non-empty, else `end_address` when non-empty,
///   else `"Unknown"`. Counts per key, sorted desc by visits (ties broken
///   by place name for deterministic output).
/// - Last Visited selects the RAW `"end_address", "geofence_exit",
///   "distance_meters"` (no InfluxQL arithmetic: expression column names
///   are uncertain, same rationale as `projected_range`) with `AND
///   "duration_seconds" >= 0`, `ORDER BY time DESC LIMIT 10000`, and the
///   `/1000 * length_unit` scaling happens in Rust. `drives` is tagged
///   vin + drive_id so the raw query fans out one series per drive:
///   consume `all_series`, combine, re-sort time DESC in Rust, and
///   truncate the combined rows to 50 (`LIMIT` applies per series, so
///   combining without truncating could over-return; the 10000 cap keeps
///   the raw fetch bounded while remaining effectively complete since
///   per-drive rows are one per series).
/// - Drives Logged is `count("distance_meters")` with time + vin ONLY:
///   the JSON has no `duration_seconds` filter here (follow the JSON).
///   Summed across series via `all_series`. Empty window → 0.
async fn locations(
    State(state): State<AppState>,
    Query(p): Query<RangeParams>,
) -> Result<Json<LocationsResponse>, ApiError> {
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
    if (to - from).num_seconds() > MAX_LOCATIONS_WINDOW_SECS {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "time range exceeds the 5-year maximum",
        ));
    }
    let from = from.to_rfc3339();
    let to = to.to_rfc3339();
    let factor = unit_factor(p.unit.as_deref())?;
    let vin = escape_literal(&p.vin);

    let rq = format!(
        "SELECT \"end_address\", \"geofence_exit\", \"distance_meters\", \"end_time\" FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0 ORDER BY time DESC LIMIT 10000"
    );
    let rjson = state
        .db
        .query(&rq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let rseries = all_series(&rjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let dq = format!(
        "SELECT count(\"distance_meters\") FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}'"
    );
    let djson = state
        .db
        .query(&dq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let dseries = all_series(&djson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    // Combined per-drive rows serve the last-visited table; the top
    // destinations come from the shared `top_destinations` helper below.
    struct PlaceRow {
        t: i64,
        address: Option<String>,
        geofence: Option<String>,
        distance: Option<f64>,
    }
    let mut place_rows: Vec<PlaceRow> = rseries
        .iter()
        .flat_map(|t| {
            t.rows.iter().map(|(time, row)| {
                let geofence = str_cell(&t.columns, row, "geofence_exit");
                let address = str_cell(&t.columns, row, "end_address");
                let distance =
                    cell(&t.columns, row, "distance_meters").map(|d| d / 1000.0 * factor);
                // Drive points are stamped with the drive START time
                // (`session.start_time`); the visit happened at the END, so
                // prefer the `end_time` field, falling back to point time.
                let t = str_cell(&t.columns, row, "end_time")
                    .and_then(|s| {
                        chrono::DateTime::parse_from_rfc3339(&s)
                            .ok()
                            .map(|dt| dt.timestamp_millis())
                    })
                    .unwrap_or(*time);
                PlaceRow {
                    t,
                    address,
                    geofence,
                    distance,
                }
            })
        })
        .collect();

    // Top destinations via the shared helper (same grouping the Drive
    // Stats page uses), computed from the raw series.
    let top = top_destinations(&rseries);

    // Raw per-drive rows fan out one series per drive_id — combine across
    // series and restore the Grafana `ORDER BY time DESC` contract across
    // series, truncating the combined rows to 50.
    place_rows.sort_by_key(|row| std::cmp::Reverse(row.t));
    let last_visited: Vec<VisitedRow> = place_rows
        .into_iter()
        .take(50)
        .map(
            |PlaceRow {
                 t,
                 address,
                 geofence,
                 distance,
             }| VisitedRow {
                t,
                address,
                geofence,
                distance,
            },
        )
        .collect();

    let drives_logged: i64 = dseries
        .iter()
        .flat_map(|t| {
            t.rows
                .iter()
                .filter_map(|(_, row)| cell(&t.columns, row, "count"))
        })
        .map(|v| v as i64)
        .sum();
    Ok(Json(LocationsResponse {
        vin: p.vin,
        top,
        last_visited,
        drives_logged,
    }))
}

#[derive(Serialize, PartialEq, Debug)]
pub struct DriveStatsResponse {
    pub vin: String,
    pub drives: i64,
    pub distance: f64,
    pub energy_kwh: Option<f64>,
    pub median_drive: Option<f64>,
    pub max_speed: Option<f64>,
    /// Raw per-poll speeds converted to the display unit (mph →
    /// `*1.60934*unit`). No timestamps: the histogram buckets values only.
    pub speeds: Vec<f64>,
    pub top_destinations: Vec<TopDestination>,
}

/// Maximum accepted query window for the drive-stats endpoint: 5 years —
/// same sparse-rows rationale as `MAX_EFFICIENCY_WINDOW_SECS` (kept as its
/// own const so each endpoint documents its own horizon). Drives are one
/// row per drive; the dense per-poll `positions` speed query is capped with
/// `LIMIT 50000` instead (Grafana streams the window unbounded, but a cap
/// keeps the JSON small; histogram shape is unaffected at these counts).
const MAX_DRIVE_STATS_WINDOW_SECS: i64 = 5 * 365 * 24 * 60 * 60;

/// Drive counts, distances, speeds, and top destinations for the Drive
/// Stats page. Mirrors the seven `drive-stats.json` panels.
///
/// Query nuances (the JSON governs):
/// - # of Drives is `count("distance_meters")` with time + vin ONLY (no
///   `duration_seconds` filter — same as the Locations Drives Logged
///   panel). Summed across series via `all_series`.
/// - Total Distance + Energy Used come from ONE combined query —
///   `count("distance_meters"), sum("distance_meters"),
///   sum("energy_used_wh")` with `AND "duration_seconds" >= 0` (columns
///   `count`, `sum`, `sum_1`; the count cell is unused) — with the
///   `/1000*$length_unit` scaling applied in Rust via `unit_factor()`.
///   Energy is kWh with NO unit factor. Known data caveat (issue #93):
///   `energy_used_wh` is currently 1000x under; the endpoint mirrors
///   Grafana exactly (same as efficiency).
/// - Median Drive is `percentile("distance_meters", 50)` (column
///   `percentile`); null when empty. Converted `/1000*unit` in Rust.
/// - Max Speed is `max("max_speed")`; stored speeds are MPH so the
///   `*1.60934*$length_unit` scaling becomes `MILES_TO_KM * unit_factor()`
///   in Rust (same pattern as mileage). Null when empty.
/// - Speed Distribution selects raw `"speed"` from `positions` WHERE
///   `"speed" > 1`, converted mph → display in Rust. First non-null value
///   per row; nulls skipped.
/// - Top Destinations reuses the shared `top_destinations` helper (same
///   place-grouping as Locations for the same range) over its own raw
///   `SELECT "end_address", "geofence_exit" ... AND "duration_seconds" >=
///   0 LIMIT 10000` query — only the two grouping columns are selected
///   (no distance/end_time needed).
async fn drive_stats(
    State(state): State<AppState>,
    Query(p): Query<RangeParams>,
) -> Result<Json<DriveStatsResponse>, ApiError> {
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
    if (to - from).num_seconds() > MAX_DRIVE_STATS_WINDOW_SECS {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "time range exceeds the 5-year maximum",
        ));
    }
    let from = from.to_rfc3339();
    let to = to.to_rfc3339();
    let factor = unit_factor(p.unit.as_deref())?;
    let speed_factor = MILES_TO_KM * factor;
    let vin = escape_literal(&p.vin);

    let aq = format!(
        "SELECT count(\"distance_meters\"), sum(\"distance_meters\"), sum(\"energy_used_wh\") FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0"
    );
    let ajson = state
        .db
        .query(&aq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let aseries = all_series(&ajson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let cq = format!(
        "SELECT count(\"distance_meters\") FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}'"
    );
    let cjson = state
        .db
        .query(&cq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let cseries = all_series(&cjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let pq = format!(
        "SELECT percentile(\"distance_meters\", 50) FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0"
    );
    let pjson = state
        .db
        .query(&pq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let pseries = all_series(&pjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let mq = format!(
        "SELECT max(\"max_speed\") FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0"
    );
    let mjson = state
        .db
        .query(&mq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let mseries = all_series(&mjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let sq = format!(
        "SELECT \"speed\" FROM \"positions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"speed\" > 1 LIMIT 50000"
    );
    let sjson = state
        .db
        .query(&sq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let sseries = all_series(&sjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let tq = format!(
        "SELECT \"end_address\", \"geofence_exit\" FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0 LIMIT 10000"
    );
    let tjson = state
        .db
        .query(&tq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let tseries = all_series(&tjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    // Aggregate sums fan out one row per drive_id series, so add them up
    // across all series (same pattern as `efficiency`). A window with no
    // sum rows at all maps distance → 0 and energy → null.
    let mut distance_sum = 0.0;
    let mut energy_sum = 0.0;
    let mut energy_rows = 0usize;
    for t in &aseries {
        for (_, row) in &t.rows {
            if let Some(d) = cell(&t.columns, row, "sum") {
                distance_sum += d;
            }
            if let Some(e) = cell(&t.columns, row, "sum_1") {
                energy_sum += e;
                energy_rows += 1;
            }
        }
    }
    let distance = distance_sum / 1000.0 * factor;
    // Mirrors Grafana exactly (see the #93 note above): no unit factor.
    let energy_kwh = if energy_rows > 0 {
        Some(energy_sum / 1000.0)
    } else {
        None
    };

    let drives: i64 = cseries
        .iter()
        .flat_map(|t| {
            t.rows
                .iter()
                .filter_map(|(_, row)| cell(&t.columns, row, "count"))
        })
        .map(|v| v as i64)
        .sum();

    let median_drive = pseries
        .iter()
        .flat_map(|t| {
            t.rows
                .iter()
                .filter_map(|(_, row)| cell(&t.columns, row, "percentile"))
        })
        .next()
        .map(|v| v / 1000.0 * factor);

    // `max` merges to one row, but take the max across series anyway so a
    // fanned-out window still reports the true maximum.
    let max_speed = mseries
        .iter()
        .flat_map(|t| {
            t.rows
                .iter()
                .filter_map(|(_, row)| cell(&t.columns, row, "max"))
        })
        .fold(None, |acc: Option<f64>, v| {
            Some(acc.map_or(v, |a: f64| a.max(v)))
        })
        .map(|v| v * speed_factor);

    let speeds: Vec<f64> = sseries
        .iter()
        .flat_map(|t| {
            t.rows
                .iter()
                .filter_map(|(_, row)| cell(&t.columns, row, "speed").map(|v| v * speed_factor))
        })
        .collect();

    let top_destinations = top_destinations(&tseries);

    Ok(Json(DriveStatsResponse {
        vin: p.vin,
        drives,
        distance,
        energy_kwh,
        median_drive,
        max_speed,
        speeds,
        top_destinations,
    }))
}

#[derive(Serialize, PartialEq, Debug)]
pub struct ChargeSessionRow {
    pub t: i64,
    pub charge_id: String,
    pub location: Option<String>,
    pub energy_kwh: Option<f64>,
    pub duration_min: Option<f64>,
    pub cost: Option<f64>,
    pub start_pct: Option<f64>,
    pub end_pct: Option<f64>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct SessionPower {
    pub charge_id: String,
    pub points: Vec<LevelPoint>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct ChargesResponse {
    pub vin: String,
    pub count: i64,
    pub energy_kwh: f64,
    pub total_cost: f64,
    pub avg_cost_per_kwh: Option<f64>,
    pub sessions: Vec<ChargeSessionRow>,
    pub energy_per_day: Vec<LevelPoint>,
    pub power_by_session: Vec<SessionPower>,
}

/// Shared charging summary: the four summary stats are IDENTICAL for the
/// Charges (Stage 9) and Charging Stats (Stage 11) pages. Count is
/// `count("energy_added_wh")` with time + vin ONLY (no `duration_seconds`
/// filter); energy/cost/avg come from ONE combined query —
/// `count("energy_added_wh"), sum("energy_added_wh"), sum("cost")` with `AND
/// "duration_seconds" >= 0` (columns `count`, `sum`, `sum_1`; the count cell
/// is unused). The `/1000` scaling and the `sum(cost)/sum(energy)*1000`
/// ratio are computed in Rust. Aggregate sums fan out one row per charge_id
/// series, so they are folded across all series. A window with no sum rows
/// at all maps energy/cost → 0 and avg → null.
///
/// `from`/`to` are RFC3339 bounds and `vin` is already escaped for InfluxQL.
async fn charging_summary(
    db: &crate::influxdb::InfluxDb,
    from: &str,
    to: &str,
    vin: &str,
) -> Result<(i64, f64, f64, Option<f64>), ApiError> {
    let aq = format!(
        "SELECT count(\"energy_added_wh\"), sum(\"energy_added_wh\"), sum(\"cost\") FROM \"charging_sessions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0"
    );
    let ajson = db
        .query(&aq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let aseries = all_series(&ajson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let cq = format!(
        "SELECT count(\"energy_added_wh\") FROM \"charging_sessions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}'"
    );
    let cjson = db
        .query(&cq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let cseries = all_series(&cjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let mut energy_sum = 0.0;
    let mut cost_sum = 0.0;
    let mut sum_rows = 0usize;
    for t in &aseries {
        for (_, row) in &t.rows {
            let e = cell(&t.columns, row, "sum");
            let c = cell(&t.columns, row, "sum_1");
            if e.is_some() || c.is_some() {
                sum_rows += 1;
                energy_sum += e.unwrap_or(0.0);
                cost_sum += c.unwrap_or(0.0);
            }
        }
    }
    let energy_kwh = energy_sum / 1000.0;
    let total_cost = cost_sum;
    let avg_cost_per_kwh = if sum_rows > 0 && energy_sum != 0.0 {
        Some(cost_sum / energy_sum * 1000.0)
    } else {
        None
    };

    let count: i64 = cseries
        .iter()
        .flat_map(|t| {
            t.rows
                .iter()
                .filter_map(|(_, row)| cell(&t.columns, row, "count"))
        })
        .map(|v| v as i64)
        .sum();
    Ok((count, energy_kwh, total_cost, avg_cost_per_kwh))
}

/// Maximum accepted query window for the charges endpoint: 5 years —
/// same sparse-rows rationale as `MAX_EFFICIENCY_WINDOW_SECS` (kept as its
/// own const so each endpoint documents its own horizon). Charging sessions
/// are one row per session, so a multi-year window stays bounded; the raw
/// sessions query is additionally capped with `LIMIT 1000` and the dense
/// per-reading power query with `LIMIT 20000`.
const MAX_CHARGES_WINDOW_SECS: i64 = 5 * 365 * 24 * 60 * 60;

/// Charge counts, energy, cost, and per-session power for the Charges page.
/// Mirrors the seven `charges.json` panels. No `unit` param: every panel is
/// energy/cost/power/battery — nothing here is a distance.
///
/// Query nuances (the JSON governs):
/// - # of Charges is `count("energy_added_wh")` with time + vin ONLY (no
///   `duration_seconds` filter — same shape as the Locations/Drive Stats
///   count panels). Summed across series via `all_series`.
/// - Energy Added + Total Cost + Avg Cost/kWh come from ONE combined query
///   — `count("energy_added_wh"), sum("energy_added_wh"), sum("cost")`
///   with `AND "duration_seconds" >= 0` (columns `count`, `sum`, `sum_1`;
///   the count cell is unused — the unfiltered count above is the stat).
///   The `/1000` scaling and the `sum(cost)/sum(energy)*1000` ratio are
///   computed in Rust (same raw-sums-then-ratio pattern as
///   `projected_range`); currency formatting is frontend-only (the backend
///   returns raw numbers, the page formats with the ChargeCost
///   `toFixed(2)` conventions). Missing/zero energy sum → null avg.
/// - Charges table selects the raw `"geofence_name", "energy_added_wh",
///   "duration_seconds", "cost", "start_battery_level",
///   "end_battery_level"` with `AND "duration_seconds" >= 0`. The JSON has
///   NO `ORDER BY` (InfluxDB defaults to time ASC), so rows are sorted ASC
///   in Rust — flagging the assumption here for a live-parity check
///   against Grafana. The JSON has NO `LIMIT`; a `LIMIT 1000` is added to
///   bound the response (sessions are sparse, so effectively complete).
///   `GROUP BY "charge_id"` keeps one series per session (a raw multi-tag
///   query without it merges into one tag-less series and the per-session
///   id — which only survives in the series tags — is lost); the id is
///   extracted from `tags` into each row.
///   Multi-series (charge_id tags) → `all_series`; energy /1000 and
///   duration /60 converted in Rust.
/// - Energy per Day is `sum("energy_added_wh")` with `GROUP BY time(1d)
///   fill(0)`, fixed daily buckets, /1000 in Rust. The `fill(0)` is a
///   DELIBERATE difference from the `fill(previous)` convention used
///   elsewhere: day buckets with no charging must read zero (bars need
///   zeros, not carried-forward values).
/// - Charging Power by Session is `mean("power")` from `charge_readings`
///   (charge power IS watts — unlike drive power, see #93 — so /1000 →
///   kW is CORRECT here). Grafana groups by the range-derived
///   `$__interval`; our rule forbids that over a bounded window, so fixed
///   `GROUP BY time(5m), "charge_id"` + `LIMIT 20000` with the multi-year
///   window capped above. One response series per charge_id tag; nulls
///   stay null (render as gaps).
async fn charges(
    State(state): State<AppState>,
    Query(p): Query<StatesParams>,
) -> Result<Json<ChargesResponse>, ApiError> {
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
    if (to - from).num_seconds() > MAX_CHARGES_WINDOW_SECS {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "time range exceeds the 5-year maximum",
        ));
    }
    let from = from.to_rfc3339();
    let to = to.to_rfc3339();
    let vin = escape_literal(&p.vin);

    let (count, energy_kwh, total_cost, avg_cost_per_kwh) =
        charging_summary(&state.db, &from, &to, &vin).await?;

    let sq = format!(
        "SELECT \"geofence_name\", \"energy_added_wh\", \"duration_seconds\", \"cost\", \"start_battery_level\", \"end_battery_level\" FROM \"charging_sessions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0 GROUP BY \"charge_id\" LIMIT 1000"
    );
    let sjson = state
        .db
        .query(&sq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let sseries = all_series(&sjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let eq = format!(
        "SELECT sum(\"energy_added_wh\") FROM \"charging_sessions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0 GROUP BY time(1d) fill(0)"
    );
    let ejson = state
        .db
        .query(&eq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (ecols, erows) = series_rows(&ejson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let pq = format!(
        "SELECT mean(\"power\") FROM \"charge_readings\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' GROUP BY time(5m), \"charge_id\" LIMIT 20000"
    );
    let pjson = state
        .db
        .query(&pq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let pseries = all_series(&pjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    // Raw session rows fan out one series per charge_id — combine across
    // series and sort time ASC (restoring the JSON's implicit
    // no-ORDER-BY/Influx-default-ASC contract across series). The explicit
    // `GROUP BY "charge_id"` keeps one series per session: without it a raw
    // multi-tag query merges into a single tag-less series and the
    // per-session id is lost. The id only survives in `tags` (see
    // `SeriesTable`), so it is read from there.
    let mut sessions: Vec<ChargeSessionRow> = sseries
        .iter()
        .flat_map(|t| {
            t.rows.iter().map(|(time, row)| ChargeSessionRow {
                t: *time,
                charge_id: t.tags.get("charge_id").cloned().unwrap_or_default(),
                location: str_cell(&t.columns, row, "geofence_name"),
                energy_kwh: cell(&t.columns, row, "energy_added_wh").map(|v| v / 1000.0),
                duration_min: cell(&t.columns, row, "duration_seconds").map(|v| v / 60.0),
                cost: cell(&t.columns, row, "cost"),
                start_pct: cell(&t.columns, row, "start_battery_level"),
                end_pct: cell(&t.columns, row, "end_battery_level"),
            })
        })
        .collect();
    sessions.sort_by_key(|s| s.t);

    // InfluxQL names the single-aggregate column `sum`.
    let energy_per_day = erows
        .iter()
        .map(|(t, row)| LevelPoint {
            t: *t,
            value: cell(&ecols, row, "sum").map(|v| v / 1000.0),
        })
        .collect();

    // One response series per charge_id tag (the tag value only survives
    // in `tags`, never as a column — see `SeriesTable`); InfluxQL names
    // the single-aggregate column `mean`. Points stay in series order
    // with time-ASC restored per series.
    let mut power_by_session: Vec<SessionPower> = pseries
        .iter()
        .map(|t| {
            let mut points: Vec<LevelPoint> = t
                .rows
                .iter()
                .map(|(time, row)| LevelPoint {
                    t: *time,
                    value: cell(&t.columns, row, "mean").map(|v| v / 1000.0),
                })
                .collect();
            points.sort_by_key(|p| p.t);
            SessionPower {
                charge_id: t.tags.get("charge_id").cloned().unwrap_or_default(),
                points,
            }
        })
        .collect();
    power_by_session.sort_by(|a, b| a.charge_id.cmp(&b.charge_id));

    Ok(Json(ChargesResponse {
        vin: p.vin,
        count,
        energy_kwh,
        total_cost,
        avg_cost_per_kwh,
        sessions,
        energy_per_day,
        power_by_session,
    }))
}

#[derive(Serialize, PartialEq, Debug)]
pub struct SocDeltaPoint {
    pub t: i64,
    pub start: Option<f64>,
    pub end: Option<f64>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct TopStation {
    pub station: String,
    pub energy_kwh: f64,
    pub sessions: i64,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct ChargingStatsResponse {
    pub vin: String,
    pub count: i64,
    pub energy_kwh: f64,
    pub total_cost: f64,
    pub avg_cost_per_kwh: Option<f64>,
    pub soc_delta: Vec<SocDeltaPoint>,
    pub top_stations: Vec<TopStation>,
}

/// Maximum accepted query window for the charging-stats endpoint: 5 years —
/// same sparse-session rationale as `MAX_CHARGES_WINDOW_SECS` (kept as its
/// own const so each endpoint documents its own horizon). Charging sessions
/// are a few per week, so a multi-year window stays bounded.
const MAX_CHARGING_STATS_WINDOW_SECS: i64 = 5 * 365 * 24 * 60 * 60;

/// Charging summaries and top stations for the Charging Stats page. Mirrors
/// the seven `charging-stats.json` panels. No `unit` param: every panel is
/// energy/cost/battery — nothing here is a distance.
///
/// Query nuances (the JSON governs):
/// - The four summary stats are IDENTICAL to the Charges page (count
///   unfiltered; energy/cost/avg with `AND "duration_seconds" >= 0`) and
///   share the `charging_summary` helper — same queries, one implementation.
/// - Charge Level Delta is `mean("start_battery_level"),
///   mean("end_battery_level")` with `AND "duration_seconds" >= 0` (columns
///   `mean`, `mean_1`). Sessions are sparse (a few per week), so fixed
///   `GROUP BY time(1d) fill(previous)` buckets are used — never
///   range-derived; 1d matches the energy-per-day granularity and keeps
///   buckets meaningful.
/// - Top Charging Stations is `sum("energy_added_wh")` +
///   `count("energy_added_wh")` with `AND "duration_seconds" >= 0`,
///   `GROUP BY "geofence_name"`. One series per station; the station label
///   comes from the series TAGS (same pattern as the locations top). Unlike
///   drives — where most drives end outside any geofence — charging sessions
///   are usually geofenced, so the JSON grouping is followed as-is (no
///   address fallback). Missing/empty tag → "Unknown". The `/1000` scaling
///   is applied in Rust. Sorted DESC by energy (the Grafana transformation
///   sorts by Energy kWh desc — NOT by visits).
async fn charging_stats(
    State(state): State<AppState>,
    Query(p): Query<StatesParams>,
) -> Result<Json<ChargingStatsResponse>, ApiError> {
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
    if (to - from).num_seconds() > MAX_CHARGING_STATS_WINDOW_SECS {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "time range exceeds the 5-year maximum",
        ));
    }
    let from = from.to_rfc3339();
    let to = to.to_rfc3339();
    let vin = escape_literal(&p.vin);

    let (count, energy_kwh, total_cost, avg_cost_per_kwh) =
        charging_summary(&state.db, &from, &to, &vin).await?;

    let dq = format!(
        "SELECT mean(\"start_battery_level\"), mean(\"end_battery_level\") FROM \"charging_sessions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0 GROUP BY time(1d) fill(previous)"
    );
    let djson = state
        .db
        .query(&dq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (dcols, drows) = series_rows(&djson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let tq = format!(
        "SELECT sum(\"energy_added_wh\"), count(\"energy_added_wh\") FROM \"charging_sessions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0 GROUP BY \"geofence_name\""
    );
    let tjson = state
        .db
        .query(&tq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let tseries = all_series(&tjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    // InfluxQL names positional same-aggregate columns `mean`, `mean_1` in
    // select order: start mean, end mean.
    let soc_delta = drows
        .iter()
        .map(|(t, row)| SocDeltaPoint {
            t: *t,
            start: cell(&dcols, row, "mean"),
            end: cell(&dcols, row, "mean_1"),
        })
        .collect();

    // `GROUP BY "geofence_name"` fans out one series per station; the
    // station name only survives in `tags` (see `SeriesTable`). Fold per
    // station across series (defensive: one series per tag value in
    // practice), then sort DESC by energy to match the Grafana `sortBy`
    // transformation on Energy kWh.
    let mut by_station: std::collections::HashMap<String, (f64, i64)> =
        std::collections::HashMap::new();
    for t in &tseries {
        let station = t
            .tags
            .get("geofence_name")
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| "Unknown".to_string());
        let mut energy = 0.0;
        let mut sessions = 0i64;
        let mut touched = false;
        for (_, row) in &t.rows {
            if let Some(e) = cell(&t.columns, row, "sum") {
                energy += e;
                touched = true;
            }
            if let Some(c) = cell(&t.columns, row, "count") {
                sessions += c as i64;
                touched = true;
            }
        }
        if touched {
            let entry = by_station.entry(station).or_insert((0.0, 0));
            entry.0 += energy;
            entry.1 += sessions;
        }
    }
    let mut top_stations: Vec<TopStation> = by_station
        .into_iter()
        .map(|(station, (energy_wh, sessions))| TopStation {
            station,
            energy_kwh: energy_wh / 1000.0,
            sessions,
        })
        .collect();
    top_stations.sort_by(|a, b| {
        b.energy_kwh
            .partial_cmp(&a.energy_kwh)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.station.cmp(&b.station))
    });

    Ok(Json(ChargingStatsResponse {
        vin: p.vin,
        count,
        energy_kwh,
        total_cost,
        avg_cost_per_kwh,
        soc_delta,
        top_stations,
    }))
}

#[derive(Serialize, PartialEq, Debug)]
pub struct ChargeCurvePoint {
    pub t: i64,
    pub battery: Option<f64>,
    pub power_kw: Option<f64>,
    pub added_kwh: Option<f64>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct ChargeDetailsResponse {
    pub charge_id: String,
    pub cost: Option<f64>,
    pub duration_s: Option<f64>,
    pub geofence: Option<String>,
    pub energy_kwh: Option<f64>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub soc_added: Option<f64>,
    pub curve: Vec<ChargeCurvePoint>,
    /// Session span from charging_sessions point times: start = earliest
    /// point, end = start + duration (a single source of truth — separate
    /// from the folded last() row, whose aggregate clock is unreliable:
    /// verified live, aggregates over null columns yield no row or a
    /// bogus time 0). Charts pin to this span, not the curve extent:
    /// readings often stop before the session closes, and fill(previous)
    /// would otherwise stretch the axis with forward-filled phantoms.
    pub start_t: i64,
    pub end_t: i64,
}

/// Per-session detail for the Charging Stats drill-down (no Grafana
/// counterpart — no unit, cost/energy/power/battery only; nothing here is
/// a distance). Takes a single `charge_id` (NOT vin + time range: one
/// session has no window); missing/empty → 400, unknown id → 404.
///
/// Query nuances:
/// - Session stats select the `last(...)` of cost, duration, geofence,
///   energy, start lat/lng, and end/start battery (columns `last` ..
///   `last_7` in select order) with NO time filter — one query, one
///   session. SOC added is `end - start` in Rust (either missing → null);
///   energy /1000 in Rust. Session span: start = earliest raw point,
///   end = start + duration (single source; aggregate clocks unreliable).
/// - Curve selects `mean("battery_level"), mean("power"),
///   mean("energy_added")` from `charge_readings` with fixed
///   `GROUP BY time(1m) fill(previous)` — session-bounded (never
///   range-derived) 1-minute buckets. Charge power IS watts (unlike drive
///   power, see #93), so /1000 → kW is CORRECT here.
#[derive(Deserialize)]
pub struct ChargeDetailsParams {
    pub charge_id: Option<String>,
}

async fn charge_details(
    State(state): State<AppState>,
    Query(p): Query<ChargeDetailsParams>,
) -> Result<Json<ChargeDetailsResponse>, ApiError> {
    let id = p.charge_id.unwrap_or_default();
    if id.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "charge_id is required"));
    }
    let id_esc = escape_literal(&id);

    let sq = format!(
        "SELECT last(\"cost\"), last(\"duration_seconds\"), last(\"geofence_name\"), last(\"energy_added_wh\"), last(\"start_lat\"), last(\"start_lng\"), last(\"end_battery_level\"), last(\"start_battery_level\") FROM \"charging_sessions\" WHERE \"charge_id\"='{id_esc}'"
    );
    let sjson = state
        .db
        .query(&sq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (scols, srows) = series_rows(&sjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let Some((_t, srow)) = srows.first() else {
        return Err(err(StatusCode::NOT_FOUND, "charge not found"));
    };

    // Session span: start = earliest point time; end = start + duration
    // (one source of truth — the folded last() row clock is unreliable, so
    // no second query derives the end independently).
    let span_q = format!(
        "SELECT \"duration_seconds\" FROM \"charging_sessions\" WHERE \"charge_id\"='{id_esc}' ORDER BY time ASC LIMIT 1"
    );
    let span_json = state
        .db
        .query(&span_q, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (_, span_rows) = series_rows(&span_json)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let start_t = span_rows.first().map(|(t, _)| *t).unwrap_or(0);

    let cq = format!(
        "SELECT mean(\"battery_level\"), mean(\"power\"), mean(\"energy_added\") FROM \"charge_readings\" WHERE \"charge_id\"='{id_esc}' GROUP BY time(1m) fill(previous)"
    );
    let cjson = state
        .db
        .query(&cq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (ccols, crows) = series_rows(&cjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    // InfluxQL names positional same-aggregate columns `last`, `last_1`,
    // … in select order: cost, duration, geofence, energy, lat, lng, end
    // battery, start battery.
    let end_soc = cell(&scols, srow, "last_6");
    let start_soc = cell(&scols, srow, "last_7");
    let soc_added = match (end_soc, start_soc) {
        (Some(e), Some(s)) => Some(e - s),
        _ => None,
    };

    // Curve columns `mean`, `mean_1`, `mean_2` in select order: battery,
    // power (watts → kW in Rust), cumulative energy added (/1000 → kWh).
    let curve = crows
        .iter()
        .map(|(t, row)| ChargeCurvePoint {
            t: *t,
            battery: cell(&ccols, row, "mean"),
            power_kw: cell(&ccols, row, "mean_1").map(|v| v / 1000.0),
            added_kwh: cell(&ccols, row, "mean_2").map(|v| v / 1000.0),
        })
        .collect();

    let duration_s = cell(&scols, srow, "last_1");
    let end_t = duration_s
        .filter(|d| *d > 0.0)
        .map(|d| start_t.saturating_add((d * 1000.0) as i64))
        .unwrap_or(start_t);

    Ok(Json(ChargeDetailsResponse {
        charge_id: id,
        cost: cell(&scols, srow, "last"),
        duration_s,
        geofence: str_cell(&scols, srow, "last_2"),
        energy_kwh: cell(&scols, srow, "last_3").map(|v| v / 1000.0),
        lat: cell(&scols, srow, "last_4"),
        lng: cell(&scols, srow, "last_5"),
        soc_added,
        curve,
        start_t,
        end_t,
    }))
}

#[derive(Serialize, PartialEq, Debug)]
pub struct DriveRow {
    pub drive_id: String,
    pub t: i64,
    /// Drill-down window start (ms): the `start_time` ISO field when
    /// populated, else the point time (see `drives` docs).
    pub from_t: i64,
    /// Drill-down window end (ms): the `end_time` ISO field when populated,
    /// else `from_t` + duration (see `drives` docs).
    pub to_t: i64,
    pub distance: Option<f64>,
    pub duration_min: Option<f64>,
    pub energy_kwh: Option<f64>,
    pub avg_speed: Option<f64>,
    pub max_speed: Option<f64>,
    pub from_gf: Option<String>,
    pub to_gf: Option<String>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct DrivesResponse {
    pub vin: String,
    pub count: i64,
    pub distance: f64,
    pub duration_s: i64,
    pub energy_kwh: Option<f64>,
    pub drives: Vec<DriveRow>,
    pub per_day: Vec<LevelPoint>,
}

/// Maximum accepted query window for the drives endpoint: 5 years — same
/// sparse-rows rationale as `MAX_EFFICIENCY_WINDOW_SECS` (kept as its own
/// const so each endpoint documents its own horizon). Drives are one row per
/// drive, so a multi-year window stays bounded; the raw per-drive query is
/// additionally capped with `LIMIT 1000`.
const MAX_DRIVES_WINDOW_SECS: i64 = 5 * 365 * 24 * 60 * 60;

/// Drive counts, distances, durations, energy, per-drive rows, and daily
/// distance for the Drives page. Mirrors the seven `drives.json` panels.
///
/// Query nuances (the JSON governs):
/// - # of Drives is `count("distance_meters")` with time + vin ONLY (no
///   `duration_seconds` filter — same shape as the Locations/Drive Stats
///   count panels). Summed across series via `all_series`.
/// - Distance + Duration + Energy Used come from ONE combined query —
///   `count("distance_meters"), sum("distance_meters"),
///   sum("duration_seconds"), sum("energy_used_wh")` with `AND
///   "duration_seconds" >= 0` (columns `count`, `sum`, `sum_1`, `sum_2`;
///   the count cell is unused — the unfiltered count above is the stat) —
///   with the `/1000*$length_unit` scaling applied in Rust via
///   `unit_factor()`. Duration stays seconds (the frontend humanizes, same
///   as the charge detail view). Energy is kWh with NO unit factor, 1
///   decimal in the frontend (Grafana `decimals: 1`). Known data caveat
///   (issue #93): `energy_used_wh` is currently 1000x under; the endpoint
///   mirrors Grafana exactly (same as efficiency/drive-stats).
/// - Drives table selects the raw `"distance_meters", "duration_seconds",
///   "energy_used_wh", "average_speed", "max_speed", "geofence_enter",
///   "geofence_exit"` plus the `start_time`/`end_time` ISO fields (window
///   source — see below) with `AND "duration_seconds" >= 0`. The JSON has NO
///   `ORDER BY` (InfluxDB defaults to time ASC), so rows are sorted ASC in
///   Rust — flagging the assumption here for a live-parity check against
///   Grafana. The JSON has NO `LIMIT`; a `LIMIT 1000` is added to bound the
///   response (one row per drive_id series, so effectively complete).
///   `GROUP BY "drive_id"` keeps one series per drive (a raw multi-tag
///   query without it merges into one tag-less series and the per-drive id
///   — which only survives in the series tags — is lost); the id is
///   extracted from `tags` into each row. Multi-series (drive_id tags) →
///   `all_series`. Conversions in Rust: distance /1000*unit, duration /60,
///   energy /1000, speeds *1.60934*unit (mph-stored, mileage pattern).
/// - Every row carries the drive window for the Stage 13 drill-down page:
///   the drive writer populates the `start_time` ISO field on session start
///   and the `end_time` ISO field on close onto the same point (same
///   timestamp + tags, so InfluxDB merges both onto the final row), hence
///   both are preferred; when absent (older drives, clock fallbacks) the
///   window falls back to [point time, point time + duration].
/// - Distance per Day is `sum("distance_meters")` with `GROUP BY time(1d)
///   fill(0)`, fixed daily buckets, /1000*unit in Rust. The `fill(0)` is a
///   DELIBERATE difference from the `fill(previous)` convention used
///   elsewhere: day buckets with no driving must read zero (bars need
///   zeros, not carried-forward values). Rendered as a line (TimeChart is
///   line-only).
/// - Empty window → count 0, distance 0, duration 0, energy null, drives []
///   and per_day [] (not an error).
async fn drives(
    State(state): State<AppState>,
    Query(p): Query<RangeParams>,
) -> Result<Json<DrivesResponse>, ApiError> {
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
    if (to - from).num_seconds() > MAX_DRIVES_WINDOW_SECS {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "time range exceeds the 5-year maximum",
        ));
    }
    let from = from.to_rfc3339();
    let to = to.to_rfc3339();
    let factor = unit_factor(p.unit.as_deref())?;
    let speed_factor = MILES_TO_KM * factor;
    let vin = escape_literal(&p.vin);

    let aq = format!(
        "SELECT count(\"distance_meters\"), sum(\"distance_meters\"), sum(\"duration_seconds\"), sum(\"energy_used_wh\") FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0"
    );
    let ajson = state
        .db
        .query(&aq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let aseries = all_series(&ajson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let cq = format!(
        "SELECT count(\"distance_meters\") FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}'"
    );
    let cjson = state
        .db
        .query(&cq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let cseries = all_series(&cjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let rq = format!(
        "SELECT \"distance_meters\", \"duration_seconds\", \"energy_used_wh\", \"average_speed\", \"max_speed\", \"geofence_enter\", \"geofence_exit\", \"start_time\", \"end_time\" FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0 GROUP BY \"drive_id\" LIMIT 1000"
    );
    let rjson = state
        .db
        .query(&rq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let rseries = all_series(&rjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let dq = format!(
        "SELECT sum(\"distance_meters\") FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0 GROUP BY time(1d) fill(0)"
    );
    let djson = state
        .db
        .query(&dq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (dcols, drows) = series_rows(&djson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    // Aggregate sums fan out one row per drive_id series, so add them up
    // across all series (same folding pattern as `drive_stats`/`charges`).
    // A window with no sum rows at all maps distance/duration → 0 and
    // energy → null.
    let mut distance_sum = 0.0;
    let mut duration_sum = 0.0;
    let mut energy_sum = 0.0;
    let mut energy_rows = 0usize;
    for t in &aseries {
        for (_, row) in &t.rows {
            if let Some(d) = cell(&t.columns, row, "sum") {
                distance_sum += d;
            }
            if let Some(d) = cell(&t.columns, row, "sum_1") {
                duration_sum += d;
            }
            if let Some(e) = cell(&t.columns, row, "sum_2") {
                energy_sum += e;
                energy_rows += 1;
            }
        }
    }
    let distance = distance_sum / 1000.0 * factor;
    let duration_s = duration_sum as i64;
    // Mirrors Grafana exactly (see the #93 note above): no unit factor.
    let energy_kwh = if energy_rows > 0 {
        Some(energy_sum / 1000.0)
    } else {
        None
    };

    let count: i64 = cseries
        .iter()
        .flat_map(|t| {
            t.rows
                .iter()
                .filter_map(|(_, row)| cell(&t.columns, row, "count"))
        })
        .map(|v| v as i64)
        .sum();

    // Raw per-drive rows fan out one series per drive_id — combine across
    // series and sort time ASC (restoring the JSON's implicit
    // no-ORDER-BY/Influx-default-ASC contract across series). The explicit
    // `GROUP BY "drive_id"` keeps one series per drive: without it a raw
    // multi-tag query merges into a single tag-less series and the
    // per-drive id is lost. The id only survives in `tags` (see
    // `SeriesTable`), so it is read from there.
    let mut drives: Vec<DriveRow> = rseries
        .iter()
        .flat_map(|t| {
            t.rows.iter().map(|(time, row)| {
                let duration_s = cell(&t.columns, row, "duration_seconds");
                // Drill-down window: prefer the `start_time`/`end_time` ISO
                // fields the drive writer populates (points are stamped with
                // the drive start, so the ISO end is the true window end);
                // fall back to [point time, point time + duration] when a
                // field is absent.
                let from_t = str_cell(&t.columns, row, "start_time")
                    .and_then(|s| {
                        chrono::DateTime::parse_from_rfc3339(&s)
                            .ok()
                            .map(|dt| dt.timestamp_millis())
                    })
                    .unwrap_or(*time);
                let to_t = str_cell(&t.columns, row, "end_time")
                    .and_then(|s| {
                        chrono::DateTime::parse_from_rfc3339(&s)
                            .ok()
                            .map(|dt| dt.timestamp_millis())
                    })
                    .unwrap_or_else(|| {
                        duration_s
                            .filter(|d| *d > 0.0)
                            .map(|d| from_t.saturating_add((d * 1000.0) as i64))
                            .unwrap_or(from_t)
                    });
                DriveRow {
                    drive_id: t.tags.get("drive_id").cloned().unwrap_or_default(),
                    t: *time,
                    from_t,
                    to_t,
                    distance: cell(&t.columns, row, "distance_meters").map(|v| v / 1000.0 * factor),
                    duration_min: duration_s.map(|v| v / 60.0),
                    // Mirrors Grafana exactly (see the #93 note above).
                    energy_kwh: cell(&t.columns, row, "energy_used_wh").map(|v| v / 1000.0),
                    avg_speed: cell(&t.columns, row, "average_speed").map(|v| v * speed_factor),
                    max_speed: cell(&t.columns, row, "max_speed").map(|v| v * speed_factor),
                    from_gf: str_cell(&t.columns, row, "geofence_enter"),
                    to_gf: str_cell(&t.columns, row, "geofence_exit"),
                }
            })
        })
        .collect();
    drives.sort_by_key(|d| d.t);

    // InfluxQL names the single-aggregate column `sum`.
    let per_day = drows
        .iter()
        .map(|(t, row)| LevelPoint {
            t: *t,
            value: cell(&dcols, row, "sum").map(|v| v / 1000.0 * factor),
        })
        .collect();

    Ok(Json(DrivesResponse {
        vin: p.vin,
        count,
        distance,
        duration_s,
        energy_kwh,
        drives,
        per_day,
    }))
}

/// Epoch millis → RFC3339 (UTC). Millis from InfluxDB are always in range;
/// out-of-range values (unreachable in practice) map to the epoch.
fn ms_to_rfc3339(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00+00:00".to_string())
}

#[derive(Serialize, PartialEq, Debug)]
pub struct DriveSeriesPoint {
    pub t: i64,
    pub speed: Option<f64>,
    pub power_kw: Option<f64>,
    pub battery: Option<f64>,
    pub ideal: Option<f64>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct RoutePoint {
    pub lat: f64,
    pub lng: f64,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct TempPoint {
    pub t: i64,
    pub outside: Option<f64>,
    pub inside: Option<f64>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct DriveDetailsStats {
    pub duration_s: Option<f64>,
    pub distance: Option<f64>,
    pub energy_kwh: Option<f64>,
    pub avg_speed: Option<f64>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct OdometerRange {
    pub from: Option<f64>,
    pub to: Option<f64>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct DriveWindow {
    pub from: String,
    pub to: String,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct DriveDetailsResponse {
    pub drive_id: String,
    pub window: DriveWindow,
    pub series: Vec<DriveSeriesPoint>,
    pub route: Vec<RoutePoint>,
    pub elevation: Vec<LevelPoint>,
    pub temps: Vec<TempPoint>,
    pub stats: DriveDetailsStats,
    pub odometer: OdometerRange,
}

/// Per-drive detail for the Drives drill-down page. Mirrors the
/// `drive-details.json` panels. Takes a single `drive_id` (+ unit) — the
/// SERVER resolves the drive window from the drives table itself (no
/// from/to params: the row's query params are ignored by the endpoint);
/// missing/empty → 400, unknown id (no drive rows) → 404.
///
/// Query nuances (the JSON governs):
/// - Window is a raw `SELECT "start_time", "end_time",
///   "duration_seconds" ... WHERE "drive_id"` with NO time filter — one
///   drive, one lookup. Same derivation as the Stage 12 table rows: the
///   `start_time`/`end_time` ISO fields preferred, fallback [point time,
///   point time + duration]. Formatted back to RFC3339 for the positions
///   queries AND returned as `window: {from, to}` (ISO strings).
/// - Positions queries span the resolved [from, to] with NO vin filter —
///   the window implies the drive (matches the JSON, which filters on
///   $dstart/$dend only).
/// - Drive series is fixed 10s buckets: `mean("speed")` (*1.60934*unit in
///   Rust, mph-stored), `mean("power")` PASSTHROUGH kW — NO /1000: drive
///   power is natively kW (issue #93), the opposite of charge power (which
///   IS watts, so /1000 is correct there) — `mean("battery_level")`,
///   `mean("ideal_battery_range_km")` (*unit in Rust). Columns
///   mean..mean_3.
/// - Route is raw `"latitude", "longitude" ... AND "latitude" > -90 ORDER
///   BY time ASC LIMIT 10000` (the cap bounds long drives; the filter drops
///   unset readings). Positions carry TWO tags (vin + car_id), so the raw
///   query fans out one series per tag set: consume `all_series`, combine,
///   and re-sort ASC in Rust (restores the JSON's ORDER BY across series).
/// - Elevation is `mean("elevation")` 10s; old drives lack SRTM data, so
///   nulls pass through (a data gap, not a UI bug).
/// - Temps are `mean("outside_temp"), mean("inside_temp")` 10s (sparse:
///   streaming points carry no climate fields); nulls pass through as gaps.
/// - Stats fold ONE drives query — `last("duration_seconds"),
///   last("distance_meters"), last("energy_used_wh"), last("average_speed")`
///   WHERE drive_id (no time filter). Conversions in Rust: distance
///   /1000*unit, energy /1000 (same #93 caveat as drives/drive-stats),
///   avg speed *1.60934*unit (mph-stored).
/// - Odometer is `FIRST("odometer"), LAST("odometer")` in [from, to] (no
///   vin filter — follows the JSON). Stored odometer is miles (mileage
///   pattern): *1.60934*unit in Rust. Null when the window holds no
///   odometer points (old drives).
#[derive(Deserialize)]
pub struct DriveDetailsParams {
    pub drive_id: Option<String>,
    pub unit: Option<String>,
}

async fn drive_details(
    State(state): State<AppState>,
    Query(p): Query<DriveDetailsParams>,
) -> Result<Json<DriveDetailsResponse>, ApiError> {
    let id = p.drive_id.unwrap_or_default();
    if id.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "drive_id is required"));
    }
    let factor = unit_factor(p.unit.as_deref())?;
    let speed_factor = MILES_TO_KM * factor;
    let id_esc = escape_literal(&id);

    let wq = format!(
        "SELECT \"start_time\", \"end_time\", \"duration_seconds\" FROM \"drives\" WHERE \"drive_id\"='{id_esc}'"
    );
    let wjson = state
        .db
        .query(&wq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let wseries = all_series(&wjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    // One drive → effectively one row; scan series in order and take the
    // first row found. No rows at all means the id is unknown → 404.
    let win_row = wseries
        .iter()
        .flat_map(|t| t.rows.iter().map(move |(time, row)| (*time, t, row)))
        .next();
    let Some((point_t, wtable, wrow)) = win_row else {
        return Err(err(StatusCode::NOT_FOUND, "drive not found"));
    };
    let start_iso = str_cell(&wtable.columns, wrow, "start_time");
    let end_iso = str_cell(&wtable.columns, wrow, "end_time");
    let duration_s = cell(&wtable.columns, wrow, "duration_seconds");
    let from_ms = start_iso
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(point_t);
    let to_ms = end_iso
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or_else(|| {
            duration_s
                .filter(|d| *d > 0.0)
                .map(|d| from_ms.saturating_add((d * 1000.0) as i64))
                .unwrap_or(from_ms)
        });
    let from = ms_to_rfc3339(from_ms);
    let to = ms_to_rfc3339(to_ms);

    let sq = format!(
        "SELECT mean(\"speed\"), mean(\"power\"), mean(\"battery_level\"), mean(\"ideal_battery_range_km\") FROM \"positions\" WHERE time >= '{from}' AND time <= '{to}' GROUP BY time(10s) fill(previous)"
    );
    let sjson = state
        .db
        .query(&sq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (scols, srows) = series_rows(&sjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let rq = format!(
        "SELECT \"latitude\", \"longitude\" FROM \"positions\" WHERE time >= '{from}' AND time <= '{to}' AND \"latitude\" > -90 ORDER BY time ASC LIMIT 10000"
    );
    let rjson = state
        .db
        .query(&rq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let rseries = all_series(&rjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let eq = format!(
        "SELECT mean(\"elevation\") FROM \"positions\" WHERE time >= '{from}' AND time <= '{to}' GROUP BY time(10s) fill(previous)"
    );
    let ejson = state
        .db
        .query(&eq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (ecols, erows) = series_rows(&ejson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let tq = format!(
        "SELECT mean(\"outside_temp\"), mean(\"inside_temp\") FROM \"positions\" WHERE time >= '{from}' AND time <= '{to}' GROUP BY time(10s) fill(previous)"
    );
    let tjson = state
        .db
        .query(&tq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (tcols, trows) = series_rows(&tjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let aq = format!(
        "SELECT last(\"duration_seconds\"), last(\"distance_meters\"), last(\"energy_used_wh\"), last(\"average_speed\") FROM \"drives\" WHERE \"drive_id\"='{id_esc}'"
    );
    let ajson = state
        .db
        .query(&aq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let aseries = all_series(&ajson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let oq = format!(
        "SELECT FIRST(\"odometer\"), LAST(\"odometer\") FROM \"positions\" WHERE time >= '{from}' AND time <= '{to}'"
    );
    let ojson = state
        .db
        .query(&oq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let oseries = all_series(&ojson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    // InfluxQL names positional same-aggregate columns `mean`, `mean_1`,
    // … in select order: speed, power, battery, ideal range. Power passes
    // through as kW (see the #93 note above); speed converts mph → display.
    let series = srows
        .iter()
        .map(|(t, row)| DriveSeriesPoint {
            t: *t,
            speed: cell(&scols, row, "mean").map(|v| v * speed_factor),
            power_kw: cell(&scols, row, "mean_1"),
            battery: cell(&scols, row, "mean_2"),
            ideal: cell(&scols, row, "mean_3").map(|v| v * factor),
        })
        .collect();

    // Raw route rows fan out one series per (vin, car_id) tag set —
    // combine across series and restore the JSON's ORDER BY time ASC.
    let mut route_t: Vec<(i64, RoutePoint)> = rseries
        .iter()
        .flat_map(|t| {
            t.rows.iter().filter_map(|(time, row)| {
                match (
                    cell(&t.columns, row, "latitude"),
                    cell(&t.columns, row, "longitude"),
                ) {
                    (Some(lat), Some(lng)) => Some((*time, RoutePoint { lat, lng })),
                    _ => None,
                }
            })
        })
        .collect();
    route_t.sort_by_key(|(t, _)| *t);
    let route: Vec<RoutePoint> = route_t.into_iter().map(|(_, p)| p).collect();

    // InfluxQL names the single-aggregate column `mean`; nulls (old
    // drives without SRTM data) pass through as gaps.
    let elevation = erows
        .iter()
        .map(|(t, row)| LevelPoint {
            t: *t,
            value: cell(&ecols, row, "mean"),
        })
        .collect();

    // Columns `mean`, `mean_1` in select order: outside, inside. Nulls
    // (streaming points carry no climate) pass through as gaps.
    let temps = trows
        .iter()
        .map(|(t, row)| TempPoint {
            t: *t,
            outside: cell(&tcols, row, "mean"),
            inside: cell(&tcols, row, "mean_1"),
        })
        .collect();

    // One drive → effectively one row; fold defensively (first non-null
    // per column). Columns `last` .. `last_3` in select order: duration,
    // distance, energy, average speed.
    let mut stat_duration: Option<f64> = None;
    let mut stat_distance: Option<f64> = None;
    let mut stat_energy: Option<f64> = None;
    let mut stat_avg: Option<f64> = None;
    for t in &aseries {
        for (_, row) in &t.rows {
            if stat_duration.is_none() {
                stat_duration = cell(&t.columns, row, "last");
            }
            if stat_distance.is_none() {
                stat_distance = cell(&t.columns, row, "last_1");
            }
            if stat_energy.is_none() {
                stat_energy = cell(&t.columns, row, "last_2");
            }
            if stat_avg.is_none() {
                stat_avg = cell(&t.columns, row, "last_3");
            }
        }
    }
    let stats = DriveDetailsStats {
        duration_s: stat_duration,
        distance: stat_distance.map(|v| v / 1000.0 * factor),
        // Mirrors Grafana exactly (see the #93 note above): no unit factor.
        energy_kwh: stat_energy.map(|v| v / 1000.0),
        avg_speed: stat_avg.map(|v| v * speed_factor),
    };

    // FIRST/LAST fan out per tag set in principle; one drive window holds
    // one (vin, car_id), so take the first non-null per column. Columns
    // `first`, `last`.
    let mut odo_from: Option<f64> = None;
    let mut odo_to: Option<f64> = None;
    for t in &oseries {
        for (_, row) in &t.rows {
            if odo_from.is_none() {
                odo_from = cell(&t.columns, row, "first");
            }
            if odo_to.is_none() {
                odo_to = cell(&t.columns, row, "last");
            }
        }
    }
    let odometer = OdometerRange {
        from: odo_from.map(|v| v * speed_factor),
        to: odo_to.map(|v| v * speed_factor),
    };

    Ok(Json(DriveDetailsResponse {
        drive_id: id,
        window: DriveWindow { from, to },
        series,
        route,
        elevation,
        temps,
        stats,
        odometer,
    }))
}

#[derive(Serialize, PartialEq, Debug)]
pub struct TripBatteryPoint {
    pub t: i64,
    pub level: Option<f64>,
    pub ideal: Option<f64>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct TripChargeRow {
    pub t: i64,
    pub charge_id: String,
    pub location: Option<String>,
    pub energy_kwh: Option<f64>,
    pub cost: Option<f64>,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct TripResponse {
    pub vin: String,
    pub distance: f64,
    /// Driving vs charging split for the Time Spent pie (seconds).
    pub driving_s: i64,
    pub charging_s: i64,
    pub avg_speed: Option<f64>,
    /// Always Wh/km (no unit factor — same as efficiency).
    pub consumption_wh_per_km: Option<f64>,
    pub charging_cost: f64,
    /// Same row shape as the Drives page (displayed subset: time,
    /// distance, duration, from/to — the Trip table has no energy/speed
    /// columns, but the shape is shared so drill-down windows travel).
    pub drives: Vec<DriveRow>,
    pub charges: Vec<TripChargeRow>,
    pub battery: Vec<TripBatteryPoint>,
    pub route: Vec<RoutePoint>,
}

/// Maximum accepted query window for the trip endpoint: 90 days — same
/// bound as `MAX_WINDOW_SECS` (kept as its own const so each endpoint
/// documents its own horizon). The battery chart uses fixed 5-minute
/// buckets (~26k points for 90 days); a multi-year window would
/// materialize an unbounded result set. A trip window is days, so the
/// 90-day cap is generous; the sparse per-drive/session tables additionally
/// cap with `LIMIT 1000` and the route with `LIMIT 10000`.
const MAX_TRIP_WINDOW_SECS: i64 = 90 * 24 * 60 * 60;

/// Trip summary for the Trip page. Mirrors the nine `trip.json` panels:
/// every primitive already exists on earlier pages; only the pie chart is
/// new (frontend-only — the backend just returns the two sums).
///
/// Query nuances (the JSON governs):
/// - Distance is `sum("distance_meters")` with `AND "duration_seconds" >=
///   0`, /1000*unit in Rust. Empty window → 0.
/// - Time Spent is `sum("duration_seconds")` with `AND "duration_seconds"
///   >= 0` from `drives` (Driving) and from `charging_sessions`
///   (Charging). Folded across series via `all_series`; empty → 0.
/// - Avg Speed is `sum(dist)/sum(dur)` in Rust:
///   `sum(distance)/1000*unit / (sum(duration)/3600)` (missing/zero
///   duration → null). Computed from the SAME drive sums as distance (one
///   query serves distance + avg speed + driving seconds).
/// - Consumption is `sum("energy_used_wh")/sum("distance_meters")*1000`
///   with `AND "duration_seconds" >= 0 AND "distance_meters" > 100` (same
///   filters as the efficiency avg panel), always Wh/km. Known data caveat
///   (issue #93): `energy_used_wh` is 1000x under; the endpoint mirrors
///   Grafana exactly (same as efficiency/drive-stats).
/// - Charging Cost is `sum("cost")` with `AND "duration_seconds" >= 0`.
///   Empty → 0.
/// - Drives table selects the SAME raw fields as the Drives page table
///   (shared `DriveRow` shape, incl. the drill-down window); the Trip page
///   displays the Grafana subset (time, distance, duration, from, to).
/// - Charges table selects raw `"geofence_name", "energy_added_wh",
///   "cost"` with `AND "duration_seconds" >= 0`, `GROUP BY "charge_id"`
///   (per-session id from tags — same pattern as the Charges page), sorted
///   ASC in Rust. Energy /1000 in Rust.
/// - Battery Level & Range is `mean("battery_level")`,
///   `mean("ideal_battery_range_km")` with fixed `GROUP BY time(5m)
///   fill(previous)` (never range-derived `$__interval` over a bounded
///   window — same rule as `overview`); ideal *unit in Rust.
/// - Trip Map is raw `"latitude", "longitude" ... AND "latitude" > -90
///   ORDER BY time ASC LIMIT 10000` (same shape as the drive-details
///   route): positions carry two tags (vin + car_id), so consume
///   `all_series`, combine, re-sort ASC in Rust.
async fn trip(
    State(state): State<AppState>,
    Query(p): Query<RangeParams>,
) -> Result<Json<TripResponse>, ApiError> {
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
    if (to - from).num_seconds() > MAX_TRIP_WINDOW_SECS {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "time range exceeds the 90-day maximum",
        ));
    }
    let from = from.to_rfc3339();
    let to = to.to_rfc3339();
    let factor = unit_factor(p.unit.as_deref())?;
    let speed_factor = MILES_TO_KM * factor;
    let vin = escape_literal(&p.vin);

    let aq = format!(
        "SELECT sum(\"distance_meters\"), sum(\"duration_seconds\"), sum(\"energy_used_wh\") FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0"
    );
    let ajson = state
        .db
        .query(&aq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let aseries = all_series(&ajson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let cq = format!(
        "SELECT sum(\"energy_used_wh\"), sum(\"distance_meters\") FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0 AND \"distance_meters\" > 100"
    );
    let cjson = state
        .db
        .query(&cq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let cseries = all_series(&cjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let hq = format!(
        "SELECT sum(\"duration_seconds\"), sum(\"cost\") FROM \"charging_sessions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0"
    );
    let hjson = state
        .db
        .query(&hq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let hseries = all_series(&hjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let rq = format!(
        "SELECT \"distance_meters\", \"duration_seconds\", \"energy_used_wh\", \"average_speed\", \"max_speed\", \"geofence_enter\", \"geofence_exit\", \"start_time\", \"end_time\" FROM \"drives\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0 GROUP BY \"drive_id\" LIMIT 1000"
    );
    let rjson = state
        .db
        .query(&rq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let rseries = all_series(&rjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let sq = format!(
        "SELECT \"geofence_name\", \"energy_added_wh\", \"cost\" FROM \"charging_sessions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"duration_seconds\" >= 0 GROUP BY \"charge_id\" LIMIT 1000"
    );
    let sjson = state
        .db
        .query(&sq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let sseries = all_series(&sjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let bq = format!(
        "SELECT mean(\"battery_level\"), mean(\"ideal_battery_range_km\") FROM \"positions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' GROUP BY time(5m) fill(previous)"
    );
    let bjson = state
        .db
        .query(&bq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let (bcols, brows) = series_rows(&bjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    let mq = format!(
        "SELECT \"latitude\", \"longitude\" FROM \"positions\" WHERE time >= '{from}' AND time <= '{to}' AND \"vin\"='{vin}' AND \"latitude\" > -90 ORDER BY time ASC LIMIT 10000"
    );
    let mjson = state
        .db
        .query(&mq, "ms")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;
    let mseries = all_series(&mjson)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("query failed: {e}")))?;

    // Aggregate sums fan out one row per drive_id series, so add them up
    // across all series (same folding pattern as `drives`/`drive_stats`).
    // Columns `sum`, `sum_1`, `sum_2` in select order: distance, duration,
    // energy.
    let mut distance_sum = 0.0;
    let mut duration_sum = 0.0;
    for t in &aseries {
        for (_, row) in &t.rows {
            if let Some(d) = cell(&t.columns, row, "sum") {
                distance_sum += d;
            }
            if let Some(d) = cell(&t.columns, row, "sum_1") {
                duration_sum += d;
            }
        }
    }
    let distance = distance_sum / 1000.0 * factor;
    let driving_s = duration_sum as i64;
    let avg_speed = if duration_sum != 0.0 {
        Some(distance_sum / 1000.0 * factor / (duration_sum / 3600.0))
    } else {
        None
    };

    // Consumption sums carry the extra `distance_meters > 100` filter, so
    // they fold separately (same same-aggregate-column pattern:
    // `sum` = energy, `sum_1` = distance). A window with no sum rows at all
    // (or a zero distance total) maps to null.
    let mut energy_sum = 0.0;
    let mut con_distance_sum = 0.0;
    let mut con_rows = 0usize;
    for t in &cseries {
        for (_, row) in &t.rows {
            let e = cell(&t.columns, row, "sum");
            let d = cell(&t.columns, row, "sum_1");
            if e.is_some() || d.is_some() {
                con_rows += 1;
                energy_sum += e.unwrap_or(0.0);
                con_distance_sum += d.unwrap_or(0.0);
            }
        }
    }
    let consumption_wh_per_km = if con_rows > 0 && con_distance_sum != 0.0 {
        Some(energy_sum / con_distance_sum * 1000.0)
    } else {
        None
    };

    // Columns `sum`, `sum_1` in select order: duration, cost.
    let mut charging_duration = 0.0;
    let mut cost_sum = 0.0;
    for t in &hseries {
        for (_, row) in &t.rows {
            if let Some(d) = cell(&t.columns, row, "sum") {
                charging_duration += d;
            }
            if let Some(c) = cell(&t.columns, row, "sum_1") {
                cost_sum += c;
            }
        }
    }
    let charging_s = charging_duration as i64;
    let charging_cost = cost_sum;

    // Same per-drive row construction as the Drives page table (shared
    // `DriveRow` shape, incl. the Stage 13 drill-down window); the Trip
    // page displays the Grafana subset.
    let mut drives: Vec<DriveRow> = rseries
        .iter()
        .flat_map(|t| {
            t.rows.iter().map(|(time, row)| {
                let duration_s = cell(&t.columns, row, "duration_seconds");
                let from_t = str_cell(&t.columns, row, "start_time")
                    .and_then(|s| {
                        chrono::DateTime::parse_from_rfc3339(&s)
                            .ok()
                            .map(|dt| dt.timestamp_millis())
                    })
                    .unwrap_or(*time);
                let to_t = str_cell(&t.columns, row, "end_time")
                    .and_then(|s| {
                        chrono::DateTime::parse_from_rfc3339(&s)
                            .ok()
                            .map(|dt| dt.timestamp_millis())
                    })
                    .unwrap_or_else(|| {
                        duration_s
                            .filter(|d| *d > 0.0)
                            .map(|d| from_t.saturating_add((d * 1000.0) as i64))
                            .unwrap_or(from_t)
                    });
                DriveRow {
                    drive_id: t.tags.get("drive_id").cloned().unwrap_or_default(),
                    t: *time,
                    from_t,
                    to_t,
                    distance: cell(&t.columns, row, "distance_meters").map(|v| v / 1000.0 * factor),
                    duration_min: duration_s.map(|v| v / 60.0),
                    energy_kwh: cell(&t.columns, row, "energy_used_wh").map(|v| v / 1000.0),
                    avg_speed: cell(&t.columns, row, "average_speed").map(|v| v * speed_factor),
                    max_speed: cell(&t.columns, row, "max_speed").map(|v| v * speed_factor),
                    from_gf: str_cell(&t.columns, row, "geofence_enter"),
                    to_gf: str_cell(&t.columns, row, "geofence_exit"),
                }
            })
        })
        .collect();
    drives.sort_by_key(|d| d.t);

    // Raw session rows fan out one series per charge_id — combine across
    // series and sort time ASC (same pattern as the Charges page table).
    let mut charges: Vec<TripChargeRow> = sseries
        .iter()
        .flat_map(|t| {
            t.rows.iter().map(|(time, row)| TripChargeRow {
                t: *time,
                charge_id: t.tags.get("charge_id").cloned().unwrap_or_default(),
                location: str_cell(&t.columns, row, "geofence_name"),
                energy_kwh: cell(&t.columns, row, "energy_added_wh").map(|v| v / 1000.0),
                cost: cell(&t.columns, row, "cost"),
            })
        })
        .collect();
    charges.sort_by_key(|c| c.t);

    // InfluxQL names positional same-aggregate columns `mean`, `mean_1` in
    // select order: battery, ideal range (converted *unit in Rust).
    let battery = brows
        .iter()
        .map(|(t, row)| TripBatteryPoint {
            t: *t,
            level: cell(&bcols, row, "mean"),
            ideal: cell(&bcols, row, "mean_1").map(|v| v * factor),
        })
        .collect();

    // Raw route rows fan out one series per (vin, car_id) tag set —
    // combine across series and restore the JSON's ORDER BY time ASC.
    let mut route_t: Vec<(i64, RoutePoint)> = mseries
        .iter()
        .flat_map(|t| {
            t.rows.iter().filter_map(|(time, row)| {
                match (
                    cell(&t.columns, row, "latitude"),
                    cell(&t.columns, row, "longitude"),
                ) {
                    (Some(lat), Some(lng)) => Some((*time, RoutePoint { lat, lng })),
                    _ => None,
                }
            })
        })
        .collect();
    route_t.sort_by_key(|(t, _)| *t);
    let route: Vec<RoutePoint> = route_t.into_iter().map(|(_, p)| p).collect();

    Ok(Json(TripResponse {
        vin: p.vin,
        distance,
        driving_s,
        charging_s,
        avg_speed,
        consumption_wh_per_km,
        charging_cost,
        drives,
        charges,
        battery,
        route,
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

    const UPDATES_COUNT_ROWS: &str = r#"{"results": [{"series": [{
        "name": "updates",
        "columns": ["time", "count"],
        "values": [[1700000000000, 2]]
    }]}]}"#;

    const UPDATES_DETAIL_ROWS: &str = r#"{"results": [{"series": [{
        "name": "updates",
        "columns": ["time", "version_before", "version_after", "install_start", "install_end", "status"],
        "values": [
            [1700000300000, "2023.38.6", "2023.44.1", "2023-11-14T10:00:00Z", "2023-11-14T10:25:00Z", "installed"],
            [1700000000000, "2023.32.4", "2023.38.6", null, "2023-11-14T09:25:00Z", "installed"]
        ]
    }]}]}"#;

    async fn mock_updates_db() -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(|req: &wiremock::Request| {
                // The count query selects count("status"); the row query
                // selects version_before.
                let q = req.url.query().unwrap_or("");
                let body = if q.contains("count") {
                    UPDATES_COUNT_ROWS
                } else {
                    UPDATES_DETAIL_ROWS
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        server
    }

    async fn get_updates(query: &str) -> (StatusCode, serde_json::Value) {
        let server = mock_updates_db().await;
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/updates{query}"))
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
    async fn updates_returns_series() {
        let (status, json) =
            get_updates("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["vin"], "VIN1");
        assert_eq!(json["count"], 2);
        assert_eq!(json["updates"].as_array().unwrap().len(), 2);
        assert_eq!(json["updates"][0]["t"], 1700000300000i64);
        assert_eq!(json["updates"][0]["version_before"], "2023.38.6");
        assert_eq!(json["updates"][0]["version_after"], "2023.44.1");
        assert_eq!(json["updates"][0]["status"], "installed");
        // Second row has a null install_start → null in the response.
        assert!(json["updates"][1]["install_start"].is_null());
        assert_eq!(json["updates"][1]["version_after"], "2023.38.6");
    }

    /// `updates` is tagged by vin + update_id, so InfluxQL fans raw and
    /// count queries out into one series per update: every series must be
    /// consumed (counts summed, rows combined time-desc), not just the first.
    #[tokio::test]
    async fn updates_flattens_multiple_series() {
        const COUNT_SPLIT: &str = r#"{"results": [{"series": [
            {"name": "updates", "tags": {"update_id": "up-1"},
             "columns": ["time", "count"], "values": [[1700000300000, 1]]},
            {"name": "updates", "tags": {"update_id": "up-2"},
             "columns": ["time", "count"], "values": [[1700000000000, 1]]}
        ]}]}"#;
        const ROWS_SPLIT: &str = r#"{"results": [{"series": [
            {"name": "updates", "tags": {"update_id": "up-1"},
             "columns": ["time", "version_before", "version_after", "install_start", "install_end", "status"],
             "values": [[1700000000000, "2023.32.4", "2023.38.6", null, "2023-11-14T09:25:00Z", "installed"]]},
            {"name": "updates", "tags": {"update_id": "up-2"},
             "columns": ["time", "version_before", "version_after", "install_start", "install_end", "status"],
             "values": [[1700000300000, "2023.38.6", "2023.44.1", "2023-11-14T10:00:00Z", "2023-11-14T10:25:00Z", "installed"]]}
        ]}]}"#;
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(|req: &wiremock::Request| {
                let q = req.url.query().unwrap_or("");
                let body = if q.contains("count") {
                    COUNT_SPLIT
                } else {
                    ROWS_SPLIT
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/updates?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Counts sum across series; rows combine newest-first.
        assert_eq!(json["count"], 2);
        assert_eq!(json["updates"].as_array().unwrap().len(), 2);
        assert_eq!(json["updates"][0]["t"], 1700000300000i64);
        assert_eq!(json["updates"][0]["version_after"], "2023.44.1");
        assert_eq!(json["updates"][1]["t"], 1700000000000i64);
        assert_eq!(json["updates"][1]["version_after"], "2023.38.6");
    }

    #[tokio::test]
    async fn updates_empty_window_is_empty_not_error() {
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
                    .uri("/updates?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["count"], 0);
        assert_eq!(json["updates"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn updates_rejects_inverted_range() {
        let (status, json) =
            get_updates("?vin=VIN1&from=2023-11-15T00:00:00Z&to=2023-11-14T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("from is after to"));
    }

    #[tokio::test]
    async fn updates_rejects_oversized_range() {
        let (status, json) =
            get_updates("?vin=VIN1&from=2015-01-01T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("5-year maximum"));
    }

    #[tokio::test]
    async fn updates_rejects_bad_time() {
        let (status, json) = get_updates("?vin=VIN1&from=not-a-time&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid time"));
    }

    #[tokio::test]
    async fn updates_rejects_empty_vin() {
        let (status, _) =
            get_updates("?vin=&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn updates_upstream_failure_is_502() {
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
                    .uri("/updates?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    const MILEAGE_ROWS: &str = r#"{"results": [{"series": [{
        "name": "positions",
        "columns": ["time", "max"],
        "values": [[1700000000000, 160934.0], [1700086400000, null]]
    }]}]}"#;

    async fn mock_mileage_db(body: &str) -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(body.to_string(), "application/json"),
            )
            .mount(&server)
            .await;
        server
    }

    async fn get_mileage(query: &str) -> (StatusCode, serde_json::Value) {
        let server = mock_mileage_db(MILEAGE_ROWS).await;
        get_mileage_on(&server, query).await
    }

    async fn get_mileage_on(
        server: &wiremock::MockServer,
        query: &str,
    ) -> (StatusCode, serde_json::Value) {
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/mileage{query}"))
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
    async fn mileage_returns_series() {
        let (status, json) =
            get_mileage("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-16T00:00:00Z&unit=km")
                .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["vin"], "VIN1");
        assert_eq!(json["points"].as_array().unwrap().len(), 2);
        assert_eq!(json["points"][0]["t"], 1700000000000i64);
        assert_eq!(json["points"][0]["value"], 160934.0);
        assert!(json["points"][1]["value"].is_null());
    }

    #[tokio::test]
    async fn mileage_unit_mi_applies_factor() {
        // Stored odometer is miles: the mi query multiplies by
        // 1.60934 * 0.621371 ≈ 1.0, so a 100-mile odometer reads ~100.
        let body = r#"{"results": [{"series": [{
            "name": "positions",
            "columns": ["time", "max"],
            "values": [[1700000000000, 100.0]]
        }]}]}"#;
        let server = mock_mileage_db(body).await;
        let (status, json) = get_mileage_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=mi",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let v = json["points"][0]["value"].as_f64().unwrap();
        assert!((v - 100.0).abs() < 0.01, "expected ~100.0, got {v}");
        // The outgoing InfluxQL must carry the combined mi factor. The
        // InfluxQL rides URL-encoded in `q`, but digits/dots survive
        // encoding, so match the raw query text.
        let received = server.received_requests().await.unwrap();
        let q = received
            .iter()
            .find_map(|r| r.url.query().map(|s| s.to_string()))
            .unwrap_or_default();
        assert!(
            q.contains("0.9999"),
            "mi query should contain the combined 1.60934*0.621371 factor, got: {q}"
        );
        assert!(q.contains("odometer"));
    }

    #[tokio::test]
    async fn mileage_empty_window_is_empty_not_error() {
        let server = mock_mileage_db(r#"{"results": [{}]}"#).await;
        let (status, json) = get_mileage_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["points"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn mileage_rejects_inverted_range() {
        let (status, json) =
            get_mileage("?vin=VIN1&from=2023-11-15T00:00:00Z&to=2023-11-14T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("from is after to"));
    }

    #[tokio::test]
    async fn mileage_rejects_oversized_range() {
        let (status, json) =
            get_mileage("?vin=VIN1&from=2010-01-01T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("10-year maximum"));
    }

    #[tokio::test]
    async fn mileage_rejects_bad_time() {
        let (status, json) = get_mileage("?vin=VIN1&from=not-a-time&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid time"));
    }

    #[tokio::test]
    async fn mileage_rejects_bad_unit() {
        let (status, json) =
            get_mileage("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=parsecs")
                .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid unit"));
    }

    #[tokio::test]
    async fn mileage_rejects_empty_vin() {
        let (status, _) =
            get_mileage("?vin=&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn mileage_upstream_failure_is_502() {
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
                    .uri("/mileage?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    const PROJECTED_SUMS_ROWS: &str = r#"{"results": [{"series": [{
        "name": "positions",
        "columns": ["time", "sum", "sum_1", "sum_2"],
        "values": [[1700000000000, 200, 50, 180], [1700000300000, 100, null, 90], [1700000600000, 100, 0, 90]]
    }]}]}"#;

    const PROJECTED_TEMP_ROWS: &str = r#"{"results": [{"series": [{
        "name": "positions",
        "columns": ["time", "mean"],
        "values": [[1700000000000, 12.5]]
    }]}]}"#;

    async fn mock_projected_db(body_sums: &str, body_temp: &str) -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        let sums = body_sums.to_string();
        let temp = body_temp.to_string();
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(move |req: &wiremock::Request| {
                // The sums query selects battery_level; the temp query
                // selects outside_temp.
                let q = req.url.query().unwrap_or("");
                let body = if q.contains("outside_temp") {
                    temp.clone()
                } else {
                    sums.clone()
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        server
    }

    async fn get_projected_range_on(
        server: &wiremock::MockServer,
        query: &str,
    ) -> (StatusCode, serde_json::Value) {
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/projected-range{query}"))
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

    async fn get_projected_range(query: &str) -> (StatusCode, serde_json::Value) {
        let server = mock_projected_db(PROJECTED_SUMS_ROWS, PROJECTED_TEMP_ROWS).await;
        get_projected_range_on(&server, query).await
    }

    #[tokio::test]
    async fn projected_range_returns_series() {
        let (status, json) = get_projected_range(
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=km",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["vin"], "VIN1");
        // Ratio math: sums ideal=200, battery=50 → 200/50*100 = 400.0;
        // rated 180/50*100 = 360.0.
        assert_eq!(json["projected"][0]["t"], 1700000000000i64);
        assert_eq!(json["projected"][0]["ideal"], 400.0);
        assert_eq!(json["projected"][0]["rated"], 360.0);
        // Null battery sum → null points.
        assert!(json["projected"][1]["ideal"].is_null());
        assert!(json["projected"][1]["rated"].is_null());
        // Zero battery sum → null points (no divide-by-zero).
        assert!(json["projected"][2]["ideal"].is_null());
        assert!(json["projected"][2]["rated"].is_null());
        assert_eq!(json["outside_temp"][0]["value"], 12.5);
    }

    #[tokio::test]
    async fn projected_range_unit_mi_applies_factor() {
        let server = mock_projected_db(PROJECTED_SUMS_ROWS, PROJECTED_TEMP_ROWS).await;
        let (status, json) = get_projected_range_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=mi",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let v = json["projected"][0]["ideal"].as_f64().unwrap();
        assert!((v - 400.0 * 0.621371).abs() < 0.001, "got {v}");
    }

    #[tokio::test]
    async fn projected_range_empty_window_is_empty_not_error() {
        let server = mock_projected_db(r#"{"results": [{}]}"#, r#"{"results": [{}]}"#).await;
        let (status, json) = get_projected_range_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["projected"].as_array().unwrap().len(), 0);
        assert_eq!(json["outside_temp"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn projected_range_rejects_inverted_range() {
        let (status, json) =
            get_projected_range("?vin=VIN1&from=2023-11-15T00:00:00Z&to=2023-11-14T00:00:00Z")
                .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("from is after to"));
    }

    #[tokio::test]
    async fn projected_range_rejects_oversized_range() {
        let (status, json) =
            get_projected_range("?vin=VIN1&from=2020-01-01T00:00:00Z&to=2023-11-15T00:00:00Z")
                .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("90-day maximum"));
    }

    #[tokio::test]
    async fn projected_range_rejects_bad_time() {
        let (status, json) =
            get_projected_range("?vin=VIN1&from=not-a-time&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid time"));
    }

    #[tokio::test]
    async fn projected_range_rejects_bad_unit() {
        let (status, json) = get_projected_range(
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=parsecs",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid unit"));
    }

    #[tokio::test]
    async fn projected_range_rejects_empty_vin() {
        let (status, _) =
            get_projected_range("?vin=&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn projected_range_upstream_failure_is_502() {
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
                    .uri("/projected-range?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    const EFFICIENCY_AVG_ROWS: &str = r#"{"results": [{"series": [{
        "name": "drives",
        "columns": ["time", "sum", "sum_1"],
        "values": [[1700000000000, 3740, 20000]]
    }]}]}"#;

    const EFFICIENCY_DIST_ROWS: &str = r#"{"results": [{"series": [{
        "name": "drives",
        "columns": ["time", "sum"],
        "values": [[1700000000000, 150000]]
    }]}]}"#;

    // `drives` is tagged vin + drive_id, so the raw per-drive query fans
    // out into one series per drive (deliberately out of order here): one
    // normal row, one zero-distance row (→ null), one null-energy row.
    const EFFICIENCY_PER_DRIVE_ROWS: &str = r#"{"results": [{"series": [
        {"name": "drives", "tags": {"drive_id": "d-2"},
         "columns": ["time", "energy_used_wh", "distance_meters"],
         "values": [[1700000300000, 500, 0]]},
        {"name": "drives", "tags": {"drive_id": "d-1"},
         "columns": ["time", "energy_used_wh", "distance_meters"],
         "values": [[1700000000000, 1870, 10000], [1700000100000, null, 5000]]}
    ]}]}"#;

    async fn mock_efficiency_db(
        body_avg: &str,
        body_dist: &str,
        body_drives: &str,
    ) -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        let avg = body_avg.to_string();
        let dist = body_dist.to_string();
        let drives = body_drives.to_string();
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(move |req: &wiremock::Request| {
                // The avg query selects sum("energy_used_wh"), the distance
                // query selects sum("distance_meters") only, and the
                // per-drive query selects the raw fields (no sum).
                // Match on bare identifiers: reqwest percent-encodes the
                // query text, so `sum(` arrives as `sum%28`.
                let q = req.url.query().unwrap_or("");
                let body = if q.contains("energy_used_wh") && q.contains("sum") {
                    avg.clone()
                } else if q.contains("sum") {
                    dist.clone()
                } else {
                    drives.clone()
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        server
    }

    async fn get_efficiency_on(
        server: &wiremock::MockServer,
        query: &str,
    ) -> (StatusCode, serde_json::Value) {
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/efficiency{query}"))
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

    async fn get_efficiency(query: &str) -> (StatusCode, serde_json::Value) {
        let server = mock_efficiency_db(
            EFFICIENCY_AVG_ROWS,
            EFFICIENCY_DIST_ROWS,
            EFFICIENCY_PER_DRIVE_ROWS,
        )
        .await;
        get_efficiency_on(&server, query).await
    }

    #[tokio::test]
    async fn efficiency_returns_series() {
        let (status, json) =
            get_efficiency("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=km")
                .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["vin"], "VIN1");
        // Ratio math on the raw sums: 3740/20000*1000 = 187.0 Wh/km.
        assert_eq!(json["avg_consumption_wh_per_km"], 187.0);
        // Logged distance: 150000m/1000*1 = 150.0 km.
        assert_eq!(json["logged_distance"], 150.0);
        // Per-drive rows combine across both drive_id series, sorted ASC:
        // 1870/10000*1000 = 187.0, null energy → null, zero distance → null.
        assert_eq!(json["per_drive"].as_array().unwrap().len(), 3);
        assert_eq!(json["per_drive"][0]["t"], 1700000000000i64);
        assert_eq!(json["per_drive"][0]["value"], 187.0);
        assert_eq!(json["per_drive"][1]["t"], 1700000100000i64);
        assert!(json["per_drive"][1]["value"].is_null());
        assert_eq!(json["per_drive"][2]["t"], 1700000300000i64);
        assert!(json["per_drive"][2]["value"].is_null());
    }

    #[tokio::test]
    async fn efficiency_unit_mi_applies_to_distance_only() {
        let server = mock_efficiency_db(
            EFFICIENCY_AVG_ROWS,
            EFFICIENCY_DIST_ROWS,
            EFFICIENCY_PER_DRIVE_ROWS,
        )
        .await;
        let (status, json) = get_efficiency_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=mi",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // Avg consumption is always Wh/km (no unit factor).
        assert_eq!(json["avg_consumption_wh_per_km"], 187.0);
        // Logged distance scales: 150.0 * 0.621371.
        let v = json["logged_distance"].as_f64().unwrap();
        assert!((v - 150.0 * 0.621371).abs() < 0.001, "got {v}");
    }

    #[tokio::test]
    async fn efficiency_empty_window_is_empty_not_error() {
        let server = mock_efficiency_db(
            r#"{"results": [{}]}"#,
            r#"{"results": [{}]}"#,
            r#"{"results": [{}]}"#,
        )
        .await;
        let (status, json) = get_efficiency_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(json["avg_consumption_wh_per_km"].is_null());
        assert_eq!(json["logged_distance"], 0.0);
        assert_eq!(json["per_drive"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn efficiency_rejects_inverted_range() {
        let (status, json) =
            get_efficiency("?vin=VIN1&from=2023-11-15T00:00:00Z&to=2023-11-14T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("from is after to"));
    }

    #[tokio::test]
    async fn efficiency_rejects_oversized_range() {
        let (status, json) =
            get_efficiency("?vin=VIN1&from=2015-01-01T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("5-year maximum"));
    }

    #[tokio::test]
    async fn efficiency_rejects_bad_time() {
        let (status, json) =
            get_efficiency("?vin=VIN1&from=not-a-time&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid time"));
    }

    #[tokio::test]
    async fn efficiency_rejects_bad_unit() {
        let (status, json) = get_efficiency(
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=parsecs",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid unit"));
    }

    #[tokio::test]
    async fn efficiency_rejects_empty_vin() {
        let (status, _) =
            get_efficiency("?vin=&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn efficiency_upstream_failure_is_502() {
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
                    .uri("/efficiency?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    // Raw per-drive rows fan out one series per drive_id (deliberately
    // out of order): two drives ending at the same address with no
    // geofence, one geofenced drive with no address, one fully unknown.
    // The same rows serve both top destinations (grouped in Rust) and
    // last visited.
    const LOCATIONS_ROWS_ROWS: &str = r#"{"results": [{"series": [
        {"name": "drives", "tags": {"drive_id": "d-2"},
         "columns": ["time", "end_address", "geofence_exit", "distance_meters", "end_time"],
         "values": [[1700000100000, "123 Main St", null, 20000, "2023-11-14T22:28:20Z"]]},
        {"name": "drives", "tags": {"drive_id": "d-4"},
         "columns": ["time", "end_address", "geofence_exit", "distance_meters", "end_time"],
         "values": [[1700000300000, null, null, 5000, "2023-11-14T22:31:40Z"]]},
        {"name": "drives", "tags": {"drive_id": "d-1"},
         "columns": ["time", "end_address", "geofence_exit", "distance_meters", "end_time"],
         "values": [[1700000000000, "123 Main St", null, 10000, "2023-11-14T22:26:40Z"]]},
        {"name": "drives", "tags": {"drive_id": "d-3"},
         "columns": ["time", "end_address", "geofence_exit", "distance_meters", "end_time"],
         "values": [[1700000200000, null, "Work", 12300, null]]}
    ]}]}"#;

    const LOCATIONS_LOGGED_ROWS: &str = r#"{"results": [{"series": [
        {"name": "drives", "tags": {"drive_id": "d-1"},
         "columns": ["time", "count"], "values": [[1700000000000, 2]]},
        {"name": "drives", "tags": {"drive_id": "d-2"},
         "columns": ["time", "count"], "values": [[1700000000000, 1]]}
    ]}]}"#;

    async fn mock_locations_db(body_rows: &str, body_logged: &str) -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        let rows = body_rows.to_string();
        let logged = body_logged.to_string();
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(move |req: &wiremock::Request| {
                // The drives-logged query is a `count(...)` aggregation
                // (matched on the bare "count" substring: the query text
                // is URL-encoded, so the parenthesis is percent-escaped);
                // the combined raw per-drive query (serving top +
                // last_visited) is the remaining branch.
                let q = req.url.query().unwrap_or("");
                let body = if q.contains("count") {
                    logged.clone()
                } else {
                    rows.clone()
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        server
    }

    async fn get_locations_on(
        server: &wiremock::MockServer,
        query: &str,
    ) -> (StatusCode, serde_json::Value) {
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/locations{query}"))
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

    async fn get_locations(query: &str) -> (StatusCode, serde_json::Value) {
        let server = mock_locations_db(LOCATIONS_ROWS_ROWS, LOCATIONS_LOGGED_ROWS).await;
        get_locations_on(&server, query).await
    }

    #[tokio::test]
    async fn locations_returns_top_last_visited_and_logged() {
        let (status, json) =
            get_locations("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=km")
                .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["vin"], "VIN1");
        // Top destinations group by place key (geofence else address else
        // "Unknown"), sorted desc by visits: the shared address pairs up.
        assert_eq!(json["top"].as_array().unwrap().len(), 3);
        assert_eq!(json["top"][0]["place"], "123 Main St");
        assert_eq!(json["top"][0]["visits"], 2);
        assert_eq!(json["top"][1]["place"], "Unknown");
        assert_eq!(json["top"][1]["visits"], 1);
        assert_eq!(json["top"][2]["place"], "Work");
        assert_eq!(json["top"][2]["visits"], 1);
        // Last visited combines all drive_id series newest-first, with
        // raw meters converted to km in Rust (5000m → 5.0). Times are the
        // drive END times (`end_time` field — points are stamped with the
        // drive start); d-3 has a null end_time and falls back to point
        // time (1700000200000).
        assert_eq!(json["last_visited"].as_array().unwrap().len(), 4);
        assert_eq!(json["last_visited"][0]["t"], 1700001100000i64);
        assert!(json["last_visited"][0]["address"].is_null());
        assert!(json["last_visited"][0]["geofence"].is_null());
        assert_eq!(json["last_visited"][0]["distance"], 5.0);
        assert_eq!(json["last_visited"][1]["t"], 1700000900000i64);
        assert_eq!(json["last_visited"][1]["address"], "123 Main St");
        assert_eq!(json["last_visited"][1]["distance"], 20.0);
        assert_eq!(json["last_visited"][2]["t"], 1700000800000i64);
        assert_eq!(json["last_visited"][2]["address"], "123 Main St");
        assert_eq!(json["last_visited"][2]["distance"], 10.0);
        assert_eq!(json["last_visited"][3]["t"], 1700000200000i64);
        assert_eq!(json["last_visited"][3]["geofence"], "Work");
        assert_eq!(json["last_visited"][3]["distance"], 12.3);
        // Drives logged sums the per-series counts (2 + 1).
        assert_eq!(json["drives_logged"], 3);
    }

    #[tokio::test]
    async fn locations_unit_mi_applies_to_distance() {
        let server = mock_locations_db(LOCATIONS_ROWS_ROWS, LOCATIONS_LOGGED_ROWS).await;
        let (status, json) = get_locations_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=mi",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let v = json["last_visited"][3]["distance"].as_f64().unwrap();
        assert!((v - 12.3 * 0.621371).abs() < 0.0001, "got {v}");
    }

    #[tokio::test]
    async fn locations_empty_window_is_empty_not_error() {
        let server = mock_locations_db(r#"{"results": [{}]}"#, r#"{"results": [{}]}"#).await;
        let (status, json) = get_locations_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["top"].as_array().unwrap().len(), 0);
        assert_eq!(json["last_visited"].as_array().unwrap().len(), 0);
        assert_eq!(json["drives_logged"], 0);
    }

    #[tokio::test]
    async fn locations_rejects_inverted_range() {
        let (status, json) =
            get_locations("?vin=VIN1&from=2023-11-15T00:00:00Z&to=2023-11-14T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("from is after to"));
    }

    #[tokio::test]
    async fn locations_rejects_oversized_range() {
        let (status, json) =
            get_locations("?vin=VIN1&from=2015-01-01T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("5-year maximum"));
    }

    #[tokio::test]
    async fn locations_rejects_bad_time() {
        let (status, json) =
            get_locations("?vin=VIN1&from=not-a-time&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid time"));
    }

    #[tokio::test]
    async fn locations_rejects_bad_unit() {
        let (status, json) = get_locations(
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=parsecs",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid unit"));
    }

    #[tokio::test]
    async fn locations_rejects_empty_vin() {
        let (status, _) =
            get_locations("?vin=&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn locations_upstream_failure_is_502() {
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
                    .uri("/locations?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    // `drives` is tagged vin + drive_id, so the combined aggregates fan
    // out one row per drive: distance/energy sums add up across series.
    const DRIVE_STATS_COMBINED_ROWS: &str = r#"{"results": [{"series": [
        {"name": "drives", "tags": {"drive_id": "d-1"},
         "columns": ["time", "count", "sum", "sum_1"],
         "values": [[1700000000000, 2, 30000, 5000]]},
        {"name": "drives", "tags": {"drive_id": "d-2"},
         "columns": ["time", "count", "sum", "sum_1"],
         "values": [[1700000100000, 1, 15000, 2500]]}
    ]}]}"#;

    const DRIVE_STATS_COUNT_ROWS: &str = r#"{"results": [{"series": [
        {"name": "drives", "tags": {"drive_id": "d-1"},
         "columns": ["time", "count"], "values": [[1700000000000, 2]]},
        {"name": "drives", "tags": {"drive_id": "d-2"},
         "columns": ["time", "count"], "values": [[1700000000000, 1]]}
    ]}]}"#;

    const DRIVE_STATS_PERCENTILE_ROWS: &str = r#"{"results": [{"series": [{
        "name": "drives",
        "columns": ["time", "percentile"],
        "values": [[1700000000000, 12000]]
    }]}]}"#;

    const DRIVE_STATS_MAX_ROWS: &str = r#"{"results": [{"series": [{
        "name": "drives",
        "columns": ["time", "max"],
        "values": [[1700000000000, 80]]
    }]}]}"#;

    const DRIVE_STATS_SPEEDS_ROWS: &str = r#"{"results": [{"series": [{
        "name": "positions",
        "columns": ["time", "speed"],
        "values": [[1700000000000, 10], [1700000100000, 60], [1700000200000, null]]
    }]}]}"#;

    const DRIVE_STATS_TOP_ROWS: &str = r#"{"results": [{"series": [
        {"name": "drives", "tags": {"drive_id": "d-1"},
         "columns": ["time", "end_address", "geofence_exit"],
         "values": [[1700000000000, null, "Home"]]},
        {"name": "drives", "tags": {"drive_id": "d-2"},
         "columns": ["time", "end_address", "geofence_exit"],
         "values": [[1700000100000, "123 Main St", null]]},
        {"name": "drives", "tags": {"drive_id": "d-3"},
         "columns": ["time", "end_address", "geofence_exit"],
         "values": [[1700000200000, null, "Home"]]},
        {"name": "drives", "tags": {"drive_id": "d-4"},
         "columns": ["time", "end_address", "geofence_exit"],
         "values": [[1700000300000, null, null]]}
    ]}]}"#;

    async fn mock_drive_stats_db() -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(|req: &wiremock::Request| {
                // Route by query content (bare identifiers: reqwest
                // percent-encodes the query text, so `count(` arrives as
                // `count%28` — match on the bare names instead).
                let q = req.url.query().unwrap_or("");
                let body = if q.contains("positions") {
                    DRIVE_STATS_SPEEDS_ROWS
                } else if q.contains("end_address") {
                    DRIVE_STATS_TOP_ROWS
                } else if q.contains("percentile") {
                    DRIVE_STATS_PERCENTILE_ROWS
                } else if q.contains("max_speed") {
                    DRIVE_STATS_MAX_ROWS
                } else if q.contains("count") && q.contains("sum") {
                    DRIVE_STATS_COMBINED_ROWS
                } else {
                    DRIVE_STATS_COUNT_ROWS
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        server
    }

    async fn get_drive_stats_on(
        server: &wiremock::MockServer,
        query: &str,
    ) -> (StatusCode, serde_json::Value) {
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/drive-stats{query}"))
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

    async fn get_drive_stats(query: &str) -> (StatusCode, serde_json::Value) {
        let server = mock_drive_stats_db().await;
        get_drive_stats_on(&server, query).await
    }

    #[tokio::test]
    async fn drive_stats_returns_full_stats() {
        let (status, json) =
            get_drive_stats("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=km")
                .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["vin"], "VIN1");
        // Unfiltered count sums across series (2 + 1).
        assert_eq!(json["drives"], 3);
        // Combined sums: distance (30000+15000)m/1000 = 45.0 km.
        assert_eq!(json["distance"], 45.0);
        // Energy mirrors Grafana (kWh, no unit factor): 7500/1000 = 7.5.
        assert_eq!(json["energy_kwh"], 7.5);
        // Median: 12000m/1000 = 12.0 km.
        assert_eq!(json["median_drive"], 12.0);
        // Max speed is stored MPH: 80 * 1.60934 = 128.7472 km/h.
        let m = json["max_speed"].as_f64().unwrap();
        assert!((m - 80.0 * 1.60934).abs() < 0.0001, "got {m}");
        // Speeds convert mph → km/h in Rust; the null row is skipped.
        let speeds = json["speeds"].as_array().unwrap();
        assert_eq!(speeds.len(), 2);
        assert!((speeds[0].as_f64().unwrap() - 10.0 * 1.60934).abs() < 0.0001);
        assert!((speeds[1].as_f64().unwrap() - 60.0 * 1.60934).abs() < 0.0001);
        // Top destinations share the Locations grouping: Home pairs up.
        assert_eq!(json["top_destinations"].as_array().unwrap().len(), 3);
        assert_eq!(json["top_destinations"][0]["place"], "Home");
        assert_eq!(json["top_destinations"][0]["visits"], 2);
        assert_eq!(json["top_destinations"][1]["place"], "123 Main St");
        assert_eq!(json["top_destinations"][2]["place"], "Unknown");
    }

    #[tokio::test]
    async fn drive_stats_unit_mi_applies_factors() {
        let (status, json) =
            get_drive_stats("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=mi")
                .await;
        assert_eq!(status, StatusCode::OK);
        // Distance + median scale by 0.621371; energy stays kWh.
        let d = json["distance"].as_f64().unwrap();
        assert!((d - 45.0 * 0.621371).abs() < 0.0001, "got {d}");
        let med = json["median_drive"].as_f64().unwrap();
        assert!((med - 12.0 * 0.621371).abs() < 0.0001, "got {med}");
        assert_eq!(json["energy_kwh"], 7.5);
        // Stored-mph max speed: 80 * 1.60934 * 0.621371 ≈ 80.0 mi/h.
        let m = json["max_speed"].as_f64().unwrap();
        assert!((m - 80.0).abs() < 0.01, "got {m}");
        // Speeds land back on the raw mph figures under mi.
        let speeds = json["speeds"].as_array().unwrap();
        assert!((speeds[0].as_f64().unwrap() - 10.0).abs() < 0.01);
        assert!((speeds[1].as_f64().unwrap() - 60.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn drive_stats_speeds_query_carries_limit() {
        let server = mock_drive_stats_db().await;
        let (status, _) = get_drive_stats_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // The dense per-poll positions query must be capped; the
        // top-destinations raw query carries its own LIMIT 10000.
        let received = server.received_requests().await.unwrap();
        let queries: Vec<String> = received
            .iter()
            .filter_map(|r| r.url.query().map(|s| s.to_string()))
            .collect();
        let speeds_q = queries
            .iter()
            .find(|q| q.contains("positions"))
            .cloned()
            .unwrap_or_default();
        assert!(
            speeds_q.contains("LIMIT") && speeds_q.contains("50000"),
            "speeds query should carry LIMIT 50000, got: {speeds_q}"
        );
        assert!(
            speeds_q.contains("speed"),
            "speeds query should select speed, got: {speeds_q}"
        );
        let top_q = queries
            .iter()
            .find(|q| q.contains("end_address"))
            .cloned()
            .unwrap_or_default();
        assert!(
            top_q.contains("10000"),
            "top query should carry LIMIT 10000, got: {top_q}"
        );
    }

    #[tokio::test]
    async fn drive_stats_empty_window_is_empty_not_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(r#"{"results": [{}]}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let (status, json) = get_drive_stats_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["drives"], 0);
        assert_eq!(json["distance"], 0.0);
        assert!(json["energy_kwh"].is_null());
        assert!(json["median_drive"].is_null());
        assert!(json["max_speed"].is_null());
        assert_eq!(json["speeds"].as_array().unwrap().len(), 0);
        assert_eq!(json["top_destinations"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn drive_stats_rejects_inverted_range() {
        let (status, json) =
            get_drive_stats("?vin=VIN1&from=2023-11-15T00:00:00Z&to=2023-11-14T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("from is after to"));
    }

    #[tokio::test]
    async fn drive_stats_rejects_oversized_range() {
        let (status, json) =
            get_drive_stats("?vin=VIN1&from=2015-01-01T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("5-year maximum"));
    }

    #[tokio::test]
    async fn drive_stats_rejects_bad_time() {
        let (status, json) =
            get_drive_stats("?vin=VIN1&from=not-a-time&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid time"));
    }

    #[tokio::test]
    async fn drive_stats_rejects_bad_unit() {
        let (status, json) = get_drive_stats(
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=parsecs",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid unit"));
    }

    #[tokio::test]
    async fn drive_stats_rejects_empty_vin() {
        let (status, _) =
            get_drive_stats("?vin=&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn drive_stats_upstream_failure_is_502() {
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
                    .uri("/drive-stats?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    // `charging_sessions` is tagged vin + charge_id, so aggregates fan
    // out one row per session: energy/cost sums add up across series.
    const CHARGES_COMBINED_ROWS: &str = r#"{"results": [{"series": [
        {"name": "charging_sessions", "tags": {"charge_id": "ch-1"},
         "columns": ["time", "count", "sum", "sum_1"],
         "values": [[1700000000000, 2, 20000, 6.0]]},
        {"name": "charging_sessions", "tags": {"charge_id": "ch-2"},
         "columns": ["time", "count", "sum", "sum_1"],
         "values": [[1700000100000, 1, 10000, 3.0]]}
    ]}]}"#;

    // The unfiltered # of Charges count fans out the same way (2 + 1).
    const CHARGES_COUNT_ROWS: &str = r#"{"results": [{"series": [
        {"name": "charging_sessions", "tags": {"charge_id": "ch-1"},
         "columns": ["time", "count"], "values": [[1700000000000, 2]]},
        {"name": "charging_sessions", "tags": {"charge_id": "ch-2"},
         "columns": ["time", "count"], "values": [[1700000000000, 1]]}
    ]}]}"#;

    // Raw session rows fan out one series per charge_id (deliberately out
    // of order here): one full row, one row with nulls.
    const CHARGES_SESSIONS_ROWS: &str = r#"{"results": [{"series": [
        {"name": "charging_sessions", "tags": {"charge_id": "ch-2"},
         "columns": ["time", "geofence_name", "energy_added_wh", "duration_seconds", "cost", "start_battery_level", "end_battery_level"],
         "values": [[1700000300000, null, 10000, 3600, 3.0, 40, 70]]},
        {"name": "charging_sessions", "tags": {"charge_id": "ch-1"},
         "columns": ["time", "geofence_name", "energy_added_wh", "duration_seconds", "cost", "start_battery_level", "end_battery_level"],
         "values": [[1700000000000, "Home", 20000, 7200, 6.0, 20, 80], [1700000100000, "Work", null, null, null, null, null]]}
    ]}]}"#;

    // fill(0) buckets: an explicit zero-energy day passes through as 0.0.
    const CHARGES_ENERGY_PER_DAY_ROWS: &str = r#"{"results": [{"series": [{
        "name": "charging_sessions",
        "columns": ["time", "sum"],
        "values": [[1700000000000, 20000], [1700086400000, 0], [1700172800000, null]]
    }]}]}"#;

    // Power fans out one series per charge_id tag; watts → kW in Rust.
    const CHARGES_POWER_ROWS: &str = r#"{"results": [{"series": [
        {"name": "charge_readings", "tags": {"charge_id": "ch-1"},
         "columns": ["time", "mean"],
         "values": [[1700000000000, 11000], [1700000300000, null]]},
        {"name": "charge_readings", "tags": {"charge_id": "ch-2"},
         "columns": ["time", "mean"],
         "values": [[1700000100000, 5500]]}
    ]}]}"#;

    async fn mock_charges_db() -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(|req: &wiremock::Request| {
                // Route by query content (bare identifiers: reqwest
                // percent-encodes the query text, so `mean(` arrives as
                // `mean%28` — match on the bare names instead).
                let q = req.url.query().unwrap_or("");
                let body = if q.contains("charge_readings") {
                    CHARGES_POWER_ROWS
                } else if q.contains("geofence_name") {
                    CHARGES_SESSIONS_ROWS
                } else if q.contains("1d") {
                    CHARGES_ENERGY_PER_DAY_ROWS
                } else if q.contains("cost") {
                    CHARGES_COMBINED_ROWS
                } else {
                    CHARGES_COUNT_ROWS
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        server
    }

    async fn get_charges_on(
        server: &wiremock::MockServer,
        query: &str,
    ) -> (StatusCode, serde_json::Value) {
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/charges{query}"))
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

    async fn get_charges(query: &str) -> (StatusCode, serde_json::Value) {
        let server = mock_charges_db().await;
        get_charges_on(&server, query).await
    }

    #[tokio::test]
    async fn charges_returns_full_stats() {
        let (status, json) =
            get_charges("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["vin"], "VIN1");
        // Unfiltered count sums across series (2 + 1).
        assert_eq!(json["count"], 3);
        // Folded sums: energy (20000+10000)/1000 = 30.0 kWh, cost 9.0.
        assert_eq!(json["energy_kwh"], 30.0);
        assert_eq!(json["total_cost"], 9.0);
        // Avg ratio: 9/30000*1000 = 0.3.
        assert_eq!(json["avg_cost_per_kwh"], 0.3);
        // Sessions combine across both charge_id series, sorted ASC:
        // energy /1000, duration /60 in Rust. Each row carries its
        // series' charge_id (read from tags, never a column).
        assert_eq!(json["sessions"].as_array().unwrap().len(), 3);
        assert_eq!(json["sessions"][0]["t"], 1700000000000i64);
        assert_eq!(json["sessions"][0]["charge_id"], "ch-1");
        assert_eq!(json["sessions"][0]["location"], "Home");
        assert_eq!(json["sessions"][0]["energy_kwh"], 20.0);
        assert_eq!(json["sessions"][0]["duration_min"], 120.0);
        assert_eq!(json["sessions"][0]["cost"], 6.0);
        assert_eq!(json["sessions"][0]["start_pct"], 20.0);
        assert_eq!(json["sessions"][0]["end_pct"], 80.0);
        assert_eq!(json["sessions"][1]["t"], 1700000100000i64);
        assert_eq!(json["sessions"][1]["charge_id"], "ch-1");
        assert_eq!(json["sessions"][1]["location"], "Work");
        assert!(json["sessions"][1]["energy_kwh"].is_null());
        assert_eq!(json["sessions"][2]["t"], 1700000300000i64);
        assert_eq!(json["sessions"][2]["charge_id"], "ch-2");
        assert!(json["sessions"][2]["location"].is_null());
        assert_eq!(json["sessions"][2]["energy_kwh"], 10.0);
        assert_eq!(json["sessions"][2]["duration_min"], 60.0);
        // Energy per day: /1000 in Rust; the fill(0) zero stays 0.0.
        assert_eq!(json["energy_per_day"].as_array().unwrap().len(), 3);
        assert_eq!(json["energy_per_day"][0]["value"], 20.0);
        assert_eq!(json["energy_per_day"][1]["value"], 0.0);
        assert!(json["energy_per_day"][2]["value"].is_null());
        // Power: one series per charge_id, watts → kW; nulls stay null.
        assert_eq!(json["power_by_session"].as_array().unwrap().len(), 2);
        assert_eq!(json["power_by_session"][0]["charge_id"], "ch-1");
        assert_eq!(json["power_by_session"][0]["points"][0]["value"], 11.0);
        assert!(json["power_by_session"][0]["points"][1]["value"].is_null());
        assert_eq!(json["power_by_session"][1]["charge_id"], "ch-2");
        assert_eq!(json["power_by_session"][1]["points"][0]["value"], 5.5);
    }

    #[tokio::test]
    async fn charges_null_energy_maps_avg_to_null() {
        const EMPTY: &str = r#"{"results": [{}]}"#;
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(|req: &wiremock::Request| {
                let q = req.url.query().unwrap_or("");
                let body = if q.contains("charge_readings")
                    || q.contains("geofence_name")
                    || q.contains("1d")
                {
                    EMPTY
                } else if q.contains("cost") {
                    // Cost without energy: ratio has no denominator.
                    r#"{"results": [{"series": [{
                        "name": "charging_sessions",
                        "columns": ["time", "count", "sum", "sum_1"],
                        "values": [[1700000000000, 1, null, 3.0]]
                    }]}]}"#
                } else {
                    r#"{"results": [{"series": [{
                        "name": "charging_sessions",
                        "columns": ["time", "count"],
                        "values": [[1700000000000, 1]]
                    }]}]}"#
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        let (status, json) = get_charges_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["count"], 1);
        assert_eq!(json["energy_kwh"], 0.0);
        assert_eq!(json["total_cost"], 3.0);
        assert!(json["avg_cost_per_kwh"].is_null());
    }

    #[tokio::test]
    async fn charges_queries_carry_expected_clauses() {
        let server = mock_charges_db().await;
        let (status, _) = get_charges_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let received = server.received_requests().await.unwrap();
        let queries: Vec<String> = received
            .iter()
            .filter_map(|r| r.url.query().map(|s| s.to_string()))
            .collect();
        // Energy per day follows the JSON's fill(0) (not fill(previous)).
        let day_q = queries
            .iter()
            .find(|q| q.contains("1d"))
            .cloned()
            .unwrap_or_default();
        // The query text rides URL-encoded (`fill(0)` arrives as
        // `fill%280%29`), so match the bare `fill` token.
        assert!(
            day_q.contains("fill"),
            "energy-per-day query should carry fill(0), got: {day_q}"
        );
        // The sessions table carries a bounding LIMIT the JSON lacks.
        let sessions_q = queries
            .iter()
            .find(|q| q.contains("geofence_name"))
            .cloned()
            .unwrap_or_default();
        assert!(
            sessions_q.contains("LIMIT") && sessions_q.contains("1000"),
            "sessions query should carry LIMIT 1000, got: {sessions_q}"
        );
        // The sessions query groups by charge_id so the raw multi-tag
        // query fans out one series per session (without it InfluxQL
        // merges raw rows into one tag-less series and the per-session
        // id is lost). The SELECT list never names charge_id, so its
        // presence pins the GROUP BY clause.
        assert!(
            sessions_q.contains("charge_id"),
            "sessions query should GROUP BY charge_id, got: {sessions_q}"
        );
        // Power uses fixed 5-minute buckets (never $__interval) + a cap.
        let power_q = queries
            .iter()
            .find(|q| q.contains("charge_readings"))
            .cloned()
            .unwrap_or_default();
        assert!(
            power_q.contains("5m"),
            "power query should use fixed 5m buckets, got: {power_q}"
        );
        assert!(
            power_q.contains("20000"),
            "power query should carry LIMIT 20000, got: {power_q}"
        );
    }

    #[tokio::test]
    async fn charges_empty_window_is_empty_not_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(r#"{"results": [{}]}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let (status, json) = get_charges_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["count"], 0);
        assert_eq!(json["energy_kwh"], 0.0);
        assert_eq!(json["total_cost"], 0.0);
        assert!(json["avg_cost_per_kwh"].is_null());
        assert_eq!(json["sessions"].as_array().unwrap().len(), 0);
        assert_eq!(json["energy_per_day"].as_array().unwrap().len(), 0);
        assert_eq!(json["power_by_session"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn charges_rejects_inverted_range() {
        let (status, json) =
            get_charges("?vin=VIN1&from=2023-11-15T00:00:00Z&to=2023-11-14T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("from is after to"));
    }

    #[tokio::test]
    async fn charges_rejects_oversized_range() {
        let (status, json) =
            get_charges("?vin=VIN1&from=2015-01-01T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("5-year maximum"));
    }

    #[tokio::test]
    async fn charges_rejects_bad_time() {
        let (status, json) = get_charges("?vin=VIN1&from=not-a-time&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid time"));
    }

    #[tokio::test]
    async fn charges_rejects_empty_vin() {
        let (status, _) =
            get_charges("?vin=&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn charges_upstream_failure_is_502() {
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
                    .uri("/charges?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
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

    // Charging-stats fixtures reuse the charges aggregate math (the shared
    // `charging_summary` helper): unfiltered count 2 + 1, folded sums
    // 30000 Wh / 9.0 cost.
    const CHARGING_STATS_SOC_ROWS: &str = r#"{"results": [{"series": [{
        "name": "charging_sessions",
        "columns": ["time", "mean", "mean_1"],
        "values": [[1700000000000, 30, 75], [1700086400000, null, 60]]
    }]}]}"#;

    // `GROUP BY "geofence_name"` fans out one series per station; the last
    // two series exercise the Unknown fallback (missing tag, empty tag).
    // Energies are deliberately unordered so the test pins the DESC-by-energy
    // sort (NOT by visits): Office has the most sessions but less energy
    // than Home.
    const CHARGING_STATS_TOP_ROWS: &str = r#"{"results": [{"series": [
        {"name": "charging_sessions", "tags": {"geofence_name": "Office"},
         "columns": ["time", "sum", "count"], "values": [[1700000000000, 10000, 3]]},
        {"name": "charging_sessions", "tags": {"geofence_name": "Home"},
         "columns": ["time", "sum", "count"], "values": [[1700000000000, 20000, 2]]},
        {"name": "charging_sessions",
         "columns": ["time", "sum", "count"], "values": [[1700000000000, 5000, 1]]},
        {"name": "charging_sessions", "tags": {"geofence_name": ""},
         "columns": ["time", "sum", "count"], "values": [[1700000000000, 7000, 1]]}
    ]}]}"#;

    async fn mock_charging_stats_db() -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(|req: &wiremock::Request| {
                // Route by query content (bare identifiers: reqwest
                // percent-encodes the query text, so `mean(` arrives as
                // `mean%28` — match on the bare names instead). The
                // soc-delta query is the only one selecting
                // start_battery_level; the top-stations query is the only
                // one grouping by geofence_name.
                let q = req.url.query().unwrap_or("");
                let body = if q.contains("start_battery_level") {
                    CHARGING_STATS_SOC_ROWS
                } else if q.contains("geofence_name") {
                    CHARGING_STATS_TOP_ROWS
                } else if q.contains("cost") {
                    CHARGES_COMBINED_ROWS
                } else {
                    CHARGES_COUNT_ROWS
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        server
    }

    async fn get_charging_stats_on(
        server: &wiremock::MockServer,
        query: &str,
    ) -> (StatusCode, serde_json::Value) {
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/charging-stats{query}"))
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

    async fn get_charging_stats(query: &str) -> (StatusCode, serde_json::Value) {
        let server = mock_charging_stats_db().await;
        get_charging_stats_on(&server, query).await
    }

    #[tokio::test]
    async fn charging_stats_returns_full_stats() {
        let (status, json) =
            get_charging_stats("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["vin"], "VIN1");
        // Shared-helper math matches the charges endpoint: unfiltered count
        // 2 + 1, energy (20000+10000)/1000 = 30.0 kWh, cost 9.0, avg 0.3.
        assert_eq!(json["count"], 3);
        assert_eq!(json["energy_kwh"], 30.0);
        assert_eq!(json["total_cost"], 9.0);
        assert_eq!(json["avg_cost_per_kwh"], 0.3);
        // SOC delta means: start/end per daily bucket, null stays null.
        assert_eq!(json["soc_delta"].as_array().unwrap().len(), 2);
        assert_eq!(json["soc_delta"][0]["t"], 1700000000000i64);
        assert_eq!(json["soc_delta"][0]["start"], 30.0);
        assert_eq!(json["soc_delta"][0]["end"], 75.0);
        assert!(json["soc_delta"][1]["start"].is_null());
        assert_eq!(json["soc_delta"][1]["end"], 60.0);
        // Top stations sorted DESC by energy (not visits): Home 20 kWh first
        // despite fewer sessions than Office; the missing/empty tags fold
        // into one Unknown bucket (5 + 7 = 12 kWh, 2 sessions).
        assert_eq!(json["top_stations"].as_array().unwrap().len(), 3);
        assert_eq!(json["top_stations"][0]["station"], "Home");
        assert_eq!(json["top_stations"][0]["energy_kwh"], 20.0);
        assert_eq!(json["top_stations"][0]["sessions"], 2);
        assert_eq!(json["top_stations"][1]["station"], "Unknown");
        assert_eq!(json["top_stations"][1]["energy_kwh"], 12.0);
        assert_eq!(json["top_stations"][1]["sessions"], 2);
        assert_eq!(json["top_stations"][2]["station"], "Office");
        assert_eq!(json["top_stations"][2]["energy_kwh"], 10.0);
        assert_eq!(json["top_stations"][2]["sessions"], 3);
    }

    #[tokio::test]
    async fn charging_stats_queries_carry_expected_clauses() {
        let server = mock_charging_stats_db().await;
        let (status, _) = get_charging_stats_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let received = server.received_requests().await.unwrap();
        let queries: Vec<String> = received
            .iter()
            .filter_map(|r| r.url.query().map(|s| s.to_string()))
            .collect();
        // SOC delta uses fixed daily buckets with fill(previous) — never a
        // range-derived $__interval.
        let soc_q = queries
            .iter()
            .find(|q| q.contains("start_battery_level"))
            .cloned()
            .unwrap_or_default();
        assert!(
            soc_q.contains("1d"),
            "soc-delta query should use fixed 1d buckets, got: {soc_q}"
        );
        assert!(
            soc_q.contains("fill"),
            "soc-delta query should carry fill(previous), got: {soc_q}"
        );
        // Top stations groups by geofence_name.
        let top_q = queries
            .iter()
            .find(|q| q.contains("geofence_name"))
            .cloned()
            .unwrap_or_default();
        assert!(
            top_q.contains("GROUP"),
            "top-stations query should GROUP BY geofence_name, got: {top_q}"
        );
    }

    #[tokio::test]
    async fn charging_stats_empty_window_is_empty_not_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(r#"{"results": [{}]}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let (status, json) = get_charging_stats_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["count"], 0);
        assert_eq!(json["energy_kwh"], 0.0);
        assert_eq!(json["total_cost"], 0.0);
        assert!(json["avg_cost_per_kwh"].is_null());
        assert_eq!(json["soc_delta"].as_array().unwrap().len(), 0);
        assert_eq!(json["top_stations"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn charging_stats_rejects_inverted_range() {
        let (status, json) =
            get_charging_stats("?vin=VIN1&from=2023-11-15T00:00:00Z&to=2023-11-14T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("from is after to"));
    }

    #[tokio::test]
    async fn charging_stats_rejects_oversized_range() {
        let (status, json) =
            get_charging_stats("?vin=VIN1&from=2015-01-01T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("5-year maximum"));
    }

    #[tokio::test]
    async fn charging_stats_rejects_bad_time() {
        let (status, json) =
            get_charging_stats("?vin=VIN1&from=not-a-time&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid time"));
    }

    #[tokio::test]
    async fn charging_stats_rejects_empty_vin() {
        let (status, _) =
            get_charging_stats("?vin=&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn charging_stats_upstream_failure_is_502() {
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
                    .uri("/charging-stats?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    // Session stats: positional `last` .. `last_7` in select order (cost,
    // duration, geofence, energy Wh, lat, lng, end SOC, start SOC).
    const CHARGE_DETAILS_SESSION_ROWS: &str = r#"{"results": [{"series": [{
        "name": "charging_sessions",
        "columns": ["time", "last", "last_1", "last_2", "last_3", "last_4", "last_5", "last_6", "last_7"],
        "values": [[1700000300000, 6.0, 7200, "Home", 20000, 37.5, -122.1, 80, 20]]
    }]}]}"#;

    // Curve: battery %, power watts, cumulative energy Wh at 1m buckets
    // (one null energy cell to pin null passthrough).
    const CHARGE_DETAILS_CURVE_ROWS: &str = r#"{"results": [{"series": [{
        "name": "charge_readings",
        "columns": ["time", "mean", "mean_1", "mean_2"],
        "values": [[1700000000000, 50, 11000, 5000], [1700000060000, 55, 5500, null]]
    }]}]}"#;

    // Earliest session point (≈ session start) for the single span query
    // (`ORDER BY time ASC LIMIT 1`); end = start + duration.
    const CHARGE_DETAILS_START_ROWS: &str = r#"{"results": [{"series": [{
        "name": "charging_sessions",
        "columns": ["time", "duration_seconds"],
        "values": [[1700000000000, 451]]
    }]}]}"#;

    async fn mock_charge_details_db(session_body: &str, curve_body: &str) -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        let session = session_body.to_string();
        let curve = curve_body.to_string();
        let start = CHARGE_DETAILS_START_ROWS.to_string();
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(move |req: &wiremock::Request| {
                let q = req.url.query().unwrap_or("");
                // Span query sorts ASC; the curve reads charge_readings;
                // the folded last() stats are the remaining branch.
                let body = if q.contains("charge_readings") {
                    curve.clone()
                } else if q.contains("ASC") {
                    start.clone()
                } else {
                    session.clone()
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        server
    }

    async fn get_charge_details_on(
        server: &wiremock::MockServer,
        query: &str,
    ) -> (StatusCode, serde_json::Value) {
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/charge-details{query}"))
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

    async fn get_charge_details(query: &str) -> (StatusCode, serde_json::Value) {
        let server =
            mock_charge_details_db(CHARGE_DETAILS_SESSION_ROWS, CHARGE_DETAILS_CURVE_ROWS).await;
        get_charge_details_on(&server, query).await
    }

    #[tokio::test]
    async fn charge_details_returns_full_stats() {
        let server =
            mock_charge_details_db(CHARGE_DETAILS_SESSION_ROWS, CHARGE_DETAILS_CURVE_ROWS).await;
        let (status, json) = get_charge_details_on(&server, "?charge_id=ch-1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["charge_id"], "ch-1");
        assert_eq!(json["cost"], 6.0);
        assert_eq!(json["duration_s"], 7200.0);
        assert_eq!(json["geofence"], "Home");
        // Energy /1000 in Rust.
        assert_eq!(json["energy_kwh"], 20.0);
        assert_eq!(json["lat"], 37.5);
        assert_eq!(json["lng"], -122.1);
        // SOC added is end − start in Rust.
        assert_eq!(json["soc_added"], 60.0);
        // Session span: start from the earliest point, end = start +
        // duration (7200s → +7_200_000ms).
        assert_eq!(json["start_t"], 1700000000000i64);
        assert_eq!(json["end_t"], 1700007200000i64);
        // Curve: battery passthrough, watts → kW, Wh → kWh; null stays null.
        assert_eq!(json["curve"].as_array().unwrap().len(), 2);
        assert_eq!(json["curve"][0]["t"], 1700000000000i64);
        assert_eq!(json["curve"][0]["battery"], 50.0);
        assert_eq!(json["curve"][0]["power_kw"], 11.0);
        assert_eq!(json["curve"][0]["added_kwh"], 5.0);
        assert_eq!(json["curve"][1]["t"], 1700000060000i64);
        assert_eq!(json["curve"][1]["battery"], 55.0);
        assert_eq!(json["curve"][1]["power_kw"], 5.5);
        assert!(json["curve"][1]["added_kwh"].is_null());

        // The curve uses fixed 1-minute buckets (never range-derived); the
        // session query filters by charge_id with no time window.
        let received = server.received_requests().await.unwrap();
        let queries: Vec<String> = received
            .iter()
            .filter_map(|r| r.url.query().map(|s| s.to_string()))
            .collect();
        let curve_q = queries
            .iter()
            .find(|q| q.contains("charge_readings"))
            .cloned()
            .unwrap_or_default();
        assert!(
            curve_q.contains("1m"),
            "curve query should use fixed 1m buckets, got: {curve_q}"
        );
        assert!(
            curve_q.contains("fill"),
            "curve query should carry fill(previous), got: {curve_q}"
        );
        let session_q = queries
            .iter()
            .find(|q| q.contains("charging_sessions"))
            .cloned()
            .unwrap_or_default();
        assert!(
            session_q.contains("charge_id"),
            "session query should filter by charge_id, got: {session_q}"
        );
        assert!(
            session_q.contains("last"),
            "session query should select last() aggregates, got: {session_q}"
        );
    }

    #[tokio::test]
    async fn charge_details_unknown_id_is_404() {
        let server =
            mock_charge_details_db(r#"{"results": [{}]}"#, CHARGE_DETAILS_CURVE_ROWS).await;
        let (status, json) = get_charge_details_on(&server, "?charge_id=nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["error"], "charge not found");
    }

    #[tokio::test]
    async fn charge_details_empty_curve_is_empty_not_error() {
        let server =
            mock_charge_details_db(CHARGE_DETAILS_SESSION_ROWS, r#"{"results": [{}]}"#).await;
        let (status, json) = get_charge_details_on(&server, "?charge_id=ch-1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["charge_id"], "ch-1");
        assert_eq!(json["cost"], 6.0);
        assert_eq!(json["soc_added"], 60.0);
        assert_eq!(json["curve"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn charge_details_rejects_missing_or_empty_id() {
        let (status, json) = get_charge_details("").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("charge_id"));
        let (status, json) = get_charge_details("?charge_id=").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("charge_id"));
    }

    #[tokio::test]
    async fn charge_details_upstream_failure_is_502() {
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
                    .uri("/charge-details?charge_id=ch-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    // Folded count + sums fan out one row per drive_id series (2 rows here).
    const DRIVES_COMBINED_ROWS: &str = r#"{"results": [{"series": [
        {"name": "drives", "tags": {"drive_id": "d-1"},
         "columns": ["time", "count", "sum", "sum_1", "sum_2"],
         "values": [[1700000000000, 1, 20000, 3600, 5000]]},
        {"name": "drives", "tags": {"drive_id": "d-2"},
         "columns": ["time", "count", "sum", "sum_1", "sum_2"],
         "values": [[1700000300000, 1, 10000, 1800, 2500]]}
    ]}]}"#;

    // The unfiltered # of Drives count fans out the same way (2 + 1).
    const DRIVES_COUNT_ROWS: &str = r#"{"results": [{"series": [
        {"name": "drives", "tags": {"drive_id": "d-1"},
         "columns": ["time", "count"], "values": [[1700000000000, 2]]},
        {"name": "drives", "tags": {"drive_id": "d-2"},
         "columns": ["time", "count"], "values": [[1700000000000, 1]]}
    ]}]}"#;

    // Raw drive rows fan out one series per drive_id (deliberately out of
    // order here). d-1 carries both ISO time fields; d-2 carries neither
    // (falls back to [t, t + duration]) and has a zero distance plus nulls.
    const DRIVES_TABLE_ROWS: &str = r#"{"results": [{"series": [
        {"name": "drives", "tags": {"drive_id": "d-2"},
         "columns": ["time", "distance_meters", "duration_seconds", "energy_used_wh", "average_speed", "max_speed", "geofence_enter", "geofence_exit", "start_time", "end_time"],
         "values": [[1700000300000, 0, 600, null, null, 60, null, null, null, null]]},
        {"name": "drives", "tags": {"drive_id": "d-1"},
         "columns": ["time", "distance_meters", "duration_seconds", "energy_used_wh", "average_speed", "max_speed", "geofence_enter", "geofence_exit", "start_time", "end_time"],
         "values": [[1700000000000, 20000, 3600, 5000, 50, 70, "Home", "Work", "2023-11-14T22:13:20Z", "2023-11-14T23:13:20Z"]]}
    ]}]}"#;

    // fill(0) buckets: an explicit zero-distance day passes through as 0.0.
    const DRIVES_PER_DAY_ROWS: &str = r#"{"results": [{"series": [{
        "name": "drives",
        "columns": ["time", "sum"],
        "values": [[1700000000000, 20000], [1700086400000, 0], [1700172800000, null]]
    }]}]}"#;

    async fn mock_drives_db() -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(|req: &wiremock::Request| {
                // Route by query content (bare identifiers: reqwest
                // percent-encodes the query text, so `sum(` arrives as
                // `sum%28` — match on the bare names instead). The table
                // query is the only one selecting the ISO time fields; the
                // folded query is the only other one naming energy_used_wh.
                let q = req.url.query().unwrap_or("");
                let body = if q.contains("1d") {
                    DRIVES_PER_DAY_ROWS
                } else if q.contains("start_time") {
                    DRIVES_TABLE_ROWS
                } else if q.contains("energy_used_wh") {
                    DRIVES_COMBINED_ROWS
                } else {
                    DRIVES_COUNT_ROWS
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        server
    }

    async fn get_drives_on(
        server: &wiremock::MockServer,
        query: &str,
    ) -> (StatusCode, serde_json::Value) {
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/drives{query}"))
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

    async fn get_drives(query: &str) -> (StatusCode, serde_json::Value) {
        let server = mock_drives_db().await;
        get_drives_on(&server, query).await
    }

    #[tokio::test]
    async fn drives_returns_full_stats() {
        let (status, json) =
            get_drives("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=km").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["vin"], "VIN1");
        // Unfiltered count sums across series (2 + 1); the folded count
        // cell (1 + 1) is unused.
        assert_eq!(json["count"], 3);
        // Folded sums: distance (20000+10000)/1000 = 30.0 km, duration
        // 3600+1800 = 5400 s, energy (5000+2500)/1000 = 7.5 kWh.
        assert_eq!(json["distance"], 30.0);
        assert_eq!(json["duration_s"], 5400);
        assert_eq!(json["energy_kwh"], 7.5);
        // Table rows combine across both drive_id series, sorted ASC. Each
        // row carries its series' drive_id (read from tags, never a column).
        assert_eq!(json["drives"].as_array().unwrap().len(), 2);
        assert_eq!(json["drives"][0]["t"], 1700000000000i64);
        assert_eq!(json["drives"][0]["drive_id"], "d-1");
        assert_eq!(json["drives"][0]["distance"], 20.0);
        assert_eq!(json["drives"][0]["duration_min"], 60.0);
        assert_eq!(json["drives"][0]["energy_kwh"], 5.0);
        // Speeds are mph-stored: 50 mph → 50*1.60934 km/h.
        assert!((json["drives"][0]["avg_speed"].as_f64().unwrap() - 80.467).abs() < 0.001);
        assert!((json["drives"][0]["max_speed"].as_f64().unwrap() - 112.6538).abs() < 0.001);
        assert_eq!(json["drives"][0]["from_gf"], "Home");
        assert_eq!(json["drives"][0]["to_gf"], "Work");
        // Window prefers the ISO fields: start == point time here, end ==
        // start + 3600 s.
        assert_eq!(json["drives"][0]["from_t"], 1700000000000i64);
        assert_eq!(json["drives"][0]["to_t"], 1700003600000i64);
        // Zero distance converts to 0.0 (not null); nulls stay null.
        assert_eq!(json["drives"][1]["drive_id"], "d-2");
        assert_eq!(json["drives"][1]["distance"], 0.0);
        assert_eq!(json["drives"][1]["duration_min"], 10.0);
        assert!(json["drives"][1]["energy_kwh"].is_null());
        assert!(json["drives"][1]["avg_speed"].is_null());
        assert!(json["drives"][1]["from_gf"].is_null());
        assert!(json["drives"][1]["to_gf"].is_null());
        // Window falls back to [t, t + duration] without ISO fields.
        assert_eq!(json["drives"][1]["from_t"], 1700000300000i64);
        assert_eq!(json["drives"][1]["to_t"], 1700000900000i64);
        // Distance per day: /1000 in Rust; the fill(0) zero stays 0.0.
        assert_eq!(json["per_day"].as_array().unwrap().len(), 3);
        assert_eq!(json["per_day"][0]["value"], 20.0);
        assert_eq!(json["per_day"][1]["value"], 0.0);
        assert!(json["per_day"][2]["value"].is_null());
    }

    #[tokio::test]
    async fn drives_unit_mi_applies_factors() {
        let server = mock_drives_db().await;
        let (status, json) = get_drives_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=mi",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // 30000 m → 30 * 0.621371 mi.
        let d = json["distance"].as_f64().unwrap();
        assert!((d - 18.64113).abs() < 0.001, "expected ~18.64, got {d}");
        // Speeds: mph → 1.60934 * 0.621371 ≈ identity.
        let avg = json["drives"][0]["avg_speed"].as_f64().unwrap();
        assert!((avg - 50.0).abs() < 0.01, "expected ~50.0, got {avg}");
        let per_day = json["per_day"][0]["value"].as_f64().unwrap();
        assert!(
            (per_day - 12.42742).abs() < 0.001,
            "expected ~12.43, got {per_day}"
        );
        // Duration and energy carry no unit factor.
        assert_eq!(json["duration_s"], 5400);
        assert_eq!(json["energy_kwh"], 7.5);
    }

    #[tokio::test]
    async fn drives_queries_carry_expected_clauses() {
        let server = mock_drives_db().await;
        let (status, _) = get_drives_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let received = server.received_requests().await.unwrap();
        let queries: Vec<String> = received
            .iter()
            .filter_map(|r| r.url.query().map(|s| s.to_string()))
            .collect();
        // Distance per day follows the JSON's fill(0) (not fill(previous)).
        // The query text rides URL-encoded (`fill(0)` arrives as
        // `fill%280%29`), so match the bare `fill` token.
        let day_q = queries
            .iter()
            .find(|q| q.contains("1d"))
            .cloned()
            .unwrap_or_default();
        assert!(
            day_q.contains("fill"),
            "distance-per-day query should carry fill(0), got: {day_q}"
        );
        // The table query groups by drive_id so the raw multi-tag query
        // fans out one series per drive (without it InfluxQL merges raw
        // rows into one tag-less series and the per-drive id is lost). The
        // SELECT list never names drive_id, so its presence pins the
        // GROUP BY clause. It also carries a bounding LIMIT the JSON lacks
        // (one row per drive_id series, so effectively complete).
        let table_q = queries
            .iter()
            .find(|q| q.contains("geofence_enter"))
            .cloned()
            .unwrap_or_default();
        assert!(
            table_q.contains("drive_id"),
            "drives query should GROUP BY drive_id, got: {table_q}"
        );
        assert!(
            table_q.contains("LIMIT") && table_q.contains("1000"),
            "drives query should carry LIMIT 1000, got: {table_q}"
        );
    }

    #[tokio::test]
    async fn drives_empty_window_is_empty_not_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(r#"{"results": [{}]}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let (status, json) = get_drives_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["count"], 0);
        assert_eq!(json["distance"], 0.0);
        assert_eq!(json["duration_s"], 0);
        assert!(json["energy_kwh"].is_null());
        assert_eq!(json["drives"].as_array().unwrap().len(), 0);
        assert_eq!(json["per_day"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn drives_rejects_inverted_range() {
        let (status, json) =
            get_drives("?vin=VIN1&from=2023-11-15T00:00:00Z&to=2023-11-14T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("from is after to"));
    }

    #[tokio::test]
    async fn drives_rejects_oversized_range() {
        let (status, json) =
            get_drives("?vin=VIN1&from=2015-01-01T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("5-year maximum"));
    }

    #[tokio::test]
    async fn drives_rejects_bad_time() {
        let (status, json) = get_drives("?vin=VIN1&from=not-a-time&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid time"));
    }

    #[tokio::test]
    async fn drives_rejects_bad_unit() {
        let (status, json) =
            get_drives("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=parsecs")
                .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid unit"));
    }

    #[tokio::test]
    async fn drives_rejects_empty_vin() {
        let (status, _) =
            get_drives("?vin=&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn drives_upstream_failure_is_502() {
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
                    .uri("/drives?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    // Window carrying both ISO fields (preferred derivation).
    const DD_WINDOW_ISO_ROWS: &str = r#"{"results": [{"series": [{
        "name": "drives",
        "columns": ["time", "start_time", "end_time", "duration_seconds"],
        "values": [[1700000000000, "2023-11-14T22:13:20Z", "2023-11-14T23:13:20Z", 3600]]
    }]}]}"#;

    // Window without ISO fields (falls back to [point t, t + duration]).
    const DD_WINDOW_FALLBACK_ROWS: &str = r#"{"results": [{"series": [{
        "name": "drives",
        "columns": ["time", "start_time", "end_time", "duration_seconds"],
        "values": [[1700000000000, null, null, 600]]
    }]}]}"#;

    // Drive series: speed 50 mph, power 7.0 (kW native), battery 80%,
    // ideal 300 km — plus a null row (gaps pass through).
    const DD_SERIES_ROWS: &str = r#"{"results": [{"series": [{
        "name": "positions",
        "columns": ["time", "mean", "mean_1", "mean_2", "mean_3"],
        "values": [[1700000000000, 50, 7.0, 80, 300], [1700000010000, null, null, null, null]]
    }]}]}"#;

    // Route fans out one series per (vin, car_id) tag set — deliberately
    // out of order here so the combine + ASC sort is exercised.
    const DD_ROUTE_ROWS: &str = r#"{"results": [{"series": [
        {"name": "positions", "tags": {"vin": "VIN1", "car_id": "1"},
         "columns": ["time", "latitude", "longitude"],
         "values": [[1700000010000, 37.1, -122.1]]},
        {"name": "positions", "tags": {"vin": "VIN1", "car_id": "1"},
         "columns": ["time", "latitude", "longitude"],
         "values": [[1700000000000, 37.0, -122.0]]}
    ]}]}"#;

    // Elevation with a leading null (old drives lack SRTM data).
    const DD_ELEV_ROWS: &str = r#"{"results": [{"series": [{
        "name": "positions",
        "columns": ["time", "mean"],
        "values": [[1700000000000, null], [1700000010000, 150]]
    }]}]}"#;

    // Temps with a leading all-null row (streaming points carry no
    // climate fields).
    const DD_TEMPS_ROWS: &str = r#"{"results": [{"series": [{
        "name": "positions",
        "columns": ["time", "mean", "mean_1"],
        "values": [[1700000000000, null, null], [1700000010000, 15.0, 20.0]]
    }]}]}"#;

    // Folded stats: duration 3600 s, distance 20000 m, energy 5000 Wh,
    // average speed 50 mph.
    const DD_STATS_ROWS: &str = r#"{"results": [{"series": [{
        "name": "drives",
        "columns": ["time", "last", "last_1", "last_2", "last_3"],
        "values": [[1700000000000, 3600, 20000, 5000, 50]]
    }]}]}"#;

    // Odometer in stored miles: 10000 → 10020.
    const DD_ODO_ROWS: &str = r#"{"results": [{"series": [{
        "name": "positions",
        "columns": ["time", "first", "last"],
        "values": [[1700000000000, 10000, 10020]]
    }]}]}"#;

    async fn mock_drive_details_db_iso() -> wiremock::MockServer {
        mock_drive_details_db_with(DD_WINDOW_ISO_ROWS).await
    }

    async fn mock_drive_details_db_fallback() -> wiremock::MockServer {
        mock_drive_details_db_with(DD_WINDOW_FALLBACK_ROWS).await
    }

    // The window body varies per test (ISO vs fallback); every other
    // fixture is fixed. The responder closure only captures the 'static
    // window body, so it stays Clone as wiremock requires.
    async fn mock_drive_details_db_with(window_rows: &'static str) -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(move |req: &wiremock::Request| {
                // Route by query content (bare identifiers: reqwest
                // percent-encodes the query text, so `mean(` arrives as
                // `mean%28` — match on the bare names instead). The window
                // query also names duration_seconds, so its start_time arm
                // must come first.
                let q = req.url.query().unwrap_or("");
                let body = if q.contains("start_time") {
                    window_rows
                } else if q.contains("ideal_battery_range_km") {
                    DD_SERIES_ROWS
                } else if q.contains("latitude") {
                    DD_ROUTE_ROWS
                } else if q.contains("elevation") {
                    DD_ELEV_ROWS
                } else if q.contains("outside_temp") {
                    DD_TEMPS_ROWS
                } else if q.contains("odometer") {
                    DD_ODO_ROWS
                } else {
                    DD_STATS_ROWS
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        server
    }

    async fn get_drive_details_on(
        server: &wiremock::MockServer,
        query: &str,
    ) -> (StatusCode, serde_json::Value) {
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/drive-details{query}"))
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

    async fn get_drive_details(query: &str) -> (StatusCode, serde_json::Value) {
        let server = mock_drive_details_db_iso().await;
        get_drive_details_on(&server, query).await
    }

    #[tokio::test]
    async fn drive_details_prefers_iso_window() {
        let (status, json) = get_drive_details("?drive_id=d-1&unit=km").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["drive_id"], "d-1");
        // ISO fields re-emitted as RFC3339 (Z normalizes to +00:00).
        assert_eq!(json["window"]["from"], "2023-11-14T22:13:20+00:00");
        assert_eq!(json["window"]["to"], "2023-11-14T23:13:20+00:00");
    }

    #[tokio::test]
    async fn drive_details_falls_back_to_point_plus_duration() {
        let server = mock_drive_details_db_fallback().await;
        let (status, json) = get_drive_details_on(&server, "?drive_id=d-2&unit=km").await;
        assert_eq!(status, StatusCode::OK);
        // No ISO fields → [point t, t + 600 s].
        assert_eq!(json["window"]["from"], "2023-11-14T22:13:20+00:00");
        assert_eq!(json["window"]["to"], "2023-11-14T22:23:20+00:00");
    }

    #[tokio::test]
    async fn drive_details_series_conversions() {
        let (status, json) = get_drive_details("?drive_id=d-1&unit=km").await;
        assert_eq!(status, StatusCode::OK);
        let series = json["series"].as_array().unwrap();
        assert_eq!(series.len(), 2);
        assert_eq!(series[0]["t"], 1700000000000i64);
        // Speed is mph-stored: 50 mph → 50*1.60934 km/h.
        assert!((series[0]["speed"].as_f64().unwrap() - 80.467).abs() < 0.001);
        // Power passes through as kW: NO /1000 (drive power is natively
        // kW per #93 — the opposite of charge power). 7.0 stays 7.0.
        assert_eq!(series[0]["power_kw"], 7.0);
        assert_eq!(series[0]["battery"], 80.0);
        assert_eq!(series[0]["ideal"], 300.0);
        // Nulls pass through as gaps.
        assert!(series[1]["speed"].is_null());
        assert!(series[1]["power_kw"].is_null());
        assert!(series[1]["battery"].is_null());
        assert!(series[1]["ideal"].is_null());
    }

    #[tokio::test]
    async fn drive_details_route_combines_series_asc() {
        let (status, json) = get_drive_details("?drive_id=d-1&unit=km").await;
        assert_eq!(status, StatusCode::OK);
        // Two (vin, car_id) series arrive out of order; the endpoint
        // combines them and restores time ASC.
        let route = json["route"].as_array().unwrap();
        assert_eq!(route.len(), 2);
        assert_eq!(route[0]["lat"], 37.0);
        assert_eq!(route[0]["lng"], -122.0);
        assert_eq!(route[1]["lat"], 37.1);
        assert_eq!(route[1]["lng"], -122.1);
    }

    #[tokio::test]
    async fn drive_details_elevation_and_temps_nulls() {
        let (status, json) = get_drive_details("?drive_id=d-1&unit=km").await;
        assert_eq!(status, StatusCode::OK);
        // Old drives lack SRTM data: the null passes through.
        let elev = json["elevation"].as_array().unwrap();
        assert_eq!(elev.len(), 2);
        assert!(elev[0]["value"].is_null());
        assert_eq!(elev[1]["value"], 150.0);
        // Streaming points carry no climate: sparse nulls pass through.
        let temps = json["temps"].as_array().unwrap();
        assert_eq!(temps.len(), 2);
        assert!(temps[0]["outside"].is_null());
        assert!(temps[0]["inside"].is_null());
        assert_eq!(temps[1]["outside"], 15.0);
        assert_eq!(temps[1]["inside"], 20.0);
    }

    #[tokio::test]
    async fn drive_details_stats_math() {
        let (status, json) = get_drive_details("?drive_id=d-1&unit=km").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["stats"]["duration_s"], 3600.0);
        assert_eq!(json["stats"]["distance"], 20.0);
        assert_eq!(json["stats"]["energy_kwh"], 5.0);
        assert!((json["stats"]["avg_speed"].as_f64().unwrap() - 80.467).abs() < 0.001);
    }

    #[tokio::test]
    async fn drive_details_odometer() {
        let (status, json) = get_drive_details("?drive_id=d-1&unit=km").await;
        assert_eq!(status, StatusCode::OK);
        // Stored odometer is miles: *1.60934 in Rust.
        assert!((json["odometer"]["from"].as_f64().unwrap() - 16093.4).abs() < 0.01);
        assert!((json["odometer"]["to"].as_f64().unwrap() - 16125.5868).abs() < 0.01);
    }

    #[tokio::test]
    async fn drive_details_unit_mi_applies_factors() {
        let server = mock_drive_details_db_iso().await;
        let (status, json) = get_drive_details_on(&server, "?drive_id=d-1&unit=mi").await;
        assert_eq!(status, StatusCode::OK);
        // 20000 m → 20 * 0.621371 mi.
        let d = json["stats"]["distance"].as_f64().unwrap();
        assert!((d - 12.42742).abs() < 0.001, "expected ~12.43, got {d}");
        // Speeds: mph → 1.60934 * 0.621371 ≈ identity.
        let avg = json["stats"]["avg_speed"].as_f64().unwrap();
        assert!((avg - 50.0).abs() < 0.01, "expected ~50.0, got {avg}");
        let speed = json["series"][0]["speed"].as_f64().unwrap();
        assert!((speed - 50.0).abs() < 0.01, "expected ~50.0, got {speed}");
        // Odometer: miles → identity under mi.
        let from = json["odometer"]["from"].as_f64().unwrap();
        assert!((from - 10000.0).abs() < 0.1, "expected ~10000, got {from}");
        // Power, battery, duration, and energy carry no unit factor.
        assert_eq!(json["series"][0]["power_kw"], 7.0);
        assert_eq!(json["series"][0]["battery"], 80.0);
        assert_eq!(json["stats"]["duration_s"], 3600.0);
        assert_eq!(json["stats"]["energy_kwh"], 5.0);
    }

    #[tokio::test]
    async fn drive_details_queries_carry_expected_clauses() {
        let server = mock_drive_details_db_iso().await;
        let (status, _) = get_drive_details_on(&server, "?drive_id=d-1&unit=km").await;
        assert_eq!(status, StatusCode::OK);
        let queries = decoded_queries(&server).await;
        // Window + stats carry the drive_id and NO time filter (one
        // drive, one lookup — the row's from/to are never sent).
        let window_q = queries
            .iter()
            .find(|q| q.contains("start_time"))
            .cloned()
            .unwrap_or_default();
        assert!(
            window_q.contains("\"drive_id\"='d-1'"),
            "window query should filter drive_id, got: {window_q}"
        );
        assert!(
            !window_q.contains("time >="),
            "window query should have no time filter, got: {window_q}"
        );
        let stats_q = queries
            .iter()
            .find(|q| q.contains("average_speed"))
            .cloned()
            .unwrap_or_default();
        assert!(
            stats_q.contains("\"drive_id\"='d-1'"),
            "stats query should filter drive_id, got: {stats_q}"
        );
        assert!(
            !stats_q.contains("time >="),
            "stats query should have no time filter, got: {stats_q}"
        );
        // Positions queries span the resolved window with NO vin filter
        // (the window implies the drive).
        let series_q = queries
            .iter()
            .find(|q| q.contains("ideal_battery_range_km"))
            .cloned()
            .unwrap_or_default();
        assert!(
            series_q.contains("GROUP BY time(10s)"),
            "series query should use fixed 10s buckets, got: {series_q}"
        );
        assert!(
            series_q.contains("2023-11-14T22:13:20+00:00")
                && series_q.contains("2023-11-14T23:13:20+00:00"),
            "series query should span the resolved window, got: {series_q}"
        );
        assert!(
            !series_q.contains("vin"),
            "series query should have no vin filter, got: {series_q}"
        );
        // Route carries the lat filter, ASC order, and the response cap.
        let route_q = queries
            .iter()
            .find(|q| q.contains("longitude"))
            .cloned()
            .unwrap_or_default();
        assert!(
            route_q.contains("\"latitude\" > -90"),
            "route query should filter unset latitudes, got: {route_q}"
        );
        assert!(
            route_q.contains("ORDER BY time ASC"),
            "route query should order ASC, got: {route_q}"
        );
        assert!(
            route_q.contains("LIMIT") && route_q.contains("10000"),
            "route query should carry LIMIT 10000, got: {route_q}"
        );
        assert!(
            !route_q.contains("vin"),
            "route query should have no vin filter, got: {route_q}"
        );
        // Odometer follows the JSON (no vin filter).
        let odo_q = queries
            .iter()
            .find(|q| q.contains("odometer"))
            .cloned()
            .unwrap_or_default();
        assert!(
            odo_q.contains("FIRST") && odo_q.contains("LAST"),
            "odometer query should select FIRST/LAST, got: {odo_q}"
        );
        assert!(
            !odo_q.contains("vin"),
            "odometer query should have no vin filter, got: {odo_q}"
        );
    }

    #[tokio::test]
    async fn drive_details_unknown_id_is_404() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(r#"{"results": [{}]}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let (status, json) = get_drive_details_on(&server, "?drive_id=nope&unit=km").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(json["error"].as_str().unwrap().contains("drive not found"));
    }

    #[tokio::test]
    async fn drive_details_rejects_missing_id() {
        let (status, _) = get_drive_details("?unit=km").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn drive_details_rejects_empty_id() {
        let (status, _) = get_drive_details("?drive_id=&unit=km").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn drive_details_rejects_bad_unit() {
        let (status, json) = get_drive_details("?drive_id=d-1&unit=parsecs").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid unit"));
    }

    #[tokio::test]
    async fn drive_details_upstream_failure_is_502() {
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
                    .uri("/drive-details?drive_id=d-1&unit=km")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    /// Decoded InfluxQL query texts (`q` params) sent to the mock, in
    /// request order. Percent-decoding matters: raw `req.url.query()`
    /// leaves `+00:00` as `%2B00%3A00`, while `query_pairs` decodes it.
    async fn decoded_queries(server: &wiremock::MockServer) -> Vec<String> {
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter_map(|r| {
                r.url
                    .query_pairs()
                    .find_map(|(k, v)| (k == "q").then(|| v.into_owned()))
            })
            .collect()
    }

    // Folded drive sums fan out one row per drive_id series (2 rows here).
    const TRIP_DRIVE_SUMS_ROWS: &str = r#"{"results": [{"series": [
        {"name": "drives", "tags": {"drive_id": "d-1"},
         "columns": ["time", "sum", "sum_1", "sum_2"],
         "values": [[1700000000000, 20000, 3600, 5000]]},
        {"name": "drives", "tags": {"drive_id": "d-2"},
         "columns": ["time", "sum", "sum_1", "sum_2"],
         "values": [[1700000300000, 10000, 1800, 2500]]}
    ]}]}"#;

    const TRIP_CONSUMPTION_ROWS: &str = r#"{"results": [{"series": [
        {"name": "drives", "tags": {"drive_id": "d-1"},
         "columns": ["time", "sum", "sum_1"],
         "values": [[1700000000000, 5000, 20000]]},
        {"name": "drives", "tags": {"drive_id": "d-2"},
         "columns": ["time", "sum", "sum_1"],
         "values": [[1700000300000, 2500, 10000]]}
    ]}]}"#;

    const TRIP_CHARGING_SUMS_ROWS: &str = r#"{"results": [{"series": [{
        "name": "charging_sessions",
        "columns": ["time", "sum", "sum_1"],
        "values": [[1700000000000, 7200, 4.5]]
    }]}]}"#;

    const TRIP_DRIVES_TABLE_ROWS: &str = r#"{"results": [{"series": [
        {"name": "drives", "tags": {"drive_id": "d-2"},
         "columns": ["time", "distance_meters", "duration_seconds", "energy_used_wh", "average_speed", "max_speed", "geofence_enter", "geofence_exit", "start_time", "end_time"],
         "values": [[1700000300000, 10000, 1800, 2500, 40, 60, null, null, null, null]]},
        {"name": "drives", "tags": {"drive_id": "d-1"},
         "columns": ["time", "distance_meters", "duration_seconds", "energy_used_wh", "average_speed", "max_speed", "geofence_enter", "geofence_exit", "start_time", "end_time"],
         "values": [[1700000000000, 20000, 3600, 5000, 50, 70, "Home", "Work", "2023-11-14T22:13:20Z", "2023-11-14T23:13:20Z"]]}
    ]}]}"#;

    const TRIP_CHARGES_TABLE_ROWS: &str = r#"{"results": [{"series": [{
        "name": "charging_sessions", "tags": {"charge_id": "ch-1"},
        "columns": ["time", "geofence_name", "energy_added_wh", "cost"],
        "values": [[1700000600000, "Home", 12000, 4.5]]
    }]}]}"#;

    const TRIP_BATTERY_ROWS: &str = r#"{"results": [{"series": [{
        "name": "positions",
        "columns": ["time", "mean", "mean_1"],
        "values": [[1700000000000, 82, 270.5], [1700000300000, null, 268.0]]
    }]}]}"#;

    const TRIP_ROUTE_ROWS: &str = r#"{"results": [{"series": [
        {"name": "positions", "tags": {"vin": "VIN1", "car_id": "1"},
         "columns": ["time", "latitude", "longitude"],
         "values": [[1700000300000, 37.2, -122.1]]},
        {"name": "positions", "tags": {"vin": "VIN1", "car_id": "1"},
         "columns": ["time", "latitude", "longitude"],
         "values": [[1700000000000, 37.1, -122.0]]}
    ]}]}"#;

    async fn mock_trip_db() -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(|req: &wiremock::Request| {
                // Route by query content (bare identifiers: reqwest
                // percent-encodes the query text, so `sum(` arrives as
                // `sum%28` — match on the bare names instead). The drives
                // table query is the only one selecting geofence_enter; the
                // charges table is the only one grouping by charge_id; the
                // consumption query is the only drives-sums query naming a
                // `> 100` filter (matched on the bare `100` — no other
                // remaining query contains it).
                let q = req.url.query().unwrap_or("");
                let body = if q.contains("ideal_battery_range_km") {
                    TRIP_BATTERY_ROWS
                } else if q.contains("latitude") {
                    TRIP_ROUTE_ROWS
                } else if q.contains("geofence_enter") {
                    TRIP_DRIVES_TABLE_ROWS
                } else if q.contains("charge_id") {
                    TRIP_CHARGES_TABLE_ROWS
                } else if q.contains("cost") {
                    TRIP_CHARGING_SUMS_ROWS
                } else if q.contains("100") {
                    TRIP_CONSUMPTION_ROWS
                } else {
                    TRIP_DRIVE_SUMS_ROWS
                };
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        server
    }

    async fn get_trip_on(
        server: &wiremock::MockServer,
        query: &str,
    ) -> (StatusCode, serde_json::Value) {
        let app = router().with_state(state_with_mock(&server.uri()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/trip{query}"))
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

    async fn get_trip(query: &str) -> (StatusCode, serde_json::Value) {
        let server = mock_trip_db().await;
        get_trip_on(&server, query).await
    }

    #[tokio::test]
    async fn trip_returns_composite() {
        let (status, json) =
            get_trip("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=km").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["vin"], "VIN1");
        // 30000 m → 30 km; 5400 s driving; 7200 s charging; 4.5 cost.
        assert_eq!(json["distance"], 30.0);
        assert_eq!(json["driving_s"], 5400);
        assert_eq!(json["charging_s"], 7200);
        assert_eq!(json["charging_cost"], 4.5);
        // Avg speed: 30 km / 1.5 h = 20 km/h.
        assert_eq!(json["avg_speed"], 20.0);
        // Consumption: 7500 Wh / 30000 m * 1000 = 250 Wh/km.
        assert_eq!(json["consumption_wh_per_km"], 250.0);
        // Tables combine across series and sort time ASC.
        assert_eq!(json["drives"].as_array().unwrap().len(), 2);
        assert_eq!(json["drives"][0]["drive_id"], "d-1");
        assert_eq!(json["drives"][0]["from_gf"], "Home");
        assert_eq!(json["drives"][1]["drive_id"], "d-2");
        assert_eq!(json["charges"].as_array().unwrap().len(), 1);
        assert_eq!(json["charges"][0]["charge_id"], "ch-1");
        assert_eq!(json["charges"][0]["energy_kwh"], 12.0);
        // Battery: level passes through (incl. null), ideal * unit.
        assert_eq!(json["battery"][0]["level"], 82.0);
        assert_eq!(json["battery"][0]["ideal"], 270.5);
        assert!(json["battery"][1]["level"].is_null());
        // Route re-sorts ASC across series.
        assert_eq!(json["route"].as_array().unwrap().len(), 2);
        assert_eq!(json["route"][0]["lat"], 37.1);
        assert_eq!(json["route"][1]["lat"], 37.2);
    }

    #[tokio::test]
    async fn trip_unit_mi_applies_factor() {
        let (status, json) =
            get_trip("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=mi").await;
        assert_eq!(status, StatusCode::OK);
        // 30000 m → 30 * 0.621371 mi; avg speed scales the same way.
        let distance = json["distance"].as_f64().unwrap();
        assert!((distance - 30.0 * 0.621371).abs() < 1e-9);
        let avg = json["avg_speed"].as_f64().unwrap();
        assert!((avg - 20.0 * 0.621371).abs() < 1e-9);
        // Consumption stays Wh/km (no unit factor).
        assert_eq!(json["consumption_wh_per_km"], 250.0);
        // Ideal range scales with the unit factor.
        let ideal = json["battery"][0]["ideal"].as_f64().unwrap();
        assert!((ideal - 270.5 * 0.621371).abs() < 1e-9);
    }

    #[tokio::test]
    async fn trip_empty_window_is_empty_not_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/query"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(r#"{"results": [{}]}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let (status, json) = get_trip_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["distance"], 0.0);
        assert_eq!(json["driving_s"], 0);
        assert_eq!(json["charging_s"], 0);
        assert!(json["avg_speed"].is_null());
        assert!(json["consumption_wh_per_km"].is_null());
        assert_eq!(json["charging_cost"], 0.0);
        assert_eq!(json["drives"].as_array().unwrap().len(), 0);
        assert_eq!(json["charges"].as_array().unwrap().len(), 0);
        assert_eq!(json["battery"].as_array().unwrap().len(), 0);
        assert_eq!(json["route"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn trip_battery_uses_fixed_5m_buckets() {
        let server = mock_trip_db().await;
        let (status, _) = get_trip_on(
            &server,
            "?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let queries = decoded_queries(&server).await;
        let battery = queries
            .iter()
            .find(|q| q.contains("ideal_battery_range_km"))
            .expect("battery query issued");
        assert!(
            battery.contains("GROUP BY time(5m)"),
            "battery buckets must be fixed 5m, got: {battery}"
        );
    }

    #[tokio::test]
    async fn trip_rejects_inverted_range() {
        let (status, json) =
            get_trip("?vin=VIN1&from=2023-11-15T00:00:00Z&to=2023-11-14T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("from is after to"));
    }

    #[tokio::test]
    async fn trip_rejects_oversized_range() {
        let (status, json) =
            get_trip("?vin=VIN1&from=2020-01-01T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("90-day maximum"));
    }

    #[tokio::test]
    async fn trip_rejects_bad_time() {
        let (status, json) = get_trip("?vin=VIN1&from=not-a-time&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid time"));
    }

    #[tokio::test]
    async fn trip_rejects_bad_unit() {
        let (status, json) =
            get_trip("?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=parsecs")
                .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid unit"));
    }

    #[tokio::test]
    async fn trip_rejects_empty_vin() {
        let (status, _) = get_trip("?vin=&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn trip_upstream_failure_is_502() {
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
                    .uri("/trip?vin=VIN1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }
}
