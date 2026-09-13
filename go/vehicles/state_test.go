package vehicles

import (
	"testing"

	"github.com/henryouly/tesla-apiscraper-rs/go/tesla"
)

func TestTransitionTable(t *testing.T) {
	allowed := [][2]VehicleState{
		{StateStart, StateOnline}, {StateStart, StateSuspended},
		{StateOnline, StateDriving}, {StateOnline, StateCharging},
		{StateOnline, StateUpdating}, {StateOnline, StateAsleep},
		{StateOnline, StateOffline}, {StateOnline, StateSuspended},
		{StateDriving, StateOnline}, {StateDriving, StateCharging},
		{StateDriving, StateSuspended},
		{StateCharging, StateOnline}, {StateCharging, StateDriving},
		{StateCharging, StateSuspended},
		{StateUpdating, StateOnline}, {StateUpdating, StateSuspended},
		{StateAsleep, StateOnline}, {StateAsleep, StateSuspended},
		{StateOffline, StateOnline}, {StateOffline, StateSuspended},
		{StateSuspended, StateOnline},
		{StateError, StateOnline}, {StateError, StateSuspended},
	}
	allowedSet := map[[2]VehicleState]bool{}
	for _, p := range allowed {
		allowedSet[p] = true
		if !p[0].CanTransitionTo(p[1]) {
			t.Errorf("%v -> %v should be allowed", p[0], p[1])
		}
	}
	all := []VehicleState{StateStart, StateOnline, StateDriving, StateCharging,
		StateUpdating, StateAsleep, StateOffline, StateSuspended, StateError}
	for _, from := range all {
		for _, to := range all {
			if from == to {
				continue
			}
			// (_, Error) is always allowed; handled separately below.
			if to == StateError {
				if !from.CanTransitionTo(to) {
					t.Errorf("%v -> Error should be allowed", from)
				}
				continue
			}
			if !allowedSet[[2]VehicleState{from, to}] && from.CanTransitionTo(to) {
				t.Errorf("%v -> %v should be rejected", from, to)
			}
		}
	}
}

func strPtr(s string) *string { return &s }

func TestDeriveNextState(t *testing.T) {
	online := &tesla.VehicleDataResponse{State: "online"}
	cases := []struct {
		name  string
		state VehicleState
		data  *tesla.VehicleDataResponse
		want  VehicleState
	}{
		{"api online", StateStart, online, StateOnline},
		{"api asleep", StateOnline, &tesla.VehicleDataResponse{State: "asleep"}, StateAsleep},
		{"shift D drives", StateOnline, &tesla.VehicleDataResponse{State: "online",
			DriveState: &tesla.DriveState{ShiftState: strPtr("D")}}, StateDriving},
		{"shift R drives", StateOnline, &tesla.VehicleDataResponse{State: "online",
			DriveState: &tesla.DriveState{ShiftState: strPtr("R")}}, StateDriving},
		{"shift P stays", StateOnline, &tesla.VehicleDataResponse{State: "online",
			DriveState: &tesla.DriveState{ShiftState: strPtr("P")}}, StateOnline},
		{"charging", StateOnline, &tesla.VehicleDataResponse{State: "online",
			ChargeState: &tesla.ChargeState{ChargingState: strPtr("Charging")}}, StateCharging},
		{"charge exit forces online", StateCharging, online, StateOnline},
		{"updating blocks driving", StateUpdating, &tesla.VehicleDataResponse{State: "online",
			DriveState: &tesla.DriveState{ShiftState: strPtr("D")}}, StateOnline},
		{"installing enters updating", StateOnline, &tesla.VehicleDataResponse{State: "online",
			VehicleState: &tesla.VehicleState{SoftwareUpdate: &tesla.SoftwareUpdate{Status: strPtr("installing")}}}, StateUpdating},
		{"updating stays while installing", StateUpdating, &tesla.VehicleDataResponse{State: "online",
			VehicleState: &tesla.VehicleState{SoftwareUpdate: &tesla.SoftwareUpdate{Status: strPtr("installing")}}}, StateUpdating},
		{"update done returns online", StateUpdating, &tesla.VehicleDataResponse{State: "online",
			VehicleState: &tesla.VehicleState{SoftwareUpdate: &tesla.SoftwareUpdate{Status: strPtr("available")}}}, StateOnline},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := DeriveNextState(tc.state, tc.data); got != tc.want {
				t.Errorf("got %v want %v", got, tc.want)
			}
		})
	}
}
