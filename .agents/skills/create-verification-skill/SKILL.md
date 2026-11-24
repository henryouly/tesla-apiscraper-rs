---
name: create-verification-skill
description: "Generate a project-local verification skill under .agents/skills/verify-<app>/ that launches and drives the real app the way a user does, with a maintained feature map and proof standards. Use when asked to create a verification/control skill for this repo or when the project needs a scripted end-to-end verification harness."
---

# Create a Verification Skill

Every serious project needs a scripted way to drive the real app and prove behavior: launch it, exercise a feature the way a user would, and capture durable evidence. This skill generates that as a project-local skill (`.agents/skills/verify-<app>/`) tailored to the repo. Write the generated skill for the **next agent**, not a human: it will be read cold, mid-task, by an agent that has never seen the app.

## 1. Interview the Repo, Not the User

Answer these directly from the codebase and only ask the user what cannot be observed:

- **Surface:** What does a user actually touch? A web UI, a CLI/TUI, an API, an SSE stream, a background worker? A repo can have several; pick the primary one and note the rest.
- **Run:** How does the app start locally? Prefer the repo's own documented dev/test commands (`Makefile`, `package.json`, `scripts/`, `README.md`, `.github/workflows/ci.yml`). Note ports, env vars, seed data, and auth bootstrap.
- **Drive:** How can an agent interact with it programmatically? Prefer existing harnesses first (Playwright, `curl`, scripts already in `scripts/`). Only then pick a generic recipe (browser/CDP for web, PTY/tmux for CLI, plain HTTP for services).
- **Observe:** What evidence can be captured? Screenshots, ARIA/DOM snapshots, HTTP/SSE payloads, InfluxDB query results, YAML config files on disk, logs, exit codes.
- **Isolate:** Can two instances run side by side (ephemeral ports, isolated `CONFIG_DIR`, unique InfluxDB test database)? If not, state it explicitly: refusing to double-drive a shared instance beats corrupting live data.

### Known Baseline in `tesla-apiscraper-rs`

Verify against current files when running, as the repo evolves:
- **Primary surface:** SolidJS web SPA served by the Rust Axum binary (`WEB_DIST_DIR=web/dist`), backed by REST + SSE (`/api/*`, `/api/events`, `/health`, `/health/ready`), InfluxDB 1.8, and YAML config files (`CONFIG_DIR`: `settings.yml`, `geofences.yml`, `tokens.yml`).
- **Existing full-stack launch pattern:** See `scripts/run-fullstack-e2e.mjs` — spawns a local mock Tesla OAuth/Owner API, seeds an isolated InfluxDB 1.8 database (`e2e_fullstack_*`), starts `target/debug/tesla-apiscraper-rs` on an ephemeral `PORT` with a temp `CONFIG_DIR` and `DATA_ENCRYPTION_KEY`, waits for `/health/ready`, and drives Chromium via Playwright (`web/`).
- **Primary user routes (`web/src/App.tsx`):** `/signin`, `/` (`CarIndex` + live SSE), `/settings`, `/settings/car/:id`, `/geofences`, `/charge/:id/cost`, `/dashboards/*` (`overview`, `states`, `updates`, `mileage`, `projected-range`, `efficiency`, `locations`, `drive-stats`, `drives`, `drives/:driveId`, `charges`, `charging-stats`, `trip`).
- **External boundaries:** Tesla OAuth/Owner API (mock locally; never hit real Tesla endpoints during verification), OpenStreetMap tiles (intercept/stub in browser so Leaflet stays offline-fast).

If the checkout does not build or start as-is, fix that first (or report it precisely) before generating; a skill written against a broken base teaches wrong steps. When an irrelevant missing asset blocks startup (e.g., an empty `web/dist` or sample config), the generated skill may create it, clearly marked as verification scaffolding, and remove it in cleanup.

## 2. Generate the Skill

