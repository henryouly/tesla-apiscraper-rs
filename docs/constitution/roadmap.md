# Roadmap

Each phase is a self-contained deliverable. Phases are ordered by dependency: foundational subsystems come first, then data collection, then enrichment, then the UI, then integrations and polish.

---

## Phase 1: Project Foundation

**Goal:** A running Rust binary in a container that connects to PostgreSQL, applies schema migrations, and responds to health checks. Nothing else.

### 1.1 Project Scaffolding
- Initialize Rust project (`cargo init`)
- `Makefile` with targets: `build`, `run`, `test`, `lint`, `docker-build`, `docker-run`
- `Dockerfile` — multi-stage build (cargo-chef dependency caching → Rust builder → `scratch` with musl static binary)
- `docker-compose.yml` with `teslamate-rs`, `postgres` (TimescaleDB + PostGIS), `grafana` services
- `clippy` + `rustfmt` for linting and formatting
- `.github/workflows/ci.yml` — clippy, fmt check, test, build

### 1.2 Configuration System
- Define `Config` struct (with `serde::Deserialize`) for all environment variables
- `figment` or `envy` for parsing
- Sensible defaults for development
- Validate required fields at startup (`DATABASE_URL`, `TESLA_API_CLIENT_ID`, etc.)

### 1.3 Database Layer
- `sqlx::PgPool` async connection pool setup
- `sqlx migrate` or `refinery` for schema migrations
- Migration files:
  - `001_create_extensions` — TimescaleDB, PostGIS, earthdistance, citext
  - `002_create_cars` — vehicle identity
  - `003_create_positions` — hypertable for GPS/telemetry
  - `004_create_charges` — hypertable for individual charge readings
  - `005_create_drives` — completed drive sessions with aggregates
  - `006_create_charging_processes` — completed charging sessions with aggregates
  - `007_create_states` — vehicle online/offline/asleep state history
  - `008_create_updates` — software update installs
  - `009_create_addresses` — OpenStreetMap address cache
  - `010_create_geofences` — user-defined geofences with billing config
  - `011_create_settings` — global settings, car settings
  - `012_create_tokens` — encrypted Tesla API tokens
- `sqlx::query_as!` macro for compile-time verified, typed queries
- Run migrations automatically at startup

### 1.4 HTTP Server Skeleton
- `axum` router with middleware: request ID, structured logging, CORS
- `GET /health` — returns 200
- `GET /health/ready` — checks database connectivity
- Graceful shutdown on SIGTERM/SIGINT

### 1.5 Structured Logging
- `tracing` + `tracing-subscriber` configured for JSON (production) or compact text (development)
- Span per request/vehicle with propagated trace ID

---

## Phase 2: API Authentication

**Goal:** Authenticate with the Tesla API, store and refresh OAuth tokens, and list the owner's vehicles.

### 2.1 Tesla Auth Client
- OAuth 2.0 token exchange (POST to `auth.tesla.com`)
- JWT decoding to determine region (global vs. China)
- Token refresh with automatic retry
- Circuit breaker for auth failures (exponential backoff)

### 2.2 Token Persistence
- Encrypt access token and refresh token with AES-256-GCM
- Store encrypted tokens in PostgreSQL
- Load tokens at startup; if valid, skip re-authentication
- Auto-refresh when tokens approach expiry (75% of `expires_in`)

### 2.3 Vehicle Discovery
- `GET /api/1/products` to list vehicles on the account
- Parse response into `Vehicle` structs (VIN, display name, model, config)
- Create/update `cars` rows in the database

---

## Phase 3: Vehicle State Machine & Telemetry Collection

**Goal:** The core state machine drives per-vehicle data collection, transitioning through online/driving/charging/asleep/offline states and recording telemetry to the database.

### 3.1 Vehicle State Machine
- One `tokio::spawn` task per vehicle, managed by a `Vehicles` supervisor task
- States modeled as a Rust `enum`: `Start` → `Online` → `{Driving, Charging, Updating, Asleep, Offline, Suspended}`
- State transitions driven by API responses, `tokio::select!` over timers and channels
- Graceful handling of vehicle removal and addition at runtime
- Circuit breaker for API errors per vehicle

### 3.2 REST Polling
- Periodic polling of `GET /api/1/vehicles/{id}/vehicle_data` (configurable interval)
- Wake-up logic: minimal calls to the wake endpoint when the vehicle is asleep
- Smart sleep detection: distinguish true asleep from brief subsystem checks by examining power draw

### 3.3 Position Logging
- Insert GPS position rows into the `positions` hypertable
- Capture: lat/lng, speed, power, odometer, battery level, battery ranges, temperatures, TPMS pressures, fan/blower/defroster status
- Deduplicate consecutive identical positions
- Batch insert via COPY protocol for throughput

