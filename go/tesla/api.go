// Package tesla also contains the Owner API client (port of tesla_api.rs):
// vehicle discovery and vehicle_data polling with response-envelope unwrap.
package tesla

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

// Vehicle mirrors Rust Vehicle. ID feeds the vehicle_data path; VIN is the
// stable key used everywhere.
type Vehicle struct {
	ID          int64   `json:"id"`
	VehicleID   int64   `json:"vehicle_id"`
	VIN         string  `json:"vin"`
	DisplayName *string `json:"display_name"`
	State       string  `json:"state"`
	APIVersion  int64   `json:"api_version"`
	InService   bool    `json:"in_service"`
}

// VehicleDataResponse mirrors Rust VehicleDataResponse; every sub-object and
// scalar is optional (pointer = present).
type VehicleDataResponse struct {
	State        string        `json:"state"`
	Odometer     *float64      `json:"odometer"`
	DriveState   *DriveState   `json:"drive_state"`
	ChargeState  *ChargeState  `json:"charge_state"`
	ClimateState *ClimateState `json:"climate_state"`
	VehicleState *VehicleState `json:"vehicle_state"`
}

// DriveState mirrors Rust DriveState.
type DriveState struct {
	ShiftState *string  `json:"shift_state"`
	Speed      *float64 `json:"speed"`
	Latitude   *float64 `json:"latitude"`
	Longitude  *float64 `json:"longitude"`
	Heading    *int64   `json:"heading"`
	Power      *int64   `json:"power"`
	Elevation  *float64 `json:"elevation"`
	Timestamp  *int64   `json:"timestamp"`
}

// ChargeState mirrors Rust ChargeState.
type ChargeState struct {
	BatteryLevel         *int64   `json:"battery_level"`
	BatteryRange         *float64 `json:"battery_range"`
	IdealBatteryRange    *float64 `json:"ideal_battery_range"`
	EstBatteryRange      *float64 `json:"est_battery_range"`
	UsableBatteryLevel   *int64   `json:"usable_battery_level"`
	BatteryHeaterOn      *bool    `json:"battery_heater_on"`
	ChargingState        *string  `json:"charging_state"`
	ChargeEnergyAdded    *float64 `json:"charge_energy_added"`
	ChargerActualCurrent *int64   `json:"charger_actual_current"`
	ChargerVoltage       *int64   `json:"charger_voltage"`
	ChargerPower         *int64   `json:"charger_power"`
	ChargerPhases        *int64   `json:"charger_phases"`
	FastChargerBrand     *string  `json:"fast_charger_brand"`
	FastChargerType      *string  `json:"fast_charger_type"`
	ConnChargeCable      *string  `json:"conn_charge_cable"`
	ChargeLimitSoc       *int64   `json:"charge_limit_soc"`
	TimeToFullCharge     *float64 `json:"time_to_full_charge"`
	ChargerPilotCurrent  *int64   `json:"charger_pilot_current"`
	FastChargerPresent   *bool    `json:"fast_charger_present"`
	NotEnoughPowerToHeat *bool    `json:"not_enough_power_to_heat"`
}

// ClimateState mirrors Rust ClimateState.
type ClimateState struct {
	InsideTemp           *float64 `json:"inside_temp"`
	OutsideTemp          *float64 `json:"outside_temp"`
	FanStatus            *int64   `json:"fan_status"`
	IsFrontDefrosterOn   *bool    `json:"is_front_defroster_on"`
	IsRearDefrosterOn    *bool    `json:"is_rear_defroster_on"`
	IsClimateOn          *bool    `json:"is_climate_on"`
	DriverTempSetting    *float64 `json:"driver_temp_setting"`
	PassengerTempSetting *float64 `json:"passenger_temp_setting"`
	BatteryHeater        *bool    `json:"battery_heater"`
	BatteryHeaterNoPower *bool    `json:"battery_heater_no_power"`
	IsPreconditioning    *bool    `json:"is_preconditioning"`
	ClimateKeeperMode    *string  `json:"climate_keeper_mode"`
}

