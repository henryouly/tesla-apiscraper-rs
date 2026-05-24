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
| **HTTP Client** | `reqwest` | De facto async HTTP client. Connection pooling, TLS, cookie store, redirect following. Used for Tesla API, Nominatim, GitHub releases. |
| **WebSocket Client (Streaming API)** | `tokio-tungstenite` | Async, low-level WebSocket built on `tungstenite`. Handles connect, reconnect with exponential backoff, ping/pong, and clean shutdown. |
| **MQTT Client** | `rumqttc` | Pure-Rust async MQTT client. Supports MQTT 3.1.1/5.0, retained messages, TLS, QoS levels. Integrates with `tokio` event loop. |
| **State Machine** | Custom `enum` + `tokio::select!` loop | Rust's `enum` with exhaustive `match` maps perfectly to vehicle states. Each vehicle gets a `tokio::spawn` task with a `tokio::select!` loop over API polls, streaming data, timers, and a channel for external commands (suspend, resume, settings changes). |
| **Structured Logging** | `tracing` + `tracing-subscriber` | Structured, span-based logging. JSON output in production (`tracing-subscriber` with JSON layer), compact output in development. Span per vehicle with VIN, state, and request ID. |
| **Error Handling** | `thiserror` + `anyhow` (or `eyre`) | `thiserror` for library-level, exhaustive error types. `anyhow`/`eyre` for application-level error propagation. |
| **Configuration** | `figment` or `envy` | Parse environment variables into a typed config struct. Supports nested configs, defaults, and validation. |
| **Serialization** | `serde` + `serde_json` | De facto standard. Derive `Serialize`/`Deserialize` on all structs. Fast, zero-copy where possible. |
| **Encryption (API tokens)** | `aes-gcm` + `ring` | AES-256-GCM for encrypting Tesla API tokens at rest. `ring` for secure random key generation. |
| **Time & Date** | `chrono` + `time` | Full timezone support, duration arithmetic. Parse Tesla API timestamps. |
| **Testing** | `#[test]` + `rstest` + `wiremock` | `rstest` for parameterized/fixture-based tests. `wiremock` for HTTP mocking. |
| **CSS/Sass/JS Bundling** | `grass` (Sass compiler) + `swc` or `esbuild` (JS minifier) | Compile frontend assets as part of `cargo build`. No Node.js dependency in the builder image unless the SolidJS SPA is built separately. |
| **CLI** | `clap` derive | If a CLI subcommand is needed (run server, import data). |

## Database

All data lives in InfluxDB 2.x — no SQLite, no PostgreSQL. Configuration (geofences, settings, OAuth tokens) is stored as YAML files on disk.

### InfluxDB

| Concern | Choice | Rationale |
|---------|--------|-----------|
| **Time-Series Store** | InfluxDB 2.x | Purpose-built for append-heavy, timestamped data. Native downsampling/retention policies, Flux queries, and efficient storage. All measurements in a single `tesla` bucket. |
| **Driver** | `influxdb` (async) | Async Rust client for InfluxDB 2.x write and query APIs. Uses `reqwest` under the hood. Batched writes for throughput. |
| **Bucket Setup** | Auto-create bucket on first run via InfluxDB HTTP API | Ensure the `tesla` bucket exists at startup. Set retention period (default: 0 = infinite for self-hosted). |

#### InfluxDB Measurements