### 3.4 Drive Detection
- Detect drive start: shift state enters D or R, speed > 0
- Detect drive end: shift state enters P for more than a threshold, location unchanged
- Aggregate drive metrics on close: distance, duration, energy consumed, average/max speed, temperatures, address lookup, geofence membership
- Link positions to their owning drive

### 3.5 Charging Process Tracking
- Detect charge start: charger connected, battery current > 0
- Record individual charge readings (voltage, current, power, phases, energy added, battery level, range)
- Detect charge completion: charger disconnected or battery full
- Aggregate charge metrics on close: energy added, energy used, duration, cost, address, geofence
- Handle interrupted charges and restarts gracefully

### 3.6 Software Update Tracking
- Detect update start: `car_version` changes to a pending install version
- Record install start/end time and version string
- Handle abandoned updates

### 3.7 Suspension Logic
- Check for vehicle activity (sentry mode, dog mode, cabin overheat protection, climate on, charging, doors/locks, power draw) before allowing sleep
- Configurable suspend timer (default 21 min idle, 15 min after last idle)
- Suspend logging immediately when the user triggers it from the UI

---

## Phase 4: Data Enrichment

**Goal:** Enrich raw telemetry with elevation, addresses, geo-fences, and cost calculations.

### 4.1 Elevation Lookup
- SRTM elevation data lookup for each position (lat, lng)
- Batch processing: query elevations for N positions at a time, update in bulk
- Circuit breaker for SRTM download errors
- Periodic backfill: every 6 hours, scan for positions with NULL elevation

### 4.2 Address Reverse Geocoding
- Nominatim (OpenStreetMap) client: `reverse?lat=X&lon=Y`
- Address caching: store resolved addresses in the `addresses` table keyed by lat/lng proximity
- Batch geocode for new drives and charging processes
- Periodic repair: scan for drives/charges with NULL address, resolve them with rate limiting

### 4.3 Geo-Fencing
- Store user-defined geofences (name, center lat/lng, radius in meters)
- PostGIS spatial queries: `ST_DWithin` to determine if a position falls inside a geofence
- Apply geofences to existing drives and charging processes on geofence create/update
- Trigger charge cost recalculation when a billing geofence changes

### 4.4 Charge Cost Calculation
- Support per-kWh billing and per-minute billing
- Support session flat fees
- Derive costs from the geofence billing configuration at the charge location
- Recalculate costs when geofence billing config changes

---

## Phase 5: Streaming API Integration

**Goal:** Ingest real-time telemetry via Tesla's WebSocket streaming API for sub-second position updates and reduced API polling.

### 5.1 WebSocket Client
- Connect to `wss://streaming.vn.teslamotors.com/streaming/`
- Authenticate with bearer token
- Subscribe to vehicle data stream
- Parse comma-separated values into typed structs (time, speed, SOC, odometer, elevation, heading, lat/lng, power, shift_state, range)
- Handle disconnects with exponential backoff
- Detect stream termination signals (vehicle offline, token expired, too many disconnects)

### 5.2 Streaming-States Integration
- Feed streaming data into the vehicle state machine
- When streaming is active, reduce REST polling frequency
- Smooth transition between streaming and polling when the stream disconnects

---

## Phase 6: Web UI — Core Pages

**Goal:** A functional SolidJS SPA with live vehicle status, sign-in, and navigation.

### 6.1 Frontend Scaffolding
- Vite + SolidJS + TypeScript project in `web/`
- Tailwind CSS configuration with light/dark/system theme support
- Router setup: `/` (cars), `/signin`, `/settings`, `/settings/car/:id`, `/geofences`, `/charge/:id/cost`
- Layout shell: navbar, flash messages, dark mode toggle
- Shared component library (Button, Modal, Card, FormField, Spinner)

### 6.2 SSE Client
- `EventSource` wrapper in SolidJS with auto-reconnect
- Reactive signal per event type (position update, state change, drive start/stop, charge start/stop)
- Clean disposal on component unmount

### 6.3 Car Index Page (`/`)
- Grid of vehicle status cards
- Each card shows: name, state, battery %, estimated range, location address, last update
- Live update via SSE: card values reactively change as data arrives
- Suspend/Resume logging button per car
- Redirect to `/signin` if not authenticated

### 6.4 Sign-In Page (`/signin`)
- Form: enter Tesla API tokens
- Validate and store tokens
- Redirect to `/` on success
- Display auth errors (invalid tokens, locked account, 2FA required)

### 6.5 GPX Export
- `GET /api/v1/drives/{id}/gpx` — generate and return GPX file from drive positions
- Download button on drive detail views (future)

---

## Phase 7: Web UI — Configuration Pages

**Goal:** Full settings management, geo-fence editing with a map, and charge cost editing.

### 7.1 Global Settings (`/settings`)
- Unit of length (km / mi)
- Unit of temperature (°C / °F)
- Unit of pressure (bar / psi)
- Preferred range (rated / ideal)
- Language (60+ supported languages)
- Theme mode (light / dark / system)
- Grafana URL override
- Form validation, success/error feedback

