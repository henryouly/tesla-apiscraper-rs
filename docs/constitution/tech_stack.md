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
| **Testing** | `#[test]` + `rstest` + `sqlx::test` + `wiremock` | `rstest` for parameterized/fixture-based tests. `sqlx::test` for isolated DB test fixtures. `wiremock` for HTTP mocking. |
| **CSS/Sass/JS Bundling** | `grass` (Sass compiler) + `swc` or `esbuild` (JS minifier) | Compile frontend assets as part of `cargo build`. No Node.js dependency in the builder image unless the SolidJS SPA is built separately. |
| **CLI** | `clap` derive | If a CLI subcommand is needed (run server, run migrations, import data). |

## Database

| Concern | Choice | Rationale |
|---------|--------|-----------|
| **Primary Database** | PostgreSQL 16+ | Required for PostGIS spatial queries and proven reliability with large datasets. |
| **Time-Series Extension** | TimescaleDB | Compresses old telemetry, enables automatic partitioning (hypertables), and provides time-bucket functions for Grafana dashboards. Dramatically better query performance on multi-year position data than vanilla PostgreSQL. |
| **Spatial Extension** | PostGIS | Required for geo-fencing (`ST_DWithin`, `ST_Distance`), address proximity lookups, and visited-location analysis. |
| **Driver** | `sqlx` (async, compile-time SQL verification) | Compile-time SQL query verification against a live or offline database — catches schema mismatches at build time. Async pool with `sqlx::PgPool`. Supports `COPY` for bulk inserts. |
| **Query Approach** | Raw SQL via `sqlx::query_as!` macro | `query_as!` generates typed structs from SQL queries at compile time. No ORM. Full control over SQL while maintaining type safety. |
| **Migrations** | `sqlx-cli migrate` or `refinery` | `sqlx migrate` runs raw SQL migration files (`.up.sql` / `.down.sql`). If more control is needed (programmatic migrations), `refinery` is the Rust-native alternative. |
| **Schema Design** | Hypertables for `positions` and `charges`; regular tables for `cars`, `drives`, `charging_processes`, `states`, `updates`, `addresses`, `geofences`, `settings`, `tokens`. | Hypertables on the two highest-volume append-only tables (positions at ~1/sec, charges during charging). Everything else is low-volume and benefits from traditional relational modeling (joins, foreign keys, aggregates). |

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
| **Datasource** | PostgreSQL connector (built-in) | Queries TimescaleDB hypertables directly. |
| **Dashboards** | Port the existing 20+ JSON dashboards | Keep the same visual layout; update queries for TimescaleDB hypertables and any schema changes. |
| **Provisioning** | Grafana provisioning YAML (`datasources`, `dashboards`) | Automatically loaded at container startup. No manual setup required. |
| **Image** | Custom `Dockerfile` based on `grafana/grafana` | Adds TeslaMate logo, favicon, and provisioning files. |

## Deployment

| Concern | Choice | Rationale |
|---------|--------|-----------|
| **Containerization** | Docker (fully static Rust binary in `scratch`) | Minimal attack surface, tiny image (~10-20 MB). Multi-stage build: Rust compiler stage (cargo-chef for dependency caching, sccache) → `scratch` runtime with musl-linked static binary. |
| **Orchestration** | Docker Compose (reference) | Standard `compose.yml` with three services: teslamate-rs, postgres (with TimescaleDB + PostGIS), grafana. |
| **Port** | `4000` (web UI), SSE on same port | Matches existing convention. |
| **Health Check** | `GET /health` | Returns 200, used by Docker healthcheck and orchestrators. |

## What We Won't Use (and Why)

| Library/Tool | Why Not |
|--------------|---------|
| **Actix-Web** | `axum` is simpler, has better ergonomics (no actor system), and first-class SSE. Actix introduces unnecessary complexity for this use case. |
| **Diesel** | ORM abstraction over SQL. For time-series + spatial queries, we need precise control. `sqlx` gives compile-time SQL verification without hiding the query. |
| **ORM** (any) | ORMs obscure SQL. Copy protocol bulk inserts, hypertable chunks, and PostGIS spatial queries need raw SQL control. |
| **GraphQL** | Overkill for this use case. REST + SSE covers all needs. |
| **React / Vue / Svelte** | SolidJS is more performant for fine-grained reactive updates (car status changing every second) and has a smaller bundle. |
| **Redis / Message Queue** | No need for a broker. `tokio` channels handle all internal messaging. PostgreSQL handles persistence. |
| **gRPC / Tonic** | The frontend is a browser; REST + SSE are universally supported without protobuf tooling. |
| **InfluxDB** | TimescaleDB on PostgreSQL gives time-series performance without sacrificing spatial queries, relational integrity, and the existing Grafana Postgres connector. |
| **Rocket** | Requires nightly Rust. `axum` works on stable and has a larger ecosystem. |
