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
    // Behind a mutex so spawning works through the `Arc`-shared supervisor
    // (sign-in can start tasks for newly discovered vehicles at runtime).
    tasks: Mutex<HashMap<String, VehicleHandle>>,
    /// Owner API base URL. Resolved per discovery from the access token's
    /// region (one account shares one region) and updated on every
    /// discovery, so tasks never poll a stale default endpoint.
    api_url: std::sync::RwLock<String>,
    summaries: SummaryStore,
    events: EventBus,
}

impl Vehicles {
    pub fn new(api_url: &str) -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            api_url: std::sync::RwLock::new(api_url.to_string()),
            summaries: new_summary_store(),
            events: new_event_bus(),
        }
    }

    /// Update the Owner API base URL (called after each discovery with the
    /// region-resolved URL). Tasks spawned afterwards use it.
    pub fn set_api_url(&self, api_url: String) {
        *self.api_url.write().unwrap_or_else(|e| e.into_inner()) = api_url;
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
        &self,
        vehicles: &HashMap<String, Vehicle>,
        db: Arc<InfluxDb>,
        token_rx: watch::Receiver<Option<String>>,
        settings: Arc<Mutex<YamlConfigManager>>,
        poll_interval: Duration,
    ) -> usize {
        let mut count = 0;
        for vehicle in vehicles.values() {
            if self.spawn_one(
                vehicle.clone(),
                Arc::clone(&db),
                token_rx.clone(),
                Arc::clone(&settings),
                poll_interval,
            ) {
                count += 1;
            }
        }
        count
    }

    /// Spawn a task for one vehicle. Returns `false` when a task for the VIN
    /// already exists (the duplicate handle is aborted before doing work).
    pub fn spawn_one(
        &self,
        vehicle: Vehicle,
        db: Arc<InfluxDb>,
        token_rx: watch::Receiver<Option<String>>,
        settings: Arc<Mutex<YamlConfigManager>>,
        poll_interval: Duration,
    ) -> bool {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (state_tx, state_rx) = watch::channel(VehicleState::Start);

        let vin = vehicle.vin.clone();
        let api_url = self
            .api_url
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let summaries = Arc::clone(&self.summaries);
        let events = self.events.clone();

        // Admission, seeding, spawn, and insert under one lock: a repeat
        // spawn for a tracked VIN (e.g. re-sign-in via spawn_all) returns
        // before publishing anything, so live summaries are never wiped by
        // a telemetry-less re-seed. tokio::spawn only schedules — no await
        // points while the guard is held. Entry API (not contains_key) keeps
        // clippy::map_entry quiet.
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        if let std::collections::hash_map::Entry::Vacant(entry) = tasks.entry(vin) {
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
            entry.insert(VehicleHandle {
                cmd_tx,
                join: Mutex::new(Some(handle)),
                state_rx,
            });
            true
        } else {
            false
        }
    }

    /// Number of tracked vehicle tasks.
    #[cfg(test)]
    pub fn task_count(&self) -> usize {
        self.tasks.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    #[allow(dead_code)]
    pub fn send_cmd(&self, vin: &str, cmd: VehicleCommand) -> bool {
        match self
            .tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(vin)
        {
            Some(handle) => handle.cmd_tx.send(cmd).is_ok(),
            None => false,
        }
    }

    pub fn state_of(&self, vin: &str) -> Option<VehicleState> {
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(vin)
            .map(|h| *h.state_rx.borrow())
    }

    pub fn shutdown_all(&self) {
        let tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        let count = tasks.len();
        for (vin, handle) in tasks.iter() {
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
        let handles: Vec<JoinHandle<()>> = {
            let tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
            tasks
                .values()
                .filter_map(|h| h.join.lock().unwrap_or_else(|e| e.into_inner()).take())
                .collect()
        };
        for join in handles {
            join.await.ok();
        }
    }
}

#[cfg(test)]
mod tests;
