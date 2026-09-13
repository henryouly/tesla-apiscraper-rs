# Go Port — Implementation Plan (port of `tesla-apiscraper-rs`)

Locked decisions (revised): **standalone Go daemon, no Telegraf codebase** ·
**`github.com/influxdata/influxdb-client-go/v2`** · **InfluxDB v2 only, v1 dropped** ·
**phased delivery, P0 = scaffold + auth + poll + positions** ·
**fix (don't mirror) the two confirmed Rust data bugs, documented as divergences**.

## 0. Scope

Standalone Go service at `go/` in this repo (own module, branch `feat/go-port`)
that mirrors the Rust telemetry pipeline: auth → poll → state machine → sessions →
enrichment → writes, direct to InfluxDB v2. The Rust HTTP API (`/api/auth/*`,
`/api/vehicles/*`, suspend/resume) and SSE stay in Rust or become a separate Go
service later (future work, §8). No Telegraf framework, no fork, no execd shim.

## 1. Layout + dependencies

- Module `github.com/henryouly/tesla-apiscraper-rs/go`, Go ≥1.22, layout:
  `go/main.go`, `go/config/`, `go/tesla/` (auth + Owner API clients),
  `go/vehicles/` (supervisor + task loop + state machine + sessions),
  `go/enrich/` (geocode + elevation), `go/store/` (InfluxDB v2 writer).
- Deps: **stdlib-first** (`net/http`, `encoding/json`, `crypto/aes`+GCM, `sync`,
  `time`, `log/slog`); external only: `influxdb-client-go/v2` (writes/queries),
  and later `nhooyr.io/websocket` for §6 (gorilla is archived).
- Config is **env vars mirroring Rust exactly** (zero new deps, compose-compatible).
  Only the InfluxDB block changes (v2 credentials replace v1 ones):

| Rust env | Go env | Notes |
|---|---|---|
| `INFLUXDB_URL` | same | v2 base URL, same port 8086 |
| `INFLUXDB_USERNAME/PASSWORD` | **dropped** | v2 has no Basic-auth writes |
| `INFLUXDB_DATABASE` | **dropped** | replaced by bucket below |
| — | `INFLUXDB_TOKEN` (required) | API token with write on the bucket (plus read for e2e) |
| — | `INFLUXDB_ORG` (required) | org name or ID |
| — | `INFLUXDB_BUCKET` (default `tesla`) | must pre-exist with DBRP mapping only if InfluxQL reads are needed; the app itself never reads Influx |
| everything else (`HOST/PORT/CONFIG_DIR/TESLA_*/DATA_ENCRYPTION_KEY/RUST_LOG→LOG_LEVEL/POLL_INTERVAL_SECONDS/STREAMING_ENABLED/…`) | same names/semantics | token file, YAML geofences/settings keep their schemas so existing `config/` works untouched |

## 2. InfluxDB v2 writer (`go/store/`, replaces `src/influxdb.rs`)

- `client := influxdb2.NewClient(url, token)` → `api := client.WriteAPI(org, bucket)`
  (async, batched, built-in retry). **Must drain `api.Errors()` in a goroutine and
  log** — otherwise write failures are silent.
- Points via `write.NewPoint(measurement, tags, fields, ts)` (`time.Time` from the
  same second-precision Unix timestamps; client default ns precision is fine).
- Health: `client.Ping(ctx)` + `client.Ready(ctx)` at startup (replaces `GET /ping`).
- **No `ensure_database` at all.** Buckets are operator-provisioned; the entire
  CREATE-vs-skip debate evaporates — startup is ping/ready, then go.
- Session upsert semantics preserved: initial row at session start + rewrite of the
  same series + timestamp at close overwrites in place (v2 point identity =
  measurement + tag set + timestamp, same as v1).
- `Option<T>` → omit field when nil (matches Rust LP omission); ints → `int64`,
  floats → `float64`, bools → `bool`, shift/state → strings.

## 3. Auth + API clients (direct port)

- `POST {auth_url}/oauth2/v3/token`, `grant_type=refresh_token`, scopes
  `openid email offline_access`; retry 3×, exp backoff 1s→120s on transport/5xx.
- Token file: same JSON shape + AES-256-GCM `nonce‖ciphertext` b64 layout as Rust
  (`token_file` env, default alongside YAML); refresh when `expires_at - now ≤ 3600s`
  at startup + 60s background loop; persist after each refresh.
- JWT region decode: split `.`, base64url payload, `aud` → `.cn` / `.eu` /
  `owner-api` / default URL table (mirror exactly).
- `GET {api}/api/1/products` → map by VIN; `GET …/vehicles/{id}/vehicle_data` →
  Go structs mirroring every serde field in `tesla_api.rs` (all optional/pointer,
  `response`-envelope unwrap, Bearer auth, non-2xx → typed error).
- Tests mirror Rust's cases with `httptest`: success, null sub-objects, asleep,
  401, 500, null GPS, region table.

## 4. Vehicle supervisor + state machine (goroutines, stdlib only)

- Supervisor: `map[VIN]*vehicleTask`, one goroutine per VIN, `context.Context`
  cancel for shutdown; per-vehicle `chan Command{Suspend,Resume,Shutdown}`.
- `VehicleState` string enum + **exact transition table** (`state.rs:37-65`) and
  `deriveNextState` precedence: api-state base → D/R → charging → charge-exit →
  updating guards. Apply change only if allowed (mirrors `task.rs:151-155`).
- Poll loop (`select` over command chan / timer / token updates): Suspended → skip;
  no token → skip; intervals **Driving 2.5s / Charging `clamp(250/power,5,20)s`
  else 5s / else `poll_interval`** (default 15s if zero).
- Suspend rules verbatim from `sleep.rs` + `task.rs:190-211` (blocked states,
  activity vetoes incl. `require_unlocked`, 21m idle + 15m since-resume; manual
  suspend bypasses checks).
- Positions emission per poll (dedup: skip when parked AND coords == last;
  parked+null → skip; driving+null → anchor `last_lat_lng`, elevation null).

## 5. Sessions (P1; same semantics, v2 writes)

| Series | Tags | Trigger |
|---|---|---|
| `drives` | `vin`, `drive_id={vin}_{ts}` | start row (start_* only) on D/R; close row on first non-Driving poll (Haversine, max/avg speed, energy, geocoded addresses, enter/exit geofences) |
| `charging_sessions` | `vin`, `charge_id={vin}_{ts}` | start/end rows; cost = per-kWh/per-minute + fee from charge-location geofence; interrupted-charge fallbacks + `.max(0)` clamp |
| `charge_readings` | `vin`, `charge_id` | **every** charging poll |
| `updates` | `vin`, `update_id={vin}_{now}` | installing → completed/cancelled (+ `prev_car_version`) |

**Divergences from Rust (locked: fix, don't mirror):**
- D1: drive `energy_used_wh` is kWh-scale + unclamped (Charging clamps, Driving
  doesn't). Port stores true Wh (`power_kW × dt / 3600 × 1000`, regen clamped).
  Grafana queries adjusting by ×1000 must be updated — note per panel.
- D2: `range` fields arrive in miles, mislabeled `*_km`. Port documents mile
  units; keep field names for series continuity, note per panel.

## 6. Streaming WS client (P3; port the *intended* 5.1)

`control:hello` (15s) → `data:subscribe_oauth{token, value: COLUMNS, tag}` with
**canonical** `speed,odometer,soc,elevation,est_heading,est_lat,est_lng,power,shift_state,range,est_range,heading`
(verified vs TeslaMate/TeslaPy/timdorr; bare `lat/lng` returns garbage) → CSV parse
(`est_*`→lat/lng/heading, raw kW/mph/miles/ms documented) → 30s no-data watchdog →
reconnect loop (transient `min(1s·2ⁿ,10s)`+jitter, long `min(15s·2ⁿ,30s)`, terminal
on token-expiry, ≥10-disconnect escalation, counter reset on productive
connection) → graceful `data:unsubscribe` + close 1000 → channel-fed data into the
vehicle loop. Region-aware URL (global vs `tesla.cn`). Gated by
`STREAMING_ENABLED` + per-car flag.

## 7. Enrichment (P2; direct port)

- Nominatim `reverse?lat&lon&format=json&addressdetails=0`, custom UA, 5s timeout;
  OpenTopoData SRTM, same timeout; truncated `"{:.4}_{:.4}"` map+mutex caches
  storing nil negatives, in-memory, no TTL; called at session close (addresses)
  and per-position when `elevation` null.

## 8. Explicit non-goals / follow-ups

HTTP API + sign-in + suspend/resume + SSE (companion service); Grafana dashboards
(same measurements/tags, adjust D1/D2 panels); MQTT + TeslaFi import.

## 9. Delivery phases (each independently runnable)

- **P0**: module scaffold + env config + auth + Owner API poll + supervisor/state
  machine + positions → v2. Run: `go run ./go` with the existing `.env` (+ v2 token vars).
- **P1**: sessions (§5) with start/close upsert semantics + D1 fix.
- **P2**: enrichment (§7) + geofence billing.
- **P3**: streaming client (§6) + state-machine feed.
- **P4**: hardening (structured logs, failure-path tests), compose service, docs.

## 10. Verification (mirror of Rust gates)

- `go build ./...`, `go vet ./...`, `gofmt -l .`, `go test ./...` (httptest suites
  per client, table-driven CSV/error/backoff/transition tests, session open/close
  golden points).
- Live: run against the real v2.8.0 (`INFLUXDB_URL/TOKEN/ORG/BUCKET` from `.env`),
  then Flux read-back via client `QueryAPI` (`from |> range |> filter`) to prove
  the round trip; metric-parity check (Rust vs Go output for the same drive window).
