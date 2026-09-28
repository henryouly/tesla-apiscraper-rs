//! Home Assistant MQTT publisher (Phase 8.1).
//!
//! Topic contract (`<base>/<index>/<attribute>`, e.g.
//! `teslamate/cars/1/battery_level`) mirrors the original TeslaMate layout
//! so existing dashboards keep working. Conventions, all deliberate:
//! - Car index is 1-based over lexicographically sorted VINs (stable
//!   across restarts for a fixed fleet).
//! - Booleans publish straight as `"true"`/`"false"`. In particular
//!   `locked=true` publishes `"true"`: Home Assistant binary-sensor
//!   classes treat ON as the attention state (`lock` ON = unlocked), so
//!   straight values display correctly.
//! - Values publish raw in Owner API units (drive power in watts, speeds
//!   in mph, distances in miles, charger power in kW, temps in °C); unit
//!   conversion happens in Home Assistant.
//!   Power/temps/energy pass through (kW, °C, kWh per the API).
//! - Missing (`None`) values publish nothing — except the door/window
//!   aggregates, which read unknown as closed (Tesla returns vehicle
//!   state all-or-nothing; an asleep car reads as closed for dashboard
//!   continuity).
//! - `since` is RFC3339 of the last per-car value change (tracked here).

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS};
use tracing::{info, warn};

use crate::config::Config;
use crate::vehicle_summary::{UiEvent, VehicleSummary};
use crate::vehicles::VehicleState;

// ---------------------------------------------------------------------------
// Pure mapping (no I/O — unit-tested)
// ---------------------------------------------------------------------------

fn b(v: bool) -> String {
    v.to_string()
}

fn nonzero(v: Option<f64>) -> Option<bool> {
    v.map(|x| x != 0.0)
}

/// Model letter from Tesla `car_type` (`models` → `S`, …); unknown values
/// pass through raw rather than vanishing.
pub fn model_name(car_type: Option<&str>) -> Option<String> {
    let t = car_type?;
    Some(
        if t.eq_ignore_ascii_case("models") {
            "S"
        } else if t.eq_ignore_ascii_case("model3") {
            "3"
        } else if t.eq_ignore_ascii_case("modelx") {
            "X"
        } else if t.eq_ignore_ascii_case("modely") {
            "Y"
        } else if t.eq_ignore_ascii_case("cybertruck") {
            "Cybertruck"
        } else if t.eq_ignore_ascii_case("roadster") {
            "Roadster"
        } else {
            t
        }
        .to_string(),
    )
}

