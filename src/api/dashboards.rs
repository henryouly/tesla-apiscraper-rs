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

/// One InfluxQL series: column names plus rows.
pub type SeriesTable = (Vec<String>, Vec<SeriesRow>);

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
            out.push((columns, rows));
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
        .flat_map(|(cols, rows)| rows.iter().filter_map(|(_, row)| cell(cols, row, "count")))
        .map(|v| v as i64)
        .sum();
    // Raw rows fan out one series per update_id — combine and keep the
    // Grafana `ORDER BY time DESC` contract across series.
    let mut updates: Vec<SoftwareUpdate> = rseries
        .iter()
        .flat_map(|(cols, rows)| {
            rows.iter().map(|(t, row)| SoftwareUpdate {
                t: *t,
                version_before: str_cell(cols, row, "version_before"),
                version_after: str_cell(cols, row, "version_after"),
                install_start: str_cell(cols, row, "install_start"),
                install_end: str_cell(cols, row, "install_end"),
                status: str_cell(cols, row, "status"),
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
    for (cols, rows) in &aseries {
        for (_, row) in rows {
            let e = cell(cols, row, "sum");
            let d = cell(cols, row, "sum_1");
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
        .flat_map(|(cols, rows)| rows.iter().filter_map(|(_, row)| cell(cols, row, "sum")))
        .sum();
    let logged_distance = logged_sum / 1000.0 * factor;

    // Raw per-drive rows fan out one series per drive_id — combine across
    // series and restore chronological order (single-series windows sort
    // to themselves).
    let mut per_drive: Vec<LevelPoint> = pseries
        .iter()
        .flat_map(|(cols, rows)| {
            rows.iter().map(|(t, row)| {
                let value = match (
                    cell(cols, row, "energy_used_wh"),
                    cell(cols, row, "distance_meters"),
                ) {
                    (Some(e), Some(d)) if d != 0.0 => Some(e / d * 1000.0),
                    _ => None,
                };
                LevelPoint { t: *t, value }
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
