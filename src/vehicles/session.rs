use std::time::Duration;

use influxdb::{InfluxDbWriteable, Timestamp};
use tracing::{info, warn};

use crate::config_yaml::Geofence;
use crate::influxdb::InfluxDb;
use crate::tesla_api::VehicleDataResponse;
use crate::vehicles::state::VehicleState;

/// Tracks accumulated data for a single drive.
pub(crate) struct DriveSession {
    pub(crate) drive_id: String,
    pub(crate) start_time: i64,
    pub(crate) start_local_ts: u64,
    pub(crate) last_poll_ts: u64,
    pub(crate) start_lat: f64,
    pub(crate) start_lng: f64,
    pub(crate) prev_lat: f64,
    pub(crate) prev_lng: f64,
    pub(crate) distance_meters: f64,
    pub(crate) energy_used_wh: f64,
    pub(crate) max_speed: f64,
    pub(crate) speed_sum: f64,
    pub(crate) speed_count: u64,
    pub(crate) outside_temp_sum: f64,
    pub(crate) outside_temp_count: u64,
    pub(crate) inside_temp_sum: f64,
    pub(crate) inside_temp_count: u64,
}

/// Tracks accumulated data for a single charging session.
pub(crate) struct ChargeSession {
    pub(crate) charge_id: String,
    pub(crate) start_time: i64,
    pub(crate) start_local_ts: u64,
    pub(crate) last_poll_ts: u64,
    pub(crate) start_lat: f64,
    pub(crate) start_lng: f64,
    pub(crate) start_battery_level: i64,
    pub(crate) start_range: f64,
    pub(crate) start_rated_range: f64,
    pub(crate) first_energy_added_kwh: f64,
    pub(crate) max_energy_added_kwh: f64,
    pub(crate) energy_used_wh: f64,
    pub(crate) outside_temp_sum: f64,
    pub(crate) outside_temp_count: u64,
    pub(crate) inside_temp_sum: f64,
    pub(crate) inside_temp_count: u64,
}

/// Tracks an in-progress software update.
pub(crate) struct UpdateSession {
    pub(crate) update_id: String,
    pub(crate) version_before: Option<String>,
    pub(crate) install_start: String,
    pub(crate) update_id_ts: u128,
}

