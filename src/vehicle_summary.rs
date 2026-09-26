//! Phase 6 contracts: in-memory vehicle summary + UI event bus.
//!
//! The vehicle task caches the latest [`VehicleSummary`] per VIN after each
//! successful poll (and streaming update). HTTP serves it directly — no
//! InfluxDB reads on the hot path — and broadcasts [`UiEvent`]s over a
//! bounded Tokio broadcast channel to SSE subscribers.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::broadcast;

use crate::tesla_api::{Vehicle, VehicleDataResponse};
use crate::vehicles::VehicleState;

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

/// Latest known display state for one vehicle (memory-only, lost on restart).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VehicleSummary {
    pub vin: String,
    pub display_name: Option<String>,
    pub state: VehicleState,
    pub battery_level: Option<i64>,
    pub battery_range: Option<f64>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub speed: Option<f64>,
    pub odometer: Option<f64>,
    /// Unix seconds of the last update that produced this summary.
    pub last_updated_at: i64,
}

impl VehicleSummary {
    /// Seed shown before any data exists. `last_updated_at` is 0 ("never"):
    /// strictly older than any real timestamp, so a later last-known or
    /// live seed always wins timestamp merges.
    pub fn initial(vehicle: &Vehicle, state: VehicleState) -> Self {
        Self {
            vin: vehicle.vin.clone(),
            display_name: vehicle.display_name.clone(),
            state,
            battery_level: None,
            battery_range: None,
            latitude: None,
            longitude: None,
            speed: None,
            odometer: None,
            last_updated_at: 0,
        }
    }

    /// Whether any telemetry beyond identity/state is present.
    pub fn has_telemetry(&self) -> bool {
        self.battery_level.is_some()
            || self.battery_range.is_some()
            || self.latitude.is_some()
            || self.longitude.is_some()
            || self.speed.is_some()
            || self.odometer.is_some()
    }

    pub fn from_data(
        vehicle: &Vehicle,
        state: VehicleState,
        data: &VehicleDataResponse,
        now_unix: i64,
    ) -> Self {
        Self {
            vin: vehicle.vin.clone(),
            display_name: vehicle.display_name.clone(),
            state,
            battery_level: data.charge_state.as_ref().and_then(|c| c.battery_level),
            battery_range: data.charge_state.as_ref().and_then(|c| c.battery_range),
            latitude: data.drive_state.as_ref().and_then(|d| d.latitude),
            longitude: data.drive_state.as_ref().and_then(|d| d.longitude),
            speed: data.drive_state.as_ref().and_then(|d| d.speed),
            odometer: data.odometer,
            last_updated_at: now_unix,
        }
    }
}

/// Map Tesla discovery state to our state machine vocabulary for seeds.
/// Unknown values fall back to `Start`; the first poll corrects it.
pub fn discovery_state(api_state: &str) -> VehicleState {
    if api_state.eq_ignore_ascii_case("online") {
        VehicleState::Online
    } else if api_state.eq_ignore_ascii_case("asleep") {
        VehicleState::Asleep
    } else if api_state.eq_ignore_ascii_case("offline") {
        VehicleState::Offline
    } else {
        VehicleState::Start
    }
}

/// Escape a VIN for embedding in an InfluxQL string literal.
fn escape_vin(vin: &str) -> String {
    vin.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Last-known telemetry for one VIN: the latest `positions` row, mapped to
/// core display fields only (`battery_range` stays empty — stored ranges
/// are km while live ones follow vehicle units). `None` when no row exists
/// or anything fails; callers fall back to [`VehicleSummary::initial`].
pub async fn last_known_summary(
    db: &crate::influxdb::InfluxDb,
    vehicle: &Vehicle,
    state: VehicleState,
) -> Option<VehicleSummary> {
    let q = format!(
        "SELECT battery_level, latitude, longitude, speed, odometer FROM positions WHERE vin='{}' ORDER BY time DESC LIMIT 1",
        escape_vin(&vehicle.vin)
    );
    let json = db.query(&q).await.ok()?;
    parse_latest_row(&json, vehicle, state)
}

fn num_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))
}

