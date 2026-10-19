# Stage 1 — Overview (`grafana/dashboards/overview.json`)

Simplest page; mostly live data. Proves the backend→frontend loop end to end.

## Source panels (2 timeseries)

| Panel | Query (InfluxQL) |
|---|---|
| Charge Level | `mean(battery_level)`, `mean(usable_battery_level)` FROM `positions` WHERE time + vin, `GROUP BY time($__interval) fill(previous)` |
| Ideal Range | `mean(ideal_battery_range_km)*1.60934*$length_unit` (stored range is miles) FROM `positions`, same grouping |

Dashboard default range `now-7d`; `car` + `length_unit` variables.

## Backend

- `GET /api/dashboards/overview?vin&from&to&unit` → `{ battery: [{t, level, usable}], ideal_range: [{t, km_or_mi}] }` (epoch ms, bucketed server-side; fixed bucket, not range-derived — see Drive Details lesson: never use a range-derived interval over a bounded window).

## Frontend

- Route `/dashboards/overview`, VIN picker (shared header scope), shared
  Grafana-style time range picker (presets + custom; range lives in the
  shared scope so later tabs reuse it), km/mi from global settings, two
  `<TimeChart>`s.

## Verification

- Same car + 7d range: values match Grafana Overview point-for-point (spot-check 3 timestamps).
