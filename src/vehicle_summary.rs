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
    /// Seed shown before the first successful poll (no telemetry yet).
    /// Guarantees the UI lists every discovered vehicle immediately, even
    /// when the car is offline and polls keep failing.
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
            last_updated_at: now_unix(),
        }
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
}
