---
name: integration-test-pr-workflow
description: End-to-end workflow for building real-service integration tests (e.g., databases, brokers, or external engines in Docker & CI), organizing them into clean per-interface modules with atomic commits, documenting developer commands, opening a PR, and triaging automated/human PR review comments with strict reproduce-first TDD.
---

# Integration Test & PR Review Workflow

Use this skill when adding real-service integration tests (databases, message brokers, or other external engines) to a repository, wiring them into CI, and iterating on PR review feedback. Examples below use Rust + InfluxDB 1.8 for illustration — adapt paths, env vars, and commands to the target stack and service.

## Phase 1 — Audit & Inventory

1. **Inspect existing coverage & CI** (e.g. `.github/workflows/ci.yml`, `Makefile`, `package.json`, `Cargo.toml` / project test configuration).
2. **Inventory every real-service operation** across the codebase (e.g. health/ping, schema/database init, every write & upsert path, background writers/publishers, and every read/query endpoint).
3. **Identify mock blind spots** where wiremock/in-memory stubs cannot catch real engine semantics (query syntax, field vs. tag grouping, chunked HTTP responses, time-bucket bounds, type conflicts).

## Phase 2 — Branch, Harness & CI Setup

1. Create a clean feature branch from `main`:
   ```bash
   git checkout -b feat/<service>-integration-tests main
   ```
2. Start a local real service container via Docker for live verification during development (e.g. `docker run -d --name test-<service> -p <local_port>:<service_port> <image>:<tag>`), and remove it when finished.
3. Build a hermetic test harness (`mod.rs` or equivalent):
   - Gate integration tests with an opt-in marker (e.g. `#[ignore = "requires a running <service> (<SERVICE>_TEST_URL)"]` in Rust) so the default unit test command stays fast and offline-safe.
   - Provision an **isolated per-test database/namespace** (`TestDb::new(prefix)`) and drop it in `cleanup()`:
     - Always **consume the full HTTP/protocol response body and propagate transport/read errors** (do not swallow body-read failures on `2xx` responses) so chunked DDL calls finish committing before subsequent writes.
     - Always **parse and validate application/result-level errors** inside `200 OK` payloads (e.g. InfluxDB 1.8 `results[0].error`) so setup failures surface immediately instead of failing later on the first write.
   - Read the service endpoint from an env var (`<SERVICE>_TEST_URL`, falling back to `<SERVICE>_URL` then localhost; e.g. `INFLUXDB_TEST_URL`).
4. Wire up CI and developer entrypoints in the first commit:
   - Add a dedicated CI job in `.github/workflows/ci.yml` with a Docker `services:` container + healthcheck. Serialize execution (e.g. `--test-threads=1`) when the engine's DDL/meta-store is sensitive to concurrent schema mutations.
   - Expose convenient local commands in `Makefile` (e.g. `make test-<service>`) and/or `package.json` (e.g. `pnpm test:integration`).

## Phase 3 — Incremental Coverage with Atomic Commits

1. **One operation per commit**: add integration tests one interface/operation at a time, running each new test against the real Docker container before committing.
2. **Separate bugfixes from tests**: when a real-service test exposes a production bug (e.g. an unbounded `GROUP BY time(...) fill(previous)` timing out on historical data, or unconsumed chunked DDL responses), commit the production fix in its own `fix(...): ...` commit separate from the `test(...): ...` commit.
3. **Organize by domain/interface**:
   - Place tests in a dedicated directory (e.g. `src/<service>_integration/`): `mod.rs` for shared helpers, one file per core interface, and consolidated domain files where cohesive (e.g. `health.rs`, `writes.rs`, `dashboard.rs`).
4. **Verify the full suite** before opening the PR:
   - Formatter check (e.g. `cargo fmt --check`)
   - Lints across all targets (e.g. `cargo clippy --all-targets -- -D warnings`)
   - Default unit suite (e.g. `cargo test --quiet`)
   - Full real-service integration suite (e.g. `<SERVICE>_TEST_URL=... pnpm test:integration`)

## Phase 4 — Documentation & Pull Request

1. Update `README.md` (and harness module docs) with copy-pasteable instructions:
   - Starting/stopping the local Docker service container
   - Running via the repo's developer command(s) (`pnpm`, `make`, or native test runner)
   - Environment variable defaults and CI job behavior
2. Push the feature branch and open a PR via `gh pr create --base main --head <branch>` with:
   - Clear summary of added coverage, CI job, tooling/docs, and any production bugs uncovered & fixed
   - Checklist of verified commands and pass counts

## Phase 5 — Reproduce-First PR Review Triage Loop

When reviewers (e.g. GitHub Copilot or teammates) leave PR comments, **never blindly apply or dismiss them**:

1. **Fetch all PR feedback (reviews, top-level conversation comments, inline review comments) & CI status**:
   ```bash
   gh pr view <num> --json reviews,comments,statusCheckRollup
   gh api repos/<owner>/<repo>/pulls/<num>/comments --paginate \
     --jq '.[] | {id, created_at, path, line, user: .user.login, body}'
   ```
2. **Verify against code & real service**:
   - Check whether each comment is factually accurate and whether a fix is genuinely warranted (avoid breaking existing production schemas when a query-side fix is safer).
3. **Reproduce first (Red)**:
   - Add a minimal, deterministic unit test (e.g. raw `TcpListener` simulating a truncated chunked response) or real-container integration test (e.g. asserting actual grouping or querying an open historical session).
   - Run it against the unfixed code and confirm the exact failure reproduces the reported issue.
4. **Fix minimally (Green)**:
   - Apply the smallest clean fix, update any outdated mock-only unit assertions if the query contract improved, and re-run formatting, lints, unit tests, and the full real-service integration suite.
5. **Commit & Push**:
   - Commit each verified fix with a descriptive `fix(...): ...` message, push to the PR branch, and re-check CI and follow-up comments until all checks and reviews are green.
