# Tech Stack

## Language & Runtime

| Component | Choice | Rationale |
|-----------|--------|-----------|
| **Language** | Rust (stable) | Zero-cost abstractions, no GC, exhaustive `match` on state transitions. Excellent async ecosystem (`tokio`). Single static binary, tiny memory footprint — ideal for Raspberry Pi. |
| **Compiler** | `rustc` via `cargo` | Cross-compilation with `cross` or `--target` for ARM (Raspberry Pi) and x86_64 musl. |
| **Async Runtime** | `tokio` (multi-threaded, work-stealing) | De facto standard. Drives the HTTP server, WebSocket streams, MQTT client, and all I/O. One `tokio::spawn` task per vehicle for the state machine loop. |
| **Build** | `cargo build --release` + Docker multi-stage | `cargo` for local dev and dependency management. Multi-stage Dockerfile: Rust builder (sccache, cargo-chef) → `scratch` runtime with a fully static musl binary. |

## Backend Framework & Libraries

| Concern | Choice | Rationale |
|---------|--------|-----------|
| **Web Framework** | `axum` | Tower-based, typesafe extractors, first-class SSE support, idiomatic. `axum::extract::State` for shared app state (DB pool, config). |
| **HTTP Client** | `reqwest` | De facto async HTTP client. Connection pooling, rustls TLS. Built with `default-features = false` (`json` + `rustls-tls` only). Used for Tesla API, Nominatim, and InfluxDB HTTP. |
| **WebSocket Client (Streaming API)** | `tokio-tungstenite` | Async, low-level WebSocket built on `tungstenite`. Handles connect, reconnect with exponential backoff, ping/pong, and clean shutdown. |
| **MQTT Client** | `rumqttc` | Pure-Rust async MQTT client. Supports MQTT 3.1.1/5.0, retained messages, QoS levels. Plain TCP only by design — no TLS mode (remote access belongs one layer down, e.g. VPN/Tailscale). Integrates with `tokio` event loop. |
| **State Machine** | Custom `enum` + `tokio::select!` loop | Rust's `enum` with exhaustive `match` maps perfectly to vehicle states. Each vehicle gets a `tokio::spawn` task with a `tokio::select!` loop over API polls, streaming data, timers, and a channel for external commands (suspend, resume, settings changes). |
| **Structured Logging** | `tracing` + `tracing-subscriber` | Structured, span-based logging. JSON output in production (`tracing-subscriber` with JSON layer), compact output in development. Span per vehicle with VIN, state, and request ID. |
| **Error Handling** | `thiserror` + `anyhow` (or `eyre`) | `thiserror` for library-level, exhaustive error types. `anyhow`/`eyre` for application-level error propagation. |
| **Configuration** | `figment` or `envy` | Parse environment variables into a typed config struct. Supports nested configs, defaults, and validation. |
| **Serialization** | `serde` + `serde_json` | De facto standard. Derive `Serialize`/`Deserialize` on all structs. Fast, zero-copy where possible. |
| **Encryption (API tokens)** | `aes-gcm` + `rand` | AES-256-GCM for encrypting Tesla API tokens at rest. The key comes from `DATA_ENCRYPTION_KEY` config; `rand` mints the per-message nonces. |
| **Time & Date** | `chrono` + `time` | Full timezone support, duration arithmetic. Parse Tesla API timestamps. |
| **Testing** | `#[test]` + `wiremock` | Built-in test harness. `wiremock` for HTTP mocking. |
| **CSS/Sass/JS Bundling** | Tailwind CSS v4 via `@tailwindcss/vite` + `tsc` | Frontend built by the `node:22-alpine` Docker stage (`npm run build` → `web/dist`), baked into the runtime image and served by the Rust binary. |
| **CLI** | `clap` derive | If a CLI subcommand is needed (run server, import data). |

## Database

All data lives in InfluxDB v1 — no SQLite, no PostgreSQL. Configuration (geofences, settings, OAuth tokens) is stored as YAML files on disk.

### InfluxDB

| Concern | Choice | Rationale |
|---------|--------|-----------|
| **Time-Series Store** | InfluxDB v1 | Purpose-built for append-heavy, timestamped data. InfluxQL queries, retention policies, and efficient storage. All measurements in a single `tesla` database. |
| **Driver** | `reqwest` (HTTP) + `influxdb` crate (derive + line protocol) | The `influxdb` crate provides `InfluxDbWriteable` derive + `WriteQuery`/`Timestamp`/`Query` types for building line protocol. All HTTP calls (ping, write, query) go directly through `reqwest`. |
| **Database Setup** | Auto-create database on first run via v1 query API (`CREATE DATABASE`, idempotent no-op if it exists) | Ensure the `tesla` database exists at startup. |

