package vehicles

import (
	"log/slog"
	"strconv"
	"time"

	"github.com/henryouly/tesla-apiscraper-rs/go/store"
	"github.com/henryouly/tesla-apiscraper-rs/go/tesla"
)

// recordPosition mirrors Rust record_position (session.rs:661-848): every
// successful poll writes one `positions` point subject to the dedup rule,
// with null-GPS handling that differs parked vs driving.
func (t *vehicleTask) recordPosition(data *tesla.VehicleDataResponse, driving bool) {
	vin := t.vehicle.VIN
	ds := data.DriveState
	if ds == nil {
		slog.Debug("positions: SKIPPED (no drive_state)", "vin", vin)
		return
	}

	// Fresh coords when both are present.
	var lat, lng *float64
	if ds.Latitude != nil && ds.Longitude != nil {
		lat, lng = ds.Latitude, ds.Longitude
	}

	if !driving && lat != nil && lng != nil &&
		t.sessions.lastLat != nil && t.sessions.lastLng != nil &&
		*lat == *t.sessions.lastLat && *lng == *t.sessions.lastLng {
		slog.Debug("positions: SKIPPED (unchanged, parked)", "vin", vin)
		return // dedup applies parked only; driving logs every poll
	}

	elevation := ds.Elevation
	if elevation == nil && lat != nil && lng != nil && t.elevation != nil {
		elevation = t.elevation(*lat, *lng) // P2 SRTM fallback; nil in P0
	}

	var outLat, outLng *float64
	var outElev *float64
	switch {
	case lat != nil && lng != nil:
		outLat, outLng, outElev = lat, lng, elevation
	case driving && t.sessions.lastLat != nil && t.sessions.lastLng != nil:
		// Driving without a fresh fix: anchor telemetry to last known.
		outLat, outLng, outElev = t.sessions.lastLat, t.sessions.lastLng, nil
	default:
		slog.Debug("positions: SKIPPED (null GPS, parked)", "vin", vin)
		return
	}

	ts := time.Now()
	if ds.Timestamp != nil {
		ts = time.Unix(*ds.Timestamp/1000, 0) // mirrors ds.timestamp/1000
	}

	cs := data.ChargeState
	cl := data.ClimateState
	vs := data.VehicleState

	t.store.Write(store.Point{
		Measurement: "positions",
		Tags: map[string]string{
			"vin":    vin,
			"car_id": strconv.FormatInt(t.vehicle.VehicleID, 10),
		},
		Fields: map[string]interface{}{
			"latitude":                outLat,
			"longitude":               outLng,
			"speed":                   f64ptr(ds.Speed),
			"power":                   i64ptr(ds.Power),
			"odometer":                f64ptr(data.Odometer),
			"battery_level":           i64ptr(chargeInt(cs, func(c *tesla.ChargeState) *int64 { return c.BatteryLevel })),
			"rated_battery_range_km":  chargeFloat(cs, func(c *tesla.ChargeState) *float64 { return c.BatteryRange }),
			"outside_temp":            climateFloat(cl, func(c *tesla.ClimateState) *float64 { return c.OutsideTemp }),
			"inside_temp":             climateFloat(cl, func(c *tesla.ClimateState) *float64 { return c.InsideTemp }),
			"heading":                 i64ptr(ds.Heading),
			"elevation":               outElev,
			"shift_state":             strptr(ds.ShiftState),
			"tpms_pressure_fl":        vsFloat(vs, func(v *tesla.VehicleState) *float64 { return v.TPMSPressureFL }),
			"tpms_pressure_fr":        vsFloat(vs, func(v *tesla.VehicleState) *float64 { return v.TPMSPressureFR }),
			"tpms_pressure_rl":        vsFloat(vs, func(v *tesla.VehicleState) *float64 { return v.TPMSPressureRL }),
			"tpms_pressure_rr":        vsFloat(vs, func(v *tesla.VehicleState) *float64 { return v.TPMSPressureRR }),
			"fan_status":              climateInt(cl, func(c *tesla.ClimateState) *int64 { return c.FanStatus }),
			"is_front_defroster_on":   climateBool(cl, func(c *tesla.ClimateState) *bool { return c.IsFrontDefrosterOn }),
			"is_rear_defroster_on":    climateBool(cl, func(c *tesla.ClimateState) *bool { return c.IsRearDefrosterOn }),
			"ideal_battery_range_km":  chargeFloat(cs, func(c *tesla.ChargeState) *float64 { return c.IdealBatteryRange }),
			"est_battery_range_km":    chargeFloat(cs, func(c *tesla.ChargeState) *float64 { return c.EstBatteryRange }),
			"usable_battery_level":    chargeInt(cs, func(c *tesla.ChargeState) *int64 { return c.UsableBatteryLevel }),
			"is_climate_on":           climateBool(cl, func(c *tesla.ClimateState) *bool { return c.IsClimateOn }),
			"driver_temp_setting":     climateFloat(cl, func(c *tesla.ClimateState) *float64 { return c.DriverTempSetting }),
			"passenger_temp_setting":  climateFloat(cl, func(c *tesla.ClimateState) *float64 { return c.PassengerTempSetting }),
			"battery_heater":          climateBool(cl, func(c *tesla.ClimateState) *bool { return c.BatteryHeater }),
			"battery_heater_on":       chargeBool(cs, func(c *tesla.ChargeState) *bool { return c.BatteryHeaterOn }),
			"battery_heater_no_power": climateBool(cl, func(c *tesla.ClimateState) *bool { return c.BatteryHeaterNoPower }),
			"is_preconditioning":      climateBool(cl, func(c *tesla.ClimateState) *bool { return c.IsPreconditioning }),
			"climate_keeper_mode":     climateStr(cl, func(c *tesla.ClimateState) *string { return c.ClimateKeeperMode }),
			"locked":                  vsBool(vs, func(v *tesla.VehicleState) *bool { return v.Locked }),
			"is_user_present":         vsBool(vs, func(v *tesla.VehicleState) *bool { return v.IsUserPresent }),
			"sentry_mode":             vsBool(vs, func(v *tesla.VehicleState) *bool { return v.SentryMode }),
		},
		Time: ts,
	})

	// last_lat_lng updates only on successful write with fresh coords.
	if lat != nil && lng != nil {
		t.sessions.lastLat, t.sessions.lastLng = lat, lng
	}
	slog.Info("positions: WRITTEN", "vin", vin,
		"lat", optF64(outLat), "lng", optF64(outLng),
		"speed", optF64(f64ptr(ds.Speed)), "driving", driving)
}

