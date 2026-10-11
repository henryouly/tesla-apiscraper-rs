# Stage 12 — Drives (`grafana/dashboards/drives.json`)

List page with drill-down links (pattern consumed by Stage 13).

## Source panels

| Panel | Type | Query |
|---|---|---|
| # of Drives / Distance / Duration / Energy Used | stats | count/sum aggregates FROM `drives` + `duration_seconds >= 0` (see #93 units note) |
| Drives | table | distance, duration min, energy kWh, avg/max speed, From/To geofences FROM `drives` |
| Distance per Day | timeseries | `sum(distance_meters)/1000*$length_unit … GROUP BY time(1d) fill(0)` |

Reference the Stage 10 detail pattern; rows link to
`/dashboards/drives/:driveId` (Stage 13). Time range for the detail view
travels with the navigation (Grafana cannot auto-fit axis to a variable —
see drive-details history), so pass explicit from/to of the drive window.

## Backend

- `GET /api/dashboards/drives?vin&from&to&unit` → `{ count, distance, duration_s, energy_wh, drives: [...rows], per_day: [...] }`.

## Frontend

- Route `/dashboards/drives`; row click navigates with the drive's time
  window.

## Verification

- Stats + table + daily bars match Grafana; clicking a row opens the
  matching detail page.