#### InfluxDB Measurements

| Measurement | Tags | Fields | Description |
|-------------|------|--------|-------------|
| `positions` | `car_id`, `vin` | `latitude`, `longitude`, `speed`, `power`, `odometer`, `battery_level`, `rated_battery_range_km`, `ideal_battery_range_km`, `est_battery_range_km`, `usable_battery_level`, `outside_temp`, `inside_temp`, `heading`, `elevation`, `shift_state`, `tpms_pressure_fl`, `tpms_pressure_fr`, `tpms_pressure_rl`, `tpms_pressure_rr`, `fan_status`, `is_front_defroster_on`, `is_rear_defroster_on`, `is_climate_on`, `driver_temp_setting`, `passenger_temp_setting`, `battery_heater`, `battery_heater_on`, `battery_heater_no_power`, `is_preconditioning`, `climate_keeper_mode`, `locked`, `is_user_present`, `sentry_mode` | Raw GPS + telemetry (polled, ~1-60s interval) |
| `charge_readings` | `vin`, `charge_id` | `voltage`, `current`, `power`, `phases`, `energy_added`, `battery_level`, `battery_range`, `charger_power`, `charger_voltage`, `charger_phases`, `outside_temp`, `fast_charger_brand`, `fast_charger_type`, `conn_charge_cable`, `usable_battery_level`, `charger_pilot_current`, `fast_charger_present`, `battery_heater_on`, `not_enough_power_to_heat`, `ideal_battery_range`, `rated_battery_range` | Individual charge data points during a session |
| `drives` | `vin`, `drive_id` | `start_lat`, `start_lng`, `end_lat`, `end_lng`, `start_address`, `end_address`, `start_time`, `end_time`, `distance_meters`, `duration_seconds`, `energy_used_wh`, `max_speed`, `average_speed`, `outside_temp_avg`, `inside_temp_avg`, `geofence_enter`, `geofence_exit`, `is_merged` | Aggregated drive sessions (partial on start, overwritten on end) |
| `charging_sessions` | `vin`, `charge_id` | `start_lat`, `start_lng`, `end_lat`, `end_lng`, `start_address`, `start_range`, `end_range`, `start_rated_range`, `end_rated_range`, `start_battery_level`, `end_battery_level`, `energy_added_wh`, `duration_seconds`, `cost`, `geofence_id`, `geofence_name`, `charge_energy_used`, `connector_type`, `outside_temp_avg`, `inside_temp_avg` | Aggregated charge sessions (partial on start, overwritten on end) |
| `states` | `vin` | `state`, `inside_temp`, `outside_temp`, `battery_level`, `locked`, `sentry_mode`, `dog_mode`, `cabin_overheat_protection` | Vehicle state schema (defined + serialization-tested; no production writes yet) |
| `updates` | `vin`, `update_id` | `version_before`, `version_after`, `install_start`, `install_end`, `status`, `abandoned` | Software update install events |

#### Update-on-close pattern

For `drives` and `charging_sessions`, the app writes a point with partial data when the session begins. When the session ends, it writes the same measurement + tag set + timestamp with all fields populated — InfluxDB upserts (overwrites) the point. This avoids the need for an UPDATE-capable relational store.

### YAML Config Files

| Concern | Choice | Rationale |
|---------|--------|-----------|
| **Format** | YAML via `serde_yaml` | Human-readable, editable by hand or via the web UI. Parsed into typed Rust structs at startup. Auto-saved when modified via API. |
| **Files** | `config/geofences.yml`, `config/settings.yml`, `config/tokens.yml` | Three files on a Docker volume. `tokens.yml` contains encrypted OAuth tokens (AES-256-GCM), auto-written by the auth flow. `settings.yml` supports both global and per-car overrides keyed by VIN. |

#### Vehicle Identity

Cars are discovered from the Tesla API on startup (`GET /api/1/products`) and kept in memory as `Vehicle` structs. VIN is the stable identifier used across all InfluxDB tags and YAML config keys. No `cars` table needed.

## Frontend