// VehicleState mirrors Rust VehicleStateData (named to avoid clashing with
// the state-machine enum in package vehicles).
type VehicleState struct {
	TPMSPressureFL *float64        `json:"tpms_pressure_fl"`
	TPMSPressureFR *float64        `json:"tpms_pressure_fr"`
	TPMSPressureRL *float64        `json:"tpms_pressure_rl"`
	TPMSPressureRR *float64        `json:"tpms_pressure_rr"`
	CarVersion     *string         `json:"car_version"`
	SoftwareUpdate *SoftwareUpdate `json:"software_update"`
	SentryMode     *bool           `json:"sentry_mode"`
	IsUserPresent  *bool           `json:"is_user_present"`
	DF             *float64        `json:"df"`
	PF             *float64        `json:"pf"`
	DR             *float64        `json:"dr"`
	PR             *float64        `json:"pr"`
	FT             *float64        `json:"ft"`
	RT             *float64        `json:"rt"`
	Locked         *bool           `json:"locked"`
}

// SoftwareUpdate mirrors Rust SoftwareUpdate.
type SoftwareUpdate struct {
	DownloadPerc        *int64  `json:"download_perc"`
	ExpectedDurationSec *int64  `json:"expected_duration_sec"`
	InstallPerc         *int64  `json:"install_perc"`
	ScheduledTimeMs     *int64  `json:"scheduled_time_ms"`
	Status              *string `json:"status"`
	Version             *string `json:"version"`
}

// APIClient performs Owner API calls with a bearer token.
type APIClient struct {
	HTTP *http.Client
}

// NewAPIClient builds a client with a 30s HTTP timeout.
func NewAPIClient() *APIClient {
	return &APIClient{HTTP: &http.Client{Timeout: 30 * time.Second}}
}

func (c *APIClient) getEnvelope(ctx context.Context, accessToken, url string) (json.RawMessage, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+accessToken)

	resp, err := c.HTTP.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
		return nil, &APIError{Status: resp.StatusCode, Body: string(body)}
	}

	var envelope struct {
		Response json.RawMessage `json:"response"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&envelope); err != nil {
		return nil, &APIError{Status: 502, Body: "invalid response envelope: " + err.Error()}
	}
	return envelope.Response, nil
}

// ListProducts mirrors Rust list_products: GET /api/1/products.
func (c *APIClient) ListProducts(ctx context.Context, accessToken, apiURL string) ([]Vehicle, error) {
	raw, err := c.getEnvelope(ctx, accessToken, strings.TrimSuffix(apiURL, "/")+"/api/1/products")
	if err != nil {
		return nil, err
	}
	var vehicles []Vehicle
	if err := json.Unmarshal(raw, &vehicles); err != nil {
		return nil, &APIError{Status: 502, Body: "invalid /api/1/products response: " + err.Error()}
	}
	return vehicles, nil
}

// FetchVehicleData mirrors Rust fetch_vehicle_data: GET
// /api/1/vehicles/{id}/vehicle_data with the response envelope unwrapped.
// A JSON shape mismatch surfaces as a 502-style APIError like Rust.
// Unknown fields are ignored, matching serde's default.
func (c *APIClient) FetchVehicleData(ctx context.Context, accessToken, apiURL string, vehicleID int64) (VehicleDataResponse, error) {
	url := fmt.Sprintf("%s/api/1/vehicles/%d/vehicle_data", strings.TrimSuffix(apiURL, "/"), vehicleID)
	raw, err := c.getEnvelope(ctx, accessToken, url)
	if err != nil {
		return VehicleDataResponse{}, err
	}
	var data VehicleDataResponse
	if err := json.Unmarshal(raw, &data); err != nil {
		return VehicleDataResponse{}, &APIError{Status: 502, Body: fmt.Sprintf("invalid /api/1/vehicles/{id}/vehicle_data response: %s", err.Error())}
	}
	return data, nil
}