Write `.agents/skills/verify-<app>/SKILL.md` with YAML frontmatter (`name: verify-<app>` and a `description` naming the app, surface, and when to reach for it) and these sections, each grounded in what the repo interview actually found (no placeholders):

- **Launch:** Exact command(s) to build/start the app for verification and how to tell it is **usable** (not merely listening — e.g., `/health/ready` returns 200 AND InfluxDB seed writes are queryable). Launch must work from an empty `CONFIG_DIR` and fresh InfluxDB test database. Include teardown.
- **Doctor:** One read-only check that answers "is this instance worth driving?" — process alive, expected binary/build, port owned by this run, `/health/ready` 200, expected isolated InfluxDB DB and `CONFIG_DIR` (never the user's production DB/config). Agents run this first whenever anything looks off.
- **Drive:** Concrete harness recipes with real routes, ARIA roles/labels, and commands from this repo — not generic examples. Prefer stable handles (ARIA roles, accessible names, labels, route paths) over coordinates or brittle CSS selectors.
- **Evidence:** Exact proof format and output directory. State the proof standards:
  - Exercise the real user path, not internal setters or test-only shortcuts.
  - Capture both the action and the resulting state (and persistence across reload / second read-only view for mutations).
  - Verify side effects (YAML files written, InfluxDB points updated, SSE events received) alongside what is visible in the UI.
  - Mock only at real external production boundaries (Tesla API, map tiles).
  - When using a dry-run or test mode, observe what it actually touches rather than trusting its name.
- **Cleanup:** How to tear down every process, temp directory, and test InfluxDB database created by the run. Never kill by bare process name (`pkill`); kill only PIDs started by the run. **Cleanup removes instances and scratch state, never the evidence:** proof artifacts must survive teardown at the named path.
- **Helpers:** Any helper script shipped inside `.agents/skills/verify-<app>/scripts/` must be executable (`chmod +x`) and its exact invocation must be documented in `SKILL.md`. A helper the reader has to reverse-engineer is not a helper.

## 3. Seed the Feature Map

Create `.agents/skills/verify-<app>/features/README.md` plus one file per user-facing feature (aim for the top 3–5 core flows to start, such as sign-in + live car card, settings persistence, geofences CRUD, charge cost editing, and native dashboards).

Follow the exact structure in [`references/feature-map-example/`](references/feature-map-example/):
- `README.md`: baseline preconditions, driving conventions, proof and skip reporting, feature entry contract, and index of feature files.
- One `<feature>.md` per feature, with an H1 title, one user-POV summary paragraph, and **exactly four H2 sections** in order:
  1. `Sub-features`
  2. `How to get to it (user POV)`
  3. `Driving it with <harness>` (starting with `Preconditions:`)
  4. `Gotchas`

If seeded data lacks a precondition that a journey needs, the journey must report a failure and record the reason in `Gotchas`. Never weaken the journey to make it pass. The map is the repo's maintained verification source; a proof that only exercises one convenient entry point is incomplete when the map lists others.

## 4. Prove the Generated Skill Before Handing It Over

Run the generated skill's own instructions end to end once:
1. Launch the isolated stack.
2. Run `doctor`.
3. Drive **at least one** mapped feature end to end and capture evidence.
4. Run cleanup.
5. Confirm after cleanup that:
   - No verification processes, temp config dirs, or test InfluxDB databases remain.
   - The captured evidence **still exists** at the named location (a cleanup that deletes the proof fails this step).

Fix anything that fails, and run cleanup after every failed iteration too so broken attempts do not strand processes, ports, or test databases. A generated skill that was never executed is a draft, not a deliverable. If proved in a stand-in environment (e.g., local InfluxDB container vs. host service), list every setup step the stand-in did that another environment will not.

## 5. Offer the Maintenance Loop

Once the generated verification skill is proved, point the user at the `maintain-verification-skill` skill to keep the harness and feature map honest as `tesla-apiscraper-rs` evolves.
