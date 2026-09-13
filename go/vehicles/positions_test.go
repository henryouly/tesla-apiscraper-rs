package vehicles

import (
	"testing"
	"time"

	"github.com/henryouly/tesla-apiscraper-rs/go/store"
	"github.com/henryouly/tesla-apiscraper-rs/go/tesla"
)

type recordWriter struct {
	points []store.Point
}

func (f *recordWriter) Write(p store.Point) { f.points = append(f.points, p) }

func testTask() (*vehicleTask, *recordWriter) {
	fake := &recordWriter{}
	return &vehicleTask{
		vehicle:  tesla.Vehicle{ID: 1, VehicleID: 42, VIN: "VIN1"},
		store:    fake,
		sessions: &sessions{},
	}, fake
}

func fptr(f float64) *float64 { return &f }
func sptr(s string) *string   { return &s }

func driveData(lat, lng, speed *float64, shift string) *tesla.VehicleDataResponse {
	return &tesla.VehicleDataResponse{
		State: "online",
		DriveState: &tesla.DriveState{
			Latitude: lat, Longitude: lng, Speed: speed,
			ShiftState: sptr(shift), Timestamp: func() *int64 { t := int64(1700000000000); return &t }(),
		},
	}
}

func TestParkedDedupSkipsRepeat(t *testing.T) {
	task, fake := testTask()
	lat, lng, speed := 37.8, -122.5, 0.0
	task.recordPosition(driveData(&lat, &lng, &speed, "P"), false)
	task.recordPosition(driveData(&lat, &lng, &speed, "P"), false)
	if len(fake.points) != 1 {
		t.Fatalf("expected 1 write, got %d", len(fake.points))
	}
	p := fake.points[0]
	if p.Measurement != "positions" || p.Tags["vin"] != "VIN1" || p.Tags["car_id"] != "42" {
		t.Errorf("measurement/tags: %+v", p)
	}
	if p.Time.Unix() != 1700000000 {
		t.Errorf("timestamp should be ds.timestamp/1000, got %v", p.Time)
	}
}

func TestParkedMovedWrites(t *testing.T) {
	task, fake := testTask()
	lat1, lng, speed := 37.8, -122.5, 0.0
	lat2 := 37.8001
	task.recordPosition(driveData(&lat1, &lng, &speed, "P"), false)
	task.recordPosition(driveData(&lat2, &lng, &speed, "P"), false)
	if len(fake.points) != 2 {
		t.Fatalf("expected 2 writes, got %d", len(fake.points))
	}
}

func TestDrivingWritesEveryPoll(t *testing.T) {
	task, fake := testTask()
	lat, lng, speed := 37.8, -122.5, 65.0
	task.recordPosition(driveData(&lat, &lng, &speed, "D"), true)
	task.recordPosition(driveData(&lat, &lng, &speed, "D"), true)
	if len(fake.points) != 2 {
		t.Fatalf("driving must log every poll, got %d", len(fake.points))
	}
}

func TestNullGPSParkedSkips(t *testing.T) {
	task, fake := testTask()
	speed := 0.0
	task.recordPosition(driveData(nil, nil, &speed, "P"), false)
	if len(fake.points) != 0 {
		t.Fatalf("parked null GPS must skip, got %d", len(fake.points))
	}
}

func TestNullGPSDrivingAnchorsLast(t *testing.T) {
	task, fake := testTask()
	lat, lng, speed := 37.8, -122.5, 65.0
	task.recordPosition(driveData(&lat, &lng, &speed, "D"), true)
	task.recordPosition(driveData(nil, nil, &speed, "D"), true)
	if len(fake.points) != 2 {
		t.Fatalf("expected 2 writes, got %d", len(fake.points))
	}
	anchored := fake.points[1].Fields["latitude"]
	f, ok := anchored.(*float64)
	if !ok || f == nil || *f != 37.8 {
		t.Errorf("expected anchor to last lat, got %#v", anchored)
	}
	if fake.points[1].Fields["elevation"] != nil {
		// Fields carry typed-nil pointers (store.Write drops them); a
		// non-nil elevation here would be a real value.
		if p, ok := fake.points[1].Fields["elevation"].(*float64); !ok || p != nil {
			t.Errorf("anchored point must have nil elevation, got %#v", fake.points[1].Fields["elevation"])
		}
	}
}

func TestNoDriveStateSkips(t *testing.T) {
	task, fake := testTask()
	task.recordPosition(&tesla.VehicleDataResponse{State: "online"}, true)
	if len(fake.points) != 0 {
		t.Fatalf("no drive_state must skip, got %d", len(fake.points))
	}
}

func TestIntervalFor(t *testing.T) {
	cfg := taskConfig{pollInterval: 60 * time.Second}
	if got := intervalFor(StateDriving, cfg, nil); got != 2500*time.Millisecond {
		t.Errorf("driving: %v", got)
	}
	if got := intervalFor(StateOnline, cfg, nil); got != 60*time.Second {
		t.Errorf("online: %v", got)
	}
	power := int64(10)
	if got := intervalFor(StateCharging, cfg, &power); got != 20*time.Second {
		t.Errorf("charging 10kW: %v (want clamp 250/10=25→20s)", got)
	}
	power = 100
	if got := intervalFor(StateCharging, cfg, &power); got != 5*time.Second {
		t.Errorf("charging 100kW: %v (want clamp 250/100→5s floor)", got)
	}
	if got := intervalFor(StateCharging, cfg, nil); got != 5*time.Second {
		t.Errorf("charging unknown power: %v", got)
	}
}
