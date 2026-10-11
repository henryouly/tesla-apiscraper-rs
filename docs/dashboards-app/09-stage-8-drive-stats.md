# Stage 8 — Drive Stats (`grafana/dashboards/drive-stats.json`)

Introduces the histogram component.

## Source panels

| Panel | Type | Query |
|---|---|---|
| # of Drives | stat | `count(distance_meters)` FROM `drives` + filters |
| Total Distance | stat | `sum(distance_meters)/1000*$length_unit` + filters |
| Energy Used | stat | `sum(energy_used_wh)/1000` + filters (see #93 units note in Stage 6) |
| Median Drive | stat | `percentile(distance_meters, 50)/1000*$length_unit` + filters |
| Max Speed | stat | `max(max_speed)*1.60934*$length_unit` + filters |
| Speed Distribution | histogram | `"speed"*1.60934*$length_unit` FROM `positions` WHERE time + vin + `speed > 1` (raw points, bucketed client-side) |
| Top Destinations | bargauge | same shape as Locations (GROUP BY `geofence_exit`) |

Filters: `$timeFilter`, `vin`, `duration_seconds >= 0` throughout.

## Backend

- `GET /api/dashboards/drive-stats?vin&from&to&unit` → `{ drives, distance, energy_wh, median_drive, max_speed, speeds: [raw mph values], top_destinations: [...] }` (histogram bucketing client-side in uPlot or hand-rolled bars — decide at implementation; histogram is the only one of its kind).

## Frontend

- New `<Histogram>` component (reuse `<BarGauge>` primitives where possible).
- Route `/dashboards/drive-stats`.

## Verification

- All five stats match Grafana; histogram shape matches (same buckets);
  Top Destinations matches Locations page for the same range.
