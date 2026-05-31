mod session;
mod sleep;
mod state;
mod task;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::info;

use crate::config_yaml::YamlConfigManager;
use crate::influxdb::InfluxDb;
use crate::tesla_api::Vehicle;
pub use sleep::cannot_suspend_state;
pub use state::VehicleState;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum VehicleCommand {
    Shutdown,
    Suspend,
    Resume,
}

pub struct VehicleHandle {
    cmd_tx: mpsc::UnboundedSender<VehicleCommand>,
    _join_handle: JoinHandle<()>,
    state_rx: watch::Receiver<VehicleState>,
}

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
        let handle = tokio::spawn(task::vehicle_task_loop(
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

    #[allow(dead_code)]
    pub fn send_cmd(&self, vin: &str, cmd: VehicleCommand) -> bool {
        match self.tasks.get(vin) {
            Some(handle) => handle.cmd_tx.send(cmd).is_ok(),
            None => false,
        }
    }

    pub fn state_of(&self, vin: &str) -> Option<VehicleState> {
        self.tasks.get(vin).map(|h| *h.state_rx.borrow())
    }

    pub fn shutdown_all(&self) {
        let count = self.tasks.len();
        for (vin, handle) in &self.tasks {
            handle.cmd_tx.send(VehicleCommand::Shutdown).ok();
            info!(%vin, "vehicle task shutdown sent");
        }
        info!(count, "all vehicle tasks signalled for shutdown");
    }
}

#[cfg(test)]
mod tests;