fn num_i64(v: &serde_json::Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// Parse an InfluxDB v1 query envelope, selecting the newest valid row
/// across ALL series and rows by timestamp.
///
/// `positions` rows carry two tags (`vin`, `car_id`), so one VIN can span
/// multiple series and InfluxQL applies `LIMIT` per series — the first
/// series is not necessarily the newest. Rows without an integer `time`
/// are skipped; `None` only when no valid row exists anywhere (never a
/// summary with a fabricated timestamp).
fn parse_latest_row(
    json: &serde_json::Value,
    vehicle: &Vehicle,
    state: VehicleState,
) -> Option<VehicleSummary> {
    let mut best: Option<(i64, Vec<&str>, Vec<serde_json::Value>)> = None;
    let results = json.get("results")?.as_array()?;
    for result in results {
        let Some(series) = result.get("series").and_then(|s| s.as_array()) else {
            continue;
        };
        for s in series {
            let Some(col_array) = s.get("columns").and_then(|c| c.as_array()) else {
                continue;
            };
            let columns: Vec<&str> = col_array.iter().filter_map(|c| c.as_str()).collect();
            let Some(rows) = s.get("values").and_then(|v| v.as_array()) else {
                continue;
            };
            for row in rows {
                let Some(row) = row.as_array() else {
                    continue;
                };
                let Some(time) = columns
                    .iter()
                    .position(|c| *c == "time")
                    .and_then(|i| row.get(i))
                    .and_then(|t| t.as_i64())
                else {
                    continue;
                };
                let newer = best.as_ref().is_none_or(|(t, _, _)| time > *t);
                if newer {
                    best = Some((time, columns.clone(), row.clone()));
                }
            }
        }
    }
    let (time, columns, row) = best?;
    let get = |name: &str| -> Option<&serde_json::Value> {
        columns
            .iter()
            .position(|c| *c == name)
            .and_then(|i| row.get(i))
    };
    Some(VehicleSummary {
        vin: vehicle.vin.clone(),
        display_name: vehicle.display_name.clone(),
        state,
        battery_level: get("battery_level").and_then(num_i64),
        battery_range: None,
        latitude: get("latitude").and_then(num_f64),
        longitude: get("longitude").and_then(num_f64),
        speed: get("speed").and_then(num_f64),
        odometer: get("odometer").and_then(num_f64),
        last_updated_at: time,
    })
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Event delivered to SSE subscribers.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UiEvent {
    /// `"summary"` (full snapshot) or `"state"` (VIN + state only).
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub vin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<VehicleSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<VehicleState>,
    /// Unix seconds when the event was produced.
    pub at: i64,
}

impl UiEvent {
    pub fn summary(summary: VehicleSummary) -> Self {
        Self {
            kind: "summary",
            vin: Some(summary.vin.clone()),
            summary: Some(summary),
            state: None,
            at: now_unix(),
        }
    }

    pub fn state(vin: &str, state: VehicleState) -> Self {
        Self {
            kind: "state",
            vin: Some(vin.to_string()),
            summary: None,
            state: Some(state),
            at: now_unix(),
        }
    }
}

// ---------------------------------------------------------------------------
// Shared stores
// ---------------------------------------------------------------------------

/// Latest summary per VIN, updated by vehicle tasks, read by HTTP handlers.
pub type SummaryStore = Arc<RwLock<HashMap<String, VehicleSummary>>>;

pub fn new_summary_store() -> SummaryStore {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Bounded broadcast bus for [`UiEvent`]s (slow SSE consumers lag, never block).
pub type EventBus = broadcast::Sender<UiEvent>;

/// Channel capacity: a few polls + streaming bursts per vehicle fit easily;
/// laggards get `Lagged` and are told to refetch summaries.
pub const EVENT_BUS_CAPACITY: usize = 64;

pub fn new_event_bus() -> EventBus {
    broadcast::channel(EVENT_BUS_CAPACITY).0
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_vehicle() -> Vehicle {
        Vehicle {
            id: 1,
            vehicle_id: 100,
            vin: "VIN001".into(),
            display_name: Some("Car One".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        }
    }

    fn test_data() -> VehicleDataResponse {
        serde_json::from_value(serde_json::json!({
            "state": "online",
            "odometer": 50000.5,
            "drive_state": {
                "latitude": 37.7, "longitude": -122.4, "speed": 65.0
            },
            "charge_state": { "battery_level": 85, "battery_range": 270.0 }
        }))
        .unwrap()
    }

    #[test]
    fn from_data_maps_fields() {
        let s = VehicleSummary::from_data(
            &test_vehicle(),
            VehicleState::Driving,
            &test_data(),
            1700000000,
        );
        assert_eq!(s.vin, "VIN001");
        assert_eq!(s.display_name.as_deref(), Some("Car One"));
        assert_eq!(s.state, VehicleState::Driving);
        assert_eq!(s.battery_level, Some(85));
        assert_eq!(s.battery_range, Some(270.0));
        assert_eq!(s.latitude, Some(37.7));
        assert_eq!(s.longitude, Some(-122.4));
        assert_eq!(s.speed, Some(65.0));
        assert_eq!(s.odometer, Some(50000.5));
        assert_eq!(s.last_updated_at, 1700000000);
    }

    #[test]
    fn from_data_missing_sub_objects_yields_nones() {
        let data: VehicleDataResponse = serde_json::from_value(serde_json::json!({
            "state": "asleep"
        }))
        .unwrap();
        let s = VehicleSummary::from_data(&test_vehicle(), VehicleState::Asleep, &data, 1);
        assert!(s.battery_level.is_none());
        assert!(s.latitude.is_none());
        assert!(s.longitude.is_none());
        assert!(s.speed.is_none());
        assert!(s.odometer.is_none());
    }

    #[test]
    fn initial_has_no_telemetry() {
        let s = VehicleSummary::initial(&test_vehicle(), VehicleState::Start);
        assert_eq!(s.vin, "VIN001");
        assert_eq!(s.display_name.as_deref(), Some("Car One"));
        assert_eq!(s.state, VehicleState::Start);
        assert!(s.battery_level.is_none());
        assert!(s.latitude.is_none());
        assert!(!s.has_telemetry());
    }

    #[test]
    fn range_only_counts_as_telemetry() {
        let mut s = VehicleSummary::initial(&test_vehicle(), VehicleState::Start);
        s.battery_range = Some(250.0);
        assert!(s.has_telemetry());
    }

    #[test]
    fn ui_event_shapes() {
        let s = VehicleSummary::from_data(&test_vehicle(), VehicleState::Online, &test_data(), 7);
        let ev = UiEvent::summary(s.clone());
        assert_eq!(ev.kind, "summary");
        assert_eq!(ev.vin.as_deref(), Some("VIN001"));
        assert_eq!(ev.summary, Some(s));

        let st = UiEvent::state("VIN001", VehicleState::Suspended);
        assert_eq!(st.kind, "state");
        assert!(st.summary.is_none());
        assert_eq!(st.state, Some(VehicleState::Suspended));
    }

    #[test]
    fn event_bus_delivers() {
        let bus = new_event_bus();
        let mut rx = bus.subscribe();
        let ev = UiEvent::state("V", VehicleState::Online);
        bus.send(ev.clone()).unwrap();
        assert_eq!(rx.try_recv().unwrap(), ev);
    }

    fn row_fixture() -> serde_json::Value {
        serde_json::json!({
            "results": [{
                "series": [{
                    "name": "positions",
                    "columns": ["time", "battery_level", "latitude", "longitude", "speed", "odometer"],
                    "values": [[1700000000, 82, 37.7, -122.4, null, 50000.5]]
                }]
            }]
        })
    }

    #[test]
    fn parse_latest_row_maps_fields() {
        let s = parse_latest_row(&row_fixture(), &test_vehicle(), VehicleState::Asleep).unwrap();
        assert_eq!(s.vin, "VIN001");
        assert_eq!(s.state, VehicleState::Asleep);
        assert_eq!(s.battery_level, Some(82));
        assert!(s.battery_range.is_none());
        assert_eq!(s.latitude, Some(37.7));
        assert_eq!(s.longitude, Some(-122.4));
        assert!(s.speed.is_none());
        assert_eq!(s.odometer, Some(50000.5));
        assert_eq!(s.last_updated_at, 1700000000);
    }

    #[test]
    fn parse_latest_row_rejects_missing_parts() {
        assert!(
            parse_latest_row(&serde_json::json!({}), &test_vehicle(), VehicleState::Start)
                .is_none()
        );
        assert!(
            parse_latest_row(
                &serde_json::json!({"results": [{"series": []}]}),
                &test_vehicle(),
                VehicleState::Start,
            )
            .is_none()
        );
        // Row without a timestamp must not fabricate one.
        assert!(
            parse_latest_row(
                &serde_json::json!({"results": [{"series": [{
                    "columns": ["battery_level"],
                    "values": [[80]],
                }]}]}),
                &test_vehicle(),
                VehicleState::Start,
            )
            .is_none()
        );
    }

    #[test]
    fn parse_latest_row_selects_newest_across_series() {
        // One VIN, two series (e.g. car_id changed): LIMIT applies per
        // series, so both rows arrive — the older series is listed first.
        let json = serde_json::json!({"results": [{"series": [
            {
                "name": "positions",
                "tags": {"vin": "VIN001", "car_id": "100"},
                "columns": ["time", "battery_level", "latitude", "longitude", "speed", "odometer"],
                "values": [[1700000000, 60, 1.0, 2.0, 0.0, 10000.0]],
            },
            {
                "name": "positions",
                "tags": {"vin": "VIN001", "car_id": "200"},
                "columns": ["time", "battery_level", "latitude", "longitude", "speed", "odometer"],
                "values": [[1700001000, 82, 3.0, 4.0, 5.0, 20000.0]],
            },
        ]}]});
        let s = parse_latest_row(&json, &test_vehicle(), VehicleState::Asleep).unwrap();
        assert_eq!(s.battery_level, Some(82));
        assert_eq!(s.latitude, Some(3.0));
        assert_eq!(s.odometer, Some(20000.0));
        assert_eq!(s.last_updated_at, 1700001000);
    }

    #[test]
    fn parse_latest_row_skips_null_timestamps() {
        let json = serde_json::json!({"results": [{"series": [{
            "columns": ["time", "battery_level"],
            "values": [[null, 99], [1700000000, 70]],
        }]}]});
        let s = parse_latest_row(&json, &test_vehicle(), VehicleState::Start).unwrap();
        assert_eq!(s.battery_level, Some(70));
        assert_eq!(s.last_updated_at, 1700000000);
    }

    #[test]
    fn discovery_state_maps_known_values() {
        assert_eq!(discovery_state("asleep"), VehicleState::Asleep);
        assert_eq!(discovery_state("ASLEEP"), VehicleState::Asleep);
        assert_eq!(discovery_state("online"), VehicleState::Online);
        assert_eq!(discovery_state("offline"), VehicleState::Offline);
        assert_eq!(discovery_state("whatever"), VehicleState::Start);
    }

    #[test]
    fn escape_vin_quotes_string_literal() {
        assert_eq!(escape_vin("ABC' OR '1'='1"), "ABC\\' OR \\'1\\'=\\'1");
    }
}
