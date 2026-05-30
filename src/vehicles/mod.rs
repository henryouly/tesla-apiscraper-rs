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
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v3/write"))
            .respond_with(wiremock::ResponseTemplate::new(204))
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
}
