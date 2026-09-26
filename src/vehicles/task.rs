use futures_util::future::OptionFuture;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tracing::{info, trace, warn};

use crate::config_yaml::YamlConfigManager;
use crate::influxdb::InfluxDb;
use crate::streaming::{StreamEndReason, StreamingData};
use crate::tesla_api::Vehicle;
use crate::vehicle_summary::{EventBus, SummaryStore, UiEvent, VehicleSummary, now_unix};
use crate::vehicles::VehicleCommand;
use crate::vehicles::db_writer::{DEFAULT_CAPACITY, DbWriter};
use crate::vehicles::session::{self, ChargeSession, DriveSession, UpdateSession};
use crate::vehicles::sleep::can_fall_asleep;
use crate::vehicles::state::{VehicleState, derive_next_state};

/// A single live streaming connection: its data channel and task handle.
///
/// The channel and the task always exist together; dropping the link detaches
/// the task, so [`abort`](Self::abort) must be used to stop it. The end reason
/// is logged inside the task itself — the loop only needs to know the channel
/// closed (the task exits, its sender drops, and [`recv`](mpsc::Receiver::recv)
/// returns `None`).
struct StreamLink {
    data_rx: mpsc::Receiver<StreamingData>,
    join: tokio::task::JoinHandle<()>,
}

impl StreamLink {
    fn spawn(token: String, vin: &str, vehicle_id: i64) -> Self {
        let (data_tx, data_rx) = mpsc::channel(64);
        let v = vin.to_string();
        info!(%vin, "streaming: starting");
        let join = tokio::spawn(async move {
            let reason =
                crate::streaming::stream_vehicle_data(&token, vehicle_id, &v, data_tx).await;
            match &reason {
                StreamEndReason::VehicleOffline => {
                    warn!(vin = %v, "streaming ended: vehicle offline");
                }
                StreamEndReason::TokenExpired => {
                    warn!(vin = %v, "streaming ended: token expired");
                }
                StreamEndReason::IoError(e) => {
                    warn!(vin = %v, error = %e, "streaming ended: io error");
                }
                StreamEndReason::Shutdown => {
                    info!(vin = %v, "streaming ended");
                }
            }
        });
        Self { data_rx, join }
    }

    /// Abort the streaming task and drop its channel.
    fn abort(self, vin: &str) {
        self.join.abort();
        info!(%vin, "streaming: stopped");
    }
}

/// How long after the last streaming message the stream still counts as
/// fresh. Live telemetry arrives at ~4Hz, so any message inside this window
/// means the socket is delivering; past it, REST resumes full-rate polling.
const STREAM_FRESH_WINDOW: Duration = Duration::from_secs(30);

/// Whether the stream recently delivered data.
pub(crate) fn stream_is_fresh(
    last_stream_msg: Option<tokio::time::Instant>,
    now: tokio::time::Instant,
) -> bool {
    last_stream_msg.is_some_and(|t| now.duration_since(t) < STREAM_FRESH_WINDOW)
}

/// Interval until the next REST poll tick.
///
/// While driving with a fresh stream, GPS/speed/power arrive over the
/// socket, so REST drops to the heartbeat rate — polls still feed state
/// transitions, session close, and enrichment. Charging keeps its own
/// cadence (the stream carries no charger fields); all other states
/// already poll at the heartbeat.
pub(crate) fn next_poll_interval(
    state: VehicleState,
    last_charger_power: Option<i64>,
    poll_interval: Duration,
    driving_interval: Duration,
    stream_fresh: bool,
) -> Duration {
    match state {
        VehicleState::Driving if stream_fresh => poll_interval,
        VehicleState::Driving => driving_interval,
        VehicleState::Charging => session::charging_poll_interval(last_charger_power),
        _ => poll_interval,
    }
}

/// Update only the cached summary's state (no fresh telemetry).
fn set_summary_state(summaries: &SummaryStore, vin: &str, state: VehicleState) {
    if let Some(s) = summaries
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(vin)
    {
        s.state = state;
    }
}

