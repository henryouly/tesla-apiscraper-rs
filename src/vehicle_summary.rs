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
///
/// Besides the card fields, this carries the full Home Assistant attribute
/// set so the MQTT publisher needs no second data path. All telemetry is
/// `None` until the first successful poll (or DB seed for core fields).
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
    // --- Extended telemetry (MQTT + future display) ---
    pub ideal_battery_range: Option<f64>,
    pub est_battery_range: Option<f64>,
    pub usable_battery_level: Option<i64>,
    pub charging_state: Option<String>,
    pub charge_energy_added: Option<f64>,
    pub charge_limit_soc: Option<i64>,
    pub charger_actual_current: Option<i64>,
    pub charger_voltage: Option<i64>,
    pub charger_power: Option<i64>,
    pub charger_phases: Option<i64>,
    pub conn_charge_cable: Option<String>,
    pub time_to_full_charge: Option<f64>,
    pub scheduled_charging_start_time: Option<String>,
    pub charge_port_door_open: Option<bool>,
    pub inside_temp: Option<f64>,
    pub outside_temp: Option<f64>,
    pub is_climate_on: Option<bool>,
    pub is_preconditioning: Option<bool>,
    pub sentry_mode: Option<bool>,
    pub is_user_present: Option<bool>,
    pub locked: Option<bool>,
    pub shift_state: Option<String>,
    pub power: Option<i64>,
    pub heading: Option<i64>,
    pub elevation: Option<f64>,
    pub df: Option<f64>,
    pub pf: Option<f64>,
    pub dr: Option<f64>,
    pub pr: Option<f64>,
    pub ft: Option<f64>,
    pub rt: Option<f64>,
    pub fd_window: Option<f64>,
    pub fp_window: Option<f64>,
    pub rd_window: Option<f64>,
    pub rp_window: Option<f64>,
    pub car_version: Option<String>,
    pub software_update_status: Option<String>,
    pub software_update_version: Option<String>,
    pub car_type: Option<String>,
    pub trim_badging: Option<String>,
    pub exterior_color: Option<String>,
    pub wheel_type: Option<String>,
    pub spoiler_type: Option<String>,
    pub geofence_name: Option<String>,
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
            ideal_battery_range: None,
            est_battery_range: None,
            usable_battery_level: None,
            charging_state: None,
            charge_energy_added: None,
            charge_limit_soc: None,
            charger_actual_current: None,
            charger_voltage: None,
            charger_power: None,
            charger_phases: None,
            conn_charge_cable: None,
            time_to_full_charge: None,
            scheduled_charging_start_time: None,
            charge_port_door_open: None,
            inside_temp: None,
            outside_temp: None,
            is_climate_on: None,
            is_preconditioning: None,
            sentry_mode: None,
            is_user_present: None,
            locked: None,
            shift_state: None,
            power: None,
            heading: None,
            elevation: None,
            df: None,
            pf: None,
            dr: None,
            pr: None,
            ft: None,
            rt: None,
            fd_window: None,
            fp_window: None,
            rd_window: None,
            rp_window: None,
            car_version: None,
            software_update_status: None,
            software_update_version: None,
            car_type: None,
            trim_badging: None,
            exterior_color: None,
            wheel_type: None,
            spoiler_type: None,
            geofence_name: None,
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
        let cs = data.charge_state.as_ref();
        let ds = data.drive_state.as_ref();
        let cl = data.climate_state.as_ref();
        let vs = data.vehicle_state.as_ref();
        let vc = data.vehicle_config.as_ref();
        let su = vs.and_then(|v| v.software_update.as_ref());
        Self {
            vin: vehicle.vin.clone(),
            display_name: vehicle.display_name.clone(),
            state,
            battery_level: cs.and_then(|c| c.battery_level),
            battery_range: cs.and_then(|c| c.battery_range),
            latitude: ds.and_then(|d| d.latitude),
            longitude: ds.and_then(|d| d.longitude),
            speed: ds.and_then(|d| d.speed),
            odometer: data.odometer,
            last_updated_at: now_unix,
            ideal_battery_range: cs.and_then(|c| c.ideal_battery_range),
            est_battery_range: cs.and_then(|c| c.est_battery_range),
            usable_battery_level: cs.and_then(|c| c.usable_battery_level),
            charging_state: cs.and_then(|c| c.charging_state.clone()),
            charge_energy_added: cs.and_then(|c| c.charge_energy_added),
            charge_limit_soc: cs.and_then(|c| c.charge_limit_soc),
            charger_actual_current: cs.and_then(|c| c.charger_actual_current),
            charger_voltage: cs.and_then(|c| c.charger_voltage),
            charger_power: cs.and_then(|c| c.charger_power),
            charger_phases: cs.and_then(|c| c.charger_phases),
            conn_charge_cable: cs.and_then(|c| c.conn_charge_cable.clone()),
            time_to_full_charge: cs.and_then(|c| c.time_to_full_charge),
            scheduled_charging_start_time: cs.and_then(|c| {
                c.scheduled_charging_start_time
                    .as_ref()
                    .and_then(|v| v.as_str().map(str::to_string))
            }),
            charge_port_door_open: cs.and_then(|c| c.charge_port_door_open),
            inside_temp: cl.and_then(|c| c.inside_temp),
            outside_temp: cl.and_then(|c| c.outside_temp),
            is_climate_on: cl.and_then(|c| c.is_climate_on),
            is_preconditioning: cl.and_then(|c| c.is_preconditioning),
            sentry_mode: vs.and_then(|v| v.sentry_mode),
            is_user_present: vs.and_then(|v| v.is_user_present),
            locked: vs.and_then(|v| v.locked),
            shift_state: ds.and_then(|d| d.shift_state.clone()),
            power: ds.and_then(|d| d.power),
            heading: ds.and_then(|d| d.heading),
            elevation: ds.and_then(|d| d.elevation),
            df: vs.and_then(|v| v.df),
            pf: vs.and_then(|v| v.pf),
            dr: vs.and_then(|v| v.dr),
            pr: vs.and_then(|v| v.pr),
            ft: vs.and_then(|v| v.ft),
            rt: vs.and_then(|v| v.rt),
            fd_window: vs.and_then(|v| v.fd_window),
            fp_window: vs.and_then(|v| v.fp_window),
            rd_window: vs.and_then(|v| v.rd_window),
            rp_window: vs.and_then(|v| v.rp_window),
            car_version: vs.and_then(|v| v.car_version.clone()),
            software_update_status: su.and_then(|s| s.status.clone()),
            software_update_version: su.and_then(|s| s.version.clone()),
            car_type: vc.and_then(|v| v.car_type.clone()),
            trim_badging: vc.and_then(|v| v.trim_badging.clone()),
            exterior_color: vc.and_then(|v| v.exterior_color.clone()),
            wheel_type: vc.and_then(|v| v.wheel_type.clone()),
            spoiler_type: vc.and_then(|v| v.spoiler_type.clone()),
            // Set by the task at publish time from the current geofences.
            geofence_name: None,
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
/// are km while live ones follow vehicle units). Queried at millisecond
/// precision so same-second rows from different series order correctly;
/// the summary keeps the seconds contract. `None` when no row exists or
/// anything fails; callers fall back to [`VehicleSummary::initial`].
pub async fn last_known_summary(
    db: &crate::influxdb::InfluxDb,
    vehicle: &Vehicle,
    state: VehicleState,
) -> Option<VehicleSummary> {
    let q = format!(
        "SELECT battery_level, latitude, longitude, speed, odometer FROM positions WHERE vin='{}' ORDER BY time DESC LIMIT 1",
        escape_vin(&vehicle.vin)
    );
    let json = db.query(&q, "ms").await.ok()?;
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
/// series is not necessarily the newest. Row times are epoch milliseconds
/// (queried as such so same-second rows order correctly) and normalized
/// to the summary's seconds contract. Rows without an integer `time` are
/// skipped; `None` only when no valid row exists anywhere (never a summary
/// with a fabricated timestamp).
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
                let Some(time_ms) = columns
                    .iter()
                    .position(|c| *c == "time")
                    .and_then(|i| row.get(i))
                    .and_then(|t| t.as_i64())
                else {
                    continue;
                };
                let newer = best.as_ref().is_none_or(|(t, _, _)| time_ms > *t);
                if newer {
                    best = Some((time_ms, columns.clone(), row.clone()));
                }
            }
        }
    }
    let (time_ms, columns, row) = best?;
    let get = |name: &str| -> Option<&serde_json::Value> {
        columns
            .iter()
            .position(|c| *c == name)
            .and_then(|i| row.get(i))
    };
    // DB seeds carry core display fields only; extended telemetry arrives
    // with the first live poll.
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
        last_updated_at: time_ms / 1000,
        ideal_battery_range: None,
        est_battery_range: None,
        usable_battery_level: None,
        charging_state: None,
        charge_energy_added: None,
        charge_limit_soc: None,
        charger_actual_current: None,
        charger_voltage: None,
        charger_power: None,
        charger_phases: None,
        conn_charge_cable: None,
        time_to_full_charge: None,
        scheduled_charging_start_time: None,
        charge_port_door_open: None,
        inside_temp: None,
        outside_temp: None,
        is_climate_on: None,
        is_preconditioning: None,
        sentry_mode: None,
        is_user_present: None,
        locked: None,
        shift_state: None,
        power: None,
        heading: None,
        elevation: None,
        df: None,
        pf: None,
        dr: None,
        pr: None,
        ft: None,
        rt: None,
        fd_window: None,
        fp_window: None,
        rd_window: None,
        rp_window: None,
        car_version: None,
        software_update_status: None,
        software_update_version: None,
        car_type: None,
        trim_badging: None,
        exterior_color: None,
        wheel_type: None,
        spoiler_type: None,
        geofence_name: None,
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
                    "values":  [[1700000000000i64, 82, 37.7, -122.4, null, 50000.5]]
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
                "values":  [[1700000000000i64, 60, 1.0, 2.0, 0.0, 10000.0]],
            },
            {
                "name": "positions",
                "tags": {"vin": "VIN001", "car_id": "200"},
                "columns": ["time", "battery_level", "latitude", "longitude", "speed", "odometer"],
                "values":  [[1700001000000i64, 82, 3.0, 4.0, 5.0, 20000.0]],
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
            "values": [[null, 99], [1700000000000i64, 70]],
        }]}]});
        let s = parse_latest_row(&json, &test_vehicle(), VehicleState::Start).unwrap();
        assert_eq!(s.battery_level, Some(70));
        assert_eq!(s.last_updated_at, 1700000000);
    }

    #[test]
    fn parse_latest_row_orders_same_second_by_milliseconds() {
        // Both rows normalize to the same second: raw ms decides.
        let json = serde_json::json!({"results": [{"series": [
            {
                "columns": ["time", "battery_level"],
                "values":  [[1700000000900i64, 82]],
            },
            {
                "columns": ["time", "battery_level"],
                "values":  [[1700000000500i64, 60]],
            },
        ]}]});
        let s = parse_latest_row(&json, &test_vehicle(), VehicleState::Start).unwrap();
        assert_eq!(s.battery_level, Some(82));
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
