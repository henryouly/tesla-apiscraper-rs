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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tesla_api::VehicleDataResponse;

    fn data_from(v: serde_json::Value) -> VehicleDataResponse {
        serde_json::from_value(v).unwrap()
    }

    fn idle_data() -> VehicleDataResponse {
        data_from(serde_json::json!({
            "state": "online",
            "vehicle_state": {
                "locked": true,
                "is_user_present": false,
                "sentry_mode": false,
                "df": 0,
                "pf": 0,
                "dr": 0,
                "pr": 0,
                "ft": 0,
                "rt": 0
            },
            "climate_state": {
                "is_preconditioning": false,
                "climate_keeper_mode": "off"
            },
            "drive_state": {
                "power": 0
            }
        }))
    }

    #[test]
    fn cannot_suspend_blocks_only_active_states() {
        assert_eq!(
            cannot_suspend_state(&VehicleState::Driving),
            Some("vehicle is driving")
        );
        assert_eq!(
            cannot_suspend_state(&VehicleState::Charging),
            Some("vehicle is charging")
        );
        assert_eq!(
            cannot_suspend_state(&VehicleState::Updating),
            Some("software update in progress")
        );
        for allowed in [
            VehicleState::Start,
            VehicleState::Online,
            VehicleState::Asleep,
            VehicleState::Offline,
            VehicleState::Suspended,
        ] {
            assert_eq!(cannot_suspend_state(&allowed), None, "{allowed:?}");
        }
    }

    #[test]
    fn can_fall_asleep_allows_idle_vehicle() {
        assert_eq!(can_fall_asleep(&idle_data(), true), Ok(()));
    }

    #[test]
    fn can_fall_asleep_blocks_user_present() {
        let mut d = idle_data();
        d.vehicle_state.as_mut().unwrap().is_user_present = Some(true);
        assert_eq!(can_fall_asleep(&d, false), Err("user_present"));
    }

    #[test]
    fn can_fall_asleep_blocks_preconditioning_and_dogmode() {
        let d = data_from(serde_json::json!({
            "state": "online",
            "climate_state": { "is_preconditioning": true }
        }));
        assert_eq!(can_fall_asleep(&d, false), Err("preconditioning"));

        let d = data_from(serde_json::json!({
            "state": "online",
            "climate_state": { "climate_keeper_mode": "dog" }
        }));
        assert_eq!(can_fall_asleep(&d, false), Err("dogmode"));
    }

    #[test]
    fn can_fall_asleep_blocks_downloading_update_until_100_percent() {
        let d = data_from(serde_json::json!({
            "state": "online",
            "vehicle_state": {
                "software_update": { "status": "downloading", "download_perc": 45 }
            }
        }));
        assert_eq!(can_fall_asleep(&d, false), Err("downloading_update"));

        let d_done = data_from(serde_json::json!({
            "state": "online",
            "vehicle_state": {
                "software_update": { "status": "downloading", "download_perc": 100 }
            }
        }));
        assert_eq!(can_fall_asleep(&d_done, false), Ok(()));
    }

    #[test]
    fn can_fall_asleep_blocks_open_doors_and_trunks() {
        let mut d = idle_data();
        d.vehicle_state.as_mut().unwrap().dr = Some(1.0);
        assert_eq!(can_fall_asleep(&d, false), Err("doors_open"));

        let mut d = idle_data();
        d.vehicle_state.as_mut().unwrap().ft = Some(1.0);
        assert_eq!(can_fall_asleep(&d, false), Err("trunk_open"));

        let mut d = idle_data();
        d.vehicle_state.as_mut().unwrap().rt = Some(1.0);
        assert_eq!(can_fall_asleep(&d, false), Err("trunk_open"));
    }

    #[test]
    fn can_fall_asleep_respects_require_unlocked_setting() {
        let d = data_from(serde_json::json!({
            "state": "online",
            "vehicle_state": { "locked": false }
        }));
        assert_eq!(can_fall_asleep(&d, false), Ok(()));
        assert_eq!(can_fall_asleep(&d, true), Err("unlocked"));
    }

    #[test]
    fn can_fall_asleep_blocks_positive_power_only() {
        let d = data_from(serde_json::json!({
            "state": "online",
            "drive_state": { "power": 3 }
        }));
        assert_eq!(can_fall_asleep(&d, false), Err("power_usage"));

        let d_neg = data_from(serde_json::json!({
            "state": "online",
            "drive_state": { "power": -1 }
        }));
        assert_eq!(can_fall_asleep(&d_neg, false), Ok(()));
    }
}
