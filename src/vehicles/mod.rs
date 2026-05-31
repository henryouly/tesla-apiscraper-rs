mod state;

use influxdb::{InfluxDbWriteable, Timestamp};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::config_yaml::YamlConfigManager;
use crate::influxdb::InfluxDb;
use crate::tesla_api::Vehicle;
pub use state::VehicleState;

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum VehicleCommand {
    Shutdown,
    Suspend,
    Resume,
}

// ---------------------------------------------------------------------------
// VehicleHandle
// ---------------------------------------------------------------------------

pub struct VehicleHandle {
    cmd_tx: mpsc::UnboundedSender<VehicleCommand>,
    _join_handle: JoinHandle<()>,
    state_rx: watch::Receiver<VehicleState>,
}

// ---------------------------------------------------------------------------
// Vehicles supervisor
// ---------------------------------------------------------------------------

pub struct Vehicles {
    tasks: HashMap<String, VehicleHandle>,
    api_url: String,
}

impl Vehicles {
    pub fn new(api_url: &str) -> Self {
        Self {
            tasks: HashMap::new(),
            api_url: api_url.to_string(),
        }
    }

    /// Spawn a task for every vehicle not already tracked.
    pub fn spawn_all(
        &mut self,
        vehicles: &HashMap<String, Vehicle>,
        db: Arc<InfluxDb>,
        token_rx: watch::Receiver<Option<String>>,
        settings: Arc<Mutex<YamlConfigManager>>,
        poll_interval: Duration,
    ) -> usize {
        let mut count = 0;
        for (vin, vehicle) in vehicles {
            if self.tasks.contains_key(vin) {
                continue;
            }
            self.spawn_one(
                vehicle.clone(),
                Arc::clone(&db),
                token_rx.clone(),
                Arc::clone(&settings),
                poll_interval,
            );
            count += 1;
        }
        count
    }

    pub fn spawn_one(
        &mut self,
        vehicle: Vehicle,
        db: Arc<InfluxDb>,
        token_rx: watch::Receiver<Option<String>>,
        settings: Arc<Mutex<YamlConfigManager>>,
        poll_interval: Duration,
    ) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (state_tx, state_rx) = watch::channel(VehicleState::Start);

        let vin = vehicle.vin.clone();
        let api_url = self.api_url.clone();
        let handle = tokio::spawn(vehicle_task_loop(
            vehicle,
            db,
            api_url,
            token_rx,
            settings,
            poll_interval,
            cmd_rx,
            state_tx,
        ));

        self.tasks.insert(
            vin,
            VehicleHandle {
                cmd_tx,
                _join_handle: handle,
                state_rx,
            },
        );
    }

    /// Send a command to a specific vehicle by VIN.
    #[allow(dead_code)]
    pub fn send_cmd(&self, vin: &str, cmd: VehicleCommand) -> bool {
        match self.tasks.get(vin) {
            Some(handle) => handle.cmd_tx.send(cmd).is_ok(),
            None => false,
        }
    }

    /// Returns the current state of a vehicle, or `None` if not tracked.
    pub fn state_of(&self, vin: &str) -> Option<VehicleState> {
        self.tasks.get(vin).map(|h| *h.state_rx.borrow())
    }

    /// Send Shutdown to all tracked vehicles.
    /// Does **not** await the tasks — they terminate when the runtime drops.
    pub fn shutdown_all(&self) {
        let count = self.tasks.len();
        for (vin, handle) in &self.tasks {
            handle.cmd_tx.send(VehicleCommand::Shutdown).ok();
            info!(%vin, "vehicle task shutdown sent");
        }
        info!(count, "all vehicle tasks signalled for shutdown");
    }
}

// ---------------------------------------------------------------------------
// Drive session tracking
// ---------------------------------------------------------------------------

/// Tracks accumulated data for a single drive.
struct DriveSession {
    drive_id: String,
    start_time: i64,
    /// Local wall-clock time at drive start (for duration, avoids clock skew with Tesla API).
    start_local_ts: u64,
    /// Local wall-clock time of the last successful poll (for accurate energy Δt).
    last_poll_ts: u64,
    start_lat: f64,
    start_lng: f64,
    prev_lat: f64,
    prev_lng: f64,
    distance_meters: f64,
    energy_used_wh: f64,
    max_speed: f64,
    speed_sum: f64,
    speed_count: u64,
    outside_temp_sum: f64,
    outside_temp_count: u64,
    inside_temp_sum: f64,
    inside_temp_count: u64,
}