// --- tiny accessors (nil-safe sub-object reads) ---

func f64ptr(p *float64) *float64 { return p }
func i64ptr(p *int64) *int64     { return p }
func strptr(p *string) *string   { return p }

func chargeInt(cs *tesla.ChargeState, f func(*tesla.ChargeState) *int64) *int64 {
	if cs == nil {
		return nil
	}
	return f(cs)
}
func chargeFloat(cs *tesla.ChargeState, f func(*tesla.ChargeState) *float64) *float64 {
	if cs == nil {
		return nil
	}
	return f(cs)
}
func chargeBool(cs *tesla.ChargeState, f func(*tesla.ChargeState) *bool) *bool {
	if cs == nil {
		return nil
	}
	return f(cs)
}
func climateInt(cl *tesla.ClimateState, f func(*tesla.ClimateState) *int64) *int64 {
	if cl == nil {
		return nil
	}
	return f(cl)
}
func climateFloat(cl *tesla.ClimateState, f func(*tesla.ClimateState) *float64) *float64 {
	if cl == nil {
		return nil
	}
	return f(cl)
}
func climateBool(cl *tesla.ClimateState, f func(*tesla.ClimateState) *bool) *bool {
	if cl == nil {
		return nil
	}
	return f(cl)
}
func climateStr(cl *tesla.ClimateState, f func(*tesla.ClimateState) *string) *string {
	if cl == nil {
		return nil
	}
	return f(cl)
}
func vsFloat(vs *tesla.VehicleState, f func(*tesla.VehicleState) *float64) *float64 {
	if vs == nil {
		return nil
	}
	return f(vs)
}
func vsBool(vs *tesla.VehicleState, f func(*tesla.VehicleState) *bool) *bool {
	if vs == nil {
		return nil
	}
	return f(vs)
}

func optF64(p *float64) interface{} {
	if p == nil {
		return nil
	}
	return *p
}
