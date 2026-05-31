# tesla-apiscraper-rs

A high-performance, self-hosted Tesla vehicle data logger. Continuously tracks one or more Tesla vehicles, stores detailed telemetry in InfluxDB 3, and provides rich visualization via Grafana — all on your own infrastructure.

This is a **Rust rewrite** of the original [TeslaMate](https://github.com/adriankumpf/teslamate) project (Elixir/Phoenix). It preserves the same data model and Grafana dashboards while delivering a smaller footprint, simpler deployment, and better performance on constrained hardware like a Raspberry Pi 4.

## Features

- **High-fidelity data collection** — GPS positions, battery state, climate, drive/charge sessions, software updates
- **Vehicle state machine** — Per-vehicle async tasks with smart sleep/wake/offline detection
- **Streaming API** — Sub-second telemetry via Tesla's WebSocket streaming API
- **Charge cost tracking** — Per-kWh or per-minute billing with geo-fence-based rates
- **Smart Home integration** — MQTT publisher for Home Assistant, Node-RED, etc.
- **Rich Grafana dashboards** — 20+ pre-built dashboards for drives, charging, battery health, vampire drain, and more
- **Web UI** — Lightweight SolidJS SPA with live vehicle status, settings, and geo-fence editing
- **Data ownership** — All data stays on your infrastructure. No third-party telemetry.
- **Import compatibility** — Import historical data from TeslaFi CSV exports
- **GPX export** — Export GPX tracks for individual drives

## Architecture

```
┌─────────────┐     ┌──────────────────┐     ┌──────────────┐
│  Tesla API   │────▶│ tesla-apiscraper │────▶│  InfluxDB 3  │
│ (REST+WS)    │     │  (Rust + tokio)  │     │  (time-series)│
└─────────────┘     └──────┬───────────┘     └──────────────┘
                           │                          │
                           ▼                          ▼
                    ┌──────────────┐          ┌──────────────┐
                    │  MQTT Broker  │          │   Grafana    │
                    │ (Home Asst.)  │          │ (dashboards) │
                    └──────────────┘          └──────────────┘
                           │
                           ▼
                    ┌──────────────┐
                    │  SolidJS SPA │
                    │  (web UI)    │
                    └──────────────┘
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
| Time-series DB | InfluxDB 3 Core |
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

# Copy and edit configuration
cp .env.example .env

# Start all services
make docker-build
docker compose up -d
```

This starts three containers:
- **tesla-apiscraper-rs** on port 4000
- **InfluxDB 3** on port 8181
- **Grafana** on port 3000

### Configuration

All configuration is via environment variables or a `.env` file:

| Variable | Default | Description |
|----------|---------|-------------|
| `HOST` | `0.0.0.0` | HTTP server bind address |
| `PORT` | `4000` | HTTP server port |
| `INFLUXDB_URL` | `http://localhost:8181` | InfluxDB 3 endpoint |
| `INFLUXDB_TOKEN` | — | InfluxDB authentication token |
| `INFLUXDB_DATABASE` | `tesla` | InfluxDB database name |
| `TESLA_API_CLIENT_ID` | `ownerapi` | Tesla API client ID |
| `TESLA_AUTH_URL` | `https://auth.tesla.com` | Tesla auth endpoint |
| `TESLA_API_URL` | `https://owner-api.teslamotors.com` | Tesla Owner API endpoint |
| `DATA_ENCRYPTION_KEY` | — | 32-byte hex key for AES-256-GCM token encryption |
| `POLL_INTERVAL_SECONDS` | `60` | Polling interval when parked/online |
| `STREAMING_ENABLED` | `false` | Enable WebSocket streaming API |
| `MQTT_HOST` | — | MQTT broker host (optional) |
| `GRAFANA_URL` | — | Grafana URL override (optional) |

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

## Roadmap

| Phase | Focus | Status |
|-------|-------|--------|
| 1 | Project foundation: scaffolding, config, InfluxDB, health checks | ✅ |
| 2 | Tesla API authentication & vehicle discovery | ✅ |
| 3 | Vehicle state machine & telemetry collection | ✅ |
| 4 | Data enrichment: elevation, addresses, geo-fencing, costs | 🏗 |
| 5 | Streaming API integration (WebSocket) | ❌ |
| 6 | Web UI — core pages (SolidJS SPA) | ❌ |
| 7 | Web UI — settings, geo-fence editor, charge cost editing | ❌ |
| 8 | Integrations: MQTT, TeslaFi import | ❌ |
| 9 | Grafana dashboard migration | ❌ |
| 10 | Polish, performance, documentation | ❌ |

See [docs/constitution/roadmap.md](docs/constitution/roadmap.md) for the full plan.

## Documentation

- [Project Goal](docs/constitution/goal.md) — vision, goals, success criteria
- [Tech Stack](docs/constitution/tech_stack.md) — detailed technology decisions
- [Roadmap](docs/constitution/roadmap.md) — phased implementation plan

## License

[AGPL-3.0](LICENSE)
