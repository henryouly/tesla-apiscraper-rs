# Stage 15 — Cutover (decommission Grafana)

Entry criteria: the parity checklist in `00-overview.md` is signed off per
dashboard (Stages 1–14 verification sections).

## Removal PR (single PR, backend + frontend + ops + docs)

- `docker-compose.yml`: remove the `grafana` service.
- Volumes: drop `grafana-data` (after confirming no user data worth
  migrating — dashboards are files; Grafana's sqlite holds only
  preferences).
- Delete `grafana/` provisioning + dashboards (keep
  `references/source/grafana` — it's a read-only upstream reference, not
  ours).
- Docs: remove Grafana references (`README.md`, `docs/api.md`,
  web README, compose comments); update the dashboards-app docs to
  "shipped".
- Live instance: back up `grafana-data` volume first, then redeploy.

## Verification

- `docker compose config` validates; fresh `docker compose up` serves the
  app with zero Grafana references (`grep -ri grafana` only matches
  history docs and `references/`).
- Full test suite + web build green.
