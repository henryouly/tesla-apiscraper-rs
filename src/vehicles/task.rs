use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tracing::{info, trace, warn};

use influxdb::{InfluxDbWriteable, Timestamp};

use crate::config_yaml::YamlConfigManager;
use crate::influxdb::InfluxDb;
use crate::tesla_api::Vehicle;
use crate::vehicles::VehicleCommand;
use crate::vehicles::session::{self, ChargeSession, DriveSession, UpdateSession};
use crate::vehicles::sleep::can_fall_asleep;
use crate::vehicles::state::VehicleState;

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
    state_tx.send(state.clone()).ok();
    let driving_interval = Duration::from_secs_f64(2.5);
    let poll_interval = if poll_interval.is_zero() {
        Duration::from_secs(15)
    } else {
        poll_interval
    };

    let sleep = tokio::time::sleep(poll_interval);
    tokio::pin!(sleep);

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
                            state_tx.send(state.clone()).ok();
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
                            state_tx.send(state.clone()).ok();
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
                            _ => state.clone(),
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

                        let new_state = if state == VehicleState::Charging
                            && new_state != VehicleState::Charging
                        {
                            VehicleState::Online
                        } else {
                            new_state
                        };

                        let new_state = if new_state == VehicleState::Driving || new_state == VehicleState::Charging {
                            if state == VehicleState::Updating {
                                VehicleState::Online
                            } else {
                                new_state
                            }
                        } else {
                            let su_present = data.vehicle_state
                                .as_ref()
                                .and_then(|vs| vs.software_update.as_ref())
                                .is_some();

                            match state {
                                VehicleState::Updating => {
                                    if data.state == "online" {
                                        if su_present {
                                            let still_installing = data.vehicle_state
                                                .as_ref()
                                                .and_then(|vs| vs.software_update.as_ref())
                                                .and_then(|su| su.status.as_deref())
                                                == Some("installing");
                                            if still_installing {
                                                VehicleState::Updating
                                            } else {
                                                VehicleState::Online
                                            }
                                        } else if data.vehicle_state.is_some() {
                                            VehicleState::Online
                                        } else {
                                            VehicleState::Updating
                                        }
                                    } else {
                                        VehicleState::Updating
                                    }
                                }
                                _ => {
                                    if su_present
                                        && data.vehicle_state
                                            .as_ref()
                                            .and_then(|vs| vs.software_update.as_ref())
                                            .and_then(|su| su.status.as_deref())
                                            == Some("installing")
                                    {
                                        let target = VehicleState::Updating;
                                        if state.can_transition_to(target.clone()) {
                                            target
                                        } else {
                                            VehicleState::Online
                                        }
                                    } else {
                                        new_state
                                    }
                                }
                            }
                        };

                        if new_state != state && state.can_transition_to(new_state.clone()) {
                            state = new_state;
                            state_tx.send(state.clone()).ok();
                        }

                        if state == VehicleState::Updating && data.state != "online" {
                            warn!(%vin, api_state = %data.state, "vehicle went offline while updating");
                        }

                        // Drive lifecycle
                        if state == VehicleState::Driving {
                            if drive_session.is_none() {
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
                                        start_local_ts: session::now_secs(),
                                        last_poll_ts: session::now_secs(),
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
                                        session::now_secs() as u128
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
                                if let (Some(lat), Some(lng)) =
                                    (ds.latitude, ds.longitude)
                                {
                                    let d = session::haversine_distance(
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
                                if let Some(power) = ds.power {
                                    let now = session::now_secs();
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
                            let end_ts = session::now_secs();
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

                        // Charge lifecycle
                        if state == VehicleState::Charging {
                            if charge_session.is_none() {
                                if let Some(ref cs) = data.charge_state {
                                    let ts = session::now_secs() as i64;

                                    let lat = data.drive_state.as_ref()
                                        .and_then(|ds| ds.latitude).unwrap_or(0.0);
                                    let lng = data.drive_state.as_ref()
                                        .and_then(|ds| ds.longitude).unwrap_or(0.0);
                                    let charge_id = format!("{vin}_{ts}");

                                    let energy_added = cs.charge_energy_added.unwrap_or(0.0);

                                    charge_session = Some(ChargeSession {
                                        charge_id: charge_id.clone(),
                                        start_time: ts,
                                        start_local_ts: session::now_secs(),
                                        last_poll_ts: session::now_secs(),
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
                                        session::now_secs() as u128
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
                                let now = session::now_secs();
                                let dt = now.saturating_sub(session.last_poll_ts);
                                session.last_poll_ts = now;

                                last_charger_power = cs.charger_power;

                                let current_energy = cs.charge_energy_added.unwrap_or(0.0);
                                if current_energy > session.max_energy_added_kwh {
                                    session.max_energy_added_kwh = current_energy;
                                }

                                let energy_wh = if let Some(phases) = cs.charger_phases
                                    && let (Some(current), Some(voltage)) =
                                        (cs.charger_actual_current, cs.charger_voltage)
                                {
                                    current as f64 * voltage as f64 * phases as f64
                                        * dt as f64 / 3600.0
                                } else if let Some(power) = cs.charger_power {
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

                                let ts_secs = session::now_secs() as u128;

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
                            let end_ts = session::now_secs();
                            let end_lat = data.drive_state.as_ref()
                                .and_then(|ds| ds.latitude);
                            let end_lng = data.drive_state.as_ref()
                                .and_then(|ds| ds.longitude);
                            let duration_secs = end_ts.saturating_sub(session.start_local_ts);

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

                        // Update lifecycle
                        if state == VehicleState::Updating {
                            if update_session.is_none() {
                                let ts = session::now_secs();
                                let update_id = format!("{vin}_{ts}");

                                let version_before = data.vehicle_state
                                    .as_ref()
                                    .and_then(|vs| vs.car_version.clone())
                                    .or_else(|| prev_car_version.clone());

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
                        } else if update_session.is_some() {
                            let session = update_session.take().unwrap();
                            let version_after = data.vehicle_state
                                .as_ref()
                                .and_then(|vs| vs.car_version.clone());

                            let install_end = chrono::DateTime::from_timestamp(session::now_secs() as i64, 0)
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_default();

                            let is_cancelled = data.vehicle_state
                                .as_ref()
                                .and_then(|vs| vs.software_update.as_ref())
                                .and_then(|su| su.status.as_deref())
                                == Some("available");

                            let status = if is_cancelled { "cancelled" } else { "completed" };

                            let update_id = session.update_id.clone();
                            let version_before = session.version_before.clone();
                            let install_start = session.install_start.clone();

                            let final_update = crate::influxdb::Update {
                                time: Timestamp::Seconds(session.update_id_ts),
                                vin: vehicle.vin.clone(),
                                update_id,
                                version_before: version_before.clone(),
                                version_after,
                                install_start: Some(install_start.clone()),
                                install_end: Some(install_end),
                                status: Some(status.into()),
                                abandoned: Some(false),
                            };

                            match db
                                .write_query(final_update.into_query("updates"))
                                .await
                            {
                                Ok(_) => {}
                                Err(e) => {
                                    warn!(%vin, error = %e, "failed to write final update");
                                    update_session = Some(session);
                                }
                            }
                        }

                        if let Some(ref vs) = data.vehicle_state
                            && let Some(ref cv) = vs.car_version
                            && prev_car_version.as_deref() != Some(cv.as_str())
                        {
                            info!(%vin, ?prev_car_version, new = %cv, "car_version changed");
                            prev_car_version = Some(cv.clone());
                        }

                        if let Some(ref ds) = data.drive_state
                            && let (Some(lat), Some(lng)) = (ds.latitude, ds.longitude)
                            && last_lat_lng.is_none_or(|(pl, pn)| pl != lat || pn != lng)
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
                                        state_tx.send(state.clone()).ok();
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
