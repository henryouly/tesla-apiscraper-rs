# Auth & Car Index

Auth & Car Index protects telemetry pages when no tokens are stored, lets a user sign in with a Tesla refresh token, encrypts and persists the tokens on the server, and displays discovered vehicles with live battery telemetry on the home page.

## Sub-features

- `auth-redirect` redirects an unauthenticated visit to `/` to `/signin`.
- `auth-signin` exchanges a refresh token via `/api/auth/sign_in`, discovers vehicles, and navigates to `/`.
- `auth-token-persistence` stores encrypted tokens in `CONFIG_DIR/tokens.yml`.
- `car-index-telemetry` renders the discovered vehicle heading and seeded battery percentage on `/`.

## How to get to it (user POV)

- Visit `/` in a browser when no tokens exist on the server (automatically redirects to `/signin`).
- Navigate directly to `/signin`.
- Submit a valid refresh token to reach `/`.

## Driving it with verify-app.mjs

Preconditions:

- Isolated stack is launched with an empty `CONFIG_DIR` (no `tokens.yml`).
- `verify-app.mjs doctor` reports `healthy: true`.

- **Unauthenticated redirect.** Open `<baseUrl>/`. Run `.agents/skills/verify-tesla-apiscraper/scripts/verify-app.mjs drive auth-and-car-index`. Browser redirects to `/signin` with heading `Sign in`.
- **Submit refresh token.** The driver fills `Paste refresh token` with `verify-refresh-token` and clicks button `Sign in`. Browser navigates to `/`.
- **Confirm vehicle card & telemetry.** Heading `Verify Model 3` and text `77%` become visible from the seeded InfluxDB `positions` point.
- **Confirm disk side effect.** Driver checks that `<configDir>/tokens.yml` exists.
- **Proof.** Artifacts written to `<evidenceDir>/auth-and-car-index/{proof.png,proof.aria.txt,proof.json}` showing `Verify Model 3`, `77%`, and `tokensFileExists: true`.

## Gotchas

- Before sign-in, `UnitsProvider` fetches `/api/settings` and logs a `401` console error; this single `401` is expected on unauthenticated boot and is explicitly allowlisted in the driver.
- Subsequent features (`settings`, `geofences`, `charge-cost`, `dashboards`) reuse the signed-in server tokens in `CONFIG_DIR/tokens.yml`, or sign in automatically if run first.
