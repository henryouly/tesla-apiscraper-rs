# Goal

## Project Vision

Build a high-performance, self-hosted service that continuously tracks one or more Tesla vehicles over years, storing detailed telemetry in a scalable time-series database for personal analytics, cost tracking, and battery health monitoring.

## Core Goals

### 1. High-Fidelity Data Collection
Continuously record GPS positions, battery state, climate settings, drive sessions, charging sessions, and software updates for every Tesla linked to an owner account. The system must capture data at sufficient resolution to reconstruct trips, analyze efficiency, and detect battery degradation trends — without introducing additional vampire drain (the car must be allowed to fall asleep whenever possible).

### 2. Long-Term Scalability
The database must handle years of per-second telemetry from multiple vehicles without degradation. Query performance for dashboards (aggregated stats, time-range views, spatial lookups) must remain responsive as the dataset grows into millions of rows.

### 3. Data Ownership & Self-Hosting
All data lives on the user's own infrastructure. No telemetry is sent to third parties. The service runs as a set of Docker containers (the app, InfluxDB, and Grafana), deployable on a home server, NAS, or cloud VM.

### 4. Rich Visualization
Provide pre-built Grafana dashboards covering:
- Drive history, stats, and map-based trip replay
- Charging history, stats, cost tracking, and energy efficiency
- Battery health and projected range degradation
- Vampire drain analysis
- Vehicle state timeline (online, asleep, offline)
- Software update history
- Visited locations and geo-fences

### 5. Real-Time Web Interface
Serve a lightweight, reactive web UI for:
- Live vehicle status (battery, range, location, climate, doors, TPMS)
- Starting and stopping data logging per vehicle
- Managing global and per-vehicle settings (units, language, sleep behavior)
- Creating and editing geo-fences with an interactive map
- Editing charging costs
- Importing historical data from TeslaFi and similar tools
- Exporting GPX tracks for individual drives

### 6. Smart Home Integration
Publish vehicle telemetry over MQTT for seamless integration with Home Assistant, Node-RED, and other automation platforms.

### 7. Import Compatibility
Support importing historical data from TeslaFi CSV exports and tesla-apiscraper format, allowing users to migrate without losing years of collected data.

## Non-Goals

- **Fleet management** — The system is designed for individual owners, not commercial fleet operators.
- **Mobile app** — The web UI is responsive and mobile-friendly, but no native iOS/Android app.
- **Real-time vehicle control** — The system reads data from the Tesla API; it does not send commands (lock/unlock, climate start, etc.).
- **Multi-user / multi-tenant** — Single owner, single Tesla account.
- **Cloud-hosted SaaS** — This is strictly self-hosted software.

## Success Criteria

1. A Rust binary in a Docker container, paired with InfluxDB + SQLite and Grafana, captures all available telemetry from a Tesla fleet.
2. Data ingestion keeps pace with Tesla's streaming API (~1 position/sec) without backpressure or data loss.
3. Dashboard queries return in under 2 seconds for 5+ years of single-vehicle data on modest hardware (Raspberry Pi 4 or equivalent).
4. The web UI feels instant (SSE or WebSocket-driven live updates, no polling).
5. The existing Grafana dashboards can be used with minimal query adjustments.
6. MQTT publishes vehicle state at least as frequently and completely as the current Elixir implementation.
