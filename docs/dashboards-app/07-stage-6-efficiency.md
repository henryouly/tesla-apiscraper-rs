# Stage 6 — Efficiency (`grafana/dashboards/efficiency.json`)

## Source panels

| Panel | Type | Query |
|---|---|---|
| Avg Consumption, Wh/km (gross) | stat | `sum(energy_used_wh)/sum(distance_meters)*1000` FROM `drives` WHERE time + vin + `duration_seconds >= 0` + `distance_meters > 100` |
| Logged Distance | stat | `sum(distance_meters)/1000*$length_unit` FROM `drives`, same filters |
| Consumption per Drive, Wh/km | timeseries | `"energy_used_wh"/"distance_meters"*1000` FROM `drives`, same filters (raw per-drive points) |

Drive energy is stored in watt-hours: the drive accumulator converts the
API's kW (`drive_state.power`) before integrating, so `energy_used_wh/1000`
resolves to real kWh and the Wh/km ratios above are correct.

## Backend

- `GET /api/dashboards/efficiency?vin&from&to&unit` → `{ avg_consumption_wh_per_km, logged_distance, per_drive: [{t, wh_per_km}] }`.

## Frontend

- Route `/dashboards/efficiency`.

## Verification

- Avg consumption + logged distance match Grafana; per-drive points match
  the Drives table energy/distance ratios.