/// Haversine distance in meters between two lat/lng points.
pub(crate) fn haversine_distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let d_lat = (lat2 - lat1).to_radians();
    let d_lon = (lon2 - lon1).to_radians();
    let a = (d_lat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (d_lon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    const R: f64 = 6_371_000.0;
    R * c
}

/// Find the first geofence containing the given (lat, lng).
pub(crate) fn matching_geofence(lat: f64, lng: f64, geofences: &[Geofence]) -> Option<&Geofence> {
    geofences
        .iter()
        .find(|g| haversine_distance(lat, lng, g.latitude, g.longitude) <= g.radius_meters)
}

/// `SystemTime::now()` as seconds since unix epoch (for local clock timing).
pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Poll interval during charging (Elixir-compatible: 5-20s based on charger_power).
pub(crate) fn charging_poll_interval(power_kw: Option<i64>) -> Duration {
    match power_kw {
        Some(p) if p > 0 => {
            let secs = (250.0 / p as f64).round().clamp(5.0, 20.0);
            Duration::from_secs_f64(secs)
        }
        _ => Duration::from_secs(5),
    }
}

pub(crate) async fn handle_drive_session(
    state: VehicleState,
    drive_session: &mut Option<DriveSession>,
    db: &InfluxDb,
    data: &VehicleDataResponse,
    vin: &str,
    geofences: &[Geofence],
) {
    if state == VehicleState::Driving {
        if drive_session.is_none() {
            if let Some(ref ds) = data.drive_state {
                let ts = ds.timestamp.map(|ms| ms / 1000).unwrap_or_else(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64
                });
                let lat = ds.latitude.unwrap_or(0.0);
                let lng = ds.longitude.unwrap_or(0.0);
                let drive_id = format!("{vin}_{ts}");

                *drive_session = Some(DriveSession {
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

                let start_time_iso =
                    chrono::DateTime::from_timestamp(ts, 0).map(|dt| dt.to_rfc3339());

                let ts_secs = if ts == 0 {
                    now_secs() as u128
                } else {
                    ts as u128
                };

                let initial_drive = crate::influxdb::Drive {
                    time: Timestamp::Seconds(ts_secs),
                    vin: vin.to_string(),
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

                info!(%vin, lat, lng, "drive_session: STARTED");
                if let Err(e) = db.write_query(initial_drive.into_query("drives")).await {
                    warn!(%vin, error = %e, "drive_session: initial write FAILED");
                }
            } else {
                info!(%vin, "drive_session: cannot start (no drive_state in response)");
            }
        } else if let Some(ref ds) = data.drive_state
            && let Some(session) = drive_session
        {
            if let (Some(lat), Some(lng)) = (ds.latitude, ds.longitude) {
                let d = haversine_distance(session.prev_lat, session.prev_lng, lat, lng);
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
                let now = now_secs();
                let dt = now.saturating_sub(session.last_poll_ts);
                session.energy_used_wh += power as f64 * dt as f64 / 3600.0;
                session.last_poll_ts = now;
            }
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
        }
    } else if let Some(session) = drive_session.take() {
        let end_ts = now_secs();
        let end_lat = data.drive_state.as_ref().and_then(|ds| ds.latitude);
        let end_lng = data.drive_state.as_ref().and_then(|ds| ds.longitude);
        let duration_secs = end_ts.saturating_sub(session.start_local_ts);

        let end_time_iso =
            chrono::DateTime::from_timestamp(end_ts as i64, 0).map(|dt| dt.to_rfc3339());

        let avg_speed = if session.speed_count > 0 {
            Some(session.speed_sum / session.speed_count as f64)
        } else {
            None
        };

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

        let start_address =
            crate::geocode::resolve_address(session.start_lat, session.start_lng).await;
        let end_address = match (end_lat, end_lng) {
            (Some(lat), Some(lng)) => crate::geocode::resolve_address(lat, lng).await,
            _ => None,
        };

        let final_drive = crate::influxdb::Drive {
            time: Timestamp::Seconds(ts_secs),
            vin: vin.to_string(),
            drive_id: session.drive_id,
            start_lat: session.start_lat,
            start_lng: session.start_lng,
            end_lat,
            end_lng,
            start_address,
            end_address,
            start_time: None,
            end_time: end_time_iso,
            distance_meters: Some(session.distance_meters),
            duration_seconds: Some(duration_secs as i64),
            energy_used_wh: Some(session.energy_used_wh),
            max_speed: Some(session.max_speed),
            average_speed: avg_speed,
            outside_temp_avg: avg_outside_temp,
            inside_temp_avg: avg_inside_temp,
            geofence_enter: matching_geofence(session.start_lat, session.start_lng, geofences)
                .map(|g| g.name.clone()),
            geofence_exit: end_lat.and_then(|el| {
                end_lng.and_then(|en| matching_geofence(el, en, geofences).map(|g| g.name.clone()))
            }),
            is_merged: None,
        };

        info!(
            %vin,
            distance_m = session.distance_meters,
            duration_s = duration_secs,
            max_speed = session.max_speed,
            "drive_session: CLOSED"
        );
        if let Err(e) = db.write_query(final_drive.into_query("drives")).await {
            warn!(%vin, error = %e, "drive_session: final write FAILED");
        }
    }
}

pub(crate) async fn handle_charge_session(
    state: VehicleState,
    charge_session: &mut Option<ChargeSession>,
    db: &InfluxDb,
    data: &VehicleDataResponse,
    last_charger_power: &mut Option<i64>,
    vin: &str,
    geofences: &[Geofence],
) {
    if state == VehicleState::Charging {
        if charge_session.is_none() {
            if let Some(ref cs) = data.charge_state {
                let ts = now_secs() as i64;

                let lat = data
                    .drive_state
                    .as_ref()
                    .and_then(|ds| ds.latitude)
                    .unwrap_or(0.0);
                let lng = data
                    .drive_state
                    .as_ref()
                    .and_then(|ds| ds.longitude)
                    .unwrap_or(0.0);
                let charge_id = format!("{vin}_{ts}");

                let energy_added = cs.charge_energy_added.unwrap_or(0.0);

                *charge_session = Some(ChargeSession {
                    charge_id: charge_id.clone(),
                    start_time: ts,
                    start_local_ts: now_secs(),
                    last_poll_ts: now_secs(),
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
                    now_secs() as u128
                } else {
                    ts as u128
                };

                let initial_session = crate::influxdb::ChargingSession {
                    time: Timestamp::Seconds(ts_secs),
                    vin: vin.to_string(),
                    charge_id,
                    start_lat: lat,
                    start_lng: lng,
                    end_lat: None,
                    end_lng: None,
                    start_address: None,
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

                info!(%vin, battery = ?cs.battery_level, "charge_session: STARTED");
                if let Err(e) = db
                    .write_query(initial_session.into_query("charging_sessions"))
                    .await
                {
                    warn!(%vin, error = %e, "charge_session: initial write FAILED");
                }

                *last_charger_power = cs.charger_power;
            }
        } else if let Some(ref cs) = data.charge_state
            && let Some(session) = charge_session
        {
            let now = now_secs();
            let dt = now.saturating_sub(session.last_poll_ts);
            session.last_poll_ts = now;

            *last_charger_power = cs.charger_power;

            let current_energy = cs.charge_energy_added.unwrap_or(0.0);
            if current_energy > session.max_energy_added_kwh {
                session.max_energy_added_kwh = current_energy;
            }

            let energy_wh = if let Some(phases) = cs.charger_phases
                && let (Some(current), Some(voltage)) =
                    (cs.charger_actual_current, cs.charger_voltage)
            {
                current as f64 * voltage as f64 * phases as f64 * dt as f64 / 3600.0
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

            let ts_secs = now_secs() as u128;

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
                vin: vin.to_string(),
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
                outside_temp: data.climate_state.as_ref().and_then(|cl| cl.outside_temp),
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

            if let Err(e) = db.write_query(reading.into_query("charge_readings")).await {
                warn!(%vin, error = %e, "charge_readings: WRITE FAILED");
            } else {
                info!(%vin, battery = ?cs.battery_level, power = ?cs.charger_power, "charge_readings: WRITTEN");
            }
        }
    } else if let Some(session) = charge_session.take() {
        let end_ts = now_secs();
        let end_lat = data.drive_state.as_ref().and_then(|ds| ds.latitude);
        let end_lng = data.drive_state.as_ref().and_then(|ds| ds.longitude);
        let duration_secs = end_ts.saturating_sub(session.start_local_ts);

        let latest_energy = data
            .charge_state
            .as_ref()
            .and_then(|cs| cs.charge_energy_added)
            .filter(|&v| v != 0.0)
            .unwrap_or(session.max_energy_added_kwh);
        let energy_added_wh = (latest_energy - session.first_energy_added_kwh).max(0.0) * 1000.0;

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

        let connector_type = data
            .charge_state
            .as_ref()
            .and_then(|cs| cs.conn_charge_cable.clone());

        let start_address =
            crate::geocode::resolve_address(session.start_lat, session.start_lng).await;
        let geofence_name = end_lat.and_then(|el| {
            end_lng.and_then(|en| matching_geofence(el, en, geofences).map(|g| g.name.clone()))
        });

        let final_session = crate::influxdb::ChargingSession {
            time: Timestamp::Seconds(ts_secs),
            vin: vin.to_string(),
            charge_id: session.charge_id,
            start_lat: session.start_lat,
            start_lng: session.start_lng,
            end_lat,
            end_lng,
            start_address,
            start_range: Some(session.start_range),
            end_range: data
                .charge_state
                .as_ref()
                .and_then(|cs| cs.ideal_battery_range),
            start_rated_range: Some(session.start_rated_range),
            end_rated_range: data.charge_state.as_ref().and_then(|cs| cs.battery_range),
            start_battery_level: Some(session.start_battery_level),
            end_battery_level: data.charge_state.as_ref().and_then(|cs| cs.battery_level),
            energy_added_wh: Some(energy_added_wh),
            duration_seconds: Some(duration_secs as i64),
            cost: None,
            geofence_id: geofence_name.clone(),
            geofence_name,
            charge_energy_used: Some(session.energy_used_wh),
            connector_type,
            outside_temp_avg: avg_outside_temp,
            inside_temp_avg: avg_inside_temp,
        };

        info!(
            %vin,
            energy_added_wh,
            duration_s = duration_secs,
            battery_start = session.start_battery_level,
            "charge_session: CLOSED"
        );
        if let Err(e) = db
            .write_query(final_session.into_query("charging_sessions"))
            .await
        {
            warn!(%vin, error = %e, "charge_session: final write FAILED");
        }

        *last_charger_power = None;
    }
}

pub(crate) async fn handle_update_session(
    state: VehicleState,
    update_session: &mut Option<UpdateSession>,
    db: &InfluxDb,
    data: &VehicleDataResponse,
    prev_car_version: &mut Option<String>,
    vin: &str,
) {
    if state == VehicleState::Updating {
        if update_session.is_none() {
            let ts = now_secs();
            let update_id = format!("{vin}_{ts}");

            let version_before = data
                .vehicle_state
                .as_ref()
                .and_then(|vs| vs.car_version.clone())
                .or_else(|| prev_car_version.clone());

            let install_start = chrono::DateTime::from_timestamp(ts as i64, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();

            *update_session = Some(UpdateSession {
                update_id: update_id.clone(),
                version_before: version_before.clone(),
                install_start: install_start.clone(),
                update_id_ts: ts as u128,
            });

            let initial_update = crate::influxdb::Update {
                time: Timestamp::Seconds(ts as u128),
                vin: vin.to_string(),
                update_id,
                version_before: version_before.clone(),
                version_after: None,
                install_start: Some(install_start.clone()),
                install_end: None,
                status: Some("installing".into()),
                abandoned: Some(false),
            };

            info!(%vin, from = ?version_before, "update_session: STARTED");
            if let Err(e) = db.write_query(initial_update.into_query("updates")).await {
                warn!(%vin, error = %e, "update_session: initial write FAILED");
            }
        }
    } else if let Some(session) = update_session.take() {
        let version_after = data
            .vehicle_state
            .as_ref()
            .and_then(|vs| vs.car_version.clone());

        let install_end = chrono::DateTime::from_timestamp(now_secs() as i64, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();

        let is_cancelled = data
            .vehicle_state
            .as_ref()
            .and_then(|vs| vs.software_update.as_ref())
            .and_then(|su| su.status.as_deref())
            == Some("available");

        let status = if is_cancelled {
            "cancelled"
        } else {
            "completed"
        };

        let update_id = session.update_id.clone();
        let version_before = session.version_before.clone();
        let install_start = session.install_start.clone();

        let final_update = crate::influxdb::Update {
            time: Timestamp::Seconds(session.update_id_ts),
            vin: vin.to_string(),
            update_id,
            version_before: version_before.clone(),
            version_after,
            install_start: Some(install_start.clone()),
            install_end: Some(install_end),
            status: Some(status.into()),
            abandoned: Some(false),
        };

        info!(%vin, status, "update_session: CLOSED");
        match db.write_query(final_update.into_query("updates")).await {
            Ok(_) => {}
            Err(e) => {
                warn!(%vin, error = %e, "update_session: final write FAILED");
                *update_session = Some(session);
            }
        }
    }
}

pub(crate) async fn record_position(
    last_lat_lng: &mut Option<(f64, f64)>,
    db: &InfluxDb,
    data: &VehicleDataResponse,
    vin: &str,
    vehicle_id: i64,
) {
    let (lat, lng) = match data
        .drive_state
        .as_ref()
        .and_then(|ds| ds.latitude.zip(ds.longitude))
    {
        Some((lat, lng)) => (lat, lng),
        None => {
            info!(%vin, "positions: SKIPPED (no gps coords in drive_state)");
            return;
        }
    };

    if let Some((pl, pn)) = *last_lat_lng
        && pl == lat
        && pn == lng
    {
        info!(%vin, lat, lng, "positions: SKIPPED (coords unchanged)");
        return;
    }

    let (
        battery_level,
        rated_battery_range_km,
        ideal_battery_range_km,
        est_battery_range_km,
        usable_battery_level,
        battery_heater_on,
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
        inside_temp,
        outside_temp,
        fan_status,
        front_def,
        rear_def,
        is_climate_on,
        driver_temp,
        passenger_temp,
        battery_heater,
        battery_heater_no_power,
    ) = data
        .climate_state
        .as_ref()
        .map(|cl| {
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
        })
        .unwrap_or((None, None, None, None, None, None, None, None, None, None));

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

    let ds = data.drive_state.as_ref().unwrap();
    let elevation = match ds.elevation {
        Some(e) => Some(e),
        None => crate::elevation::resolve_elevation(lat, lng).await,
    };

    let pos = crate::influxdb::Position {
        time: Timestamp::Seconds(ds.timestamp.map_or_else(
            || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as u128
            },
            |t| (t / 1000) as u128,
        )),
        vin: vin.to_string(),
        car_id: vehicle_id,
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
        elevation,
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

    match db.write_query(pos.into_query("positions")).await {
        Ok(_) => {
            info!(%vin, lat, lng, speed = ?ds.speed, "positions: WRITTEN");
            *last_lat_lng = Some((lat, lng));
        }
        Err(e) => {
            warn!(%vin, error = %e, "positions: WRITE FAILED");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> Geofence {
        Geofence {
            name: "Home".into(),
            latitude: 37.7749,
            longitude: -122.4194,
            radius_meters: 100.0,
            billing: None,
        }
    }

    fn work() -> Geofence {
        Geofence {
            name: "Work".into(),
            latitude: 37.7749,
            longitude: -122.4194,
            radius_meters: 200.0,
            billing: None,
        }
    }

    #[test]
    fn matching_geofence_inside_radius() {
        let geofences = vec![home()];
        // ~50m from home center → inside 100m radius
        let result = matching_geofence(37.7749 + 0.00045, -122.4194, &geofences);
        assert_eq!(result.map(|g| g.name.as_str()), Some("Home"));
    }

    #[test]
    fn matching_geofence_outside_radius() {
        let geofences = vec![home()];
        // ~200m from home center → outside 100m radius
        let result = matching_geofence(37.7749 + 0.0018, -122.4194, &geofences);
        assert!(result.is_none());
    }

    #[test]
    fn matching_geofence_returns_first_match() {
        let geofences = vec![home(), work()];
        // Near home center → Home (first match)
        let result = matching_geofence(37.7749, -122.4194, &geofences);
        assert_eq!(result.map(|g| g.name.as_str()), Some("Home"));
    }

    #[test]
    fn matching_geofence_empty_list() {
        let geofences: Vec<Geofence> = vec![];
        let result = matching_geofence(37.7749, -122.4194, &geofences);
        assert!(result.is_none());
    }

    #[test]
    fn matching_geofence_at_center() {
        let geofences = vec![home()];
        // Exactly at the center → inside (distance=0)
        let result = matching_geofence(37.7749, -122.4194, &geofences);
        assert_eq!(result.map(|g| g.name.as_str()), Some("Home"));
    }
}
