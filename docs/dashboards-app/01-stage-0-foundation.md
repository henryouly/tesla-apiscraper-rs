# Stage 0 — Foundations (no dashboard)

## Goal

Shared building blocks every later stage uses. No user-visible dashboard;
prove with Storybook-style demo routes or unit tests only.

## Backend (`src/api/dashboards.rs`, new)

- Route group `router() -> Router<AppState>`, mounted alongside existing
  groups, behind the same auth (`require_auth`).
- InfluxDB query helper: run parameterized InfluxQL against the app's
  `InfluxDb`, map rows to JSON. Follow `api/charges.rs` (wiremock-backed
  unit tests per endpoint in later stages).
- Conventions (apply to all stages):
  - `vin` + `from`/`to` (RFC3339) query params; `unit` (`km`/`mi`) param
    matching the Grafana `length_unit` semantics (stored data is
    miles/mph-based: `*1.60934` for km, `*0.621371` inverse for mi display
    where the panels do that).
  - Epoch millis in JSON responses (uPlot wants numbers, not strings).
  - Errors: 4xx on bad params, 502 on InfluxDB failure (match existing style).

## Frontend (`web/src/`)

- `lib/charts.tsx`: `<TimeChart>` wrapper around uPlot (new dep) —
  theme-aware (light/dark via `lib/theme`), unit-aware via `lib/units`,
  shared tooltip/crosshair defaults mirroring Grafana (`graphTooltip: 1`
  behavior inside a page).
- `components/data.tsx`: `<Stat>` (label + value + unit, horizontal/center
  layout matching the Grafana stat style), `<DataTable>` (sortable,
  paginated, linkable cells for drill-downs).
- `lib/dashboards.ts`: shared fetch helpers, `from`/`to` + VIN state,
  km/mi toggle wired to the existing units context.
- Routes + nav entries scaffolded (pages land in later stages).

## Verification

- `cargo test`, clippy, fmt, eslint green.
- `TimeChart` is covered by the Overview page (Stage 1); `BigStat` and
  `DataTable` typecheck and build here and get first coverage in their
  consumer stages. (The temporary fixtures demo page was removed once
  real pages landed.)
