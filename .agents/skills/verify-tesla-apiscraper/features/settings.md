# Global Settings

Global Settings lets an authenticated user configure display preferences such as length unit, temperature unit, pressure unit, preferred range, and theme, persisting changes to `settings.yml` on disk.

## Sub-features

- `settings-open` opens `/settings` from the top navigation bar.
- `settings-save` saves updated global preferences via `/api/settings/global` and shows `Saved.`.
- `settings-persist` retains saved values after a full browser reload and in `CONFIG_DIR/settings.yml`.

## How to get to it (user POV)

- Click the `Settings` link in the navigation bar.
- Navigate directly to `/settings` while authenticated.

## Driving it with verify-app.mjs

Preconditions:

- Isolated stack is running and `verify-app.mjs doctor` reports `healthy: true`.

- **Open and save settings.** Run `.agents/skills/verify-tesla-apiscraper/scripts/verify-app.mjs drive settings`. The driver ensures the session is signed in, clicks link `Settings`, selects `km` in `Unit of length`, and clicks `Save settings`. Status `Saved.` appears.
- **Confirm persistence.** Driver reloads the page and asserts `Unit of length` still equals `km`, and verifies `<configDir>/settings.yml` contains `unit_length: km`.
- **Proof.** Artifacts written to `<evidenceDir>/settings/{proof.png,proof.aria.txt,proof.json}` showing the persisted `Settings` form and `settingsYamlContainsKm: true`.

## Gotchas

- `Saved.` confirmation alone is not sufficient proof; the driver must reload the page and inspect `settings.yml`.
- Per-car settings links (`/settings/car/:vin`) appear at the bottom of the page only after vehicle discovery completes on sign-in.
