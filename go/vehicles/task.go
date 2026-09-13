package vehicles

import (
	"context"
	"log/slog"
	"sync"
	"time"

	"github.com/henryouly/tesla-apiscraper-rs/go/store"
	"github.com/henryouly/tesla-apiscraper-rs/go/tesla"
)

// Command mirrors Rust VehicleCommand.
type Command int

const (
	CmdShutdown Command = iota
	CmdSuspend
	CmdResume
)

// drivingInterval mirrors Rust's 2.5s driving cadence (task.rs:41).
const drivingInterval = 2500 * time.Millisecond

// taskConfig carries per-vehicle tunables (mirrors per-car settings +
// poll_interval_seconds).
type taskConfig struct {
	pollInterval time.Duration
}

// pointWriter is the emission seam: *store.Writer in production, a recording
// fake in tests.
type pointWriter interface {
	Write(store.Point)
}

// vehicleTask is one supervised per-VIN poll loop (mirrors vehicle_task_loop).
// It is single-goroutine owned: no locks on mutable state.
type vehicleTask struct {
	vehicle tesla.Vehicle
	apiURL  string
	api     *tesla.APIClient
	store   pointWriter

	cmdCh chan Command
	token func() string // current access token (watch-channel equivalent)

	cfg taskConfig

	// elevation resolves SRTM elevation when the API returns null (P2);
	// nil in P0, which leaves elevation null like a cache miss.
	elevation func(lat, lng float64) *float64

	sessions *sessions
}

// sessions holds the open drive/charge/update accumulators (P1 owns the
// session lifecycles; P0 tracks position dedup state only).
type sessions struct {
	lastLat *float64
	lastLng *float64
}

// Supervisor mirrors Rust Vehicles: one task per VIN.
type Supervisor struct {
	mu     sync.Mutex
	tasks  map[string]*vehicleTask
	apiURL string
	api    *tesla.APIClient
	store  pointWriter
	cfg    taskConfig
}

// NewSupervisor builds an empty supervisor.
func NewSupervisor(apiURL string, api *tesla.APIClient, w pointWriter, pollInterval time.Duration) *Supervisor {
	if pollInterval <= 0 {
		pollInterval = 15 * time.Second // mirrors Rust default when zero
	}
	return &Supervisor{
		tasks:  map[string]*vehicleTask{},
		apiURL: apiURL,
		api:    api,
		store:  w,
		cfg:    taskConfig{pollInterval: pollInterval},
	}
}

// SpawnAll starts one task per discovered vehicle (mirrors spawn_all).
func (s *Supervisor) SpawnAll(ctx context.Context, vehicles []tesla.Vehicle, token func() string) {
	for _, v := range vehicles {
		s.SpawnOne(ctx, v, token)
	}
}

// SpawnOne starts (or restarts) the task for a VIN.
func (s *Supervisor) SpawnOne(ctx context.Context, v tesla.Vehicle, token func() string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if old, ok := s.tasks[v.VIN]; ok {
		old.cmdCh <- CmdShutdown
	}
	t := &vehicleTask{
		vehicle:  v,
		apiURL:   s.apiURL,
		api:      s.api,
		store:    s.store,
		cmdCh:    make(chan Command, 8),
		token:    token,
		cfg:      s.cfg,
		sessions: &sessions{},
	}
	s.tasks[v.VIN] = t
	go t.loop(ctx)
}

// SendCmd delivers Suspend/Resume/Shutdown (mirrors send_cmd).
func (s *Supervisor) SendCmd(vin string, cmd Command) {
	s.mu.Lock()
	t, ok := s.tasks[vin]
	s.mu.Unlock()
	if !ok {
		return
	}
	select {
	case t.cmdCh <- cmd:
	default:
		slog.Warn("vehicle command channel full, dropping", "vin", vin, "cmd", cmd)
	}
}

// ShutdownAll stops every task (mirrors shutdown_all).
func (s *Supervisor) ShutdownAll() {
	s.mu.Lock()
	defer s.mu.Unlock()
	for vin, t := range s.tasks {
		select {
		case t.cmdCh <- CmdShutdown:
		default:
			slog.Warn("shutdown channel full", "vin", vin)
		}
	}
}

// intervalFor mirrors the Rust poll cadence: Driving 2.5s, Charging by
// charger power, everything else the configured interval.
func intervalFor(state VehicleState, cfg taskConfig, chargerPower *int64) time.Duration {
	switch state {
	case StateDriving:
		return drivingInterval
	case StateCharging:
		if chargerPower != nil && *chargerPower > 0 {
			s := 250 / *chargerPower
			if s < 5 {
				s = 5
			}
			if s > 20 {
				s = 20
			}
			return time.Duration(s) * time.Second
		}
		return 5 * time.Second
	default:
		return cfg.pollInterval
	}
}

// loop mirrors vehicle_task_loop: commands, suspend gating, poll, state
// derivation, position recording. Sessions (P1) and auto-suspend hook in.
func (t *vehicleTask) loop(ctx context.Context) {
	log := slog.With("vin", t.vehicle.VIN)
	state := StateOnline
	suspended := false

	for {
		// Timer for the next poll at the current state's cadence.
		// (Charger power is unknown outside Charging; pass nil.)
		timer := time.NewTimer(intervalFor(state, t.cfg, nil))
		select {
		case <-ctx.Done():
			timer.Stop()
			log.Info("vehicle task shutting down")
			return
		case cmd := <-t.cmdCh:
			timer.Stop()
			switch cmd {
			case CmdShutdown:
				log.Info("vehicle task shutting down")
				return
			case CmdSuspend:
				if state == StateUpdating || suspended {
					continue
				}
				suspended = true
				state = StateSuspended
				log.Info("logging suspended")
				continue
			case CmdResume:
				if suspended {
					suspended = false
					state = StateOnline
					log.Info("logging resumed")
				}
				continue
			}
		case <-timer.C:
		}

		if suspended {
			continue // no API call while suspended (mirrors task.rs:114-117)
		}
		token := t.token()
		if token == "" {
			continue // no token yet (mirrors task.rs:119-122)
		}

		data, err := t.api.FetchVehicleData(ctx, token, t.apiURL, t.vehicle.ID)
		if err != nil {
			log.Warn("poll failed", "error", err)
			continue
		}
		next := DeriveNextState(state, &data)
		if next != state && stateAllows(state, next) {
			log.Info("state change", "from", state, "to", next)
			state = next
		}
		t.recordPosition(&data, state == StateDriving)
	}
}

// stateAllows applies the transition table (mirrors task.rs:151-155).
func stateAllows(from, to VehicleState) bool {
	return from.CanTransitionTo(to)
}
