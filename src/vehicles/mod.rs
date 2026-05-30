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
}
