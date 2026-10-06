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

/// Cadence of the cheap state-only check while suspended
/// (`GET /api/1/vehicles/{id}`, which never wakes the car). Matches
/// upstream's streaming default of 10 minutes; a single cadence covers
/// both streaming and non-streaming cars.
const SUSPENDED_CHECK_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Whether a streaming point shows the car in use (drive start while
/// suspended). Mirrors upstream's "Suspended / Start of drive" trigger.
pub(crate) fn stream_shows_activity(data: &StreamingData) -> bool {
    data.shift_state
        .as_deref()
        .is_some_and(|s| s == "D" || s == "R")
        || data.speed.is_some_and(|v| v > 0.0)
}

/// Whether a full poll response shows the car in use (driving or charging)
/// — the escalation exit from a suspended state-only check.
pub(crate) fn poll_shows_activity(data: &crate::tesla_api::VehicleDataResponse) -> bool {
    data.drive_state.as_ref().is_some_and(|ds| {
        ds.shift_state
            .as_deref()
            .is_some_and(|s| s == "D" || s == "R")
            || ds.speed.unwrap_or(0.0) > 0.0
    }) || data.charge_state.as_ref().is_some_and(|cs| {
        cs.charging_state
            .as_deref()
            .is_some_and(|s| s == "Starting" || s == "Charging")
    })
}

/// Map a cheap state-only check to a resting state after a failed full
/// poll. `None` means "no signal": a failed `vehicle_data` against a car
/// that still reports online is transient and must not move the state
/// machine (otherwise every blip would flap the card).
pub(crate) fn resting_state_from_api(api_state: &str) -> Option<VehicleState> {
    if api_state.eq_ignore_ascii_case("offline") {
        Some(VehicleState::Offline)
    } else if api_state.eq_ignore_ascii_case("asleep") {
        Some(VehicleState::Asleep)
    } else {
        None
    }
}

/// Whether a `vehicle_data` failure means the car itself is unreachable
/// (as opposed to a transient failure).
///
/// Upstream parity (`Tesla.Api.Vehicle.handle_response`): only HTTP 408
/// with a "vehicle unavailable" error qualifies. The body is matched with
/// `contains` rather than a prefix because our error carries the raw
/// response text (usually JSON like
/// `{"response":null,"error":"vehicle unavailable: ..."}`), whereas
/// upstream matches the parsed `error` field.
pub(crate) fn is_vehicle_unavailable(err: &crate::tesla_auth::AuthError) -> bool {
    matches!(
        err,
        crate::tesla_auth::AuthError::Api { status: 408, body }
            if body.contains("vehicle unavailable")
    )
}

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
/// yet (no successful poll so far). Recomputes `geofence_name` whenever the
/// point carries fresh GPS, so a fence crossed between REST polls does not
/// publish a stale name until the next poll; points without GPS leave the
/// last known fence untouched.
fn patch_summary_from_stream(
    summaries: &SummaryStore,
    vin: &str,
    state: VehicleState,
    data: &StreamingData,
    geofences: &[crate::config_yaml::Geofence],
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
    if let Some((la, ln)) = data.latitude.zip(data.longitude) {
        s.geofence_name = crate::vehicles::session::fence_for(la, ln, geofences);
    }
    s.state = state;
    s.last_updated_at = now_unix();
    Some(s.clone())
}

/// Fresh per-car settings for this poll tick. PUTs take effect without a
/// restart because the loop re-reads instead of using a startup snapshot.
fn car_settings_for(
    settings: &Arc<Mutex<YamlConfigManager>>,
    vin: &str,
) -> crate::config_yaml::CarSettings {
    settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .settings
        .cars
        .get(vin)
        .cloned()
        .unwrap_or_default()
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

    // Start from the discovery state (not unconditionally Online) and
    // publish it before any await, so the task agrees with the seeded
    // summary from the first microsecond — including while blocked on the
    // DB seed query or the token gate below.
    let mut state = crate::vehicle_summary::discovery_state(&vehicle.state);
    state_tx.send(state).ok();

    // Seed from last-known InfluxDB telemetry before the first poll, so an
    // asleep car shows its previous battery/GPS. Needs no token. Only fills
    // an entry with no telemetry yet — a respawn after earlier live data
    // keeps the live row.
    {
        if let Some(seed) = crate::vehicle_summary::last_known_summary(&db, &vehicle, state).await {
            let mut guard = summaries.write().unwrap_or_else(|e| e.into_inner());
            let dominated = guard.get(vin).is_some_and(|s| s.has_telemetry());
            if !dominated {
                guard.insert(vin.clone(), seed.clone());
                drop(guard);
                events.send(UiEvent::summary(seed)).ok();
            }
        }
    }

    if token_rx.borrow().is_none() {
        info!(%vin, "waiting for access token");
        if token_rx.changed().await.is_err() {
            warn!(%vin, "token channel closed, exiting");
            return;
        }
    }

    let driving_interval = Duration::from_secs_f64(2.5);
    let poll_interval = if poll_interval.is_zero() {
        Duration::from_secs(15)
    } else {
        poll_interval
    };

    let sleep = tokio::time::sleep(poll_interval);
    tokio::pin!(sleep);

    let mut poll_count: u64 = 0;
    let mut first_poll = true;
    let mut last_lat_lng: Option<(f64, f64)> = None;
    let mut prev_car_version: Option<String> = None;
    let mut drive_session: Option<DriveSession> = None;
    let mut charge_session: Option<ChargeSession> = None;
    let mut last_charger_power: Option<i64> = None;
    let mut update_session: Option<UpdateSession> = None;

    // Streaming flag at startup only; the loop re-reads settings per tick
    // (see car_settings_for) so PUTs take effect without a restart.
    let streaming_at_startup = car_settings_for(&settings, vin).use_streaming_api;

    let mut last_used: Option<tokio::time::Instant> = None;
    let mut last_resume_at: Option<tokio::time::Instant> = None;
    // Last cheap state-only check while suspended. Reset on every suspend
    // entry so checks start one full interval after suspending (a manual
    // suspend of a parked-online car must not resume on the next tick).
    let mut last_suspend_check = tokio::time::Instant::now();

    // Decoupled persistence: session fns enqueue line protocol instead of
    // awaiting the network, so a slow database cannot stall this loop.
    let writer = DbWriter::new(Arc::clone(&db), DEFAULT_CAPACITY);

    // Streaming API (per-car opt-in).
    let mut stream: Option<StreamLink> = None;
    let mut last_stream_msg: Option<tokio::time::Instant> = None;

    // Startup: start streaming if enabled and a token is already available.
    // Freshness belongs to the link's lifetime: a new socket must deliver
    // before it counts as fresh (see the reconnect path below).
    if streaming_at_startup && let Some(token) = token_rx.borrow().clone() {
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
                            // Upstream parity: the stream stays connected
                            // across suspend so a drive start can resume
                            // logging (see the stream arm); the suspend timer
                            // below runs cheap state-only checks instead.
                            last_suspend_check = tokio::time::Instant::now();
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
                // Re-read per-tick settings so PUTs take effect without a
                // restart (see car_settings_for).
                let tick = car_settings_for(&settings, vin);
                if !tick.enabled
                    && state != VehicleState::Suspended
                    && crate::vehicles::cannot_suspend_state(&state).is_none()
                {
                    // Manual-suspend parity, including its guard. Unsafe
                    // states deliberately fall through to the normal poll
                    // below: skipping the tick here would stop observing
                    // the car, so it could never reach a safe state and
                    // pending sessions would never finalize.
                    state = VehicleState::Suspended;
                    state_tx.send(state).ok();
                    set_summary_state(&summaries, vin, state);
                    events.send(UiEvent::state(vin, state)).ok();
                    info!(%vin, "vehicle disabled, logging suspended");
                    // Entry only marks the state: the Suspended arm below
                    // enforces full darkness for disabled cars (link abort,
                    // no checks). The check timer restarts so a later
                    // re-enable starts clean.
                    last_suspend_check = tokio::time::Instant::now();
                }
                if state == VehicleState::Suspended {
                    // Both user switches are honored first: a disabled car
                    // stays fully dark, and a streaming opt-out kills any
                    // lingering link (the toggle-off abort below is
                    // otherwise unreachable while suspended).
                    if !tick.enabled {
                        if let Some(s) = stream.take() {
                            s.abort(vin);
                        }
                        sleep.as_mut().reset(tokio::time::Instant::now() + poll_interval);
                        continue;
                    }
                    if !tick.use_streaming_api && let Some(s) = stream.take() {
                        s.abort(vin);
                    }
                    // Low-power watch while suspended (upstream parity): the
                    // state-only endpoint never wakes the car. Escalate to
                    // one full poll only when the car already reports
                    // online; a parked car stays quiet. Reconnect a dead
                    // stream on the same cadence — without polls it would
                    // otherwise stay dead until manual resume.
                    let now = tokio::time::Instant::now();
                    if now.duration_since(last_suspend_check) >= SUSPENDED_CHECK_INTERVAL {
                        last_suspend_check = now;
                        // Bind before any await: the borrow guard is not
                        // Send (same reason the poll path below binds first).
                        let token = token_rx.borrow().clone();
                        if let Some(token) = token {
                            if tick.use_streaming_api && stream.is_none() {
                                stream = Some(StreamLink::spawn(
                                    token.clone(),
                                    vin,
                                    vehicle.vehicle_id,
                                ));
                                last_stream_msg = None;
                            }
                            match crate::tesla_api::fetch_vehicle_state(
                                &token,
                                &api_url,
                                vehicle.id,
                            )
                            .await
                            {
                                Ok(api_state) if api_state == "online" => {
                                    match crate::tesla_api::fetch_vehicle_data(
                                        &token, &api_url, vehicle.id,
                                    )
                                    .await
                                    {
                                        Ok(data) if poll_shows_activity(&data) => {
                                            state = VehicleState::Online;
                                            last_used = Some(now);
                                            last_resume_at = Some(now);
                                            state_tx.send(state).ok();
                                            set_summary_state(&summaries, vin, state);
                                            events.send(UiEvent::state(vin, state)).ok();
                                            info!(
                                                %vin,
                                                "vehicle logging resumed (suspended check found car in use)"
                                            );
                                            sleep.as_mut().reset(now);
                                            continue;
                                        }
                                        Ok(_) => {
                                            trace!(
                                                %vin,
                                                "suspended check: online but parked, staying suspended"
                                            );
                                        }
                                        Err(e) => {
                                            warn!(%vin, error = %e, "suspended escalation poll failed");
                                        }
                                    }
                                }
                                Ok(api_state) => {
                                    trace!(%vin, api_state, "suspended check: car not online, staying suspended");
                                }
                                Err(e) => {
                                    warn!(%vin, error = %e, "suspended state check failed");
                                }
                            }
                        }
                    }
                    sleep.as_mut().reset(tokio::time::Instant::now() + poll_interval);
                    continue;
                }
                // Streaming toggle honored mid-run: stop a link that is no
                // longer wanted (the reconnect path below starts one that
                // newly is).
                if !tick.use_streaming_api && let Some(s) = stream.take() {
                    s.abort(vin);
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
                        // The discovery-seeded state is a guess; the first
                        // authoritative poll wins unconditionally (e.g. an
                        // asleep-booted car that wakes up already driving —
                        // Asleep -> Driving is not a legal steady-state
                        // transition, so the guard below would stick it).
                        let allowed = first_poll || state.can_transition_to(new_state);
                        if new_state != state && allowed {
                            state = new_state;
                            state_tx.send(state).ok();
                            set_summary_state(&summaries, vin, state);
                            events.send(UiEvent::state(vin, state)).ok();
                        }
                        first_poll = false;

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
                        if tick.use_streaming_api
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
                        let mut summary = VehicleSummary::from_data(
                            &vehicle,
                            state,
                            &data,
                            now_unix(),
                        );
                        summary.geofence_name = match (lat, lng) {
                            (Some(la), Some(ln)) => {
                                crate::vehicles::session::fence_for(la, ln, &geofences)
                            }
                            _ => None,
                        };
                        summaries
                            .write()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(vin.clone(), summary.clone());
                        events.send(UiEvent::summary(summary)).ok();

                        // Auto-suspend check
                        if !matches!(state, VehicleState::Driving | VehicleState::Charging | VehicleState::Updating) {
                            match can_fall_asleep(&data, tick.require_unlocked_for_wake) {
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
                                    let idle_min = Duration::from_secs(
                                        tick.suspend_after_idle_minutes * 60,
                                    );
                                    let min_min = Duration::from_secs(
                                        tick.suspend_minimum_minutes * 60,
                                    );
                                    if idle_duration >= idle_min
                                        && since_resume >= min_min
                                    {
                                        state = VehicleState::Suspended;
                                        last_used = None;
                                        state_tx.send(state).ok();
                                        set_summary_state(&summaries, vin, state);
                                        events.send(UiEvent::state(vin, state)).ok();
                                        info!(%vin, "auto-suspended after idle timeout");
                                        sleep.as_mut().reset(tokio::time::Instant::now() + poll_interval);
                                        // Same as manual suspend: keep the stream,
                                        // start the cheap-check timer.
                                        last_suspend_check = now;
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
                        // Upstream parity (TeslaMate
                        // `fetch_with_reachable_assumption`): only a 408
                        // "vehicle unavailable" earns a state-check fallback;
                        // anything else is transient. The state-only endpoint
                        // never wakes the car: offline/asleep rests the state
                        // (a frozen "Online" card otherwise), still-online
                        // changes nothing.
                        if is_vehicle_unavailable(&e)
                            && let Ok(api_state) =
                                crate::tesla_api::fetch_vehicle_state(
                                    &token, &api_url, vehicle.id,
                                )
                                .await
                                .inspect_err(|e| warn!(
                                    %vin, error = %e,
                                    "vehicle state check failed after poll failure"
                                ))
                            && let Some(rest) = resting_state_from_api(&api_state)
                            && rest != state
                            && (first_poll || state.can_transition_to(rest))
                        {
                            state = rest;
                            state_tx.send(state).ok();
                            set_summary_state(&summaries, vin, state);
                            events.send(UiEvent::state(vin, state)).ok();
                            info!(%vin, ?rest, "vehicle resting (poll failed, state check)");
                        }
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
                        // Gated on enabled: a lingering point in the race
                        // window between a disable PUT and the next tick
                        // must not wake a disabled car into polling.
                        let enabled = car_settings_for(&settings, vin).enabled;
                        if state == VehicleState::Suspended
                            && enabled
                            && stream_shows_activity(&data)
                        {
                            // Upstream parity ("Suspended / Start of
                            // drive"): a drive starting while suspended
                            // resumes logging immediately; the immediate
                            // poll below derives Driving and opens the
                            // session.
                            let now = tokio::time::Instant::now();
                            state = VehicleState::Online;
                            last_used = Some(now);
                            last_resume_at = Some(now);
                            state_tx.send(state).ok();
                            set_summary_state(&summaries, vin, state);
                            events.send(UiEvent::state(vin, state)).ok();
                            info!(
                                %vin,
                                "vehicle logging resumed (stream activity while suspended)"
                            );
                            sleep.as_mut().reset(now);
                        }
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
                        // Re-read geofences per message (same pattern as the
                        // poll arm) so fence edits take effect without a
                        // restart and crossings between polls publish fresh.
                        let geofences = settings
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .geofences
                            .geofences
                            .clone();
                        if let Some(updated) = patch_summary_from_stream(
                            &summaries,
                            vin,
                            state,
                            &data,
                            &geofences,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_yaml::Geofence;
    use crate::streaming::StreamingData;
    use crate::tesla_api::Vehicle;
    use crate::vehicle_summary::new_summary_store;

    fn test_geofence() -> Geofence {
        Geofence {
            name: "Home".into(),
            latitude: 37.7749,
            longitude: -122.4194,
            radius_meters: 100.0,
            billing: None,
        }
    }

    fn stream_point(shift: Option<&str>, speed: Option<f64>) -> StreamingData {
        StreamingData {
            timestamp: 0,
            speed,
            soc: None,
            odometer: None,
            elevation: None,
            heading: None,
            latitude: None,
            longitude: None,
            power: None,
            shift_state: shift.map(str::to_string),
            range: None,
            est_range: None,
        }
    }

    fn stream_point_at(lat: Option<f64>, lng: Option<f64>) -> StreamingData {
        StreamingData {
            timestamp: 0,
            speed: Some(10.0),
            soc: None,
            odometer: None,
            elevation: None,
            heading: None,
            latitude: lat,
            longitude: lng,
            power: None,
            shift_state: Some("D".into()),
            range: None,
            est_range: None,
        }
    }

    fn seed_summary(
        summaries: &SummaryStore,
        vin: &str,
        lat: Option<f64>,
        lng: Option<f64>,
        geofence: Option<String>,
    ) {
        let vehicle = Vehicle {
            id: 1,
            vehicle_id: 1,
            vin: vin.to_string(),
            display_name: None,
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let mut s = VehicleSummary::initial(&vehicle, VehicleState::Driving);
        s.latitude = lat;
        s.longitude = lng;
        s.geofence_name = geofence;
        summaries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(vin.to_string(), s);
    }

    fn poll_data(
        shift: Option<&str>,
        speed: Option<f64>,
        charging: Option<&str>,
    ) -> crate::tesla_api::VehicleDataResponse {
        serde_json::from_value(serde_json::json!({
            "state": "online",
            "drive_state": { "shift_state": shift, "speed": speed },
            "charge_state": { "charging_state": charging },
        }))
        .unwrap()
    }

    #[test]
    fn stream_activity_on_drive_shift() {
        assert!(stream_shows_activity(&stream_point(Some("D"), Some(0.0))));
        assert!(stream_shows_activity(&stream_point(Some("R"), Some(0.0))));
    }

    #[test]
    fn stream_activity_on_speed_without_shift() {
        assert!(stream_shows_activity(&stream_point(None, Some(12.0))));
        assert!(stream_shows_activity(&stream_point(Some("P"), Some(3.0))));
    }

    #[test]
    fn stream_no_activity_when_parked() {
        assert!(!stream_shows_activity(&stream_point(Some("P"), Some(0.0))));
        assert!(!stream_shows_activity(&stream_point(None, None)));
    }

    #[test]
    fn poll_activity_when_driving() {
        assert!(poll_shows_activity(&poll_data(Some("D"), Some(40.0), None)));
        assert!(poll_shows_activity(&poll_data(Some("R"), Some(0.0), None)));
        assert!(poll_shows_activity(&poll_data(None, Some(8.0), None)));
    }

    #[test]
    fn poll_activity_when_charging() {
        assert!(poll_shows_activity(&poll_data(
            Some("P"),
            Some(0.0),
            Some("Charging")
        )));
        assert!(poll_shows_activity(&poll_data(
            Some("P"),
            Some(0.0),
            Some("Starting")
        )));
    }

    #[test]
    fn poll_no_activity_when_parked() {
        assert!(!poll_shows_activity(&poll_data(
            Some("P"),
            Some(0.0),
            Some("Disconnected")
        )));
        assert!(!poll_shows_activity(&poll_data(None, None, None)));
    }

    #[test]
    fn resting_state_maps_offline_and_asleep() {
        assert_eq!(
            resting_state_from_api("offline"),
            Some(VehicleState::Offline)
        );
        assert_eq!(
            resting_state_from_api("OFFLINE"),
            Some(VehicleState::Offline)
        );
        assert_eq!(resting_state_from_api("asleep"), Some(VehicleState::Asleep));
    }

    #[test]
    fn resting_state_ignores_online_and_unknown() {
        assert_eq!(resting_state_from_api("online"), None);
        assert_eq!(resting_state_from_api("whatever"), None);
        assert_eq!(resting_state_from_api(""), None);
    }

    fn api_error(status: u16, body: &str) -> crate::tesla_auth::AuthError {
        crate::tesla_auth::AuthError::Api {
            status,
            body: body.into(),
        }
    }

    #[test]
    fn unavailable_only_on_408_vehicle_unavailable() {
        // Real wire shape: the "vehicle unavailable" marker hides in a
        // JSON body, so the match is a substring, not a prefix.
        assert!(is_vehicle_unavailable(&api_error(
            408,
            r#"{"response":null,"error":"vehicle unavailable: vehicle is offline"}"#
        )));
        assert!(!is_vehicle_unavailable(&api_error(408, "request timeout")));
        assert!(!is_vehicle_unavailable(&api_error(
            500,
            r#"{"error":"vehicle unavailable: vehicle is offline"}"#
        )));
        assert!(!is_vehicle_unavailable(&api_error(500, "internal error")));
        assert!(!is_vehicle_unavailable(
            &crate::tesla_auth::AuthError::RegionDecode("bogus".into())
        ));
    }

    #[test]
    fn suspended_check_interval_is_ten_minutes() {
        assert_eq!(SUSPENDED_CHECK_INTERVAL, Duration::from_secs(600));
    }

    #[test]
    fn stream_patch_sets_geofence_on_entry() {
        let summaries = new_summary_store();
        seed_summary(&summaries, "VIN", Some(37.78), Some(-122.43), None);
        let fences = vec![test_geofence()];
        let updated = patch_summary_from_stream(
            &summaries,
            "VIN",
            VehicleState::Driving,
            &stream_point_at(Some(37.7749), Some(-122.4194)),
            &fences,
        )
        .unwrap();
        assert_eq!(updated.geofence_name.as_deref(), Some("Home"));
        assert_eq!(updated.latitude, Some(37.7749));
    }

    #[test]
    fn stream_patch_clears_geofence_on_exit() {
        let summaries = new_summary_store();
        seed_summary(
            &summaries,
            "VIN",
            Some(37.7749),
            Some(-122.4194),
            Some("Home".into()),
        );
        let fences = vec![test_geofence()];
        // ~1km away → outside 100m radius.
        let updated = patch_summary_from_stream(
            &summaries,
            "VIN",
            VehicleState::Driving,
            &stream_point_at(Some(37.7849), Some(-122.4194)),
            &fences,
        )
        .unwrap();
        assert!(updated.geofence_name.is_none());
    }

    #[test]
    fn stream_patch_keeps_geofence_without_gps() {
        let summaries = new_summary_store();
        seed_summary(
            &summaries,
            "VIN",
            Some(37.7749),
            Some(-122.4194),
            Some("Home".into()),
        );
        let fences = vec![test_geofence()];
        let updated = patch_summary_from_stream(
            &summaries,
            "VIN",
            VehicleState::Driving,
            &stream_point(Some("D"), Some(10.0)),
            &fences,
        )
        .unwrap();
        assert_eq!(updated.geofence_name.as_deref(), Some("Home"));
        assert_eq!(updated.speed, Some(10.0));
    }

    #[test]
    fn stream_patch_returns_none_without_cached_summary() {
        let summaries = new_summary_store();
        let fences = vec![test_geofence()];
        assert!(
            patch_summary_from_stream(
                &summaries,
                "MISSING",
                VehicleState::Driving,
                &stream_point_at(Some(37.7749), Some(-122.4194)),
                &fences,
            )
            .is_none()
        );
    }
}
