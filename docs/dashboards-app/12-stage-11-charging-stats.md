# Stage 11 — Charging Stats (`grafana/dashboards/charging-stats.json`)

## Source panels

| Panel | Type | Query |
|---|---|---|
| # of Charges / Total Energy Added / Total Charging Cost / Avg Cost / kWh | stats | same aggregates as Charges (Stage 9) |
| Charge Level Delta | timeseries | `mean(start_battery_level)`, `mean(end_battery_level)` FROM `charging_sessions`, grouped |
| Top Charging Stations | table | `sum(energy_added_wh)/1000`, `count` FROM `charging_sessions` GROUP BY `geofence_name` |

## Backend

- `GET /api/dashboards/charging-stats?vin&from&to&unit` → `{ count, energy_kwh, cost, avg_cost_per_kwh, soc_delta: [...], top_stations: [...] }` (share code with the Stage 9 endpoint where queries coincide).

## Frontend

- Route `/dashboards/charging-stats`.

## Verification

- Stats match Charges page for the same range; station table matches
  Grafana ordering.