/// All topics for one snapshot: (attribute, payload), excluding `since`
/// (the publisher stamps it only when something actually changed).
pub fn topics_for(s: &VehicleSummary) -> Vec<(String, String)> {
    let mut out: Vec<(Option<String>, Option<String>)> = Vec::new();
    let mut push = |attr: &str, v: Option<String>| out.push((Some(attr.into()), v));

    // Health / identity.
    let healthy = !matches!(s.state, VehicleState::Error | VehicleState::Offline);
    push("healthy", Some(b(healthy)));
    let update_available = s
        .software_update_status
        .as_deref()
        .is_some_and(|st| st == "available");
    push("update_available", Some(b(update_available)));
    push("locked", s.locked.map(b));
    push("sentry_mode", s.sentry_mode.map(b));
    push("is_user_present", s.is_user_present.map(b));
    push("is_climate_on", s.is_climate_on.map(b));
    push("is_preconditioning", s.is_preconditioning.map(b));
    push("display_name", s.display_name.clone());
    push("state", state_string(&s.state));
    push("version", s.car_version.clone());
    push("update_version", s.software_update_version.clone());
    push("model", model_name(s.car_type.as_deref()));
    push("trim_badging", s.trim_badging.clone());
    push("exterior_color", s.exterior_color.clone());
    push("wheel_type", s.wheel_type.clone());
    push("spoiler_type", s.spoiler_type.clone());
    push(
        "geofence",
        Some(s.geofence_name.clone().unwrap_or_default()),
    );

    // Doors / windows: unknown counts as closed. Tesla returns
    // vehicle_state all-or-nothing, so partial unknowns don't occur in
    // practice, and an asleep car reads as closed rather than unknown
    // for dashboard continuity.
    let any_open = |vals: &[Option<f64>]| vals.iter().any(|v| v.is_some_and(|x| x != 0.0));
    push("doors_open", Some(b(any_open(&[s.df, s.pf, s.dr, s.pr]))));
    push(
        "windows_open",
        Some(b(any_open(&[
            s.fd_window,
            s.fp_window,
            s.rd_window,
            s.rp_window,
        ]))),
    );
    push("trunk_open", nonzero(s.rt).map(b));
    push("frunk_open", nonzero(s.ft).map(b));
    push("charge_port_door_open", s.charge_port_door_open.map(b));

    // Charging.
    let plugged_in = s
        .charging_state
        .as_deref()
        .map(|cs| !cs.is_empty() && cs != "Disconnected");
    push("plugged_in", plugged_in.map(b));
    push(
        "charge_energy_added",
        s.charge_energy_added.map(|v| format!("{:?}", v)),
    );
    push(
        "charge_limit_soc",
        s.charge_limit_soc.map(|v| v.to_string()),
    );
    push(
        "charger_actual_current",
        s.charger_actual_current.map(|v| v.to_string()),
    );
    push("charger_phases", s.charger_phases.map(|v| v.to_string()));
    push("charger_power", s.charger_power.map(|v| v.to_string()));
    push("charger_voltage", s.charger_voltage.map(|v| v.to_string()));
    push("conn_charge_cable", s.conn_charge_cable.clone());
    push(
        "scheduled_charging_start_time",
        s.scheduled_charging_start_time.clone(),
    );
    push(
        "time_to_full_charge",
        s.time_to_full_charge.map(|v| format!("{:?}", v)),
    );

    // Position / telemetry.
    push("latitude", s.latitude.map(|v| format!("{:?}", v)));
    push("longitude", s.longitude.map(|v| format!("{:?}", v)));
    push("shift_state", s.shift_state.clone());
    // Drive power arrives as watts (see the energy integral in
    // session.rs); charger power is already kW. Both publish raw.
    push("power", s.power.map(|v| v.to_string()));
    push("speed", s.speed.map(|v| format!("{:?}", v)));
    push("heading", s.heading.map(|v| v.to_string()));
    push("elevation", s.elevation.map(|v| format!("{:?}", v)));
    push("inside_temp", s.inside_temp.map(|v| format!("{:?}", v)));
    push("outside_temp", s.outside_temp.map(|v| format!("{:?}", v)));
    push("odometer", s.odometer.map(|v| format!("{:?}", v)));
    push(
        "est_battery_range_km",
        s.est_battery_range.map(|v| format!("{:?}", v)),
    );
    push(
        "rated_battery_range_km",
        s.battery_range.map(|v| format!("{:?}", v)),
    );
    push(
        "ideal_battery_range_km",
        s.ideal_battery_range.map(|v| format!("{:?}", v)),
    );
    push("battery_level", s.battery_level.map(|v| v.to_string()));
    push(
        "usable_battery_level",
        s.usable_battery_level.map(|v| v.to_string()),
    );

    out.into_iter()
        .filter_map(|(a, v)| Some((a?, v?)))
        .collect()
}

/// Canonical state string via the `Serialize` impl (not `Debug`).
fn state_string(state: &VehicleState) -> Option<String> {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
}

