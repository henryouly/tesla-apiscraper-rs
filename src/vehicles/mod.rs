mod db_writer;
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
use crate::vehicle_summary::{
    EventBus, SummaryStore, UiEvent, VehicleSummary, new_event_bus, new_summary_store,
};
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
    // Behind a mutex so `join_all` can take handles through `&self`
    // (the supervisor itself is shared via `Arc` in `main`).
    join: Mutex<Option<JoinHandle<()>>>,
    state_rx: watch::Receiver<VehicleState>,
}

pub struct Vehicles {
    tasks: HashMap<String, VehicleHandle>,
    api_url: String,
    summaries: SummaryStore,
    events: EventBus,
}

impl Vehicles {
    pub fn new(api_url: &str) -> Self {
        Self {
            tasks: HashMap::new(),
            api_url: api_url.to_string(),
            summaries: new_summary_store(),
            events: new_event_bus(),
        }
    }

    /// Latest cached summary for one VIN (memory-only).
    pub fn summary_of(&self, vin: &str) -> Option<VehicleSummary> {
        self.summaries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(vin)
            .cloned()
    }

    /// All cached summaries.
    pub fn all_summaries(&self) -> Vec<VehicleSummary> {
        self.summaries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// Subscribe to UI events (SSE).
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<UiEvent> {
        self.events.subscribe()
    }

    /// Upsert a summary and broadcast it (best-effort: no subscribers is fine).
    pub fn publish_summary(&self, summary: VehicleSummary) {
        self.summaries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(summary.vin.clone(), summary.clone());
        self.events.send(UiEvent::summary(summary)).ok();
    }

    /// Broadcast a state-only event (e.g. suspend/resume without fresh data).
    #[allow(dead_code)]
    pub fn publish_state(&self, vin: &str, state: VehicleState) {
        if let Some(s) = self
            .summaries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(vin)
        {
            s.state = state;
        }
        self.events.send(UiEvent::state(vin, state)).ok();
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
        let summaries = Arc::clone(&self.summaries);
        let events = self.events.clone();
        // Seed a telemetry-less summary so the UI lists the car immediately,
        // even if it is offline and polls keep failing.
        self.publish_summary(VehicleSummary::initial(&vehicle, VehicleState::Start));
        let handle = tokio::spawn(task::vehicle_task_loop(
            vehicle,
            db,
            api_url,
            token_rx,
            settings,
            poll_interval,
            cmd_rx,
            state_tx,
            summaries,
            events,
        ));

        self.tasks.insert(
            vin,
            VehicleHandle {
                cmd_tx,
                join: Mutex::new(Some(handle)),
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

    /// Await every vehicle task after [`Self::shutdown_all`].
    ///
    /// Without this the runtime can tear tasks down mid-shutdown, cutting
    /// off each loop's final queue flush. Takes handles out of the map, so
    /// a second call is a no-op.
    pub async fn join_all(&self) {
        let handles: Vec<JoinHandle<()>> = self
            .tasks
            .values()
            .filter_map(|h| h.join.lock().unwrap_or_else(|e| e.into_inner()).take())
            .collect();
        for join in handles {
            join.await.ok();
        }
    }
}

#[cfg(test)]
mod tests;
