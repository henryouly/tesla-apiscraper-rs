# Native In-App Dashboards — Project Scope

Rebuild the 14 Grafana dashboards natively in the Vite + Solid.js app, then
decommission Grafana. Grafana stays running untouched until per-dashboard
parity sign-off.

## Locked decisions

- **Charts:** uPlot (same engine as Grafana's timeseries panels; closest visual
  parity for the ~15 timeseries). Leaflet stays for the 3 geomaps. Stat /
  table / bar-gauge / pie / histogram / state-timeline are hand-rolled
  Tailwind components (presentational only, no lib).
- **Scope:** all 14 dashboards in `grafana/dashboards/`.
- **Data:** per-dashboard backend endpoints under a new `api/dashboards.rs`
  route group (auth-wired like existing routes), reusing the
  `api/charges.rs` pattern of querying InfluxDB server-side. Browsers never
  touch InfluxDB directly. Live values keep flowing over the existing SSE bus.
- **Grafana:** decommissioned at parity (Stage 15): remove compose service,
  volumes, provisioning, docs references in one PR.

## Stage map (one dashboard per stage, each its own PR)

| Stage | File | Dashboard | New primitives |
|---|---|---|---|
| 0 | `01-stage-0-foundation.md` | — (foundations) | TimeChart, Stat, DataTable, route group |
| 1 | `02-stage-1-overview.md` | Overview | — |
| 2 | `03-stage-2-states.md` | States | StateTimeline |
| 3 | `04-stage-3-updates.md` | Updates | — |
| 4 | `05-stage-4-mileage.md` | Mileage | — |
| 5 | `06-stage-5-projected-range.md` | Projected Range | — |
| 6 | `07-stage-6-efficiency.md` | Efficiency | — |
| 7 | `08-stage-7-locations.md` | Locations | BarGauge |
| 8 | `09-stage-8-drive-stats.md` | Drive Stats | Histogram |
| 9 | `10-stage-9-charges.md` | Charges | period aggregation shape |
| 10 | `11-stage-10-charge-details.md` | Charge Details | detail-page pattern |
| 11 | `12-stage-11-charging-stats.md` | Charging Stats | — |
| 12 | `13-stage-12-drives.md` | Drives | drill-down links |
| 13 | `14-stage-13-drive-details.md` | Drive Details | multi-series + map |
| 14 | `15-stage-14-trip.md` | Trip | PieChart (only one) |
| 15 | `16-stage-15-cutover.md` | — (cutover) | — |

Ordering rationale: live-data pages first, list pages before their detail
pages, each new visualization introduced on the simplest page that needs it.

## Per-stage contract

Every stage PR contains: backend endpoint(s) with unit tests (wiremock
InfluxDB), frontend page + route + nav, and a verification section in its
scope file checked off against the live Grafana twin (same car, same time
range, same numbers).

## Parity checklist (Stage 15 entry criteria)

- [ ] All 14 pages render the same values as Grafana for a reference
      car + time range (spot-checked, incl. km/mi toggle where applicable)
- [ ] Drill-downs work (Drives → Drive Details with drive time range)
- [ ] Geomaps render routes/points via Leaflet
- [ ] No browser→InfluxDB traffic (all data via backend)
- [ ] `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`,
      `eslint` green per stage PR
