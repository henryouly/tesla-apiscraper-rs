# Geofences

Geofences lets an authenticated user define named circular locations on a map with optional charging billing rules, persist them to `geofences.yml`, and delete them with a two-step confirmation.

## Sub-features

- `geofences-create` creates a named geofence with coordinates and radius.
- `geofences-persist` retains the created geofence across a full page reload and in `CONFIG_DIR/geofences.yml`.
- `geofences-delete` requires clicking `Delete` then `Confirm delete?` to remove a geofence.

## How to get to it (user POV)

- Click the `Geofences` link in the navigation bar.
- Navigate directly to `/geofences` while authenticated.

## Driving it with verify-app.mjs

Preconditions:

- Isolated stack is running and `verify-app.mjs doctor` reports `healthy: true`.
- No geofence named `Verify Home` exists yet.

- **Create and persist geofence.** Run `.agents/skills/verify-tesla-apiscraper/scripts/verify-app.mjs drive geofences`. The driver opens `Geofences`, fills `Name` with `Verify Home` and `Radius (m)` with `150`, clicks `Create`, reloads the page, and confirms heading `Verify Home` remains visible and `<configDir>/geofences.yml` contains `Verify Home`.
- **Proof.** Artifacts written to `<evidenceDir>/geofences/{proof.png,proof.aria.txt,proof.json}` before deleting the test geofence.
- **Restore clean state.** Driver clicks `Delete` then `Confirm delete?` and waits for `Verify Home` to detach from the DOM.

## Gotchas

- Leaflet requests map tiles from `tile.openstreetmap.org`; the driver stubs those external tile URLs with a transparent PNG so runs stay deterministic and offline-fast.
- Deletion is intentionally two-step (`Delete` → `Confirm delete?`); clicking `Delete` once does not remove the geofence.
