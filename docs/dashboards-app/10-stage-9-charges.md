# Stage 9 — Charges (`grafana/dashboards/charges.json`)

First fleet-period aggregation shape; pattern reused by Stages 11–12.

## Source panels (under `Summary of this period` row)

| Panel | Type | Query |
|---|---|---|
| # of Charges | stat | `count(energy_added_wh)` FROM `charging_sessions` + filters |
| Energy Added | stat | `sum(energy_added_wh)/1000` + `duration_seconds >= 0` |
| Total Cost | stat | `sum(cost)` + `duration_seconds >= 0` |
| Avg Cost / kWh | stat | `sum(cost)/sum(energy_added_wh)*1000` + `duration_seconds >= 0` |
| Charges | table | location, energy kWh, duration min, cost, start/end % FROM `charging_sessions` |
| Energy per Day | timeseries | `sum(energy_added_wh)/1000 … GROUP BY time(1d) fill(0)` |
| Charging Power by Session | timeseries | `mean(power)/1000` FROM `charge_readings` GROUP BY time bucket + `charge_id` (charge power IS watts — unlike drive power, see #93) |

Filters: `$timeFilter`, `vin` (`=~ /^$car$/`).

## Backend

- `GET /api/dashboards/charges?vin&from&to&unit` → `{ count, energy_kwh, total_cost, avg_cost_per_kwh, sessions: [...], energy_per_day: [...], power_by_session: [...] }`.

## Frontend

- Route `/dashboards/charges`. Cost formatting follows billing currency
  already in settings (check `ChargeCost.tsx` conventions).

## Verification

- Stats + table match Grafana for the same range; per-day bars match;
  power curves match per session.
