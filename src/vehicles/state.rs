#![allow(dead_code)]

use serde::Serialize;

use crate::tesla_api::VehicleDataResponse;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehicleState {
    Start,
    Online,
    Driving,
    Charging,
    Updating,
    Asleep,
    Offline,
    Suspended,
    Error,
}

impl Serialize for VehicleState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            VehicleState::Start => "Start",
            VehicleState::Online => "Online",
            VehicleState::Driving => "Driving",
            VehicleState::Charging => "Charging",
            VehicleState::Updating => "Updating",
            VehicleState::Asleep => "Asleep",
            VehicleState::Offline => "Offline",
            VehicleState::Suspended => "Suspended",
            VehicleState::Error => "Error",
        })
    }
}

impl VehicleState {
    pub fn can_transition_to(&self, next: VehicleState) -> bool {
        matches!(
            (self, next),
            (VehicleState::Start, VehicleState::Online)
                | (VehicleState::Start, VehicleState::Suspended)
                | (VehicleState::Online, VehicleState::Driving)
                | (VehicleState::Online, VehicleState::Charging)
                | (VehicleState::Online, VehicleState::Updating)
                | (VehicleState::Online, VehicleState::Asleep)
                | (VehicleState::Online, VehicleState::Offline)
                | (VehicleState::Online, VehicleState::Suspended)
                | (VehicleState::Driving, VehicleState::Online)
                | (VehicleState::Driving, VehicleState::Charging)
                | (VehicleState::Driving, VehicleState::Suspended)
                | (VehicleState::Charging, VehicleState::Online)
                | (VehicleState::Charging, VehicleState::Driving)
                | (VehicleState::Charging, VehicleState::Suspended)
                | (VehicleState::Updating, VehicleState::Online)
                | (VehicleState::Updating, VehicleState::Suspended)
                | (VehicleState::Asleep, VehicleState::Online)
                | (VehicleState::Asleep, VehicleState::Suspended)
                | (VehicleState::Offline, VehicleState::Online)
                | (VehicleState::Offline, VehicleState::Suspended)
                | (VehicleState::Suspended, VehicleState::Online)
                | (_, VehicleState::Error)
                | (VehicleState::Error, VehicleState::Online)
                | (VehicleState::Error, VehicleState::Suspended)
        )
    }
}

