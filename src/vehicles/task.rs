use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tracing::{info, trace, warn};

use crate::config_yaml::YamlConfigManager;
use crate::influxdb::InfluxDb;
use crate::tesla_api::Vehicle;
use crate::vehicles::VehicleCommand;
use crate::vehicles::session::{self, ChargeSession, DriveSession, UpdateSession};
use crate::vehicles::sleep::can_fall_asleep;
use crate::vehicles::state::{VehicleState, derive_next_state};

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
                        if state == VehicleState::Updating {
                            warn!(%vin, "cannot suspend while software update in progress");
                        } else if state == VehicleState::Suspended {
                            // already suspended
                        } else {
                            state = VehicleState::Suspended;
                            state_tx.send(state).ok();
                            sleep.as_mut().reset(tokio::time::Instant::now() + poll_interval);
                            info!(%vin, "vehicle logging suspended");
                        }
                    }
                    Some(VehicleCommand::Resume) => {
                        if state == VehicleState::Suspended {
                            state = VehicleState::Online;
                            let now = tokio::time::Instant::now();
                            last_used = Some(now);
                            last_resume_at = Some(now);
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
                        }

                        if state == VehicleState::Updating && data.state != "online" {
                            warn!(%vin, api_state = %data.state, "vehicle went offline while updating");
                        }

                        session::handle_drive_session(state, &mut drive_session, &db, &data, vin).await;
                        session::handle_charge_session(state, &mut charge_session, &db, &data, &mut last_charger_power, vin).await;
                        session::handle_update_session(state, &mut update_session, &db, &data, &mut prev_car_version, vin).await;

                        if let Some(ref vs) = data.vehicle_state
                            && let Some(ref cv) = vs.car_version
                            && prev_car_version.as_deref() != Some(cv.as_str())
                        {
                            info!(%vin, ?prev_car_version, new = %cv, "car_version changed");
                            prev_car_version = Some(cv.clone());
                        }

                        session::record_position(&mut last_lat_lng, &db, &data, vin, vehicle.vehicle_id).await;

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
                                        info!(%vin, "auto-suspended after idle timeout");
                                        sleep.as_mut().reset(tokio::time::Instant::now() + poll_interval);
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

                let next = match state {
                    VehicleState::Driving => driving_interval,
                    VehicleState::Charging => session::charging_poll_interval(last_charger_power),
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