| Concern | Choice | Rationale |
|---------|--------|-----------|
| **Framework** | SolidJS | Reactive primitives with no virtual DOM. Compiles to efficient direct DOM updates. Tiny bundle. |
| **Language** | TypeScript | Type safety across the frontend codebase. |
| **Build Tool** | Vite | Fast HMR, SolidJS plugin, CSS/asset bundling, production optimization. |
| **Routing** | `@solidjs/router` | Official router. Simple, reactive, works with lazy-loaded routes. |
| **CSS Framework** | Tailwind CSS | Utility-first. Avoids the CSS complexity of Bulma. Pairs well with SolidJS's component model. |
| **Maps** | Leaflet + leaflet-draw (or MapLibre GL) | Free, open-source, well-supported. Leaflet is the path of least resistance since the existing codebase already uses it. MapLibre GL is a modern alternative worth evaluating. |
| **Map Tiles** | OpenStreetMap (raster) or self-hosted | Consistent with the self-hosted ethos. |
| **Real-Time Updates** | Server-Sent Events (SSE) | Simpler than WebSockets for server→client updates. `axum` serves them via `axum::response::Sse`; the SolidJS client re-renders reactively. Mostly reads, plus small control actions (suspend/resume logging via REST). |
| **Icons** | Lucide or Material Design Icons | Lightweight, tree-shakeable SVG icons. |
| **Bundle Size Target** | < 200 KB gzipped | Keep the frontend lean for fast initial loads on mobile. |

## API Design

| Concern | Choice | Rationale |
|---------|--------|-----------|
| **Protocol** | REST + SSE | REST for CRUD operations (settings, geo-fences, charge costs), SSE for live vehicle state. |
| **Serialization** | JSON via `serde_json` | Universal, human-readable, matches the existing API contract. |
| **Documentation** | OpenAPI 3.1 via `utoipa` (planned) | Derive OpenAPI schemas from Rust structs and axum handlers. Swagger UI to be served at `/docs`. |
| **SSE Endpoint** | `GET /api/events` | Persistent connection streaming typed JSON events (`summary`, `state`, `resync` hint) with keep-alive. Fetched snapshots merge by strict server-timestamp comparison; live events apply in broadcast arrival order (lagged clients refetch); see `docs/api.md`. |

## Grafana

| Concern | Choice | Rationale |
|---------|--------|-----------|
| **Version** | Stock `grafana/grafana:latest` image | Bundled as a separate Docker container (same pattern as existing). No custom image — branding deferred (see roadmap 9.3); provisioning files mount from `./grafana/...` via compose volumes. |
| **Datasource** | InfluxDB connector (built-in) | Queries the `tesla` database directly via InfluxQL. |
| **Dashboards** | 15 JSON dashboards ported from the TeslaMate set | Same visual layout; queries rewritten from PostgreSQL to InfluxQL on InfluxDB v1. The remainder was consciously declined (needs window functions/joins InfluxQL lacks) — see roadmap Phase 9. |
| **Provisioning** | Grafana provisioning YAML (`datasources`, `dashboards`) | Automatically loaded at container startup. No manual setup required. |
| **Image** | Stock image, no custom Dockerfile | Provisioning files mount from `./grafana/...`; a branded image is deferred until branding matters. |

## Deployment

| Concern | Choice | Rationale |
|---------|--------|-----------|
| **Containerization** | Docker (fully static Rust binary in `scratch`) | Minimal attack surface, tiny image (~10-20 MB). Multi-stage build: Rust compiler stage (cargo-chef for dependency caching, sccache) → `scratch` runtime with musl-linked static binary. |
| **Orchestration** | Docker Compose (reference) | Standard `docker-compose.yml` with four services: tesla-apiscraper-rs, influxdb, mosquitto, grafana. YAML config files and InfluxDB data are persisted on Docker volumes. |
| **Port** | `4000` (web UI), SSE on same port | Matches existing convention. |
| **Health Check** | `GET /health` | Returns 200, used by Docker healthcheck and orchestrators. |

## What We Won't Use (and Why)

| Library/Tool | Why Not |
|--------------|---------|
| **Actix-Web** | `axum` is simpler, has better ergonomics (no actor system), and first-class SSE. Actix introduces unnecessary complexity for this use case. |
| **Diesel** | ORM abstraction over SQL. We don't have a relational SQL database — all queries go through InfluxDB (SQL / InfluxQL). |
| **ORM** (any) | No relational database — no ORM needed. |
| **GraphQL** | Overkill for this use case. REST + SSE covers all needs. |
| **React / Vue / Svelte** | SolidJS is more performant for fine-grained reactive updates (car status changing every second) and has a smaller bundle. |
| **Redis / Message Queue** | No need for a broker. `tokio` channels handle all internal messaging. |
| **gRPC / Tonic** | The frontend is a browser; REST + SSE are universally supported without protobuf tooling. |
| **PostgreSQL / TimescaleDB / SQLite** | Adds an RDBMS for data that fits naturally into InfluxDB (time-series) + YAML files (configuration). Two small stores beat one heavy one for this workload. |
| **Rocket** | Requires nightly Rust. `axum` works on stable and has a larger ecosystem. |
