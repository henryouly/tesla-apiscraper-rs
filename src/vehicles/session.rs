use std::time::Duration;

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
