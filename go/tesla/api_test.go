package tesla

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"
)

func vehicleDataServer(t *testing.T, body string, status int) (*httptest.Server, *APIClient, *string) {
	t.Helper()
	var gotAuth string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotAuth = r.Header.Get("Authorization")
		w.WriteHeader(status)
		fmt.Fprint(w, body)
	}))
	t.Cleanup(server.Close)
	return server, NewAPIClient(), &gotAuth
}

func TestListProducts(t *testing.T) {
	server, client, gotAuth := vehicleDataServer(t, `{"response":[
		{"id":12345,"vehicle_id":98765,"vin":"VIN1","display_name":"Car","state":"online","api_version":1,"in_service":false}
	]}`, http.StatusOK)

	vehicles, err := client.ListProducts(context.Background(), "tok", server.URL)
	if err != nil {
		t.Fatalf("ListProducts: %v", err)
	}
	if len(vehicles) != 1 {
		t.Fatalf("expected 1 vehicle, got %d", len(vehicles))
	}
	v := vehicles[0]
	if v.ID != 12345 || v.VehicleID != 98765 || v.VIN != "VIN1" {
		t.Errorf("unexpected vehicle: %+v", v)
	}
	if v.DisplayName == nil || *v.DisplayName != "Car" {
		t.Errorf("display name: %+v", v.DisplayName)
	}
	if *gotAuth != "Bearer tok" {
		t.Errorf("auth header: %q", *gotAuth)
	}
}

func TestFetchVehicleDataSuccess(t *testing.T) {
	body := `{"response":{
		"state": "online",
		"odometer": 50000.5,
		"drive_state": {"shift_state": "D", "speed": 65.0, "latitude": 37.7749, "longitude": -122.4194,
			"heading": 180, "power": 12, "elevation": 10.5, "timestamp": 1700000000000},
		"charge_state": {"battery_level": 85, "charging_state": "Disconnected"},
		"climate_state": {"inside_temp": 22.5, "is_climate_on": true},
		"vehicle_state": {"locked": true, "sentry_mode": false}
	}}`
	server, client, _ := vehicleDataServer(t, body, http.StatusOK)

	data, err := client.FetchVehicleData(context.Background(), "tok", server.URL, 12345)
	if err != nil {
		t.Fatalf("FetchVehicleData: %v", err)
	}
	if data.State != "online" {
		t.Errorf("state: %q", data.State)
	}
	if data.Odometer == nil || *data.Odometer != 50000.5 {
		t.Errorf("odometer: %+v", data.Odometer)
	}
	ds := data.DriveState
	if ds == nil {
		t.Fatal("missing drive_state")
	}
	if ds.ShiftState == nil || *ds.ShiftState != "D" {
		t.Errorf("shift: %+v", ds.ShiftState)
	}
	if ds.Latitude == nil || *ds.Latitude != 37.7749 {
		t.Errorf("lat: %+v", ds.Latitude)
	}
	if ds.Power == nil || *ds.Power != 12 {
		t.Errorf("power: %+v", ds.Power)
	}
	if data.ChargeState == nil || data.ChargeState.BatteryLevel == nil || *data.ChargeState.BatteryLevel != 85 {
		t.Errorf("charge: %+v", data.ChargeState)
	}
	if data.VehicleState == nil || data.VehicleState.Locked == nil || !*data.VehicleState.Locked {
		t.Errorf("vehicle locked: %+v", data.VehicleState)
	}
}

func TestFetchVehicleDataNulls(t *testing.T) {
	// Mirrors Rust null_sub_objects / asleep / no-drive-state cases: explicit
	// nulls and absent objects must parse to nil, never error.
	body := `{"response":{
		"state": "asleep",
		"odometer": null,
		"drive_state": {"shift_state": null, "speed": null, "latitude": null, "longitude": null}
	}}`
	server, client, _ := vehicleDataServer(t, body, http.StatusOK)

	data, err := client.FetchVehicleData(context.Background(), "tok", server.URL, 1)
	if err != nil {
		t.Fatalf("FetchVehicleData: %v", err)
	}
	if data.Odometer != nil {
		t.Errorf("odometer should be nil: %+v", data.Odometer)
	}
	if data.DriveState == nil {
		t.Fatal("drive_state object should be present")
	}
	if data.DriveState.Latitude != nil || data.DriveState.ShiftState != nil {
		t.Errorf("nulls should parse nil: %+v", data.DriveState)
	}
	if data.ChargeState != nil || data.VehicleState != nil {
		t.Errorf("absent objects should be nil")
	}
}

func TestFetchVehicleDataErrors(t *testing.T) {
	server, client, _ := vehicleDataServer(t, `{"error":"unauthorized"}`, http.StatusUnauthorized)
	_, err := client.FetchVehicleData(context.Background(), "tok", server.URL, 1)
	apiErr, ok := err.(*APIError)
	if !ok || apiErr.Status != 401 {
		t.Fatalf("expected Api 401, got %v", err)
	}

	server2, client2, _ := vehicleDataServer(t, `boom`, http.StatusOK)
	_, err = client2.FetchVehicleData(context.Background(), "tok", server2.URL, 1)
	if apiErr, ok := err.(*APIError); !ok || apiErr.Status != 502 {
		t.Fatalf("expected Api 502 for bad JSON, got %v", err)
	}
}
