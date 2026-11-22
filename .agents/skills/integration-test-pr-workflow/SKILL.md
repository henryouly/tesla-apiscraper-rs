---
name: integration-test-pr-workflow
description: End-to-end workflow for building real-service integration tests (e.g., InfluxDB 1.8 / databases in Docker & CI), organizing them into clean per-interface modules with atomic commits, documenting developer commands, opening a PR, and triaging automated/human PR review comments with strict reproduce-first TDD.
---

# Integration Test & PR Review Workflow

Use this skill when adding real-service integration tests (databases, brokers, external engines) to a repository, wiring them into CI, and iterating on PR review feedback.

## Phase 1 — Audit & Inventory

1. **Inspect existing coverage & CI** (`.github/workflows/ci.yml`, `Makefile`, `package.json`, `Cargo.toml` / test suites).
2. **Inventory every real-service operation** across the codebase (e.g. `ping`, schema/database init, every measurement/table write & upsert, background writers, and every read/query endpoint).
3. **Identify mock blind spots** where wiremock/in-memory stubs cannot catch real engine semantics (query syntax, field vs. tag grouping, chunked HTTP responses, time-bucket bounds, type conflicts).

## Phase 2 — Branch, Harness & CI Setup

1. Create a clean feature branch from `main`:
   ```bash
   git checkout -b feat/<topic>-integration-tests main
   ```
2. Start a local real service container via Docker for live verification during development (e.g. `docker run -d --name test-influxdb-18 -p 18086:8086 influxdb:1.8`), and remember to remove it when done.
3. Build a hermetic test harness (`mod.rs`):
   - Gate integration tests with `#[ignore = "requires a running ..."]` (or equivalent marker) so default `cargo test` stays fast and offline-safe.
   - Provision an **isolated per-test database/namespace** (`TestDb::new(prefix)`) and drop it in `cleanup()` — always consume full HTTP response bodies on DDL calls (`CREATE DATABASE` / `DROP DATABASE`) so chunked responses commit before subsequent writes.
   - Read the endpoint from an env var (`INFLUXDB_TEST_URL`, falling back to `INFLUXDB_URL` then localhost).
4. Wire up CI and developer entrypoints in the first commit:
   - Add a dedicated CI job in `.github/workflows/ci.yml` with a Docker `services:` container + healthcheck, running tests with `--test-threads=1` if the engine's DDL/meta-store is sensitive to concurrent schema mutations.
   - Expose convenient local commands in `Makefile` (`make test-influxdb`) and root `package.json` (`pnpm test:integration`).

## Phase 3 — Incremental Coverage with Atomic Commits

1. **One operation per commit**: add integration tests one interface/operation at a time, running each new test against the real Docker container before committing.
2. **Separate bugfixes from tests**: when a real-service test exposes a production bug (e.g. unbounded `GROUP BY time(1m) fill(previous)` timing out on historical data, or unconsumed chunked DDL responses), commit the production fix in its own `fix(...): ...` commit separate from the `test(...): ...` commit.
3. **Organize by domain/interface**:
   - Place tests in a dedicated directory (`src/influxdb_integration/`): `mod.rs` for shared helpers, one file per core interface (`health.rs`, `positions.rs`, `states.rs`, `drives.rs`, `charging.rs`, `updates.rs`, `charges_api.rs`), and grouped domain files where cohesive (`dashboard.rs`).
4. **Verify the full suite** before opening the PR:
   - Formatting (`cargo fmt --check`)
   - Lints across all targets (`cargo clippy --all-targets -- -D warnings`)
   - Default unit suite (`cargo test --quiet`)
   - Full real-service integration suite (`INFLUXDB_TEST_URL=... pnpm test:integration`)

## Phase 4 — Documentation & Pull Request

1. Update `README.md` (and module doc comments) with copy-pasteable instructions:
   - Starting/stopping the local Docker service container
   - Running via `pnpm test:integration` / `make` / `cargo`
   - Environment variable defaults and CI job behavior
2. Push the feature branch and open a PR via `gh pr create --base main --head <branch>` with:
   - Clear summary of added coverage, CI job, tooling/docs, and any production bugs uncovered & fixed
   - Checklist of verified commands and pass counts

## Phase 5 — Reproduce-First PR Review Triage Loop

When reviewers (e.g. GitHub Copilot or teammates) leave PR comments, **never blindly apply or dismiss them**:

1. **Fetch comments & CI status**:
   ```bash
   gh pr view <num> --json reviews,statusCheckRollup
   gh api repos/<owner>/<repo>/pulls/<num>/comments --paginate \
     --jq '.[] | {id, created_at, path, line, user: .user.login, body}'
   ```
2. **Verify against code & real service**:
   - Check whether the comment is factually accurate and whether a fix is genuinely warranted (and avoid breaking existing production schemas when a query-side fix is safer).
3. **Reproduce first (Red)**:
   - Add a minimal, deterministic unit test (e.g. raw `TcpListener` simulating a truncated chunked response) or real-container integration test (e.g. asserting actual station breakdown or querying an open historical session).
   - Run it against the unfixed code and confirm the exact failure reproduces the reported issue.
4. **Fix minimally (Green)**:
   - Apply the smallest clean fix, update any outdated mock-only unit assertions if the query contract improved, and re-run formatting, clippy, unit tests, and the full real-service integration suite.
5. **Commit & Push**:
   - Commit each verified fix with a descriptive `fix(...): ...` message, push to the PR branch, and re-check CI and follow-up comments until all checks and reviews are green.
