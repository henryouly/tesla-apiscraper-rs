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
//! - Distances/speeds convert mph/miles → km/km/h (Owner API imperial).
//!   Power/temps/energy pass through (kW, °C, kWh per the API).
//! - Missing (`None`) values publish nothing; HA keeps last/unknown.
//! - `since` is RFC3339 of the last per-car value change (tracked here).

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS};
use tracing::{info, warn};

use crate::config::Config;
use crate::vehicle_summary::{UiEvent, VehicleSummary};
use crate::vehicles::VehicleState;

pub const MI_TO_KM: f64 = 1.60934;

// ---------------------------------------------------------------------------
// Pure mapping (no I/O — unit-tested)
// ---------------------------------------------------------------------------

fn b(v: bool) -> String {
    v.to_string()
}

fn f0(v: f64) -> String {
    format!("{:.0}", v)
}

fn f1(v: f64) -> String {
    format!("{:.1}", v)
}

fn f2(v: f64) -> String {
    format!("{:.2}", v)
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

    // Doors / windows / openings (nonzero numeric = open).
    let any_doors = [s.df, s.pf, s.dr, s.pr]
        .iter()
        .any(|v| v.is_some_and(|x| x != 0.0));
    push("doors_open", Some(b(any_doors)));
    push(
        "windows_open",
        Some(b([s.fd_window, s.fp_window, s.rd_window, s.rp_window]
            .iter()
            .any(|v| v.is_some_and(|x| x != 0.0)))),
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
    push("charge_energy_added", s.charge_energy_added.map(f2));
    push(
        "charge_limit_soc",
        s.charge_limit_soc.map(|v| v.to_string()),
    );
    push(
        "charger_actual_current",
        s.charger_actual_current.map(|v| v.to_string()),
    );
    push("charger_phases", s.charger_phases.map(|v| v.to_string()));
    push("charger_power", s.charger_power.map(|v| f1(v as f64)));
    push("charger_voltage", s.charger_voltage.map(|v| v.to_string()));
    push(
        "scheduled_charging_start_time",
        s.scheduled_charging_start_time.clone(),
    );
    push("time_to_full_charge", s.time_to_full_charge.map(f2));

    // Position / telemetry.
    push("latitude", s.latitude.map(f0));
    push("longitude", s.longitude.map(f0));
    push("shift_state", s.shift_state.clone());
    push("power", s.power.map(|v| f1(v as f64)));
    push("speed", s.speed.map(|v| f0(v * MI_TO_KM)));
    push("heading", s.heading.map(|v| v.to_string()));
    push("elevation", s.elevation.map(f0));
    push("inside_temp", s.inside_temp.map(f1));
    push("outside_temp", s.outside_temp.map(f1));
    push("odometer", s.odometer.map(|v| f1(v * MI_TO_KM)));
    push(
        "est_battery_range_km",
        s.est_battery_range.map(|v| f1(v * MI_TO_KM)),
    );
    push(
        "rated_battery_range_km",
        s.battery_range.map(|v| f1(v * MI_TO_KM)),
    );
    push(
        "ideal_battery_range_km",
        s.ideal_battery_range.map(|v| f1(v * MI_TO_KM)),
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
        self.emit(&s.vin, index, attrs)
    }

    /// State-only event: just the state topic (plus `since` on change).
    pub fn handle_state(&mut self, vin: &str, state: VehicleState) -> Vec<(String, String)> {
        let index = self.car_index(vin);
        let attrs = state_string(&state)
            .map(|s| vec![("state".to_string(), s)])
            .unwrap_or_default();
        self.emit(vin, index, attrs)
    }

    /// Diff attribute payloads against last-sent, stamp `since` on change,
    /// and prefix full topic paths.
    fn emit(
        &mut self,
        vin: &str,
        index: u32,
        attrs: Vec<(String, String)>,
    ) -> Vec<(String, String)> {
        let car = self.cars.entry(vin.to_string()).or_insert_with(|| CarPub {
            last_sent: HashMap::new(),
            changed_at: 0,
        });
        let mut changed: Vec<(String, String)> = attrs
            .into_iter()
            .filter(|(attr, payload)| car.last_sent.get(attr) != Some(payload))
            .collect();
        if !changed.is_empty() {
            car.changed_at = Self::now_unix();
            let since_payload = since_rfc3339(car.changed_at);
            car.last_sent.insert("since".into(), since_payload.clone());
            for (attr, payload) in &changed {
                car.last_sent.insert(attr.clone(), payload.clone());
            }
            changed.push(("since".into(), since_payload));
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

/// Run the publisher: broadcast events in, retained MQTT messages out.
/// Exits on shutdown broadcast; clears retained topics first.
pub async fn run(
    client: AsyncClient,
    mut eventloop: EventLoop,
    mut events: tokio::sync::broadcast::Receiver<UiEvent>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
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
                    _ => vec![],
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
        // Conversions (API imperial → metric).
        assert_eq!(m["speed"], "105"); // 65 mph
        assert_eq!(m["odometer"], "80467.8");
        assert_eq!(m["est_battery_range_km"], "418.4");
        assert_eq!(m["rated_battery_range_km"], "434.5");
        assert_eq!(m["ideal_battery_range_km"], "482.8");
        assert_eq!(m["battery_level"], "85");
        assert_eq!(m["usable_battery_level"], "82");
        assert_eq!(m["charge_energy_added"], "11.00");
        assert_eq!(m["charge_limit_soc"], "90");
        assert_eq!(m["charger_actual_current"], "32");
        assert_eq!(m["charger_power"], "7.0");
        assert_eq!(m["charger_voltage"], "230");
        assert_eq!(m["time_to_full_charge"], "1.50");
        assert_eq!(m["inside_temp"], "24.0");
        assert_eq!(m["latitude"], "38");
        assert_eq!(m["power"], "12.0");
    }

    #[test]
    fn missing_values_publish_nothing() {
        let s = VehicleSummary::initial(&test_vehicle(), VehicleState::Start);
        let topics = topics_for(&s);
        let m = topic_map(&topics);
        // Identity/health always present; telemetry absent.
        assert_eq!(m["healthy"], "true");
        assert_eq!(m["display_name"], "Car");
        assert!(!m.contains_key("battery_level"));
        assert!(!m.contains_key("latitude"));
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
