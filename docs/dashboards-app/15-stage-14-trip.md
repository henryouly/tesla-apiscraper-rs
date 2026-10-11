# Stage 14 — Trip (`grafana/dashboards/trip.json`)

Capstone: every primitive already built. Introduces the pie chart (only one).

## Source panels

| Panel | Type | Query |
|---|---|---|
| Trip Map | geomap | raw lat/lng FROM `positions` (Leaflet polyline) |
| Distance | stat | `sum(distance_meters)` aggregate FROM `drives` |
| Time Spent | piechart | `sum(duration_seconds)` FROM `drives` vs FROM `charging_sessions` (driving vs charging split) |
| Avg Speed / Consumption Wh/km / Charging Cost | stats | same aggregate shapes as Stages 8–9 |
| Drives / Charges | tables | same row shapes as Stages 9, 12 |
| Battery Level & Range | timeseries | battery + range series FROM `positions` |

## Backend

- `GET /api/dashboards/trip?vin&from&to&unit` → composite of the above
  (reuse Stage 8/9/12 query code where shapes coincide).

## Frontend

- New `<PieChart>` component (simple SVG slices; single use — keep tiny,
  no lib).
- Route `/dashboards/trip`.

## Verification

- Full-page side-by-side with Grafana Trip: map, all stats, both tables,
  pie split, battery chart.