### 7.2 Per-Car Settings (`/settings/car/:id`)
- Suspend after idle (minutes)
- Suspend minimum (minutes)
- Require unlocked for wake
- Free supercharging toggle
- Use streaming API toggle
- Enabled/disabled toggle
- LFP battery toggle
- Live preview of how settings affect behavior

### 7.3 Geo-Fence Editor (`/geofences`)
- List all geo-fences with delete action
- Create/edit form modal with:
  - Interactive map (Leaflet + leaflet-draw or MapLibre GL)
  - Draggable circle marker for radius
  - Geocoder search to position the map
  - Name, radius, billing type (per kWh / per minute), cost per unit, session fee
- Warn when billing changes would affect existing charging processes
- Save via REST API, map re-renders list

### 7.4 Charge Cost Editor (`/charge/:id/cost`)
- Edit individual charge session cost
- Per-kWh or per-minute mode
- Cost calculation preview
- Save with feedback notification

---

## Phase 8: Integrations

**Goal:** Connect to external systems: Home Assistant via MQTT, historical data import from TeslaFi.

### 8.1 MQTT Publisher
- Connect to MQTT broker (configurable host, port, TLS, auth)
- Per-vehicle topic namespace: `teslamate/cars/{id}/{attribute}`
- Publish all vehicle attributes from the summary struct (~60 fields)
- Only publish changed values to reduce traffic
- Retained messages for stable state
- JSON payload for location and active route
- Clear retained messages on disconnect (clean session)
- Graceful handling of broker disconnects and reconnects

### 8.2 CSV Import (TeslaFi Format)
- Read CSV files from a configurable directory
- Parse rows into internal event structs (dates, positions, charges, drives)
- Feed events through `tokio::mpsc` channels to the vehicle state machine for deduplication and normalization
- Progress tracking: total rows, processed, errors, current step
- Import status polling endpoint for UI progress bar
- Import UI page with file picker, timezone selector, start button, progress indicator

---

## Phase 9: Grafana Dashboards

**Goal:** The same 20+ dashboards, updated for the new schema and TimescaleDB.

### 9.1 Dashboard Migration
- Port each existing dashboard JSON, updating queries for:
  - Hypertable `time_bucket` functions instead of `date_trunc`
  - Any renamed columns or tables
  - TimescaleDB compression-aware aggregate queries
- Validate each dashboard renders correctly against sample data

### 9.2 Dashboard Provisioning
- `grafana/datasource.yml` — pre-configured PostgreSQL datasource
- `grafana/dashboards.yml` — three providers (TeslaMate, Internal, Reports)
- Auto-import all JSON dashboard files at Grafana startup

### 9.3 Custom Grafana Image
- `grafana/Dockerfile` based on `grafana/grafana:latest`
- TeslaMate branding: logo, favicon, default home dashboard
- Security hardening: anonymous auth disabled, sign-up disabled
- Included in `docker-compose.yml`

---

## Phase 10: Polish, Performance & Documentation

**Goal:** Production-ready reliability, observability, and user documentation.

### 10.1 Performance Optimization
- Benchmark position insertion throughput (target: 10K+ rows/sec)
- Query plan analysis for all Grafana dashboard queries
- Index tuning: BRIN indexes on time columns, composite indexes for common filter patterns
- Connection pool sizing for `sqlx::PgPool`
- TimescaleDB compression policy on positions and charges hypertables (auto-compress > 7 days)
- Data retention policy (auto-drop > 3 years old, configurable)

### 10.2 Resilience
- Circuit breaker pattern for all external API calls (Tesla API, Nominatim, SRTM, GitHub releases)
- Exponential backoff with jitter on all retries
- Graceful degradation: if elevation lookup fails, log positions without elevation
- Startup health: check PostgreSQL version compatibility (16.7+)
- `tower::catch_panic` middleware on HTTP server for task boundary safety

### 10.3 Monitoring & Observability
- Prometheus metrics endpoint (`GET /metrics`) via `metrics-exporter-prometheus` or `axum-prometheus`
  - Vehicle state durations
  - API call latency and error rate histograms
  - Position write throughput
  - Tokio task count, memory usage
- `tracing` structured logging for all state transitions, API calls, and errors

### 10.4 Documentation
- `README.md`: what it is, how to run, features, screenshots
- `docs/configuration.md`: every environment variable, its default, and what it controls
- `docs/installation/`: Docker Compose guide, reverse proxy setup (Traefik, Caddy, nginx)
- `docs/architecture.md`: codebase overview, data flow, state machine diagram
- `docs/integrations/`: MQTT topic reference, Home Assistant setup, Node-RED examples
- `docs/import/`: TeslaFi import guide with screenshots