/// Derives the next vehicle state based on the current state and API data.
pub(crate) fn derive_next_state(state: VehicleState, data: &VehicleDataResponse) -> VehicleState {
    let new_state = match data.state.as_str() {
        "online" => VehicleState::Online,
        "asleep" => VehicleState::Asleep,
        "offline" => VehicleState::Offline,
        _ => state,
    };

    let new_state = if let Some(ref ds) = data.drive_state {
        if ds
            .shift_state
            .as_deref()
            .is_some_and(|s| s == "D" || s == "R")
        {
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
        if cs
            .charging_state
            .as_deref()
            .is_some_and(|s| s == "Starting" || s == "Charging")
        {
            VehicleState::Charging
        } else {
            new_state
        }
    } else {
        new_state
    };

    let new_state = if state == VehicleState::Charging && new_state != VehicleState::Charging {
        VehicleState::Online
    } else {
        new_state
    };

    if new_state == VehicleState::Driving || new_state == VehicleState::Charging {
        if state == VehicleState::Updating {
            VehicleState::Online
        } else {
            new_state
        }
    } else {
        let su_present = data
            .vehicle_state
            .as_ref()
            .and_then(|vs| vs.software_update.as_ref())
            .is_some();

        match state {
            VehicleState::Updating => {
                if data.state == "online" {
                    if su_present {
                        let still_installing = data
                            .vehicle_state
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
                    && data
                        .vehicle_state
                        .as_ref()
                        .and_then(|vs| vs.software_update.as_ref())
                        .and_then(|su| su.status.as_deref())
                        == Some("installing")
                {
                    let target = VehicleState::Updating;
                    if state.can_transition_to(target) {
                        target
                    } else {
                        VehicleState::Online
                    }
                } else {
                    new_state
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_to_online() {
        assert!(VehicleState::Start.can_transition_to(VehicleState::Online));
    }

    #[test]
    fn start_to_suspended() {
        assert!(VehicleState::Start.can_transition_to(VehicleState::Suspended));
    }

    #[test]
    fn online_driving_charging_updating_asleep_offline() {
        assert!(VehicleState::Online.can_transition_to(VehicleState::Driving));
        assert!(VehicleState::Online.can_transition_to(VehicleState::Charging));
        assert!(VehicleState::Online.can_transition_to(VehicleState::Updating));
        assert!(VehicleState::Online.can_transition_to(VehicleState::Asleep));
        assert!(VehicleState::Online.can_transition_to(VehicleState::Offline));
        assert!(VehicleState::Online.can_transition_to(VehicleState::Suspended));
    }

    #[test]
    fn driving_returns_to_online() {
        assert!(VehicleState::Driving.can_transition_to(VehicleState::Online));
        assert!(VehicleState::Driving.can_transition_to(VehicleState::Charging));
        assert!(VehicleState::Driving.can_transition_to(VehicleState::Suspended));
    }

    #[test]
    fn charging_transitions() {
        assert!(VehicleState::Charging.can_transition_to(VehicleState::Online));
        assert!(VehicleState::Charging.can_transition_to(VehicleState::Driving));
        assert!(VehicleState::Charging.can_transition_to(VehicleState::Suspended));
    }

    #[test]
    fn suspended_only_to_online_or_error() {
        let s = VehicleState::Suspended;
        assert!(s.can_transition_to(VehicleState::Online));
        assert!(s.can_transition_to(VehicleState::Error));
        assert!(!s.can_transition_to(VehicleState::Driving));
        assert!(!s.can_transition_to(VehicleState::Asleep));
    }

    #[test]
    fn asleep_only_to_online_or_suspended_or_error() {
        assert!(VehicleState::Asleep.can_transition_to(VehicleState::Online));
        assert!(VehicleState::Asleep.can_transition_to(VehicleState::Suspended));
        assert!(VehicleState::Asleep.can_transition_to(VehicleState::Error));
        assert!(!VehicleState::Asleep.can_transition_to(VehicleState::Driving));
        assert!(!VehicleState::Asleep.can_transition_to(VehicleState::Charging));
    }

    #[test]
    fn offline_only_to_online_or_suspended_or_error() {
        assert!(VehicleState::Offline.can_transition_to(VehicleState::Online));
        assert!(VehicleState::Offline.can_transition_to(VehicleState::Suspended));
        assert!(VehicleState::Offline.can_transition_to(VehicleState::Error));
        assert!(!VehicleState::Offline.can_transition_to(VehicleState::Driving));
    }

    #[test]
    fn updating_only_to_online_or_suspended_or_error() {
        assert!(VehicleState::Updating.can_transition_to(VehicleState::Online));
        assert!(VehicleState::Updating.can_transition_to(VehicleState::Suspended));
        assert!(VehicleState::Updating.can_transition_to(VehicleState::Error));
        assert!(!VehicleState::Updating.can_transition_to(VehicleState::Driving));
        assert!(!VehicleState::Updating.can_transition_to(VehicleState::Charging));
    }

    #[test]
    fn error_to_online_or_suspended() {
        assert!(VehicleState::Error.can_transition_to(VehicleState::Online));
        assert!(VehicleState::Error.can_transition_to(VehicleState::Suspended));
        assert!(!VehicleState::Error.can_transition_to(VehicleState::Driving));
        assert!(!VehicleState::Error.can_transition_to(VehicleState::Asleep));
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        assert!(!VehicleState::Asleep.can_transition_to(VehicleState::Driving));
        assert!(!VehicleState::Driving.can_transition_to(VehicleState::Asleep));
        assert!(!VehicleState::Driving.can_transition_to(VehicleState::Updating));
        assert!(!VehicleState::Charging.can_transition_to(VehicleState::Asleep));
        assert!(!VehicleState::Charging.can_transition_to(VehicleState::Updating));
        assert!(!VehicleState::Updating.can_transition_to(VehicleState::Driving));
        assert!(!VehicleState::Updating.can_transition_to(VehicleState::Asleep));
        assert!(!VehicleState::Offline.can_transition_to(VehicleState::Driving));
        assert!(!VehicleState::Offline.can_transition_to(VehicleState::Charging));
    }

    #[test]
    fn any_state_can_error() {
        for state in &[
            VehicleState::Start,
            VehicleState::Online,
            VehicleState::Driving,
            VehicleState::Charging,
            VehicleState::Updating,
            VehicleState::Asleep,
            VehicleState::Offline,
            VehicleState::Suspended,
            VehicleState::Error,
        ] {
            assert!(state.can_transition_to(VehicleState::Error));
        }
    }

    #[test]
    fn serializes_as_string() {
        let json = serde_json::to_value(VehicleState::Online).unwrap();
        assert_eq!(json, "Online");
    }
}
