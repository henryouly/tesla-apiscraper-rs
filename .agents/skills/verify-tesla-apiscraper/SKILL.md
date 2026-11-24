---
name: verify-tesla-apiscraper
description: "Launch, health-check, drive, and capture proof against a real isolated tesla-apiscraper-rs stack (Rust Axum server + built SolidJS SPA + InfluxDB 1.8 + local mock Tesla OAuth/Owner API + headless Chromium). Use to prove end-to-end user-facing behavior before claiming a fix or feature works."
---

# Verify `tesla-apiscraper-rs`

This skill drives the real `tesla-apiscraper-rs` binary serving the built SolidJS SPA (`web/dist`) against a real InfluxDB 1.8 database and a local mock Tesla OAuth/Owner API. Every run uses an ephemeral port, a disposable `CONFIG_DIR`, and a unique InfluxDB test database (`verify_tesla_*`) so it never touches production data or conflicts with concurrent runs.

Read [`features/README.md`](features/README.md) first, then follow the matching feature file as the verification recipe.

## Prerequisites

Build the Rust binary and web SPA once before launching (skip if already up to date):

```bash
cargo build --locked
npm --prefix web ci && npm --prefix web run build
```

Playwright Chromium must be available under `web/` (`npx --prefix web playwright install chromium` if missing). InfluxDB 1.8 is used from `INFLUXDB_TEST_URL` / `INFLUXDB_URL` / `http://127.0.0.1:8086` if already running; otherwise `launch` automatically starts a disposable `influxdb:1.8` Docker container on an ephemeral port.

## Launch

Start an isolated verification instance and seed baseline telemetry (`positions`, `charging_sessions`, `states`):

```bash
.agents/skills/verify-tesla-apiscraper/scripts/verify-app.mjs launch
```

- Pass `--state <path>` and `--evidence-dir <dir>` to run multiple instances side by side without sharing state.
- `launch` waits until `GET /health/ready` returns `200` **and** confirms seeded `positions` points are queryable in InfluxDB before exiting.
- Outputs JSON with `baseUrl`, `dbName`, `configDir`, `evidenceDir`, and tracked PIDs/container ID.

## Doctor

Run this read-only check before the first drive and whenever anything behaves unexpectedly:

```bash
.agents/skills/verify-tesla-apiscraper/scripts/verify-app.mjs doctor
```

Verifies:
- Tracked Rust server PID and mock Tesla server PID are alive.
- `GET /health` and `GET /health/ready` both return `200`.
- `CONFIG_DIR` is a disposable directory under `/tmp` and `dbName` starts with `verify_tesla_` (never production `tesla`).
- Seeded `positions` count is `>= 2`.

Exits `0` only when all checks pass.

## Drive

Drive one mapped feature or all mapped features using real headless Chromium (Playwright) against the live Rust server:

```bash
.agents/skills/verify-tesla-apiscraper/scripts/verify-app.mjs drive <feature>
```

Supported `<feature>` values:
- `auth-and-car-index` — sign in via `/signin`, verify `/` car card + seeded `77%` battery + encrypted `tokens.yml`
- `settings` — update `Unit of length` on `/settings`, reload to confirm persistence + `settings.yml`
- `geofences` — create `Verify Home` on `/geofences`, reload to confirm persistence + `geofences.yml`, then delete via UI
- `charge-cost` — edit `/charge/verify-chg-1/cost`, verify preview + reload + direct InfluxDB `cost = 13.5` check
- `dashboards` — open `/dashboards/overview`, verify `Overview` heading and real uPlot chart from InfluxDB
- `all` — run all 5 mapped features serially

Any uncaught `pageerror` or unexpected `console.error` (other than the expected pre-sign-in `401` on `/api/settings`) fails the drive.

## Evidence

Every drive writes durable proof artifacts under `<evidenceDir>/<featureId>/` (default `/tmp/verify-tesla-apiscraper-evidence-<runId>/<featureId>/`):

- `proof.png` — full-page Chromium screenshot showing the app UI and resulting state.
- `proof.aria.txt` — ARIA accessibility tree snapshot of `body`.
- `proof.json` — structured record with `featureId`, `entryPoint`, `url`, `timestamp`, artifact paths, and verified side effects (YAML files on disk or InfluxDB query values).

Proof standards:
- Exercise the real user path in Chromium; never bypass forms or auth with internal setters.
- Verify mutations across a full page reload **and** against the underlying store (`settings.yml`, `geofences.yml`, `tokens.yml`, or InfluxQL query).
- Mock only at real external production boundaries (Tesla OAuth/Owner API and OpenStreetMap tile images).

## Cleanup

Tear down everything `launch` started (tracked PIDs only — never `pkill` by process name), drop the temporary InfluxDB database, stop/remove any Docker container `launch` created, and delete the disposable `CONFIG_DIR` and state file:

```bash
.agents/skills/verify-tesla-apiscraper/scripts/verify-app.mjs cleanup
```

**Cleanup never deletes `<evidenceDir>`.** After cleanup, verify `evidencePreserved: true` in the JSON output and inspect the files in `<evidenceDir>`.

## Helpers

- [scripts/verify-app.mjs](scripts/verify-app.mjs) — executable Node.js CLI implementing `launch`, `doctor`, `drive`, and `cleanup`. All invocations are shown above.
