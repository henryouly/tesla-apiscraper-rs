use crate::vehicles::state::VehicleState;

/// Returns an error message if the current state prevents suspension.
pub fn cannot_suspend_state(state: &VehicleState) -> Option<&'static str> {
    match state {
        VehicleState::Updating => Some("software update in progress"),
        VehicleState::Driving => Some("vehicle is driving"),
        VehicleState::Charging => Some("vehicle is charging"),
        _ => None,
    }
}

/// Checks the latest vehicle data for activity that should prevent suspension.
/// `require_unlocked` — when true, an unlocked car blocks suspension (from settings).
/// Returns `Ok(())` if the vehicle can fall asleep, or `Err(reason)` if activity
/// is detected. When activity is detected the caller should reset the idle timer.
pub fn can_fall_asleep(
    data: &crate::tesla_api::VehicleDataResponse,
    require_unlocked: bool,
) -> Result<(), &'static str> {
    if data
        .vehicle_state
        .as_ref()
        .is_some_and(|vs| vs.is_user_present == Some(true))
    {
        return Err("user_present");
    }
    if data
        .climate_state
        .as_ref()
        .is_some_and(|cl| cl.is_preconditioning == Some(true))
    {
        return Err("preconditioning");
    }
    if data
        .climate_state
        .as_ref()
        .is_some_and(|cl| cl.climate_keeper_mode.as_deref() == Some("dog"))
    {
        return Err("dogmode");
    }
    if data
        .vehicle_state
        .as_ref()
        .is_some_and(|vs| vs.sentry_mode == Some(true))
    {
        return Err("sentry_mode");
    }
    if let Some(ref vs) = data.vehicle_state
        && let Some(ref su) = vs.software_update
        && su.status.as_deref() == Some("downloading")
        && su.download_perc.unwrap_or(0) < 100
    {
        return Err("downloading_update");
    }
    if let Some(ref vs) = data.vehicle_state {
        let df = vs.df.unwrap_or(0.0);
        let pf = vs.pf.unwrap_or(0.0);
        let dr = vs.dr.unwrap_or(0.0);
        let pr = vs.pr.unwrap_or(0.0);
        if df > 0.0 || pf > 0.0 || dr > 0.0 || pr > 0.0 {
            return Err("doors_open");
        }
    }
    if let Some(ref vs) = data.vehicle_state {
        let ft = vs.ft.unwrap_or(0.0);
        let rt = vs.rt.unwrap_or(0.0);
        if ft > 0.0 || rt > 0.0 {
            return Err("trunk_open");
        }
    }
    if require_unlocked
        && data
            .vehicle_state
            .as_ref()
            .is_some_and(|vs| vs.locked == Some(false))
    {
        return Err("unlocked");
    }
    if data
        .drive_state
        .as_ref()
        .is_some_and(|ds| ds.power.unwrap_or(0) > 0)
    {
        return Err("power_usage");
    }
    Ok(())
}
