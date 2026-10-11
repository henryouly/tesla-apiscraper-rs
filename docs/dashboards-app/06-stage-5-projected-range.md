# Stage 5 — Projected Range (`grafana/dashboards/projected-range.json`)

## Source panels

| Panel | Type | Query |
|---|---|---|
| Projected Range at 100% | timeseries | `sum(ideal_battery_range_km)/sum(battery_level)*100*$length_unit` (+ rated variant) FROM `positions` WHERE time + vin |
| Outside Temperature | timeseries | `mean(outside_temp)` FROM `positions` WHERE time + vin + `ideal_battery_range_km > 0`, grouped |

## Backend

- `GET /api/dashboards/projected-range?vin&from&to&unit` → `{ projected: [{t, ideal, rated}], outside_temp: [{t, c}] }`.

## Frontend

- Route `/dashboards/projected-range`, two `<TimeChart>`s.

## Verification

- Projected 100% values match Grafana within rounding on 3 sample days.