/// Patch cached GPS/speed/odometer from a streaming point, returning the
/// updated summary for broadcast. Returns `None` when no summary is cached
/// yet (no successful poll so far).
fn patch_summary_from_stream(
    summaries: &SummaryStore,
    vin: &str,
    state: VehicleState,
    data: &StreamingData,
) -> Option<VehicleSummary> {
    let mut guard = summaries.write().unwrap_or_else(|e| e.into_inner());
    let s = guard.get_mut(vin)?;
    if data.latitude.is_some() {
        s.latitude = data.latitude;
    }
    if data.longitude.is_some() {
        s.longitude = data.longitude;
    }
    if data.speed.is_some() {
        s.speed = data.speed;
    }
    if data.odometer.is_some() {
        s.odometer = data.odometer;
    }
    s.state = state;
    s.last_updated_at = now_unix();
    Some(s.clone())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn vehicle_task_loop(
    vehicle: Vehicle,
    db: Arc<InfluxDb>,
    api_url: String,
    mut token_rx: watch::Receiver<Option<String>>,
    settings: Arc<Mutex<YamlConfigManager>>,
    poll_interval: Duration,
    mut cmd_rx: mpsc::UnboundedReceiver<VehicleCommand>,
    state_tx: watch::Sender<VehicleState>,
    summaries: SummaryStore,
    events: EventBus,
) {
    let vin = &vehicle.vin;
    let name = vehicle.display_name.as_deref().unwrap_or("?");
    let api_url = api_url.trim_end_matches('/').to_string();

    info!(%vin, name, "vehicle task starting");

    if token_rx.borrow().is_none() {
        info!(%vin, "waiting for access token");
        if token_rx.changed().await.is_err() {
            warn!(%vin, "token channel closed, exiting");
            return;
        }
    }

    let mut state = VehicleState::Online;
    state_tx.send(state).ok();
    let driving_interval = Duration::from_secs_f64(2.5);
    let poll_interval = if poll_interval.is_zero() {
        Duration::from_secs(15)
    } else {
        poll_interval
    };

    let sleep = tokio::time::sleep(poll_interval);
    tokio::pin!(sleep);

    let mut poll_count: u64 = 0;
    let mut last_lat_lng: Option<(f64, f64)> = None;
    let mut prev_car_version: Option<String> = None;
    let mut drive_session: Option<DriveSession> = None;
    let mut charge_session: Option<ChargeSession> = None;
    let mut last_charger_power: Option<i64> = None;
    let mut update_session: Option<UpdateSession> = None;

    let car_settings = settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .settings
        .cars
        .get(vin)
        .cloned()
        .unwrap_or_default();
    let suspend_after_idle_min = Duration::from_secs(car_settings.suspend_after_idle_minutes * 60);
    let suspend_minimum_min = Duration::from_secs(car_settings.suspend_minimum_minutes * 60);
    let require_unlocked = car_settings.require_unlocked_for_wake;

    let mut last_used: Option<tokio::time::Instant> = None;
    let mut last_resume_at: Option<tokio::time::Instant> = None;

    // Decoupled persistence: session fns enqueue line protocol instead of
    // awaiting the network, so a slow database cannot stall this loop.
    let writer = DbWriter::new(Arc::clone(&db), DEFAULT_CAPACITY);

    // Streaming API (per-car opt-in).
    let streaming_enabled = car_settings.use_streaming_api;
    let mut stream: Option<StreamLink> = None;
    let mut last_stream_msg: Option<tokio::time::Instant> = None;

    // Startup: start streaming if enabled and a token is already available.
    // Freshness belongs to the link's lifetime: a new socket must deliver
    // before it counts as fresh (see the reconnect path below).
    if streaming_enabled && let Some(token) = token_rx.borrow().clone() {
        stream = Some(StreamLink::spawn(token, vin, vehicle.vehicle_id));
        last_stream_msg = None;
    }

    loop {
        tokio::select! {
            biased;

            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(VehicleCommand::Shutdown) => {
                        info!(%vin, "vehicle task shutting down");
                        if let Some(s) = stream.take() {
                            s.abort(vin);
                        }
                        break;
                    }
                    Some(VehicleCommand::Suspend) => {
                        if state == VehicleState::Updating {
                            warn!(%vin, "cannot suspend while software update in progress");
                        } else if state == VehicleState::Suspended {
                            // already suspended
                        } else {
                            state = VehicleState::Suspended;
                            state_tx.send(state).ok();
                            set_summary_state(&summaries, vin, state);
                            events.send(UiEvent::state(vin, state)).ok();
                            sleep.as_mut().reset(tokio::time::Instant::now() + poll_interval);
                            info!(%vin, "vehicle logging suspended");
                            if let Some(s) = stream.take() {
                                s.abort(vin);
                            }
                        }
                    }
                    Some(VehicleCommand::Resume) => {
                        if state == VehicleState::Suspended {
                            state = VehicleState::Online;
                            let now = tokio::time::Instant::now();
                            last_used = Some(now);
                            last_resume_at = Some(now);
                            state_tx.send(state).ok();
                            set_summary_state(&summaries, vin, state);
                            events.send(UiEvent::state(vin, state)).ok();
                            info!(%vin, "vehicle logging resumed");
                        }
                    }
                    None => {
                        info!(%vin, "command channel closed, exiting");
                        if let Some(s) = stream.take() {
                            s.abort(vin);
                        }
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
                        poll_count += 1;

                        let shift = data.drive_state.as_ref().and_then(|ds| ds.shift_state.as_deref()).unwrap_or("_");
                        let lat = data.drive_state.as_ref().and_then(|ds| ds.latitude);
                        let lng = data.drive_state.as_ref().and_then(|ds| ds.longitude);
                        let speed = data.drive_state.as_ref().and_then(|ds| ds.speed);

                        info!(
                            %vin,
                            poll = poll_count,
                            state = ?state,
                            shift,
                            lat,
                            lng,
                            speed,
                            api_state = %data.state,
                            odometer = ?data.odometer,
                            battery = ?data.charge_state.as_ref().and_then(|cs| cs.battery_level),
                            "vehicle_data received"
                        );

                        let new_state = derive_next_state(state, &data);
                        if new_state != state && state.can_transition_to(new_state) {
                            state = new_state;
                            state_tx.send(state).ok();
                            set_summary_state(&summaries, vin, state);
                            events.send(UiEvent::state(vin, state)).ok();
                        }

                        if state == VehicleState::Updating && data.state != "online" {
                            warn!(%vin, api_state = %data.state, "vehicle went offline while updating");
                        }

                        let geofences = settings
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .geofences
                            .geofences
                            .clone();
                        session::handle_drive_session(state, &mut drive_session, &writer, &data, vin, &geofences, &last_lat_lng).await;
                        session::handle_charge_session(state, &mut charge_session, &writer, &data, &mut last_charger_power, vin, &geofences, &last_lat_lng).await;
                        session::handle_update_session(state, &mut update_session, &writer, &data, &mut prev_car_version, vin).await;

                        // Reconnect path: after the streaming task ended (offline/
                        // io error), a successful poll while the car is online
                        // restarts it. The poll interval acts as the backoff.
                        // The replacement starts stale: it must deliver before
                        // REST backs off, so a silent new socket cannot inherit
                        // the previous link's freshness.
                        if streaming_enabled
                            && stream.is_none()
                            && data.state == "online"
                            && let Some(token) = token_rx.borrow().clone()
                        {
                            stream = Some(StreamLink::spawn(token, vin, vehicle.vehicle_id));
                            last_stream_msg = None;
                        }

                        if let Some(ref vs) = data.vehicle_state
                            && let Some(ref cv) = vs.car_version
                            && prev_car_version.as_deref() != Some(cv.as_str())
                        {
                            info!(%vin, ?prev_car_version, new = %cv, "car_version changed");
                            prev_car_version = Some(cv.clone());
                        }

                        session::record_position(
                            &mut last_lat_lng,
                            &writer,
                            &data,
                            vin,
                            vehicle.vehicle_id,
                            state == VehicleState::Driving,
                        )
                        .await;

                        // Refresh the cached UI summary + notify SSE subscribers.
                        let summary = VehicleSummary::from_data(
                            &vehicle,
                            state,
                            &data,
                            now_unix(),
                        );
                        summaries
                            .write()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(vin.clone(), summary.clone());
                        events.send(UiEvent::summary(summary)).ok();

                        // Auto-suspend check
                        if !matches!(state, VehicleState::Driving | VehicleState::Charging | VehicleState::Updating) {
                            match can_fall_asleep(&data, require_unlocked) {
                                Err(reason) => {
                                    last_used = Some(tokio::time::Instant::now());
                                    trace!(%vin, reason, "activity detected, resetting idle timer");
                                }
                                Ok(()) => {
                                    let now = tokio::time::Instant::now();
                                    let idle_duration = last_used.map(|t| now - t).unwrap_or(Duration::ZERO);
                                    let since_resume = last_resume_at
                                        .map(|t| now - t)
                                        .unwrap_or(Duration::MAX);
                                    if idle_duration >= suspend_after_idle_min
                                        && since_resume >= suspend_minimum_min
                                    {
                                        state = VehicleState::Suspended;
                                        last_used = None;
                                        state_tx.send(state).ok();
                                        set_summary_state(&summaries, vin, state);
                                        events.send(UiEvent::state(vin, state)).ok();
                                        info!(%vin, "auto-suspended after idle timeout");
                                        sleep.as_mut().reset(tokio::time::Instant::now() + poll_interval);
                                        if let Some(s) = stream.take() {
                                            s.abort(vin);
                                        }
                                        continue;
                                    }
                                    if last_used.is_none() {
                                        last_used = Some(now);
                                    }
                                }
                            }
                        } else {
                            last_used = Some(tokio::time::Instant::now());
                        }
                    }
                    Err(e) => {
                        warn!(%vin, error = %e, "vehicle_data poll failed");
                    }
                }

                let fresh = stream.is_some()
                    && stream_is_fresh(last_stream_msg, tokio::time::Instant::now());
                let next = next_poll_interval(
                    state,
                    last_charger_power,
                    poll_interval,
                    driving_interval,
                    fresh,
                );
                sleep.as_mut().reset(tokio::time::Instant::now() + next);
            }

            _ = token_rx.changed() => {
                let has_token = token_rx.borrow().is_some();
                info!(%vin, has_token, "token updated");
                // No streaming restart here: a rotated token ends the current
                // stream with TokenExpired, and the reconnect path on the next
                // successful poll restarts it with the new token.
            }

            stream_data = OptionFuture::from(stream.as_mut().map(|s| s.data_rx.recv())), if stream.is_some() => {
                match stream_data {
                    Some(Some(data)) => {
                        last_stream_msg = Some(tokio::time::Instant::now());
                        session::update_drive_session_from_streaming(state, &mut drive_session, &data);
                        session::record_streaming_position(
                            &mut last_lat_lng,
                            &writer,
                            &data,
                            vin,
                            vehicle.vehicle_id,
                            state == VehicleState::Driving,
                        )
                        .await;
                        // Live card update: patch cached GPS/speed/odometer.
                        if let Some(updated) = patch_summary_from_stream(
                            &summaries,
                            vin,
                            state,
                            &data,
                        ) {
                            events.send(UiEvent::summary(updated)).ok();
                        }
                    }
                    Some(None) => {
                        // channel closed: the task ended (reason logged there)
                        stream = None;
                    }
                    None => {}
                }
            }
        }
    }

    // Bounded drain of queued writes before exit so a restart does not lose
    // summaries that already closed on earlier ticks. Note: a session still
    // open at shutdown is dropped without a close summary (pre-existing
    // behavior — see the graceful-shutdown issue). Stale telemetry is
    // skipped; only session records are still written.
    writer.shutdown().await;
    info!(
        %vin,
        dropped_telemetry = writer.dropped_telemetry(),
        "vehicle task exited"
    );
}
