---
name: tdd
description: "Pragmatic test-driven development (TDD) workflow tailored to tesla-apiscraper-rs. Use when fixing a bug, addressing PR review feedback, or adding behavior that has a clear, cheap local test target (Rust unit/Axum test, InfluxDB 1.8 integration test, or Playwright E2E). Skip forcing a new test when the test path is unclear, brittle, or disproportionately expensive, and use the closest executable verification instead."
---

# TDD Workflow (`tesla-apiscraper-rs`)

When fixing a bug or implementing a well-scoped behavior with a clear, cheap test path, make the broken or missing behavior executable **before** editing production code. The goal is a focused test that fails for the right reason before the change and passes after it.

Do not force a test when it would be impractical. If adding a test would require broad harness setup, brittle mocks, slow end-to-end infrastructure for a tiny fix, production-only Tesla API state, vague reproduction steps, or large unrelated fixture churn, skip adding a new test and use the closest executable verification instead.

## Project Test Tiers (Pick the Narrowest One)

Choose the lowest tier in `tesla-apiscraper-rs` that faithfully reproduces the real behavior:

| Tier | Best For | Command |
|---|---|---|
| **1. Rust unit & Axum router tests** | Pure logic, state machine (`src/vehicles/`), unit conversions, YAML config (`src/config_yaml.rs`), encryption, HTTP/SSE handlers (`src/api/` using `test_helpers`, `wiremock`, `rstest`) | `cargo test <test_name>` |
| **2. InfluxDB 1.8 integration tests** | InfluxQL queries, tag/field writes, update-on-close semantics, dashboard aggregations (`src/influxdb_integration/`) | `INFLUXDB_TEST_URL=http://127.0.0.1:8086 cargo test influxdb_integration::<mod> -- --ignored --test-threads=1` |
| **3. Web SPA mocked E2E (Playwright)** | SolidJS UI flows, routing, forms, chart/map rendering, SSE client merging (`web/e2e/*.spec.ts`, excluding `fullstack.spec.ts`) | `npm --prefix web run test:e2e -- <spec_file>` |
| **4. Full-stack E2E** | Real Rust binary + real InfluxDB 1.8 + mock Tesla OAuth/Owner API + built SPA + Chromium (`scripts/run-fullstack-e2e.mjs` + `web/e2e/fullstack.spec.ts`) | `npm run test:fullstack` |

## Workflow

1. **Understand & verify first.** Identify the intended behavior, current behavior, affected codepath, and smallest observable reproduction. For bug reports or PR review comments (human or automated), verify factually from code/tests before changing anything.
2. **Choose the narrowest executable check.** Pick the lowest applicable tier above that already covers that codepath. Do not build a new test harness from scratch just to satisfy the workflow.
3. **Write the failing test first (Red).** Add the smallest focused test that would have caught the bug or specifies the new contract. Assert observable behavior, not internal implementation details.
4. **Run the new test before fixing.** Confirm it **fails for the expected reason**. If it passes immediately or fails for an unrelated setup reason, fix the test or reproduction before touching production code.
5. **Implement the minimal change (Green).** Make the smallest, cleanest production edit that satisfies the intended behavior while preserving nearby API/schema contracts.
   - Watch project-specific invariants (e.g., `*_battery_range_km` fields in InfluxDB store **miles** and require conversion on read; `drives` and `charging_sessions` use update-on-close upserts keyed by measurement + tags + start timestamp).
6. **Rerun the targeted test.** Confirm the new test now passes.
7. **Run nearby verification gates.**
   - Rust changes: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` (and `make test-influxdb` when InfluxDB read/write logic changes).
   - Web changes: `npm --prefix web run typecheck`, `npm --prefix web run lint`, `npm --prefix web run build`, `npm run test:web`.
   - Cross-stack / API contract changes: `npm run test:fullstack`.

## If a Failing Test Is Impractical

Use the closest executable regression check instead: a targeted script, a `curl` check against the local server, a live verification drive (via the project's `verify-*` skill if present), an InfluxQL query assertion, or a focused log check.

Prefer **no new test** over a **bad test**. A bad test is one that mostly tests mocks, mirrors current implementation details, depends on wall-clock timing or shared global state, requires disproportionate infrastructure for a small change, or would be deleted immediately after proving the fix.

## Guardrails

- Do not change existing tests merely to match a broken implementation.
- Do not weaken existing assertions unless the expected behavior has genuinely changed and the reason is explicit.
- Keep the regression test focused on the defect or feature. Avoid broad fixture churn or unrelated coverage expansion.
- If a bug is flaky (e.g., async polling, SSE ordering, Playwright waits), make the test deterministic and document the signal being locked down.
- If a bug exposes a broader class of failures, land the focused regression test first, then add sibling coverage.

## Final Response

Report the evidence, not just the outcome:

- Name the failing-before test or executable check and the exact failure it produced.
- Name the passing-after test run and any nearby verification gates executed (`cargo clippy`, `test-influxdb`, `test:web`, `test:fullstack`, etc.).
- If failing-before evidence could not be demonstrated, state why and describe the closest regression check used instead.
