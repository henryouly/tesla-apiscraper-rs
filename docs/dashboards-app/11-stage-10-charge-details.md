# Stage 10 — Charge Details (`grafana/dashboards/charge-details.json`)

Detail-page pattern (variable-driven queries), reused by Stages 13–14.

## Source panels (all scoped by `$charge` = `charge_id`)

| Panel | Type | Query |
|---|---|---|
| Cost | stat | `last(cost)` FROM `charging_sessions` |
| Duration | stat | `last(duration_seconds)` |
| Location | stat | `last(geofence_name)` |
| Energy Added | stat | `last(energy_added_wh)/1000` |
| Location Map | geomap | `last(start_lat)`, `last(start_lng)` (Leaflet marker) |
| SOC Added | stat | `last(end_battery_level) - last(start_battery_level)` |
| Charge Curve | timeseries | `mean(battery_level)`, `mean(power)/1000`, `mean(energy_added)` FROM `charge_readings` GROUP BY fixed bucket + `fill(previous)` (NOT range-derived interval — see drive-details lesson) |

## Backend

- `GET /api/dashboards/charge-details?charge_id&unit` → `{ cost, duration_s, geofence, energy_kwh, lat, lng, soc_added, curve: [...] }`.

## Frontend

- Route `/dashboards/charges/:id` linked from the Charges table rows.
- Time axis spans the session (fixed bucketing; never `$__interval` over a
  bounded window).

## Verification

- Open the same session in both UIs: all stats + curve match; map pin
  matches.
