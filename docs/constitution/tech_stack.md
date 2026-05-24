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
| **Testing** | `#[test]` + `rstest` + `wiremock` | `rstest` for parameterized/fixture-based tests. `sqlx::test` for isolated SQLite test fixtures. `wiremock` for HTTP mocking. |
| **CSS/Sass/JS Bundling** | `grass` (Sass compiler) + `swc` or `esbuild` (JS minifier) | Compile frontend assets as part of `cargo build`. No Node.js dependency in the builder image unless the SolidJS SPA is built separately. |
| **CLI** | `clap` derive | If a CLI subcommand is needed (run server, run migrations, import data). |

## Database

The data model is split across two databases, each chosen for the workload it handles best.

| Concern | Choice | Rationale |
|---------|--------|-----------|
| **Time-Series Store** | InfluxDB 2.x | Purpose-built for append-heavy, timestamped data. Native downsampling/retention policies, Flux queries, and efficient storage for the high-volume position + charge readings. No need for TimescaleDB's added complexity. |
| **Relational Store** | SQLite (via `sqlx`) | File-based, zero-infrastructure relational DB for metadata: cars, settings, encrypted tokens, geo-fences, address cache. `sqlx` gives compile-time SQL verification via `sqlx::query!`. No separate Docker container needed — SQLite lives on a Docker volume. |
| **Time-Series Schema** | One InfluxDB bucket (`tesla`), two measurements: `positions` (lat, lng, speed, power, odometer, battery_level, battery_ranges, temperatures, TPMS, HVAC status) and `charges` (voltage, current, power, phases, energy_added, battery_level, range). Tags: `car_id`, `vin`. Fields: all numeric telemetry. | InfluxDB's tag/field model maps naturally: low-cardinality tags for filtering (which car), fields for the numeric telemetry. No schema migrations for adding new fields — just start writing them. |
| **Relational Schema** | Standard SQLite tables: `cars`, `settings`, `tokens`, `geofences`, `addresses`, `drives`, `charging_processes`, `updates`. | These are low-volume, relationally connected records. Foreign keys, unique constraints, and joins work naturally in SQLite. |
| **SQLite Driver** | `sqlx` (async, compile-time SQL verification) | `sqlx` with `sqlite` feature provides compile-time query checking via `sqlx::query!`. Same crate as the wider Rust ecosystem. Async pool via `sqlx::SqlitePool`. |
| **InfluxDB Driver** | `influxdb` (async) | Async Rust client for InfluxDB 2.x write and query APIs. Uses `reqwest` under the hood. Batched writes for throughput. |
| **SQLite Migrations** | `sqlx migrate run` | SQL migration files (`.up.sql` / `.down.sql`) version-controlled alongside the code. Run at startup. |
| **InfluxDB Bucket Setup** | Auto-create bucket + retention policy on first run via InfluxDB HTTP API | No migration framework needed — just ensure the bucket exists and set a retention period (default: 0 = infinite for self-hosted). |

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
| **Orchestration** | Docker Compose (reference) | Standard `compose.yml` with three services: tesla-apiscraper-rs, influxdb, grafana. SQLite data is persisted on a Docker volume. |
| **Port** | `4000` (web UI), SSE on same port | Matches existing convention. |
| **Health Check** | `GET /health` | Returns 200, used by Docker healthcheck and orchestrators. |

## What We Won't Use (and Why)

| Library/Tool | Why Not |
|--------------|---------|
| **Actix-Web** | `axum` is simpler, has better ergonomics (no actor system), and first-class SSE. Actix introduces unnecessary complexity for this use case. |
| **Diesel** | ORM abstraction over SQL. Time-series + spatial queries need precise control. `sqlx` gives compile-time SQL verification without hiding the query. |
| **ORM** (any) | ORMs obscure SQL. Raw queries give full control over time-range bucketing and spatial filtering. |
| **GraphQL** | Overkill for this use case. REST + SSE covers all needs. |
| **React / Vue / Svelte** | SolidJS is more performant for fine-grained reactive updates (car status changing every second) and has a smaller bundle. |
| **Redis / Message Queue** | No need for a broker. `tokio` channels handle all internal messaging. SQLite handles persistence. |
| **gRPC / Tonic** | The frontend is a browser; REST + SSE are universally supported without protobuf tooling. |
| **PostgreSQL / TimescaleDB** | Adds a full RDBMS + extensions for data that fits naturally into a time-series DB (positions, charges) and a file-based SQLite (metadata). Two small DBs beat one heavy one for this workload. |
| **Rocket** | Requires nightly Rust. `axum` works on stable and has a larger ecosystem. |
