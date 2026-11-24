# Native Dashboards

Native Dashboards renders historical telemetry queried server-side from InfluxDB under `/api/dashboards/*` using uPlot charts and Tailwind stat/table components.

## Sub-features

- `dashboards-overview` loads `/dashboards/overview` with the `Overview` heading and renders at least one `.uplot` time-series chart from live InfluxDB data.

## How to get to it (user POV)

- Click the `Dashboards` link in the navigation bar (redirects `/dashboards` to `/dashboards/overview`).
- Navigate directly to `/dashboards/overview` (or any sibling dashboard route such as `/dashboards/states`, `/dashboards/drives`, `/dashboards/charges`).

## Driving it with verify-app.mjs

Preconditions:

- Isolated stack is running and `verify-app.mjs doctor` reports `healthy: true`.
- Seeded InfluxDB database contains at least two recent `positions` points for `VIN=5YJ3E1EA1NF000001`.

- **Render Overview dashboard.** Run `.agents/skills/verify-tesla-apiscraper/scripts/verify-app.mjs drive dashboards`. The driver ensures the session is signed in, navigates to `/dashboards/overview`, verifies level-1 heading `Overview`, and waits for `.uplot` to become visible with zero console/page errors.
- **Proof.** Artifacts written to `<evidenceDir>/dashboards/{proof.png,proof.aria.txt,proof.json}` showing the rendered `Overview` dashboard and `uplotRendered: true`.

## Gotchas

- `positions` timestamps must fall within the default dashboard time window (seeded at `now - 300s` and `now - 60s`), otherwise uPlot charts will have empty series.
- Distance/range fields in InfluxDB (`*_battery_range_km`) are stored in miles and converted server-side by `/api/dashboards/*` according to `unit_length` in `settings.yml`.
