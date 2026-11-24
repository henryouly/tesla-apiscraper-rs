# `tesla-apiscraper-rs` Verification Map

This directory is the maintained source for verifying the user-facing behavior of `tesla-apiscraper-rs`. Read this index before driving the app, then use the matching feature file as the verification recipe.

## Baseline preconditions

- Build the Rust binary (`cargo build --locked`) and SolidJS SPA (`npm --prefix web run build`).
- Launch an isolated stack with `.agents/skills/verify-tesla-apiscraper/scripts/verify-app.mjs launch`.
- Seed vehicle `Verify Model 3` (`VIN=5YJ3E1EA1NF000001`) with `positions` (`battery_level=77`), `charging_sessions` (`charge_id=verify-chg-1`, `energy_added_wh=40000`, `duration_seconds=3600`, `cost=10`), and `states` (`state="asleep"`).
- Run `.agents/skills/verify-tesla-apiscraper/scripts/verify-app.mjs doctor` and require `healthy: true`.
- Never drive a production instance or the default `tesla` database.

## Driving conventions

- Start from the isolated stack created by `launch`.
- Prefer ARIA roles, accessible names, labels, and route paths over brittle CSS selectors.
- Treat every command as literal.
- Intercept only external OpenStreetMap tiles in the browser; never mock `/api/*` endpoints.
- Restore mutated list items (such as created geofences) when the feature recipe specifies it, and never delete proof artifacts during cleanup.

## Proof and skip reporting

- Capture both the action and the resulting state, not only an intermediate toast.
- UI proof includes `proof.png`, `proof.aria.txt`, and `proof.json` under `<evidenceDir>/<featureId>/`.
- Mutation proof requires both a full page reload and a direct read-only check of the underlying store (`tokens.yml`, `settings.yml`, `geofences.yml`, or InfluxQL query).
- Record `featureId` and `entryPoint` in `proof.json`.
- Report any unreachable path with the attempted command and exact unmet precondition; never report a skipped entry point as verified.

## Feature entry contract

Each feature file starts with an H1 title and one paragraph describing user-visible behavior, followed by exactly four H2 sections in order:

1. `Sub-features`
2. `How to get to it (user POV)`
3. `Driving it with verify-app.mjs`
4. `Gotchas`

## Features

- [Auth & Car Index](./auth-and-car-index.md) covers unauthenticated redirect, refresh-token sign-in, encrypted token storage, and live vehicle card rendering.
- [Global Settings](./settings.md) covers updating display/unit preferences, reload persistence, and YAML config storage.
- [Geofences](./geofences.md) covers creating a geofence, reload persistence, YAML storage, and two-step deletion.
- [Charge Cost](./charge-cost.md) covers editing a charging session's cost, live preview calculation, reload persistence, and InfluxDB rewrite verification.
- [Native Dashboards](./dashboards.md) covers opening `/dashboards/overview` and rendering real InfluxDB time-series charts via uPlot.