fn since_rfc3339(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Publisher runtime
// ---------------------------------------------------------------------------

/// Per-VIN state: last published payloads (change-only) and last change.
struct CarPub {
    last_sent: HashMap<String, String>,
    changed_at: i64,
}

pub struct Publisher {
    base_topic: String,
    vins: BTreeSet<String>,
    cars: HashMap<String, CarPub>,
}

impl Publisher {
    pub fn new(base_topic: String) -> Self {
        Self {
            base_topic: base_topic.trim_end_matches('/').to_string(),
            vins: BTreeSet::new(),
            cars: HashMap::new(),
        }
    }

    /// 1-based index over sorted VINs (stable for a fixed fleet).
    pub fn car_index(&mut self, vin: &str) -> u32 {
        self.vins.insert(vin.to_string());
        self.vins.iter().position(|v| v == vin).unwrap_or(0) as u32 + 1
    }

    fn now_unix() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default()
    }

    /// Full topic paths for changed values only. Appends a fresh `since`
    /// topic whenever anything changed.
    pub fn handle_summary(&mut self, s: &VehicleSummary) -> Vec<(String, String)> {
        let index = self.car_index(&s.vin);
        let attrs = topics_for(s);
        self.emit(&s.vin, index, attrs, true)
    }

    /// State-only event: just the state topic (plus `since` on change).
    /// Pruning is disabled here — a state event carries no telemetry, so
    /// absent attributes must not be mistaken for removals.
    pub fn handle_state(&mut self, vin: &str, state: VehicleState) -> Vec<(String, String)> {
        let index = self.car_index(vin);
        let attrs = state_string(&state)
            .map(|s| vec![("state".to_string(), s)])
            .unwrap_or_default();
        self.emit(vin, index, attrs, false)
    }

    /// Diff attribute payloads against last-sent, stamp `since` on change,
    /// and prefix full topic paths. With `prune`, attributes that disappear
    /// (present in `last_sent` but absent now, except synthetic `since`)
    /// are emitted with empty payloads so their retained topics clear
    /// instead of going stale in Home Assistant.
    fn emit(
        &mut self,
        vin: &str,
        index: u32,
        attrs: Vec<(String, String)>,
        prune: bool,
    ) -> Vec<(String, String)> {
        let car = self.cars.entry(vin.to_string()).or_insert_with(|| CarPub {
            last_sent: HashMap::new(),
            changed_at: 0,
        });
        let incoming: std::collections::HashSet<String> =
            attrs.iter().map(|(a, _)| a.clone()).collect();
        let mut changed: Vec<(String, String)> = attrs
            .into_iter()
            .filter(|(attr, payload)| car.last_sent.get(attr) != Some(payload))
            .collect();
        let removed: Vec<String> = if prune {
            car.last_sent
                .keys()
                .filter(|k| k.as_str() != "since" && !incoming.contains(k.as_str()))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        if !changed.is_empty() || !removed.is_empty() {
            car.changed_at = Self::now_unix();
            let since_payload = since_rfc3339(car.changed_at);
            car.last_sent.insert("since".into(), since_payload.clone());
            for (attr, payload) in &changed {
                car.last_sent.insert(attr.clone(), payload.clone());
            }
            for attr in &removed {
                car.last_sent.remove(attr);
            }
            changed.push(("since".into(), since_payload));
            for attr in removed {
                changed.push((attr, String::new()));
            }
        }
        changed
            .into_iter()
            .map(|(attr, payload)| (format!("{}/{index}/{attr}", self.base_topic), payload))
            .collect()
    }

    /// Topics to clear (empty retained payloads) on shutdown.
    pub fn clear_topics(&self) -> Vec<String> {
        let mut vins: Vec<&String> = self.cars.keys().collect();
        vins.sort();
        let mut out = Vec::new();
        for (i, vin) in vins.iter().enumerate() {
            let index = i as u32 + 1;
            if let Some(car) = self.cars.get(*vin) {
                for attr in car.last_sent.keys() {
                    out.push(format!("{}/{index}/{attr}", self.base_topic));
                }
            }
        }
        out
    }
}

/// Build rumqttc options from config. `None` when MQTT is not configured.
pub fn mqtt_options(cfg: &Config) -> Option<MqttOptions> {
    let host = cfg.mqtt_host.clone()?;
    let mut opts = MqttOptions::new("tesla-apiscraper-rs", host, cfg.mqtt_port);
    opts.set_keep_alive(Duration::from_secs(30));
    opts.set_clean_session(true);
    if let (Some(user), Some(pass)) = (cfg.mqtt_username.clone(), cfg.mqtt_password.clone()) {
        opts.set_credentials(user, pass);
    } else if let Some(user) = cfg.mqtt_username.clone() {
        opts.set_credentials(user, "");
    }
    Some(opts)
}

/// Rehydrate every snapshot (e.g. after broadcast lag): returns full
/// topic paths for anything stale or missing. Pure over the inputs for
/// testability; `run()` feeds it `all_summaries()`.
pub fn rehydrate_all(
    publisher: &mut Publisher,
    snapshots: &[VehicleSummary],
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for s in snapshots {
        out.extend(publisher.handle_summary(s));
    }
    out
}

/// Run the publisher: broadcast events in, retained MQTT messages out.
/// Exits on shutdown broadcast; clears retained topics first.
pub async fn run(
    client: AsyncClient,
    mut eventloop: EventLoop,
    mut events: tokio::sync::broadcast::Receiver<UiEvent>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    vehicles: std::sync::Arc<crate::vehicles::Vehicles>,
    base_topic: String,
) {
    // Drive the rumqttc state machine in the background.
    tokio::spawn(async move { while eventloop.poll().await.is_ok() {} });

    let mut publisher = Publisher::new(base_topic);
    loop {
        tokio::select! {
            biased;
            // Any new version (only `true` is ever sent) or dropped
            // senders means shutdown; `changed`, unlike `wait_for`, holds
            // no guard across the await so this stays `Send`.
            _ = shutdown_rx.changed() => break,
            msg = events.recv() => {
                let updates = match msg {
                    Ok(ev) if ev.kind == "summary" => ev
                        .summary
                        .as_ref()
                        .map(|s| publisher.handle_summary(s))
                        .unwrap_or_default(),
                    Ok(ev) if ev.kind == "state" => match (&ev.vin, &ev.state) {
                        (Some(vin), Some(state)) => publisher.handle_state(vin, *state),
                        _ => vec![],
                    },
                    // Lagged past the buffer: rehydrate from current store
                    // instead of gap-filling blindly. Closed senders mean
                    // teardown is underway; loop back to let shutdown win.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        rehydrate_all(&mut publisher, &vehicles.all_summaries())
                    }
                    Err(_) => vec![],
                    Ok(_) => vec![],
                };
                for (topic, payload) in updates {
                    if let Err(e) = client
                        .publish(topic.clone(), QoS::AtLeastOnce, true, payload)
                        .await
                    {
                        warn!(%topic, error = %e, "mqtt publish failed");
                    }
                }
            }
        }
    }

    info!("mqtt: clearing retained topics");
    for topic in publisher.clear_topics() {
        client
            .publish(topic, QoS::AtLeastOnce, true, Vec::<u8>::new())
            .await
            .ok();
    }
    if let Err(e) = client.disconnect().await {
        warn!(error = %e, "mqtt disconnect failed");
    }
    info!("mqtt publisher stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tesla_api::Vehicle;
    use crate::vehicles::VehicleState;

    fn test_vehicle() -> Vehicle {
        Vehicle {
            id: 1,
            vehicle_id: 100,
            vin: "VIN001".into(),
            display_name: Some("Car".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        }
    }

    fn full_summary() -> VehicleSummary {
        let data: crate::tesla_api::VehicleDataResponse =
            serde_json::from_value(serde_json::json!({
                "state": "online",
                "odometer": 50000.5,
                "drive_state": {
                    "shift_state": "D", "speed": 65.0,
                    "latitude": 37.7, "longitude": -122.4,
                    "heading": 180, "power": 12,
                    "elevation": 10.0, "timestamp": 1700000000000i64
                },
                "charge_state": {
                    "battery_level": 85, "battery_range": 270.0,
                    "ideal_battery_range": 300.0, "est_battery_range": 260.0,
                    "usable_battery_level": 82,
                    "charging_state": "Charging",
                    "charge_energy_added": 11.0,
                    "charge_limit_soc": 90,
                    "charger_actual_current": 32,
                    "charger_voltage": 230,
                    "charger_power": 7,
                    "charger_phases": 3,
                    "conn_charge_cable": "IEC",
                    "time_to_full_charge": 1.5
                },
                "climate_state": {
                    "inside_temp": 24.0, "outside_temp": 22.5,
                    "is_climate_on": true, "is_preconditioning": false
                },
                "vehicle_state": {
                    "sentry_mode": true, "is_user_present": false,
                    "df": 0.0, "pf": 0.0, "dr": 0.0, "pr": 0.0,
                    "ft": 0.0, "rt": 0.0, "locked": true,
                    "fd_window": 0.0, "fp_window": 0.0,
                    "rd_window": 0.0, "rp_window": 0.0,
                    "car_version": "2026.1",
                    "software_update": {"status": "available", "version": "2026.2"}
                },
                "vehicle_config": {
                    "car_type": "model3", "trim_badging": "Plaid?",
                    "exterior_color": "Red", "wheel_type": "Sport", "spoiler_type": "None"
                }
            }))
            .unwrap();
        let mut s =
            VehicleSummary::from_data(&test_vehicle(), VehicleState::Driving, &data, 1700000000);
        s.geofence_name = Some("Home".into());
        s
    }

    fn topic_map(topics: &[(String, String)]) -> HashMap<&str, &str> {
        topics
            .iter()
            .map(|(t, p)| (t.rsplit('/').next().unwrap(), p.as_str()))
            .collect()
    }

    #[test]
    fn topics_cover_ha_contract() {
        let s = full_summary();
        let topics = topics_for(&s);
        let m = topic_map(&topics);
        // Identity / health.
        assert_eq!(m["healthy"], "true");
        assert_eq!(m["update_available"], "true");
        assert_eq!(m["update_version"], "2026.2");
        assert_eq!(m["version"], "2026.1");
        assert_eq!(m["display_name"], "Car");
        assert_eq!(m["state"], "Driving");
        assert_eq!(m["model"], "3");
        assert_eq!(m["trim_badging"], "Plaid?");
        assert_eq!(m["geofence"], "Home");
        // Binary sensors publish straight (lock ON = unlocked per HA).
        assert_eq!(m["locked"], "true");
        assert_eq!(m["sentry_mode"], "true");
        assert_eq!(m["is_user_present"], "false");
        assert_eq!(m["doors_open"], "false");
        assert_eq!(m["windows_open"], "false");
        assert_eq!(m["trunk_open"], "false");
        assert_eq!(m["frunk_open"], "false");
        assert_eq!(m["is_climate_on"], "true");
        assert_eq!(m["plugged_in"], "true");
        // Raw API values; HA converts units.
        assert_eq!(m["speed"], "65.0"); // raw mph, HA converts
        assert_eq!(m["odometer"], "50000.5"); // raw miles, HA converts
        assert_eq!(m["est_battery_range_km"], "260.0"); // raw miles, HA converts
        assert_eq!(m["rated_battery_range_km"], "270.0"); // raw miles, HA converts
        assert_eq!(m["ideal_battery_range_km"], "300.0"); // raw miles, HA converts
        assert_eq!(m["battery_level"], "85");
        assert_eq!(m["usable_battery_level"], "82");
        assert_eq!(m["charge_energy_added"], "11.0");
        assert_eq!(m["charge_limit_soc"], "90");
        assert_eq!(m["charger_actual_current"], "32");
        assert_eq!(m["charger_power"], "7"); // already kW, passes through
        assert_eq!(m["charger_voltage"], "230");
        assert_eq!(m["conn_charge_cable"], "IEC");
        assert_eq!(m["time_to_full_charge"], "1.5");
        assert_eq!(m["inside_temp"], "24.0");
        assert_eq!(m["latitude"], "37.7");
        assert_eq!(m["power"], "12"); // raw watts, HA converts
    }

    #[test]
    fn missing_values_publish_nothing() {
        let s = VehicleSummary::initial(&test_vehicle(), VehicleState::Start);
        let topics = topics_for(&s);
        let m = topic_map(&topics);
        // Identity/health always present; telemetry absent — except the
        // door/window aggregates, which read unknown as closed.
        assert_eq!(m["healthy"], "true");
        assert_eq!(m["display_name"], "Car");
        assert!(!m.contains_key("battery_level"));
        assert!(!m.contains_key("latitude"));
        assert_eq!(m["doors_open"], "false");
        assert_eq!(m["windows_open"], "false");
        assert_eq!(m["geofence"], "");
    }

    #[test]
    fn car_index_is_sorted_one_based() {
        let mut p = Publisher::new("teslamate/cars".into());
        assert_eq!(p.car_index("VIN-B"), 1);
        assert_eq!(p.car_index("VIN-A"), 1);
        assert_eq!(p.car_index("VIN-B"), 2);
    }

    #[test]
    fn only_changes_publish() {
        let mut p = Publisher::new("teslamate/cars".into());
        let s = full_summary();
        let first = p.handle_summary(&s);
        assert!(!first.is_empty());
        assert!(
            first
                .iter()
                .all(|(t, _)| t.starts_with("teslamate/cars/1/"))
        );
        // Identical snapshot: only `since` would differ, but nothing
        // changed so even `since` stays quiet.
        let second = p.handle_summary(&s);
        assert!(second.is_empty());
        // One field flips: that topic plus refreshed `since`.
        let mut s2 = s.clone();
        s2.battery_level = Some(86);
        let third = p.handle_summary(&s2);
        let attrs: Vec<&str> = third
            .iter()
            .map(|(t, _)| t.rsplit('/').next().unwrap())
            .collect();
        assert!(attrs.contains(&"battery_level"));
        assert!(attrs.contains(&"since"));
        assert_eq!(third.len(), 2);
    }

    #[test]
    fn removed_attributes_clear_retained() {
        let mut p = Publisher::new("teslamate/cars".into());
        let s = full_summary();
        let first = p.handle_summary(&s);
        assert!(first.iter().any(|(t, _)| t.ends_with("/sentry_mode")));

        // Telemetry disappears: the topic clears with an empty retained
        // payload (plus refreshed `since`) instead of going stale.
        // (geofence stays present as "" by design, so it can't demo this.)
        let mut s2 = s.clone();
        s2.sentry_mode = None;
        let second = p.handle_summary(&s2);
        let cleared: Vec<&str> = second
            .iter()
            .filter(|(_, p)| p.is_empty())
            .map(|(t, _)| t.rsplit('/').next().unwrap())
            .collect();
        assert_eq!(cleared, vec!["sentry_mode"]);
        assert!(second.iter().any(|(t, _)| t.ends_with("/since")));

        // Steady state again: nothing further to publish.
        let third = p.handle_summary(&s2);
        assert!(third.is_empty());
    }

    #[test]
    fn rehydrate_all_republishes_current() {
        let mut p = Publisher::new("teslamate/cars".into());
        let s = full_summary();
        let first = p.handle_summary(&s);
        assert!(!first.is_empty());
        // Nothing changed: quiet.
        assert!(rehydrate_all(&mut p, std::slice::from_ref(&s)).is_empty());
        // One field moved on: full current snapshot re-emits it.
        let mut s2 = s.clone();
        s2.odometer = Some(51000.0);
        let out = rehydrate_all(&mut p, &[s2]);
        let attrs: Vec<&str> = out
            .iter()
            .map(|(t, _)| t.rsplit('/').next().unwrap())
            .collect();
        assert!(attrs.contains(&"odometer"));
        assert!(attrs.contains(&"since"));
    }

    #[test]
    fn state_event_never_prunes_telemetry() {
        let mut p = Publisher::new("teslamate/cars".into());
        let s = full_summary();
        let first = p.handle_summary(&s);
        assert!(first.iter().any(|(t, _)| t.ends_with("/battery_level")));

        // A state-only event carries no telemetry: only `state` (+`since`)
        // may emit; everything previously sent must survive untouched.
        let ev = p.handle_state("VIN001", VehicleState::Suspended);
        let attrs: Vec<&str> = ev
            .iter()
            .map(|(t, _)| t.rsplit('/').next().unwrap())
            .collect();
        assert!(attrs.contains(&"state"));
        assert!(attrs.contains(&"since"));
        assert_eq!(ev.len(), 2);
        assert!(ev.iter().all(|(_, p)| !p.is_empty()));

        // And the next identical state event is fully quiet.
        let again = p.handle_state("VIN001", VehicleState::Suspended);
        assert!(again.is_empty());
    }

    #[test]
    fn model_mapping() {
        assert_eq!(model_name(Some("models")), Some("S".into()));
        assert_eq!(model_name(Some("model3")), Some("3".into()));
        assert_eq!(model_name(Some("modelx")), Some("X".into()));
        assert_eq!(model_name(Some("modely")), Some("Y".into()));
        assert_eq!(model_name(Some("cybertruck")), Some("Cybertruck".into()));
        assert_eq!(model_name(None), None);
        assert_eq!(model_name(Some("weird")), Some("weird".into()));
    }

    #[test]
    fn mqtt_options_none_without_host() {
        let mut cfg = crate::config::Config {
            host: "0.0.0.0".into(),
            port: 4000,
            config_dir: "config".into(),
            influxdb_url: "http://localhost:8086".into(),
            influxdb_username: String::new(),
            influxdb_password: String::new(),
            influxdb_database: "tesla".into(),
            tesla_api_client_id: "ownerapi".into(),
            tesla_auth_url: "https://auth.tesla.com".into(),
            tesla_api_url: "https://owner-api.teslamotors.com".into(),
            data_encryption_key: "x".into(),
            rust_log: "info".into(),
            log_format: "text".into(),
            mqtt_host: None,
            mqtt_port: 1883,
            mqtt_username: None,
            mqtt_password: None,
            mqtt_base_topic: "teslamate/cars".into(),
            poll_interval_seconds: 60,
            streaming_enabled: false,
            grafana_url: None,
            web_dist_dir: None,
            log_file: None,
        };
        assert!(mqtt_options(&cfg).is_none());
        cfg.mqtt_host = Some("broker".into());
        assert!(mqtt_options(&cfg).is_some());
    }
}
