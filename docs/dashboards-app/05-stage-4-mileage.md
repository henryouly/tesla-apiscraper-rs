# Stage 4 — Mileage (`grafana/dashboards/mileage.json`)

Single-chart page; pure-history endpoint pattern.

## Source panels

| Panel | Type | Query |
|---|---|---|
| Mileage | timeseries | `max("odometer")*1.60934*$length_unit` FROM `positions` WHERE time + vin + `odometer > 0`, `GROUP BY time(1d) fill(previous)` |

## Backend

- `GET /api/dashboards/mileage?vin&from&to&unit` → `{ points: [{t, miles_or_km}] }` (daily buckets).

## Frontend

- Route `/dashboards/mileage`, one `<TimeChart>`.

## Verification

- Latest odometer value matches car screen (within rounding); curve matches
  Grafana Mileage.
