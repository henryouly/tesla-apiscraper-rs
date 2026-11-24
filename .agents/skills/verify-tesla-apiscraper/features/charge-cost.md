# Charge Cost

Charge Cost lets an authenticated user inspect a charging session's energy and duration, preview a calculated cost by kWh or minute plus session fee, and rewrite the session's cost in InfluxDB.

## Sub-features

- `charge-cost-load` displays energy added (`40.0 kWh`), duration, and current cost for a session ID.
- `charge-cost-preview` dynamically computes the preview total as rate and session fee change.
- `charge-cost-save` writes the new cost to InfluxDB and persists it across reloads.

## How to get to it (user POV)

- Open a charging session's cost link from the Charges dashboard or navigate directly to `/charge/<charge_id>/cost`.

## Driving it with verify-app.mjs

Preconditions:

- Isolated stack is running and `verify-app.mjs doctor` reports `healthy: true`.
- Seeded InfluxDB database contains `charging_sessions` point with `charge_id='verify-chg-1'`, `energy_added_wh=40000.0`, `duration_seconds=3600`, `cost=10.0`.

- **Edit and persist cost.** Run `.agents/skills/verify-tesla-apiscraper/scripts/verify-app.mjs drive charge-cost`. The driver opens `/charge/verify-chg-1/cost`, verifies `40.0 kWh`, enters `Cost per kWh` = `0.30` and `Session fee` = `1.50`, confirms `Preview: 13.50`, clicks `Save cost`, and confirms `Saved cost 13.50`.
- **Confirm persistence and DB side effect.** Driver reloads the page, confirms `13.5` is displayed as current cost, and queries InfluxDB directly (`SELECT cost FROM charging_sessions WHERE charge_id = 'verify-chg-1'`) to verify `cost === 13.5`.
- **Proof.** Artifacts written to `<evidenceDir>/charge-cost/{proof.png,proof.aria.txt,proof.json}` showing `13.5` and `influxDbCost: 13.5`.

## Gotchas

- Preview formats to two decimals (`13.50`), whereas the reloaded `Current cost` stat formats the numeric InfluxDB value (`13.5`).
- If the seeded `charge_id` is missing, the page displays `Session not found.` — do not weaken the check; fix seeding instead.
