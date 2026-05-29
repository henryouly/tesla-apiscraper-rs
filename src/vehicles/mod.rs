#![allow(dead_code)]

mod state;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tracing::{info, warn};

use crate::config_yaml::YamlConfigManager;
use crate::influxdb::InfluxDb;
use crate::tesla_api::Vehicle;
pub use state::VehicleState;

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
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
}

impl Vehicles {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
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
        let handle = tokio::spawn(vehicle_task_loop(
            vehicle,
            db,
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

async fn vehicle_task_loop(
    vehicle: Vehicle,
    db: Arc<InfluxDb>,
    mut token_rx: watch::Receiver<Option<String>>,
    _settings: Arc<Mutex<YamlConfigManager>>,
    poll_interval: Duration,
    mut cmd_rx: mpsc::UnboundedReceiver<VehicleCommand>,
    state_tx: watch::Sender<VehicleState>,
) {
    let vin = &vehicle.vin;
    let name = vehicle.display_name.as_deref().unwrap_or("?");

    info!(%vin, name, "vehicle task starting");

    // Wait until a token is available before entering the main loop.
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

    let mut poll_timer = tokio::time::interval(poll_interval);
    poll_timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

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

            _ = poll_timer.tick() => {
                // Placeholder: Phase 3.2 will poll the Tesla API here.
                //
                // On each tick:
                //   1. Check circuit breaker (skip if open).
                //   2. GET /api/1/vehicles/{id}/vehicle_data
                //   3. Parse response, update state, log position.
                //   4. Record success/failure on the breaker.
                //
                // For now, just log a periodic heartbeat so we know
                // the task is alive and ticking.
                if state != VehicleState::Suspended {
                    let _ = &db; // placeholder reference
                    info!(
                        %vin,
                        ?state,
                        "poll tick (placeholder)"
                    );
                }
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
        let vm = Vehicles::new();
        assert_eq!(vm.state_of("any"), None);
    }

    #[tokio::test]
    async fn spawn_one_then_remove() {
        let mut vm = Vehicles::new();
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

        // Send shutdown via send_cmd
        assert!(vm.send_cmd(&vin, VehicleCommand::Shutdown));

        // Give the task time to process
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Verify state changed to reflect shutdown
        assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
    }

    #[tokio::test]
    async fn spawn_all_skips_existing() {
        let mut vm = Vehicles::new();
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

        // Try to spawn_all with the same vehicle — should skip
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
        let vm = Vehicles::new();
        assert!(!vm.send_cmd("UNKNOWN", VehicleCommand::Shutdown));
    }

    #[tokio::test]
    async fn state_tracks_suspend_resume() {
        let mut vm = Vehicles::new();
        let vehicle = test_vehicle();
        let vin = vehicle.vin.clone();
        let (tx, token_rx) = watch::channel(Some("token".into()));
        // Simulate that a token is already available
        tx.send(Some("token".into())).ok();

        vm.spawn_one(
            vehicle,
            test_db(),
            token_rx,
            test_settings(),
            Duration::from_millis(10),
        );

        // Wait briefly for task to reach main loop (Online)
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));

        // Suspend
        vm.send_cmd(&vin, VehicleCommand::Suspend);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(vm.state_of(&vin), Some(VehicleState::Suspended));

        // Resume
        vm.send_cmd(&vin, VehicleCommand::Resume);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));

        vm.shutdown_all();
    }
}
