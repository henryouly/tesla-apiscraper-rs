# Stage 7 — Locations (`grafana/dashboards/locations.json`)

Introduces the bar-gauge component.

## Source panels

| Panel | Type | Query |
|---|---|---|
| Top Destinations | bargauge | `count(distance_meters)` AS Visits FROM `drives` WHERE time + vin + `duration_seconds >= 0` GROUP BY `geofence_exit` |
| Last Visited | table | `end_address`, `geofence_exit`, distance FROM `drives` WHERE time + vin + `duration_seconds >= 0`, ORDER BY time DESC LIMIT 50 |
| Drives Logged | stat | `count(distance_meters)` FROM `drives` WHERE time + vin |

## Backend

- `GET /api/dashboards/locations?vin&from&to&unit` → `{ top: [{geofence, visits}], last_visited: [...50 rows], drives_logged: n }`.

## Frontend

- New `<BarGauge>` component (horizontal bars with counts).
- Route `/dashboards/locations`.

## Verification

- Top destination + visit counts match Grafana; Latest-visited row matches
  the most recent drive's destination.
