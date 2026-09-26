# API Reference

Base URL is the HTTP server (`HOST:PORT`, default `0.0.0.0:4000`). Bodies are JSON, except the suspend/resume endpoints, which return an empty body on success and plain-text reasons on failure (see below).

The TypeScript mirror of these shapes lives in `web/src/lib/api.ts` — it carries the SPA's consumed subset (e.g. `signIn` drops `id_token`, `VehicleDiscovery` drops unused vehicle fields, `UiEvent` excludes `resync`, which is a separate callback), so update both together when the consumed shapes change.

## Authentication

No client login. Telemetry routes require *usable server-side tokens* (decryptable **and** unexpired, see `tokens_usable`); otherwise they return `401 {"error": "unauthorized"}`. The SPA redirects to `/signin` on 401. See the Security section in the root README for the trust model.

### `POST /api/auth/sign_in`

Bootstrap / recovery: mint and persist a token pair from a refresh token. Also broadcasts the fresh token to vehicle tasks and spawns tasks for newly discovered vehicles — no restart needed.

Request: `{ "refresh_token": string }`

Response `200`: `{ "access_token": string, "refresh_token": string, "expires_in": number, "id_token"?: string }`

Errors: `422` missing field · `400` invalid/expired refresh token (`invalid_grant`) · `502` upstream Tesla error · `503` network failure reaching Tesla (`upstream transport error: …`).

### `POST /api/auth/refresh`

Same contract as sign-in (request `{ "refresh_token" }`), same responses. Persists the new pair.

### `GET /api/auth/status`

Response `200`: `{ "authenticated": boolean }` — true only with usable stored tokens. Always public (the SPA's auth guard reads it).

## Vehicles

### `GET /api/vehicles`

Response `200`: `{ "vehicles": [{ "id", "vehicle_id", "vin", "display_name" | null, "state", "api_version", "in_service" }] }` — the discovery snapshot (Tesla-reported `state`, e.g. `"asleep"`).

### `GET /api/vehicles/summaries`

Response `200`: `{ "summaries": VehicleSummary[] }` — latest cached display state per vehicle (in-memory; seeded at task start, enriched from last-known InfluxDB telemetry when available).

### `GET /api/vehicles/{vin}/summary`

Response `200`: one `VehicleSummary`. `404 {"error": ...}` when the VIN has no cached summary yet.

### `GET /api/vehicles/{vin}/state`

Response `200`: `{ "vin": string, "state": VehicleState | null }` — the live state-machine state (`Start`, `Online`, `Driving`, `Charging`, `Updating`, `Asleep`, `Offline`, `Suspended`, `Error`; `null` for unknown VIN).

### `POST /api/vehicles/{vin}/suspend` · `POST /api/vehicles/{vin}/resume`

`204` on success. `404 "vehicle_not_found"` for unknown VIN. Suspend returns `409` with a reason (`"software update in progress"`, `"vehicle is driving"`, `"vehicle is charging"`) when the current state forbids it.

## Live events

### `GET /api/events` (SSE, `text/event-stream`)

Typed events with 15s keep-alive comments:

| Event | Payload |
|-------|---------|
| `summary` | Full `VehicleSummary` (`{ "type": "summary", "vin", "summary", "at" }`) |
| `state` | State-only (`{ "type": "state", "vin", "state", "at" }`) |
| `resync` | `{ "reason": "lagged" }` — client fell behind; refetch `/api/vehicles/summaries` |

Client merge rule, per source: fetched snapshots replace a VIN entry only when `summary.last_updated_at` is strictly newer (timestamps are server-issued unix seconds; the empty seed uses `0`). SSE `summary`/`state` events instead apply in broadcast arrival order, which is server-chronological; a lagged client receives `resync` and refetches.

`VehicleSummary`: `{ "vin", "display_name" | null, "state", "battery_level" | null, "battery_range" | null, "latitude" | null, "longitude" | null, "speed" | null, "odometer" | null, "last_updated_at" }` — range stays empty until live data (stored ranges are km, live follows vehicle units).

## Health

- `GET /health` → `200 {"status": "ok"}` (always public).
- `GET /health/ready` → `200 {"status": "ok"}` or `503 {"status": "error", "error": ...}` (InfluxDB reachability).

## Web UI serving

When `WEB_DIST_DIR` points at a built SPA, `/` serves it: assets directly, unknown non-API paths fall back to `index.html` (client-side routes), unknown `/api/*` and `/health*` paths still 404. Unset or missing `index.html` = API-only mode.
