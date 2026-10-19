# Stage 13 — Drive Details (`grafana/dashboards/drive-details.json`)

Hardest queries; everything it needs exists by now.

## Source panels (scoped by `$drive` = `drive_id`, `$dstart`/`$dend`)

| Panel | Type | Query |
|---|---|---|
| Drive | timeseries | `mean(speed)*1.60934*$length_unit`, `mean(power)` (kW native — see #93), `mean(battery_level)`, `mean(ideal_battery_range_km)*1.60934*$length_unit` (stored range is miles) FROM `positions`, fixed 10s buckets |
| Route | geomap | raw lat/lng ORDER BY time ASC (Leaflet polyline) |
| Elevation | timeseries | `mean(elevation)`, fixed 10s buckets (null on old drives: SRTM lookup failures, data gap not UI) |
| Temperatures | timeseries | `mean(outside_temp)`, `mean(inside_temp)`, fixed 10s buckets (sparse: streaming points carry no climate fields) |
| Duration / Distance / Energy Used / Avg Speed | stats | `last(...)` FROM `drives` |
| Odometer (From - To) | stat | `FIRST/LAST(odometer)*1.60934*$length_unit` FROM `positions` in window |

Shared crosshair across the three charts (matches Grafana `graphTooltip: 1`).
No chart→map hover-follow (upstream doesn't have it; would need a custom
plugin — out of scope).

## Backend

- `GET /api/dashboards/drive-details?drive_id&unit` → `{ series: {...}, route: [{lat,lng}], stats: {...}, odometer: {from,to} }` (server resolves the drive window; no `$__interval`-style range-derived bucketing ever).

## Frontend

- Route `/dashboards/drives/:driveId` (linked from Stage 12 with the
  drive's time window).

## Verification

- Same drive in both UIs: all four Drive series, elevation trace, temps,
  stats, odometer From/To, and route polyline match.