/// Haversine distance in meters between two lat/lng points.
fn haversine_distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let d_lat = (lat2 - lat1).to_radians();
    let d_lon = (lon2 - lon1).to_radians();
    let a = (d_lat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (d_lon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    const R: f64 = 6_371_000.0;
    R * c
}

/// `SystemTime::now()` as seconds since unix epoch (for local clock timing).
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Poll interval during charging (Elixir-compatible: 5-20s based on charger_power).
fn charging_poll_interval(power_kw: Option<i64>) -> Duration {
    match power_kw {
        Some(p) if p > 0 => {
            let secs = (250.0 / p as f64).round().clamp(5.0, 20.0);
            Duration::from_secs_f64(secs)
        }
        _ => Duration::from_secs(5),
    }
}

// ---------------------------------------------------------------------------
// Charge session tracking
// ---------------------------------------------------------------------------

/// Tracks accumulated data for a single charging session.
struct ChargeSession {
    charge_id: String,
    start_time: i64,
    start_local_ts: u64,
    last_poll_ts: u64,
    start_lat: f64,
    start_lng: f64,
    start_battery_level: i64,
    start_range: f64,
    start_rated_range: f64,
    /// Cumulative `charge_energy_added` at session start (for delta on close).
    first_energy_added_kwh: f64,
    /// Highest cumulative `charge_energy_added` observed (handles API reset on close).
    max_energy_added_kwh: f64,
    energy_used_wh: f64,
    outside_temp_sum: f64,
    outside_temp_count: u64,
    inside_temp_sum: f64,
    inside_temp_count: u64,
}

// ---------------------------------------------------------------------------
// Update session tracking
// ---------------------------------------------------------------------------

/// Tracks an in-progress software update.
struct UpdateSession {
    update_id: String,
    version_before: Option<String>,
    install_start: String,
    /// Unix timestamp used as InfluxDB time (for upsert on close).
    update_id_ts: u128,
}

// ---------------------------------------------------------------------------
// Per-vehicle task loop
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn vehicle_task_loop(
    vehicle: Vehicle,
    db: Arc<InfluxDb>,
    api_url: String,
    mut token_rx: watch::Receiver<Option<String>>,
    _settings: Arc<Mutex<YamlConfigManager>>,
    poll_interval: Duration,
    mut cmd_rx: mpsc::UnboundedReceiver<VehicleCommand>,
    state_tx: watch::Sender<VehicleState>,
) {
    let vin = &vehicle.vin;
    let name = vehicle.display_name.as_deref().unwrap_or("?");
    let api_url = api_url.trim_end_matches('/').to_string();

    info!(%vin, name, "vehicle task starting");

    if token_rx.borrow().is_none() {
        info!(%vin, "waiting for access token");
        if token_rx.changed().await.is_err() {
            warn!(%vin, "token watch closed before token arrived, exiting");
            return;
        }
    }
    info!(%vin, "access token available");

    let mut state = VehicleState::Online;
    state_tx.send(state).ok();

    let driving_interval = Duration::from_secs_f64(2.5);

    let sleep = tokio::time::sleep(poll_interval);
    tokio::pin!(sleep);

    let mut last_lat_lng: Option<(f64, f64)> = None;
    let mut drive_session: Option<DriveSession> = None;
    let mut charge_session: Option<ChargeSession> = None;
    let mut last_charger_power: Option<i64> = None;
    let mut prev_car_version: Option<String> = None;
    let mut update_session: Option<UpdateSession> = None;

    loop {
        tokio::select! {
            biased;

            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(VehicleCommand::Shutdown) => {
                        info!(%vin, "vehicle task shutting down");
                        break;
                    }
                    Some(VehicleCommand::Suspend) => {
                        if state != VehicleState::Suspended {
                            state = VehicleState::Suspended;
                            state_tx.send(state).ok();
                            sleep.as_mut().reset(tokio::time::Instant::now() + poll_interval);
                            info!(%vin, "vehicle logging suspended");
                        }
                    }
                    Some(VehicleCommand::Resume) => {
                        if state == VehicleState::Suspended {
                            state = VehicleState::Online;
                            state_tx.send(state).ok();
                            info!(%vin, "vehicle logging resumed");
                        }
                    }
                    None => {
                        info!(%vin, "command channel closed, exiting");
                        break;
                    }
                }
            }

            _ = &mut sleep => {
                if state == VehicleState::Suspended {
                    sleep.as_mut().reset(tokio::time::Instant::now() + poll_interval);
                    continue;
                }

                let token = match token_rx.borrow().clone() {
                    Some(t) => t,
                    None => continue,
                };

                match crate::tesla_api::fetch_vehicle_data(
                    &token, &api_url, vehicle.id,
                )
                .await
                {
                    Ok(data) => {
                        info!(%vin, ?data, "vehicle_data response");

                        let new_state = match data.state.as_str() {
                            "online" => VehicleState::Online,
                            "asleep" => VehicleState::Asleep,
                            "offline" => VehicleState::Offline,
                            _ => state,
                        };

                        let new_state = if let Some(ref ds) = data.drive_state {
                            if ds.shift_state.as_deref().is_some_and(|s| s == "D" || s == "R") {
                                VehicleState::Driving
                            } else {
                                new_state
                            }
                        } else {
                            new_state
                        };

                        let new_state = if new_state == VehicleState::Driving {
                            new_state
                        } else if let Some(ref cs) = data.charge_state {
                            if cs.charging_state.as_deref().is_some_and(|s| s == "Starting" || s == "Charging") {
                                VehicleState::Charging
                            } else {
                                new_state
                            }
                        } else {
                            new_state
                        };

                        // If charging just ended and the API reports a deep sleep state
                        // (asleep/offline), normalize to Online so the Charging→Online
                        // transition is valid and the END block fires to finalize.
                        let new_state = if state == VehicleState::Charging
                            && new_state != VehicleState::Charging
                        {
                            VehicleState::Online
                        } else {
                            new_state
                        };

                        // Updating detection — lowest priority override (after Driving,
                        // Charging, and the charge-end normalization above).
                        let new_state = if new_state == VehicleState::Driving || new_state == VehicleState::Charging {
                            new_state
                        } else {
                            let is_installing = data.vehicle_state
                                .as_ref()
                                .and_then(|vs| vs.software_update.as_ref())
                                .and_then(|su| su.status.as_deref())
                                == Some("installing");

                            match state {
                                VehicleState::Updating => {
                                    if data.state == "online" && !is_installing {
                                        VehicleState::Online
                                    } else {
                                        VehicleState::Updating
                                    }
                                }
                                _ => {
                                    if is_installing {
                                        VehicleState::Updating
                                    } else {
                                        new_state
                                    }
                                }
                            }
                        };

                        if new_state != state && state.can_transition_to(new_state) {
                            state = new_state;
                            state_tx.send(state).ok();
                        }

                        // Drive lifecycle: start on Driving entry, accumulate while Driving,
                        // finalize on exit from Driving.
                        if state == VehicleState::Driving {
                            if drive_session.is_none() {
                                // START DRIVE
                                if let Some(ref ds) = data.drive_state {
                                    let ts = ds.timestamp.unwrap_or_else(|| {
                                        std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs() as i64
                                    });
                                    let lat = ds.latitude.unwrap_or(0.0);
                                    let lng = ds.longitude.unwrap_or(0.0);
                                    let drive_id = format!("{vin}_{ts}");

                                    drive_session = Some(DriveSession {
                                        drive_id: drive_id.clone(),
                                        start_time: ts,
                                        start_local_ts: now_secs(),
                                        last_poll_ts: now_secs(),
                                        start_lat: lat,
                                        start_lng: lng,
                                        prev_lat: lat,
                                        prev_lng: lng,
                                        distance_meters: 0.0,
                                        energy_used_wh: 0.0,
                                        max_speed: ds.speed.unwrap_or(0.0),
                                        speed_sum: ds.speed.unwrap_or(0.0),
                                        speed_count: if ds.speed.is_some() { 1 } else { 0 },
                                        outside_temp_sum: 0.0,
                                        outside_temp_count: 0,
                                        inside_temp_sum: 0.0,
                                        inside_temp_count: 0,
                                    });

                                    let start_time_iso = chrono::DateTime::from_timestamp(ts, 0)
                                        .map(|dt| dt.to_rfc3339());

                                    let ts_secs = if ts == 0 {
                                        now_secs() as u128
                                    } else {
                                        ts as u128
                                    };

                                    let initial_drive = crate::influxdb::Drive {
                                        time: Timestamp::Seconds(ts_secs),
                                        vin: vehicle.vin.clone(),
                                        drive_id,
                                        start_lat: lat,
                                        start_lng: lng,
                                        end_lat: None,
                                        end_lng: None,
                                        start_address: None,
                                        end_address: None,
                                        start_time: start_time_iso,
                                        end_time: None,
                                        distance_meters: None,
                                        duration_seconds: None,
                                        energy_used_wh: None,
                                        max_speed: None,
                                        average_speed: None,
                                        outside_temp_avg: None,
                                        inside_temp_avg: None,
                                        geofence_enter: None,
                                        geofence_exit: None,
                                        is_merged: None,
                                    };

                                    if let Err(e) = db
                                        .write_query(initial_drive.into_query("drives"))
                                        .await
                                    {
                                        warn!(%vin, error = %e, "failed to write initial drive");
                                    }
                                }
                            } else if let Some(ref ds) = data.drive_state
                                && let Some(session) = &mut drive_session
                            {
                                // ACCUMULATE
                                if let (Some(lat), Some(lng)) =
                                    (ds.latitude, ds.longitude)
                                {
                                    let d = haversine_distance(
                                        session.prev_lat,
                                        session.prev_lng,
                                        lat,
                                        lng,
                                    );
                                    session.distance_meters += d;
                                    session.prev_lat = lat;
                                    session.prev_lng = lng;
                                }
                                if let Some(speed) = ds.speed {
                                    if speed > session.max_speed {
                                        session.max_speed = speed;
                                    }
                                    session.speed_sum += speed;
                                    session.speed_count += 1;
                                }
                                // power is in watts; use actual elapsed time for accuracy
                                if let Some(power) = ds.power {
                                    let now = now_secs();
                                    let dt = now.saturating_sub(session.last_poll_ts);
                                    session.energy_used_wh += power as f64
                                        * dt as f64
                                        / 3600.0;
                                    session.last_poll_ts = now;
                                }
                                if let Some(ref cs) = data.climate_state {
                                    if let Some(t) = cs.outside_temp {
                                        session.outside_temp_sum += t;
                                        session.outside_temp_count += 1;
                                    }
                                    if let Some(t) = cs.inside_temp {
                                        session.inside_temp_sum += t;
                                        session.inside_temp_count += 1;
                                    }
                                }
                            }
                        } else if let Some(session) = drive_session.take() {
                            // END DRIVE
                            let end_ts = now_secs();
                            let end_lat =
                                data.drive_state.as_ref().and_then(|ds| ds.latitude);
                            let end_lng =
                                data.drive_state.as_ref().and_then(|ds| ds.longitude);
                            let duration_secs = end_ts.saturating_sub(session.start_local_ts);

                            let end_time_iso = chrono::DateTime::from_timestamp(
                                end_ts as i64,
                                0,
                            )
                            .map(|dt| dt.to_rfc3339());

                            let avg_speed = if session.speed_count > 0 {
                                Some(session.speed_sum / session.speed_count as f64)
                            } else {
                                None
                            };

                            let avg_outside_temp = if session.outside_temp_count > 0 {
                                Some(
                                    session.outside_temp_sum
                                        / session.outside_temp_count as f64,
                                )
                            } else {
                                None
                            };

                            let avg_inside_temp = if session.inside_temp_count > 0 {
                                Some(
                                    session.inside_temp_sum
                                        / session.inside_temp_count as f64,
                                )
                            } else {
                                None
                            };

                            let ts_secs = if session.start_time == 0 {
                                end_ts as u128
                            } else {
                                session.start_time as u128
                            };

                            let final_drive = crate::influxdb::Drive {
                                time: Timestamp::Seconds(ts_secs),
                                vin: vehicle.vin.clone(),
                                drive_id: session.drive_id,
                                start_lat: session.start_lat,
                                start_lng: session.start_lng,
                                end_lat,
                                end_lng,
                                start_address: None,
                                end_address: None,
                                start_time: None,
                                end_time: end_time_iso,
                                distance_meters: Some(session.distance_meters),
                                duration_seconds: Some(duration_secs as i64),
                                energy_used_wh: Some(session.energy_used_wh),
                                max_speed: Some(session.max_speed),
                                average_speed: avg_speed,
                                outside_temp_avg: avg_outside_temp,
                                inside_temp_avg: avg_inside_temp,
                                geofence_enter: None,
                                geofence_exit: None,
                                is_merged: None,
                            };

                            if let Err(e) = db
                                .write_query(final_drive.into_query("drives"))
                                .await
                            {
                                warn!(%vin, error = %e, "failed to write final drive");
                            }
                        }

                        // Charge lifecycle: start on Charging entry, accumulate while charging,
                        // finalize on exit from Charging.
                        if state == VehicleState::Charging {
                            if charge_session.is_none() {
                                // START CHARGE
                                if let Some(ref cs) = data.charge_state {
                                    let ts = now_secs() as i64;

                                    let lat = data.drive_state.as_ref()
                                        .and_then(|ds| ds.latitude).unwrap_or(0.0);
                                    let lng = data.drive_state.as_ref()
                                        .and_then(|ds| ds.longitude).unwrap_or(0.0);
                                    let charge_id = format!("{vin}_{ts}");

                                    let energy_added = cs.charge_energy_added.unwrap_or(0.0);

                                    charge_session = Some(ChargeSession {
                                        charge_id: charge_id.clone(),
                                        start_time: ts,
                                        start_local_ts: now_secs(),
                                        last_poll_ts: now_secs(),
                                        start_lat: lat,
                                        start_lng: lng,
                                        start_battery_level: cs.battery_level.unwrap_or(0),
                                        start_range: cs.ideal_battery_range.unwrap_or(0.0),
                                        start_rated_range: cs.battery_range.unwrap_or(0.0),
                                        first_energy_added_kwh: energy_added,
                                        max_energy_added_kwh: energy_added,
                                        energy_used_wh: 0.0,
                                        outside_temp_sum: 0.0,
                                        outside_temp_count: 0,
                                        inside_temp_sum: 0.0,
                                        inside_temp_count: 0,
                                    });

                                    let ts_secs = if ts == 0 {
                                        now_secs() as u128
                                    } else {
                                        ts as u128
                                    };

                                    let initial_session = crate::influxdb::ChargingSession {
                                        time: Timestamp::Seconds(ts_secs),
                                        vin: vehicle.vin.clone(),
                                        charge_id,
                                        start_lat: lat,
                                        start_lng: lng,
                                        end_lat: None,
                                        end_lng: None,
                                        start_range: Some(cs.ideal_battery_range.unwrap_or(0.0)),
                                        end_range: None,
                                        start_rated_range: Some(cs.battery_range.unwrap_or(0.0)),
                                        end_rated_range: None,
                                        start_battery_level: cs.battery_level,
                                        end_battery_level: None,
                                        energy_added_wh: None,
                                        duration_seconds: None,
                                        cost: None,
                                        geofence_id: None,
                                        geofence_name: None,
                                        charge_energy_used: None,
                                        connector_type: cs.conn_charge_cable.clone(),
                                        outside_temp_avg: None,
                                        inside_temp_avg: None,
                                    };

                                    if let Err(e) = db
                                        .write_query(initial_session.into_query("charging_sessions"))
                                        .await
                                    {
                                        warn!(%vin, error = %e, "failed to write initial charge session");
                                    }

                                    last_charger_power = cs.charger_power;
                                }
                            } else if let Some(ref cs) = data.charge_state
                                && let Some(session) = &mut charge_session
                            {
                                // ACCUMULATE
                                let now = now_secs();
                                let dt = now.saturating_sub(session.last_poll_ts);
                                session.last_poll_ts = now;

                                last_charger_power = cs.charger_power;

                                // charge_energy_added is cumulative for the session — used
                                // directly in charge reading write below.
                                let current_energy = cs.charge_energy_added.unwrap_or(0.0);
                                if current_energy > session.max_energy_added_kwh {
                                    session.max_energy_added_kwh = current_energy;
                                }

                                // Energy used = power × Δt (in Wh).
                                // When charger_phases is available, compute from
                                // current×voltage×phases for better accuracy.
                                let energy_wh = if let Some(phases) = cs.charger_phases
                                    && let (Some(current), Some(voltage)) =
                                        (cs.charger_actual_current, cs.charger_voltage)
                                {
                                    current as f64 * voltage as f64 * phases as f64
                                        * dt as f64 / 3600.0
                                } else if let Some(power) = cs.charger_power {
                                    // API returns charger_power in kW
                                    power as f64 * 1000.0 * dt as f64 / 3600.0
                                } else {
                                    0.0
                                };
                                session.energy_used_wh += energy_wh.max(0.0);

                                if let Some(ref cl) = data.climate_state {
                                    if let Some(t) = cl.outside_temp {
                                        session.outside_temp_sum += t;
                                        session.outside_temp_count += 1;
                                    }
                                    if let Some(t) = cl.inside_temp {
                                        session.inside_temp_sum += t;
                                        session.inside_temp_count += 1;
                                    }
                                }

                                // Write per-tick charge reading
                                let ts_secs = now_secs() as u128;

                                // Compute power in watts: from current×voltage×phases when
                                // available, else convert kW to W (* 1000).
                                let reading_power = if let Some(phases) = cs.charger_phases
                                    && let (Some(current), Some(voltage)) =
                                        (cs.charger_actual_current, cs.charger_voltage)
                                {
                                    Some(current as f64 * voltage as f64 * phases as f64)
                                } else {
                                    cs.charger_power.map(|p| p as f64 * 1000.0)
                                };

                                let reading = crate::influxdb::ChargeReading {
                                    time: Timestamp::Seconds(ts_secs),
                                    vin: vehicle.vin.clone(),
                                    charge_id: session.charge_id.clone(),
                                    voltage: cs.charger_voltage.map(|v| v as f64),
                                    current: cs.charger_actual_current.map(|c| c as f64),
                                    power: reading_power,
                                    phases: cs.charger_phases,
                                    energy_added: Some(current_energy),
                                    battery_level: cs.battery_level,
                                    battery_range: cs.battery_range,
                                    charger_power: cs.charger_power,
                                    charger_voltage: cs.charger_voltage,
                                    charger_phases: cs.charger_phases,
                                    outside_temp: data.climate_state.as_ref()
                                        .and_then(|cl| cl.outside_temp),
                                    fast_charger_brand: cs.fast_charger_brand.clone(),
                                    fast_charger_type: cs.fast_charger_type.clone(),
                                    conn_charge_cable: cs.conn_charge_cable.clone(),
                                    usable_battery_level: cs.usable_battery_level,
                                    charger_pilot_current: cs.charger_pilot_current,
                                    fast_charger_present: cs.fast_charger_present,
                                    battery_heater_on: cs.battery_heater_on,
                                    not_enough_power_to_heat: cs.not_enough_power_to_heat,
                                    ideal_battery_range: cs.ideal_battery_range,
                                    rated_battery_range: cs.battery_range,
                                };

                                if let Err(e) = db
                                    .write_query(reading.into_query("charge_readings"))
                                    .await
                                {
                                    warn!(%vin, error = %e, "failed to write charge reading");
                                }
                            }
                        } else if let Some(session) = charge_session.take() {
                            // END CHARGE
                            let end_ts = now_secs();
                            let end_lat = data.drive_state.as_ref()
                                .and_then(|ds| ds.latitude);
                            let end_lng = data.drive_state.as_ref()
                                .and_then(|ds| ds.longitude);
                            let duration_secs = end_ts.saturating_sub(session.start_local_ts);

                            // Tesla API sometimes resets charge_energy_added to 0 on
                            // completion. Fall back to the max observed value.
                            let latest_energy = data.charge_state.as_ref()
                                .and_then(|cs| cs.charge_energy_added)
                                .filter(|&v| v != 0.0)
                                .unwrap_or(session.max_energy_added_kwh);
                            let energy_added_wh = (latest_energy
                                - session.first_energy_added_kwh)
                                .max(0.0) * 1000.0;

                            let avg_outside_temp = if session.outside_temp_count > 0 {
                                Some(session.outside_temp_sum / session.outside_temp_count as f64)
                            } else {
                                None
                            };

                            let avg_inside_temp = if session.inside_temp_count > 0 {
                                Some(session.inside_temp_sum / session.inside_temp_count as f64)
                            } else {
                                None
                            };

                            let ts_secs = if session.start_time == 0 {
                                end_ts as u128
                            } else {
                                session.start_time as u128
                            };

                            // Capture connector type from last API response
                            let connector_type = data.charge_state.as_ref()
                                .and_then(|cs| cs.conn_charge_cable.clone());

                            let final_session = crate::influxdb::ChargingSession {
                                time: Timestamp::Seconds(ts_secs),
                                vin: vehicle.vin.clone(),
                                charge_id: session.charge_id,
                                start_lat: session.start_lat,
                                start_lng: session.start_lng,
                                end_lat,
                                end_lng,
                                start_range: Some(session.start_range),
                                end_range: data.charge_state.as_ref()
                                    .and_then(|cs| cs.ideal_battery_range),
                                start_rated_range: Some(session.start_rated_range),
                                end_rated_range: data.charge_state.as_ref()
                                    .and_then(|cs| cs.battery_range),
                                start_battery_level: Some(session.start_battery_level),
                                end_battery_level: data.charge_state.as_ref()
                                    .and_then(|cs| cs.battery_level),
                                energy_added_wh: Some(energy_added_wh),
                                duration_seconds: Some(duration_secs as i64),
                                cost: None,
                                geofence_id: None,
                                geofence_name: None,
                                charge_energy_used: Some(session.energy_used_wh),
                                connector_type,
                                outside_temp_avg: avg_outside_temp,
                                inside_temp_avg: avg_inside_temp,
                            };

                            if let Err(e) = db
                                .write_query(final_session.into_query("charging_sessions"))
                                .await
                            {
                                warn!(%vin, error = %e, "failed to write final charge session");
                            }

                            last_charger_power = None;
                        }

                        // Update lifecycle: write start when entering Updating,
                        // write completion when exiting.
                        if state == VehicleState::Updating {
                            if update_session.is_none() {
                                // START UPDATE
                                let ts = now_secs();
                                let update_id = format!("{vin}_{ts}");

                                let version_before = data.vehicle_state
                                    .as_ref()
                                    .and_then(|vs| vs.car_version.clone());

                                let install_start = chrono::DateTime::from_timestamp(ts as i64, 0)
                                    .map(|dt| dt.to_rfc3339())
                                    .unwrap_or_default();

                                update_session = Some(UpdateSession {
                                    update_id: update_id.clone(),
                                    version_before: version_before.clone(),
                                    install_start: install_start.clone(),
                                    update_id_ts: ts as u128,
                                });

                                let initial_update = crate::influxdb::Update {
                                    time: Timestamp::Seconds(ts as u128),
                                    vin: vehicle.vin.clone(),
                                    update_id,
                                    version_before: version_before.clone(),
                                    version_after: None,
                                    install_start: Some(install_start.clone()),
                                    install_end: None,
                                    status: Some("installing".into()),
                                    abandoned: Some(false),
                                };

                                if let Err(e) = db
                                    .write_query(initial_update.into_query("updates"))
                                    .await
                                {
                                    warn!(%vin, error = %e, "failed to write initial update");
                                }
                            }
                        } else if let Some(session) = update_session.take() {
                            // END UPDATE
                            let version_after = data.vehicle_state
                                .as_ref()
                                .and_then(|vs| vs.car_version.clone());

                            let install_end = chrono::DateTime::from_timestamp(now_secs() as i64, 0)
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_default();

                            let final_update = crate::influxdb::Update {
                                time: Timestamp::Seconds(session.update_id_ts),
                                vin: vehicle.vin.clone(),
                                update_id: session.update_id,
                                version_before: session.version_before,
                                version_after,
                                install_start: Some(session.install_start),
                                install_end: Some(install_end),
                                status: Some("completed".into()),
                                abandoned: Some(false),
                            };

                            if let Err(e) = db
                                .write_query(final_update.into_query("updates"))
                                .await
                            {
                                warn!(%vin, error = %e, "failed to write final update");
                            }
                        }

                        // Track car_version changes across polls.
                        if let Some(ref vs) = data.vehicle_state
                            && let Some(ref cv) = vs.car_version
                            && prev_car_version.as_deref() != Some(cv.as_str())
                        {
                            info!(%vin, ?prev_car_version, new = %cv, "car_version changed");
                            prev_car_version = Some(cv.clone());
                        }

                        if let Some(ref ds) = data.drive_state
                            && let (Some(lat), Some(lng)) = (ds.latitude, ds.longitude)
                        {
                            // Deduplicate: skip write when lat/lng unchanged since last poll
                            if last_lat_lng.is_none_or(|(pl, pn)| pl != lat || pn != lng)
                            {
                                let (
                                    battery_level, rated_battery_range_km,
                                    ideal_battery_range_km, est_battery_range_km,
                                    usable_battery_level, battery_heater_on,
                                ) = data
                                    .charge_state
                                    .as_ref()
                                    .map(|cs| {
                                        (
                                            cs.battery_level,
                                            cs.battery_range,
                                            cs.ideal_battery_range,
                                            cs.est_battery_range,
                                            cs.usable_battery_level,
                                            cs.battery_heater_on,
                                        )
                                    })
                                    .unwrap_or((None, None, None, None, None, None));

                                let (
                                    inside_temp, outside_temp, fan_status,
                                    front_def, rear_def, is_climate_on,
                                    driver_temp, passenger_temp,
                                    battery_heater, battery_heater_no_power,
                                ) = data.climate_state.as_ref().map(|cl| {
                                    (
                                        cl.inside_temp,
                                        cl.outside_temp,
                                        cl.fan_status,
                                        cl.is_front_defroster_on,
                                        cl.is_rear_defroster_on,
                                        cl.is_climate_on,
                                        cl.driver_temp_setting,
                                        cl.passenger_temp_setting,
                                        cl.battery_heater,
                                        cl.battery_heater_no_power,
                                    )
                                }).unwrap_or((
                                    None, None, None, None, None,
                                    None, None, None, None, None,
                                ));

                                let (tpms_fl, tpms_fr, tpms_rl, tpms_rr) = data
                                    .vehicle_state
                                    .as_ref()
                                    .map(|vs| {
                                        (
                                            vs.tpms_pressure_fl,
                                            vs.tpms_pressure_fr,
                                            vs.tpms_pressure_rl,
                                            vs.tpms_pressure_rr,
                                        )
                                    })
                                    .unwrap_or((None, None, None, None));

                                let pos = crate::influxdb::Position {
                                    time: Timestamp::Seconds(ds.timestamp.map_or_else(
                                        || {
                                            std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap_or_default()
                                                .as_secs() as u128
                                        },
                                        |t| t as u128,
                                    )),
                                    vin: vehicle.vin.clone(),
                                    car_id: vehicle.vehicle_id,
                                    latitude: lat,
                                    longitude: lng,
                                    speed: ds.speed,
                                    power: ds.power,
                                    odometer: data.odometer,
                                    battery_level,
                                    rated_battery_range_km,
                                    outside_temp,
                                    inside_temp,
                                    heading: ds.heading,
                                    elevation: ds.elevation,
                                    shift_state: ds.shift_state.clone(),
                                    tpms_pressure_fl: tpms_fl,
                                    tpms_pressure_fr: tpms_fr,
                                    tpms_pressure_rl: tpms_rl,
                                    tpms_pressure_rr: tpms_rr,
                                    fan_status,
                                    is_front_defroster_on: front_def,
                                    is_rear_defroster_on: rear_def,
                                    ideal_battery_range_km,
                                    est_battery_range_km,
                                    usable_battery_level,
                                    is_climate_on,
                                    driver_temp_setting: driver_temp,
                                    passenger_temp_setting: passenger_temp,
                                    battery_heater,
                                    battery_heater_on,
                                    battery_heater_no_power,
                                };

                                match db
                                    .write_query(pos.into_query("positions"))
                                    .await
                                {
                                    Ok(_) => {
                                        last_lat_lng = Some((lat, lng));
                                    }
                                    Err(e) => {
                                        warn!(%vin, error = %e, "failed to write position");
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!(%vin, error = %e, "vehicle_data poll failed");
                    }
                }

                let next = match state {
                    VehicleState::Driving => driving_interval,
                    VehicleState::Charging => charging_poll_interval(last_charger_power),
                    _ => poll_interval,
                };
                sleep.as_mut().reset(tokio::time::Instant::now() + next);
            }

            _ = token_rx.changed() => {
                let has_token = token_rx.borrow().is_some();
                info!(%vin, has_token, "token updated");
            }
        }
    }

    info!(%vin, "vehicle task exited");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::influxdb::InfluxDb;
    use crate::tesla_api::Vehicle;
    use std::sync::{Arc, Mutex};

    fn test_vehicle() -> Vehicle {
        Vehicle {
            id: 1,
            vehicle_id: 100,
            vin: "TESTVIN000000001".into(),
            display_name: Some("Test Car".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        }
    }

    fn test_api_url() -> String {
        "http://localhost:1".into()
    }

    fn test_db() -> Arc<InfluxDb> {
        Arc::new(InfluxDb::new("http://localhost:1", "none", "test").unwrap())
    }

    fn test_settings() -> Arc<Mutex<YamlConfigManager>> {
        let dir = std::env::temp_dir().join("tesla-test-vehicles").join(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string(),
        );
        Arc::new(Mutex::new(YamlConfigManager::load(&dir).unwrap()))
    }

    // -----------------------------------------------------------------------
    // Vehicles supervisor
    // -----------------------------------------------------------------------

    #[test]
    fn new_supervisor_is_empty() {
        let vm = Vehicles::new("http://localhost:1");
        assert_eq!(vm.state_of("any"), None);
    }

    #[tokio::test]
    async fn spawn_one_then_remove() {
        let mut vm = Vehicles::new(&test_api_url());
        let vehicle = test_vehicle();
        let vin = vehicle.vin.clone();
        let (_, token_rx) = watch::channel(Some("token".into()));

        vm.spawn_one(
            vehicle,
            test_db(),
            token_rx,
            test_settings(),
            Duration::from_secs(30),
        );

        assert!(vm.state_of(&vin).is_some());

        assert!(vm.send_cmd(&vin, VehicleCommand::Shutdown));

        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
    }

    #[tokio::test]
    async fn spawn_all_skips_existing() {
        let mut vm = Vehicles::new(&test_api_url());
        let vehicle = test_vehicle();
        let vin = vehicle.vin.clone();
        let (_, token_rx) = watch::channel(Some("token".into()));

        vm.spawn_one(
            vehicle.clone(),
            test_db(),
            token_rx,
            test_settings(),
            Duration::from_secs(30),
        );

        let mut vehicles = HashMap::new();
        vehicles.insert(vin.clone(), vehicle);
        let (_, token_rx2) = watch::channel(Some("token".into()));
        let count = vm.spawn_all(
            &vehicles,
            test_db(),
            token_rx2,
            test_settings(),
            Duration::from_secs(30),
        );
        assert_eq!(count, 0, "should not spawn already-tracked vehicle");

        vm.shutdown_all();
    }

    #[tokio::test]
    async fn send_cmd_to_unknown_vin_returns_false() {
        let vm = Vehicles::new("http://localhost:1");
        assert!(!vm.send_cmd("UNKNOWN", VehicleCommand::Shutdown));
    }

    #[tokio::test]
    async fn state_tracks_suspend_resume() {
        let mut vm = Vehicles::new(&test_api_url());
        let vehicle = test_vehicle();
        let vin = vehicle.vin.clone();
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            test_db(),
            token_rx,
            test_settings(),
            Duration::from_millis(10),
        );

        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));

        vm.send_cmd(&vin, VehicleCommand::Suspend);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(vm.state_of(&vin), Some(VehicleState::Suspended));

        vm.send_cmd(&vin, VehicleCommand::Resume);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));

        vm.shutdown_all();
    }

    // -----------------------------------------------------------------------
    // Polling integration tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn poll_transitions_to_asleep() {
        let tesla_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"/api/1/vehicles/\d+/vehicle_data",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "response": {
                        "id": 1,
                        "state": "asleep",
                        "odometer": null,
                        "drive_state": null
                    }
                })),
            )
            .mount(&tesla_server)
            .await;

        let db_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&db_server)
            .await;

        let mut vm = Vehicles::new(&tesla_server.uri());
        let vehicle = Vehicle {
            id: 1,
            vehicle_id: 100,
            vin: "POLLTEST01".into(),
            display_name: Some("Poll Test".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let vin = vehicle.vin.clone();
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
            token_rx,
            test_settings(),
            Duration::from_millis(50),
        );

        // Wait for a few poll ticks
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(vm.state_of(&vin), Some(VehicleState::Asleep));

        vm.shutdown_all();
    }

    #[tokio::test]
    async fn poll_writes_position_on_tick() {
        let tesla_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"/api/1/vehicles/\d+/vehicle_data",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "response": {
                        "id": 2,
                        "state": "online",
                        "odometer": 50000.0,
                        "drive_state": {
                            "shift_state": "D",
                            "speed": 55.0,
                            "latitude": 37.7749,
                            "longitude": -122.4194,
                            "heading": 180,
                            "power": 8000,
                            "elevation": 15.0,
                            "timestamp": 1700000100
                        }
                    }
                })),
            )
            .mount(&tesla_server)
            .await;

        let db_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&db_server)
            .await;

        let mut vm = Vehicles::new(&tesla_server.uri());
        let vehicle = Vehicle {
            id: 2,
            vehicle_id: 200,
            vin: "POLLTEST02".into(),
            display_name: Some("Poll Write Test".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let vin = vehicle.vin.clone();
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
            token_rx,
            test_settings(),
            Duration::from_millis(50),
        );

        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(vm.state_of(&vin), Some(VehicleState::Driving));

        vm.shutdown_all();
    }

    #[tokio::test]
    async fn poll_skips_duplicate_position() {
        let tesla_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"/api/1/vehicles/\d+/vehicle_data",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "response": {
                        "id": 3,
                        "state": "online",
                        "odometer": 50000.0,
                        "drive_state": {
                            "shift_state": "D",
                            "speed": 55.0,
                            "latitude": 37.7749,
                            "longitude": -122.4194,
                            "heading": 180,
                            "power": 8000,
                            "elevation": 15.0,
                            "timestamp": 1700000100
                        }
                    }
                })),
            )
            .mount(&tesla_server)
            .await;

        let db_server = wiremock::MockServer::start().await;
        // Catch-all: handle drive writes and any other writes.
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&db_server)
            .await;
        // Specific: match only position writes via the car_id tag (higher priority).
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .and(wiremock::matchers::body_string_contains("car_id="))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .with_priority(1)
            .expect(1)
            .mount(&db_server)
            .await;

        let mut vm = Vehicles::new(&tesla_server.uri());
        let vehicle = Vehicle {
            id: 3,
            vehicle_id: 300,
            vin: "DEDUP0001".into(),
            display_name: Some("Dedup Test".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
            token_rx,
            test_settings(),
            Duration::from_millis(50),
        );

        tokio::time::sleep(Duration::from_millis(200)).await;

        vm.shutdown_all();
    }

    #[tokio::test]
    async fn poll_retries_after_write_failure() {
        let tesla_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"/api/1/vehicles/\d+/vehicle_data",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "response": {
                        "id": 5,
                        "state": "online",
                        "odometer": 50000.0,
                        "drive_state": {
                            "shift_state": null,
                            "speed": null,
                            "latitude": 37.7749,
                            "longitude": -122.4194,
                            "heading": null,
                            "power": 0,
                            "elevation": null,
                            "timestamp": 1700000100
                        }
                    }
                })),
            )
            .mount(&tesla_server)
            .await;

        let db_server = wiremock::MockServer::start().await;
        // Always return 500 — every tick should still retry (at least 2 attempts)
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .expect(2..)
            .mount(&db_server)
            .await;

        let mut vm = Vehicles::new(&tesla_server.uri());
        let vehicle = Vehicle {
            id: 5,
            vehicle_id: 500,
            vin: "RETRY001".into(),
            display_name: Some("Retry Test".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
            token_rx,
            test_settings(),
            Duration::from_millis(50),
        );

        tokio::time::sleep(Duration::from_millis(200)).await;

        // .expect(2) verifies both ticks attempted a write
        // despite both failing — dedup did NOT skip the second
        vm.shutdown_all();
    }

    #[tokio::test]
    async fn poll_position_includes_all_fields() {
        let tesla_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"/api/1/vehicles/\d+/vehicle_data",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "response": {
                        "id": 4,
                        "state": "online",
                        "odometer": 50000.0,
                        "drive_state": {
                            "shift_state": "D",
                            "speed": 55.0,
                            "latitude": 37.7749,
                            "longitude": -122.4194,
                            "heading": 180,
                            "power": 8000,
                            "elevation": 15.0,
                            "timestamp": 1700000100
                        },
                        "charge_state": {
                            "battery_level": 85,
                            "battery_range": 270.0,
                            "ideal_battery_range": 300.0,
                            "est_battery_range": 260.0,
                            "usable_battery_level": 82,
                            "battery_heater_on": false
                        },
                        "climate_state": {
                            "inside_temp": 24.0,
                            "outside_temp": 22.5,
                            "fan_status": 5,
                            "is_front_defroster_on": false,
                            "is_rear_defroster_on": false,
                            "is_climate_on": true,
                            "driver_temp_setting": 22.0,
                            "passenger_temp_setting": 22.0,
                            "battery_heater": false,
                            "battery_heater_no_power": false
                        },
                        "vehicle_state": {
                            "tpms_pressure_fl": 42.0,
                            "tpms_pressure_fr": 41.5,
                            "tpms_pressure_rl": 40.0,
                            "tpms_pressure_rr": 40.5
                        }
                    }
                })),
            )
            .mount(&tesla_server)
            .await;

        let db_server = wiremock::MockServer::start().await;
        // Mock only matches if the body contains all the new fields
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .and(wiremock::matchers::body_string_contains(
                "battery_level=85i",
            ))
            .and(wiremock::matchers::body_string_contains(
                "rated_battery_range_km=270",
            ))
            .and(wiremock::matchers::body_string_contains(
                "ideal_battery_range_km=300",
            ))
            .and(wiremock::matchers::body_string_contains(
                "est_battery_range_km=260",
            ))
            .and(wiremock::matchers::body_string_contains(
                "usable_battery_level=82i",
            ))
            .and(wiremock::matchers::body_string_contains("inside_temp=24"))
            .and(wiremock::matchers::body_string_contains(
                "outside_temp=22.5",
            ))
            .and(wiremock::matchers::body_string_contains("fan_status=5i"))
            .and(wiremock::matchers::body_string_contains(
                "is_front_defroster_on=false",
            ))
            .and(wiremock::matchers::body_string_contains(
                "is_rear_defroster_on=false",
            ))
            .and(wiremock::matchers::body_string_contains(
                "is_climate_on=true",
            ))
            .and(wiremock::matchers::body_string_contains(
                "driver_temp_setting=22",
            ))
            .and(wiremock::matchers::body_string_contains(
                "passenger_temp_setting=22",
            ))
            .and(wiremock::matchers::body_string_contains(
                "battery_heater=false",
            ))
            .and(wiremock::matchers::body_string_contains(
                "battery_heater_on=false",
            ))
            .and(wiremock::matchers::body_string_contains(
                "battery_heater_no_power=false",
            ))
            .and(wiremock::matchers::body_string_contains(
                "tpms_pressure_fl=42",
            ))
            .and(wiremock::matchers::body_string_contains(
                "tpms_pressure_fr=41.5",
            ))
            .and(wiremock::matchers::body_string_contains(
                "tpms_pressure_rl=40",
            ))
            .and(wiremock::matchers::body_string_contains(
                "tpms_pressure_rr=40.5",
            ))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .expect(1)
            .mount(&db_server)
            .await;

        let mut vm = Vehicles::new(&tesla_server.uri());
        let vehicle = Vehicle {
            id: 4,
            vehicle_id: 400,
            vin: "FIELDS01".into(),
            display_name: Some("Fields Test".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
            token_rx,
            test_settings(),
            Duration::from_millis(50),
        );

        tokio::time::sleep(Duration::from_millis(200)).await;

        // .expect(1) above verifies exactly one write matched
        vm.shutdown_all();
    }

    #[tokio::test]
    async fn drive_starts_when_driving() {
        let tesla_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"/api/1/vehicles/\d+/vehicle_data",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "response": {
                        "id": 10,
                        "state": "online",
                        "odometer": 60000.0,
                        "drive_state": {
                            "shift_state": "D",
                            "speed": 60.0,
                            "latitude": 37.8,
                            "longitude": -122.4,
                            "heading": 90,
                            "power": 10000,
                            "elevation": 20.0,
                            "timestamp": 1700000200
                        }
                    }
                })),
            )
            .mount(&tesla_server)
            .await;

        let db_server = wiremock::MockServer::start().await;
        // Catch-all for position writes and other writes.
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&db_server)
            .await;
        // Specific: match only drive writes via the drive_id tag (higher priority).
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .and(wiremock::matchers::body_string_contains("drive_id="))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .with_priority(1)
            .expect(1)
            .mount(&db_server)
            .await;

        let mut vm = Vehicles::new(&tesla_server.uri());
        let vehicle = Vehicle {
            id: 10,
            vehicle_id: 1000,
            vin: "DRIVE001".into(),
            display_name: Some("Drive Start Test".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
            token_rx,
            test_settings(),
            Duration::from_millis(50),
        );

        tokio::time::sleep(Duration::from_millis(200)).await;

        // .expect(1) above verifies exactly one drive write matched
        vm.shutdown_all();
    }

    #[tokio::test]
    async fn no_drive_writes_when_parked() {
        let tesla_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"/api/1/vehicles/\d+/vehicle_data",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "response": {
                        "id": 11,
                        "state": "online",
                        "odometer": 61000.0,
                        "drive_state": {
                            "shift_state": null,
                            "speed": null,
                            "latitude": 37.8,
                            "longitude": -122.4,
                            "heading": null,
                            "power": 0,
                            "elevation": null,
                            "timestamp": 1700000300
                        }
                    }
                })),
            )
            .mount(&tesla_server)
            .await;

        let db_server = wiremock::MockServer::start().await;
        // Catch-all for position writes.
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&db_server)
            .await;
        // Specific: match only drive writes (higher priority) — expect 0.
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .and(wiremock::matchers::body_string_contains("drive_id="))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .with_priority(1)
            .expect(0)
            .mount(&db_server)
            .await;

        let mut vm = Vehicles::new(&tesla_server.uri());
        let vehicle = Vehicle {
            id: 11,
            vehicle_id: 1100,
            vin: "PARKED01".into(),
            display_name: Some("Parked Test".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
            token_rx,
            test_settings(),
            Duration::from_millis(50),
        );

        tokio::time::sleep(Duration::from_millis(200)).await;

        // Vehicle should be Online, not Driving
        assert_eq!(vm.state_of("PARKED01"), Some(VehicleState::Online));
        vm.shutdown_all();
    }

    #[tokio::test]
    async fn charge_starts_when_charging() {
        let tesla_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"/api/1/vehicles/\d+/vehicle_data",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "response": {
                        "id": 20,
                        "state": "online",
                        "odometer": 70000.0,
                        "drive_state": {
                            "shift_state": null,
                            "speed": null,
                            "latitude": 37.8,
                            "longitude": -122.4,
                            "heading": null,
                            "power": 0,
                            "elevation": null,
                            "timestamp": 1700000400
                        },
                        "charge_state": {
                            "battery_level": 55,
                            "battery_range": 150.0,
                            "ideal_battery_range": 180.0,
                            "charging_state": "Charging",
                            "charge_energy_added": 2.5,
                            "charger_actual_current": 32,
                            "charger_voltage": 230,
                            "charger_power": 7000,
                            "charger_phases": 3
                        }
                    }
                })),
            )
            .mount(&tesla_server)
            .await;

        let db_server = wiremock::MockServer::start().await;
        // Catch-all for any writes
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&db_server)
            .await;
        // Specific: match only charging_sessions writes via the charge_id tag (higher priority)
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .and(wiremock::matchers::body_string_contains("charge_id="))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .with_priority(1)
            .expect(1)
            .mount(&db_server)
            .await;

        let mut vm = Vehicles::new(&tesla_server.uri());
        let vehicle = Vehicle {
            id: 20,
            vehicle_id: 2000,
            vin: "CHARGE01".into(),
            display_name: Some("Charge Start Test".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
            token_rx,
            test_settings(),
            Duration::from_millis(50),
        );

        tokio::time::sleep(Duration::from_millis(200)).await;

        vm.shutdown_all();
    }

    #[tokio::test]
    async fn no_charge_writes_when_disconnected() {
        let tesla_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"/api/1/vehicles/\d+/vehicle_data",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "response": {
                        "id": 21,
                        "state": "online",
                        "odometer": 71000.0,
                        "drive_state": {
                            "shift_state": null,
                            "speed": null,
                            "latitude": 37.8,
                            "longitude": -122.4,
                            "heading": null,
                            "power": 0,
                            "elevation": null,
                            "timestamp": 1700000500
                        },
                        "charge_state": {
                            "battery_level": 80,
                            "battery_range": 220.0,
                            "ideal_battery_range": 250.0,
                            "charging_state": "Disconnected",
                            "charge_energy_added": 0.0,
                            "charger_actual_current": 0,
                            "charger_voltage": 0,
                            "charger_power": 0,
                            "charger_phases": null
                        }
                    }
                })),
            )
            .mount(&tesla_server)
            .await;

        let db_server = wiremock::MockServer::start().await;
        // Catch-all for position writes
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&db_server)
            .await;
        // Specific: match only charge writes (higher priority) — expect 0
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .and(wiremock::matchers::body_string_contains("charge_id="))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .with_priority(1)
            .expect(0)
            .mount(&db_server)
            .await;

        let mut vm = Vehicles::new(&tesla_server.uri());
        let vehicle = Vehicle {
            id: 21,
            vehicle_id: 2100,
            vin: "DISCONN01".into(),
            display_name: Some("Disconnected Test".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
            token_rx,
            test_settings(),
            Duration::from_millis(50),
        );

        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(vm.state_of("DISCONN01"), Some(VehicleState::Online));
        vm.shutdown_all();
    }

    #[tokio::test]
    async fn charge_writes_reading_every_tick() {
        let tesla_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"/api/1/vehicles/\d+/vehicle_data",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "response": {
                        "id": 22,
                        "state": "online",
                        "odometer": 72000.0,
                        "drive_state": {
                            "shift_state": null,
                            "speed": null,
                            "latitude": 37.8,
                            "longitude": -122.4,
                            "heading": null,
                            "power": 0,
                            "elevation": null,
                            "timestamp": 1700000600
                        },
                        "charge_state": {
                            "battery_level": 60,
                            "battery_range": 160.0,
                            "ideal_battery_range": 190.0,
                            "charging_state": "Charging",
                            "charge_energy_added": 5.0,
                            "charger_actual_current": 32,
                            "charger_voltage": 230,
                            "charger_power": 7000,
                            "charger_phases": 3,
                            "fast_charger_brand": "Tesla",
                            "fast_charger_type": "Supercharger",
                            "conn_charge_cable": "CCS"
                        },
                        "climate_state": {
                            "outside_temp": 22.5,
                            "inside_temp": 24.0
                        }
                    }
                })),
            )
            .mount(&tesla_server)
            .await;

        let db_server = wiremock::MockServer::start().await;
        // Catch-all for any writes.
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&db_server)
            .await;

        let mut vm = Vehicles::new(&tesla_server.uri());
        let vehicle = Vehicle {
            id: 22,
            vehicle_id: 2200,
            vin: "READING01".into(),
            display_name: Some("Charge Reading Test".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
            token_rx,
            test_settings(),
            Duration::from_millis(50),
        );

        tokio::time::sleep(Duration::from_millis(200)).await;

        // Verify we're in Charging state (confirms lifecycle started)
        assert_eq!(vm.state_of("READING01"), Some(VehicleState::Charging));
        vm.shutdown_all();
    }

    #[tokio::test]
    async fn charge_ends_with_aggregated_write() {
        let tesla_server = wiremock::MockServer::start().await;
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

        // 3-response sequence: Charging → Charging → Complete
        let charging_resp_a = serde_json::json!({
            "response": {
                "id": 23,
                "state": "online",
                "odometer": 73000.0,
                "drive_state": {
                    "shift_state": null,
                    "speed": null,
                    "latitude": 37.8,
                    "longitude": -122.4,
                    "heading": null,
                    "power": 0,
                    "elevation": null,
                    "timestamp": 1700000700
                },
                "charge_state": {
                    "battery_level": 50,
                    "battery_range": 130.0,
                    "ideal_battery_range": 160.0,
                    "charging_state": "Charging",
                    "charge_energy_added": 0.0,
                    "charger_actual_current": 32,
                    "charger_voltage": 230,
                    "charger_power": 7000,
                    "charger_phases": 3,
                    "conn_charge_cable": "CCS"
                },
                "climate_state": {
                    "outside_temp": 20.0,
                    "inside_temp": 22.0
                }
            }
        });

        let charging_resp_b = serde_json::json!({
            "response": {
                "id": 23,
                "state": "online",
                "odometer": 73000.0,
                "drive_state": {
                    "shift_state": null,
                    "speed": null,
                    "latitude": 37.8,
                    "longitude": -122.4,
                    "heading": null,
                    "power": 0,
                    "elevation": null,
                    "timestamp": 1700000705
                },
                "charge_state": {
                    "battery_level": 55,
                    "battery_range": 145.0,
                    "ideal_battery_range": 175.0,
                    "charging_state": "Charging",
                    "charge_energy_added": 2.0,
                    "charger_actual_current": 32,
                    "charger_voltage": 230,
                    "charger_power": 7000,
                    "charger_phases": 3,
                    "conn_charge_cable": "CCS"
                },
                "climate_state": {
                    "outside_temp": 20.0,
                    "inside_temp": 22.0
                }
            }
        });

        let complete_resp = serde_json::json!({
            "response": {
                "id": 23,
                "state": "online",
                "odometer": 73000.0,
                "drive_state": {
                    "shift_state": null,
                    "speed": null,
                    "latitude": 37.8,
                    "longitude": -122.4,
                    "heading": null,
                    "power": 0,
                    "elevation": null,
                    "timestamp": 1700000700
                },
                "charge_state": {
                    "battery_level": 80,
                    "battery_range": 220.0,
                    "ideal_battery_range": 260.0,
                    "charging_state": "Complete",
                    "charge_energy_added": 15.0,
                    "charger_actual_current": 0,
                    "charger_voltage": 0,
                    "charger_power": 0,
                    "charger_phases": null
                },
                "climate_state": {
                    "outside_temp": 20.0,
                    "inside_temp": 22.0
                }
            }
        });

        let counter_clone = std::sync::Arc::clone(&counter);
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"/api/1/vehicles/\d+/vehicle_data",
            ))
            .respond_with(move |_req: &wiremock::Request| {
                let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if count == 0 {
                    wiremock::ResponseTemplate::new(200).set_body_json(charging_resp_a.clone())
                } else if count == 1 {
                    wiremock::ResponseTemplate::new(200).set_body_json(charging_resp_b.clone())
                } else {
                    wiremock::ResponseTemplate::new(200).set_body_json(complete_resp.clone())
                }
            })
            .mount(&tesla_server)
            .await;

        let db_server = wiremock::MockServer::start().await;
        // Catch-all for any writes (unlimited).
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&db_server)
            .await;
        // Assert at least one per-tick charge reading was written.
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .and(wiremock::matchers::body_string_contains("charge_readings"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .with_priority(1)
            .expect(1)
            .mount(&db_server)
            .await;
        // Assert the aggregated charge session write with duration/energy/battery.
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .and(wiremock::matchers::body_string_contains(
                "charging_sessions",
            ))
            .and(wiremock::matchers::body_string_contains("energy_added_wh="))
            .and(wiremock::matchers::body_string_contains(
                "duration_seconds=",
            ))
            .and(wiremock::matchers::body_string_contains(
                "end_battery_level=80i",
            ))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .with_priority(1)
            .expect(1)
            .mount(&db_server)
            .await;

        let mut vm = Vehicles::new(&tesla_server.uri());
        let vehicle = Vehicle {
            id: 23,
            vehicle_id: 2300,
            vin: "ENDCHRG01".into(),
            display_name: Some("Charge End Test".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
            token_rx,
            test_settings(),
            Duration::from_millis(50),
        );

        // Wait for START tick to fire (Response 0 → start session)
        tokio::time::sleep(Duration::from_millis(200)).await;

        // First Suspend/Resume: force next tick at poll_interval.
        // Response 1 → ACCUMULATE (writes a charge_reading)
        assert!(vm.send_cmd("ENDCHRG01", VehicleCommand::Suspend));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(vm.send_cmd("ENDCHRG01", VehicleCommand::Resume));
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Second Suspend/Resume: next tick fires at poll_interval.
        // Response 2+ → Complete → END (writes aggregated charging_sessions)
        assert!(vm.send_cmd("ENDCHRG01", VehicleCommand::Suspend));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(vm.send_cmd("ENDCHRG01", VehicleCommand::Resume));
        tokio::time::sleep(Duration::from_millis(100)).await;

        vm.shutdown_all();
    }

    #[tokio::test]
    async fn update_starts_when_installing() {
        let tesla_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"/api/1/vehicles/\d+/vehicle_data",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "response": {
                        "id": 30,
                        "state": "online",
                        "odometer": 80000.0,
                        "drive_state": {
                            "shift_state": null,
                            "speed": null,
                            "latitude": 37.8,
                            "longitude": -122.4,
                            "heading": null,
                            "power": 0,
                            "elevation": null,
                            "timestamp": 1700000800
                        },
                        "vehicle_state": {
                            "car_version": "2024.8",
                            "software_update": {
                                "download_perc": 100,
                                "expected_duration_sec": 2700,
                                "install_perc": 50,
                                "status": "installing",
                                "version": "2024.12"
                            }
                        }
                    }
                })),
            )
            .mount(&tesla_server)
            .await;

        let db_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&db_server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .and(wiremock::matchers::body_string_contains(
                r#"status="installing""#,
            ))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .with_priority(1)
            .expect(1)
            .mount(&db_server)
            .await;

        let mut vm = Vehicles::new(&tesla_server.uri());
        let vehicle = Vehicle {
            id: 30,
            vehicle_id: 3000,
            vin: "UPDSTRT01".into(),
            display_name: Some("Update Start Test".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let vin = vehicle.vin.clone();
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
            token_rx,
            test_settings(),
            Duration::from_millis(50),
        );

        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(vm.state_of(&vin), Some(VehicleState::Updating));
        vm.shutdown_all();
    }

    #[tokio::test]
    async fn update_completes_when_installed() {
        let tesla_server = wiremock::MockServer::start().await;
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

        let installing_resp = serde_json::json!({
            "response": {
                "id": 31,
                "state": "online",
                "odometer": 81000.0,
                "drive_state": {
                    "shift_state": null,
                    "speed": null,
                    "latitude": 37.8,
                    "longitude": -122.4,
                    "heading": null,
                    "power": 0,
                    "elevation": null,
                    "timestamp": 1700000900
                },
                "vehicle_state": {
                    "car_version": "2024.8",
                    "software_update": {
                        "status": "installing",
                        "version": "2024.12"
                    }
                }
            }
        });

        let done_resp = serde_json::json!({
            "response": {
                "id": 31,
                "state": "online",
                "odometer": 81000.0,
                "drive_state": {
                    "shift_state": null,
                    "speed": null,
                    "latitude": 37.8,
                    "longitude": -122.4,
                    "heading": null,
                    "power": 0,
                    "elevation": null,
                    "timestamp": 1700000900
                },
                "vehicle_state": {
                    "car_version": "2024.12",
                    "software_update": {
                        "status": "",
                        "version": "2024.12"
                    }
                }
            }
        });

        let counter_clone = std::sync::Arc::clone(&counter);
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"/api/1/vehicles/\d+/vehicle_data",
            ))
            .respond_with(move |_req: &wiremock::Request| {
                let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if count == 0 {
                    wiremock::ResponseTemplate::new(200).set_body_json(installing_resp.clone())
                } else {
                    wiremock::ResponseTemplate::new(200).set_body_json(done_resp.clone())
                }
            })
            .mount(&tesla_server)
            .await;

        let db_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&db_server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .and(wiremock::matchers::body_string_contains(
                r#"status="installing""#,
            ))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .with_priority(1)
            .expect(1)
            .mount(&db_server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .and(wiremock::matchers::body_string_contains("install_end="))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .with_priority(1)
            .expect(1)
            .mount(&db_server)
            .await;

        let mut vm = Vehicles::new(&tesla_server.uri());
        let vehicle = Vehicle {
            id: 31,
            vehicle_id: 3100,
            vin: "UPDDONE01".into(),
            display_name: Some("Update Done Test".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let vin = vehicle.vin.clone();
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
            token_rx,
            test_settings(),
            Duration::from_millis(50),
        );

        tokio::time::sleep(Duration::from_millis(150)).await;

        assert!(vm.send_cmd("UPDDONE01", VehicleCommand::Suspend));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(vm.send_cmd("UPDDONE01", VehicleCommand::Resume));
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
        vm.shutdown_all();
    }

    #[tokio::test]
    async fn no_update_when_no_software_update() {
        let tesla_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"/api/1/vehicles/\d+/vehicle_data",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "response": {
                        "id": 33,
                        "state": "online",
                        "odometer": 83000.0,
                        "drive_state": {
                            "shift_state": null,
                            "speed": null,
                            "latitude": 37.8,
                            "longitude": -122.4,
                            "heading": null,
                            "power": 0,
                            "elevation": null,
                            "timestamp": 1700001100
                        },
                        "vehicle_state": {
                            "tpms_pressure_fl": 42.0,
                            "tpms_pressure_fr": 41.5,
                            "tpms_pressure_rl": 40.0,
                            "tpms_pressure_rr": 40.5
                        }
                    }
                })),
            )
            .mount(&tesla_server)
            .await;

        let db_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&db_server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .and(wiremock::matchers::body_string_contains("updates"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .with_priority(1)
            .expect(0)
            .mount(&db_server)
            .await;

        let mut vm = Vehicles::new(&tesla_server.uri());
        let vehicle = Vehicle {
            id: 33,
            vehicle_id: 3300,
            vin: "NOUPD001".into(),
            display_name: Some("No Update Test".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let vin = vehicle.vin.clone();
        let (tx, token_rx) = watch::channel(Some("token".into()));
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
            token_rx,
            test_settings(),
            Duration::from_millis(50),
        );

        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
        vm.shutdown_all();
    }
}
