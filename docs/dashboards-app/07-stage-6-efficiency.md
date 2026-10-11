# Stage 6 — Efficiency (`grafana/dashboards/efficiency.json`)

## Source panels

| Panel | Type | Query |
|---|---|---|
| Avg Consumption, Wh/km (gross) | stat | `sum(energy_used_wh)/sum(distance_meters)*1000` FROM `drives` WHERE time + vin + `duration_seconds >= 0` + `distance_meters > 100` |
| Logged Distance | stat | `sum(distance_meters)/1000*$length_unit` FROM `drives`, same filters |
| Consumption per Drive, Wh/km | timeseries | `"energy_used_wh"/"distance_meters"*1000` FROM `drives`, same filters (raw per-drive points) |

Note: drive energy figures change with issue #93 (drive power is kW, not
watts — `energy_used_wh` currently 1000× under). Re-verify these numbers
after #93 lands; both read from the same fields so no rework expected.

## Backend

- `GET /api/dashboards/efficiency?vin&from&to&unit` → `{ avg_consumption_wh_per_km, logged_distance, per_drive: [{t, wh_per_km}] }`.

## Frontend

- Route `/dashboards/efficiency`.

## Verification

- Avg consumption + logged distance match Grafana; per-drive points match
  the Drives table energy/distance ratios.
