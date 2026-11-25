# tesla-apiscraper-rs

A high-performance, self-hosted Tesla vehicle data logger. Continuously tracks one or more Tesla vehicles, stores detailed telemetry in InfluxDB v1, and provides rich dashboards in a built-in web UI — all on your own infrastructure.

This is a **Rust rewrite** of the original [TeslaMate](https://github.com/adriankumpf/teslamate) project (Elixir/Phoenix). It preserves the same data model and ports the original dashboards to native in-app pages while delivering a smaller footprint, simpler deployment, and better performance on constrained hardware like a Raspberry Pi 4.

## Features

- **High-fidelity data collection** — GPS positions, battery state, climate, drive/charge sessions, software updates
- **Vehicle state machine** — Per-vehicle async tasks with smart sleep/wake/offline detection
- **Streaming API** — Sub-second telemetry via Tesla's WebSocket streaming API
- **Charge cost tracking** — Per-kWh or per-minute billing with geo-fence-based rates
- **Smart Home integration** — MQTT publisher for Home Assistant, Node-RED, etc.
- **Native dashboards** — 14 in-app dashboards for drives, charging, battery health, states, and more
- **Web UI** — Lightweight SolidJS SPA with live vehicle status, settings, geo-fence editing, and dashboards
- **Data ownership** — All data stays on your infrastructure. No third-party telemetry.
- **Import compatibility** — Import historical data from TeslaFi CSV exports
- **GPX export** — Export GPX tracks for individual drives

## Architecture

```
┌─────────────┐     ┌──────────────────┐     ┌──────────────┐
│  Tesla API   │────▶│ tesla-apiscraper │────▶│  InfluxDB v1  │
│ (REST+WS)    │     │  (Rust + tokio)  │     │  (time-series)│
└─────────────┘     └──────┬───────────┘     └──────────────┘
                           │
              ┌────────────┴────────────┐
              ▼                         ▼
      ┌──────────────┐          ┌──────────────┐
      │  MQTT Broker  │          │  SolidJS SPA │
      │ (Home Asst.)  │          │  (web UI)    │
      └──────────────┘          └──────────────┘
```

Configuration (geo-fences, settings, OAuth tokens) is stored as YAML files on disk — no relational database needed.

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Language | Rust (stable) |
| Async runtime | tokio (multi-threaded) |
| Web framework | axum |
| HTTP client | reqwest |
| WebSocket | tokio-tungstenite |
| MQTT | rumqttc |
| Time-series DB | InfluxDB v1 |
| Frontend | SolidJS + TypeScript + Vite |
| CSS | Tailwind CSS |
| Maps | Leaflet / MapLibre GL |
| Containerization | Docker (scratch image, musl static binary) |

## Quick Start

### Prerequisites

- Docker & Docker Compose
- A Tesla account with API access
- A Tesla API refresh token

### Running

```bash
# Clone the repository
git clone https://github.com/henryouly/tesla-apiscraper-rs
cd tesla-apiscraper-rs

# Copy configuration and generate a 32-byte hex encryption key
cp .env.example .env
# Edit .env and set DATA_ENCRYPTION_KEY=$(openssl rand -hex 32)

# Pull the latest release image and start all services (or use --build for local source)
docker compose pull
docker compose up -d
```

This starts three containers:
- **tesla-apiscraper-rs** on `127.0.0.1:4000` (serves the web UI and dashboards; set `APP_BIND_ADDR=0.0.0.0` for LAN access)
- **InfluxDB v1** on `127.0.0.1:8086`
- **Mosquitto MQTT** on `127.0.0.1:1883` (configured via `deploy/mosquitto.conf`, publishes `teslamate/cars/#`)

For HTTPS reverse proxy (Caddy/Nginx with SSE), InfluxDB authentication, and backups, see [`docs/deployment.md`](docs/deployment.md).

### Configuration

All configuration is via environment variables or a `.env` file:

| Variable | Default | Description |
|----------|---------|-------------|
| `HOST` | `0.0.0.0` | HTTP server bind address |
| `PORT` | `4000` | HTTP server port |
| `INFLUXDB_URL` | `http://localhost:8086` | InfluxDB v1 endpoint |
| `INFLUXDB_USERNAME` | — | InfluxDB username (optional; empty = no auth) |
| `INFLUXDB_PASSWORD` | — | InfluxDB password (optional; empty = no auth) |
| `INFLUXDB_DATABASE` | `tesla` | InfluxDB database name |
| `TESLA_API_CLIENT_ID` | `ownerapi` | Tesla API client ID |
| `TESLA_AUTH_URL` | `https://auth.tesla.com` | Tesla auth endpoint |
| `TESLA_API_URL` | `https://owner-api.teslamotors.com` | Tesla Owner API endpoint |
| `DATA_ENCRYPTION_KEY` | — | 32-byte hex key for AES-256-GCM token encryption |
| `POLL_INTERVAL_SECONDS` | `60` | Polling interval when parked/online |
| `STREAMING_ENABLED` | `false` | Enable WebSocket streaming API |
| `MQTT_HOST` | — | MQTT broker host (optional) |
| `WEB_DIST_DIR` | — | Built SPA directory (`web/dist`); unset = API-only mode. Set automatically in Docker |

YAML config files are stored in `config/` and managed via the web UI:
- `config/geofences.yml` — geo-fence definitions with billing rules
- `config/settings.yml` — global and per-vehicle settings
- `config/tokens.yml` — encrypted OAuth tokens

### Development

```bash
# Build
make build

# Run (with hot-reload via cargo-watch)
cargo run

# Test
make test

# Lint
make lint

# Full CI pipeline
make ci
```

#### Integration Tests (InfluxDB 1.8)

The real InfluxDB 1.8 integration test suite (`src/influxdb_integration/`) covers all database operations (writes, upserts, `DbWriter`, `/health/ready`, `/api/charges`, and all 14 `/api/dashboards/*` endpoints). Tests are marked `#[ignore]` so `cargo test` stays hermetic without a database; each test creates and drops its own isolated database.

Start a local InfluxDB 1.8 container and run the suite:

```bash
# Start InfluxDB 1.8
docker run -d --name influxdb-test -p 8086:8086 influxdb:1.8

# Run via pnpm (or `make test-influxdb`)
pnpm test:integration

# Or point at a custom InfluxDB address
INFLUXDB_TEST_URL=http://127.0.0.1:18086 pnpm test:integration

# Clean up the container
docker rm -f influxdb-test
```

`INFLUXDB_TEST_URL` defaults to `INFLUXDB_URL` and then `http://127.0.0.1:8086`. GitHub Actions runs this suite automatically in the `influxdb-integration` CI job against an `influxdb:1.8` service container.

#### Web Browser E2E Tests (Playwright)

Real Chromium browser tests live in `web/e2e/` and run against the built SPA (`web/dist`):

```bash
# One-time Chromium install
npm --prefix web exec playwright -- install chromium

# Fast browser suite (mocked API + real SSE + Leaflet + uPlot)
pnpm test:web

# Full-stack browser smoke test (real Rust binary + InfluxDB 1.8 + Chromium)
cargo build && npm --prefix web run build
docker run -d --name influxdb-test -p 8086:8086 influxdb:1.8
INFLUXDB_TEST_URL=http://127.0.0.1:8086 pnpm test:fullstack
docker rm -f influxdb-test
```

GitHub Actions runs both suites in parallel via the `web-e2e` and `fullstack-e2e` CI jobs.

## Roadmap

| Phase | Focus | Status |
|-------|-------|--------|
| 1 | Project foundation: scaffolding, config, InfluxDB, health checks | ✅ |
| 2 | Tesla API authentication & vehicle discovery | ✅ |
| 3 | Vehicle state machine & telemetry collection | ✅ |
| 4 | Data enrichment: elevation, addresses, geo-fencing, costs | ✅ |
| 5 | Streaming API integration (WebSocket) | ✅ (live-verified Sep 2026) |
| 6 | Web UI — core pages (SolidJS SPA) | ✅ (served at `/` when built; see `WEB_DIST_DIR`) |
| 7 | Web UI — settings, geo-fence editor, charge cost editing | ✅ |
| 8 | Integrations: MQTT, TeslaFi import | ❌ |
| 9 | Native in-app dashboards (14 pages) | ✅ |
| 10 | Polish, performance, documentation | ❌ |

See [docs/constitution/roadmap.md](docs/constitution/roadmap.md) for the full plan.

## Security

The HTTP API has no client login: anyone who can reach the port can read
vehicle telemetry (`/api/vehicles`, `/api/events`) once the server holds
Tesla tokens. This matches the self-hosted design (same trust model as the
bundled InfluxDB) — so do not expose port 4000 to untrusted
networks. Bind `HOST` to localhost or put the service behind a firewall /
reverse proxy with your own access control. Stored Tesla tokens are
AES-256-GCM encrypted at rest via `DATA_ENCRYPTION_KEY`.

## Documentation

- [Project Goal](docs/constitution/goal.md) — vision, goals, success criteria
- [Tech Stack](docs/constitution/tech_stack.md) — detailed technology decisions
- [Roadmap](docs/constitution/roadmap.md) — phased implementation plan

## License

[AGPL-3.0](LICENSE)
