# Stage 3 — Updates (`grafana/dashboards/updates.json`)

First table-pagination exercise on a tiny dataset.

## Source panels

| Panel | Type | Query |
|---|---|---|
| Software Updates | stat | count of rows FROM `updates` WHERE time + vin |
| Updates | table | version before/after, install start/end, status FROM `updates` WHERE time + vin |

## Backend

- `GET /api/dashboards/updates?vin&from&to` → `{ count: n, updates: [{version_before, version_after, install_start, install_end, status}] }`.

## Frontend

- Route `/dashboards/updates`, `<Stat>` + `<DataTable>` (paginated).

## Verification

- Row count and latest update match Grafana Updates table.
