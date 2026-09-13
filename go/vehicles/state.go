// Package vehicles ports the Rust vehicle supervisor, per-vehicle poll loop,
// state machine, and session trackers (src/vehicles/).
package vehicles

import "github.com/henryouly/tesla-apiscraper-rs/go/tesla"

// VehicleState mirrors Rust VehicleState (src/vehicles/state.rs).
type VehicleState string

const (
	StateStart     VehicleState = "Start"
	StateOnline    VehicleState = "Online"
	StateDriving   VehicleState = "Driving"
	StateCharging  VehicleState = "Charging"
	StateUpdating  VehicleState = "Updating"
	StateAsleep    VehicleState = "Asleep"
	StateOffline   VehicleState = "Offline"
	StateSuspended VehicleState = "Suspended"
	StateError     VehicleState = "Error"
)

// CanTransitionTo mirrors Rust can_transition_to exactly.
func (s VehicleState) CanTransitionTo(next VehicleState) bool {
	if next == StateError {
		return true
	}
	switch s {
	case StateStart:
		return next == StateOnline || next == StateSuspended
	case StateOnline:
		return next == StateDriving || next == StateCharging ||
			next == StateUpdating || next == StateAsleep ||
			next == StateOffline || next == StateSuspended
	case StateDriving:
		return next == StateOnline || next == StateCharging || next == StateSuspended
	case StateCharging:
		return next == StateOnline || next == StateDriving || next == StateSuspended
	case StateUpdating:
		return next == StateOnline || next == StateSuspended
	case StateAsleep:
		return next == StateOnline || next == StateSuspended
	case StateOffline:
		return next == StateOnline || next == StateSuspended
	case StateSuspended:
		return next == StateOnline
	case StateError:
		return next == StateOnline || next == StateSuspended
	}
	return false
}

func strVal(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

// DeriveNextState mirrors Rust derive_next_state precedence exactly.
func DeriveNextState(state VehicleState, data *tesla.VehicleDataResponse) VehicleState {
	var next VehicleState
	switch data.State {
	case "online":
		next = StateOnline
	case "asleep":
		next = StateAsleep
	case "offline":
		next = StateOffline
	default:
		next = state
	}

	if ds := data.DriveState; ds != nil {
		if s := strVal(ds.ShiftState); s == "D" || s == "R" {
			next = StateDriving
		}
	}

	if next != StateDriving {
		if cs := data.ChargeState; cs != nil {
			if s := strVal(cs.ChargingState); s == "Starting" || s == "Charging" {
				next = StateCharging
			}
		}
	}

	if state == StateCharging && next != StateCharging {
		next = StateOnline
	}

	if next == StateDriving || next == StateCharging {
		if state == StateUpdating {
			return StateOnline
		}
		return next
	}

	suPresent := data.VehicleState != nil && data.VehicleState.SoftwareUpdate != nil
	if state == StateUpdating {
		if data.State == "online" {
			if suPresent {
				if strVal(data.VehicleState.SoftwareUpdate.Status) == "installing" {
					return StateUpdating
				}
				return StateOnline
			}
			if data.VehicleState != nil {
				return StateOnline
			}
			return StateUpdating
		}
		return StateUpdating
	}

	if suPresent && strVal(data.VehicleState.SoftwareUpdate.Status) == "installing" {
		if state.CanTransitionTo(StateUpdating) {
			return StateUpdating
		}
		return StateOnline
	}
	return next
}