| Measurement | Tags | Fields | Description |
|-------------|------|--------|-------------|
| `positions` | `car_id`, `vin` | `lat`, `lng`, `speed`, `power`, `odometer`, `battery_level`, `battery_range_rated`, `battery_range_ideal`, `outside_temp`, `inside_temp`, `tpms_fl`, `tpms_fr`, `tpms_rl`, `tpms_rr`, `fan_status`, `blower_status`, `defroster_status`, `heading`, `est_lat`, `est_lng` | Raw GPS + telemetry (1 Hz) |
| `charge_readings` | `car_id`, `vin` | `voltage`, `current`, `power`, `phases`, `energy_added`, `battery_level`, `battery_range` | Individual charge data points during a session |
| `drives` | `car_id`, `drive_id` | `start_date`, `end_date`, `start_km`, `end_km`, `distance_km`, `duration_minutes`, `average_speed`, `max_speed`, `energy_used_kwh`, `outside_temp_avg`, `inside_temp_avg`, `fan_max_rpm`, `start_lat`, `start_lng`, `end_lat`, `end_lng`, `start_address`, `end_address`, `start_geofence`, `end_geofence` | Aggregated drive sessions (partial on start, overwritten on end) |
| `charging_sessions` | `car_id`, `charge_id` | `start_date`, `end_date`, `start_battery_level`, `end_battery_level`, `start_range`, `end_range`, `charger_phases`, `charger_power`, `energy_added_kwh`, `energy_used_kwh`, `duration_minutes`, `cost_per_kwh`, `cost_per_min`, `session_fee`, `cost`, `lat`, `lng`, `address`, `geofence` | Aggregated charge sessions (partial on start, overwritten on end) |
| `states` | `car_id`, `state` | `duration_seconds` | Vehicle state transitions (online, asleep, driving, charging, etc.) |
| `updates` | `car_id`, `version` | `status`, `install_start_date`, `install_end_date` | Software update install events |

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
| **Real-Time Updates** | Server-Sent Events (SSE) | Simpler than WebSockets for unidirectional server→client updates. `axum` has first-class SSE via `axum::response::Sse`. The Rust server pushes vehicle state changes; the SolidJS client re-renders reactively. No need for bidirectional communication (the UI only reads data, it doesn't control the car). |
| **Icons** | Lucide or Material Design Icons | Lightweight, tree-shakeable SVG icons. |
| **Bundle Size Target** | < 200 KB gzipped | Keep the frontend lean for fast initial loads on mobile. |

## API Design

| Concern | Choice | Rationale |
|---------|--------|-----------|
| **Protocol** | REST + SSE | REST for CRUD operations (settings, geo-fences, charge costs), SSE for live vehicle state. |
| **Serialization** | JSON via `serde_json` | Universal, human-readable, matches the existing API contract. |
| **Documentation** | OpenAPI 3.1 via `utoipa` | Derive OpenAPI schemas from Rust structs and axum handlers. Swagger UI served at `/docs`. |
| **SSE Endpoint** | `GET /api/v1/events?car_id=1` | Persistent connection streaming JSON-encoded events (position updates, state changes, drive/charge start/stop). Client filters by event type. |

## Grafana

| Concern | Choice | Rationale |
|---------|--------|-----------|
| **Version** | Grafana 13+ (latest stable) | Bundled as a separate Docker container (same pattern as existing). |
| **Datasource** | InfluxDB connector (built-in) | Queries the `tesla` bucket directly via Flux. |
| **Dashboards** | Port the existing 20+ JSON dashboards | Keep the same visual layout; update queries from PostgreSQL to Flux for InfluxDB. |
| **Provisioning** | Grafana provisioning YAML (`datasources`, `dashboards`) | Automatically loaded at container startup. No manual setup required. |
| **Image** | Custom `Dockerfile` based on `grafana/grafana` | Adds project logo, favicon, and provisioning files. |

## Deployment

| Concern | Choice | Rationale |
|---------|--------|-----------|
| **Containerization** | Docker (fully static Rust binary in `scratch`) | Minimal attack surface, tiny image (~10-20 MB). Multi-stage build: Rust compiler stage (cargo-chef for dependency caching, sccache) → `scratch` runtime with musl-linked static binary. |
| **Orchestration** | Docker Compose (reference) | Standard `compose.yml` with three services: tesla-apiscraper-rs, influxdb, grafana. YAML config files and InfluxDB data are persisted on Docker volumes. |
| **Port** | `4000` (web UI), SSE on same port | Matches existing convention. |
| **Health Check** | `GET /health` | Returns 200, used by Docker healthcheck and orchestrators. |

## What We Won't Use (and Why)

| Library/Tool | Why Not |
|--------------|---------|
| **Actix-Web** | `axum` is simpler, has better ergonomics (no actor system), and first-class SSE. Actix introduces unnecessary complexity for this use case. |
| **Diesel** | ORM abstraction over SQL. We don't have a SQL database — all queries are Flux (InfluxDB). |
| **ORM** (any) | No relational database — no ORM needed. |
| **GraphQL** | Overkill for this use case. REST + SSE covers all needs. |
| **React / Vue / Svelte** | SolidJS is more performant for fine-grained reactive updates (car status changing every second) and has a smaller bundle. |
| **Redis / Message Queue** | No need for a broker. `tokio` channels handle all internal messaging. |
| **gRPC / Tonic** | The frontend is a browser; REST + SSE are universally supported without protobuf tooling. |
| **PostgreSQL / TimescaleDB / SQLite** | Adds an RDBMS for data that fits naturally into InfluxDB (time-series) + YAML files (configuration). Two small stores beat one heavy one for this workload. |
| **Rocket** | Requires nightly Rust. `axum` works on stable and has a larger ecosystem. |
