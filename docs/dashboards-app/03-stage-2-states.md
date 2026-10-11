# Stage 2 — States (`grafana/dashboards/states.json`)

Introduces the state-timeline component on an otherwise simple page.

## Source panels

| Panel | Type | Query |
|---|---|---|
| Current State | stat | `last("state")` FROM `states` WHERE time + vin |
| States | state-timeline | `"state"` FROM `states` WHERE time + vin |
| State Changes | stat | `count("state")` FROM `states` WHERE time + vin |

## Backend

- `GET /api/dashboards/states?vin&from&to` → `{ current: string, timeline: [{t, state}], changes: n }`.

## Frontend

- New `<StateTimeline>` component (colored segments per state, matching
  Grafana's state-timeline; reuse for future needs).
- Route `/dashboards/states`.

## Verification

- Current state + change count match Grafana; timeline segments align on 3
  sample transitions (sleep/wake/drive).
