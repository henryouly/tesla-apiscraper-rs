---
name: interrogate
description: "Adversarial review skill for tesla-apiscraper-rs. Use when asked to \"interrogate\", \"adversarial review\", \"challenge this\", \"stress test this code\", \"find blind spots\", or \"tear this apart\". Spawns isolated parallel adversaries to break a change, PR, or design, and keeps only findings proven by an executable run. Reports findings without applying fixes."
---

# Interrogate (`tesla-apiscraper-rs`)

Adversaries try to break a change, PR, or design. **A finding survives only when an actual run proves it.** Report verified findings; **never apply fixes** during an `interrogate` pass.

## 1. Frame

1. **Determine the scope.** Use what the user named (files, PR, branch, or design/plan). If unspecified, default to the current branch diff from its merge base with `main` through the working tree:
   ```bash
   git diff "$(git merge-base HEAD main)"
   ```
2. **State the intent in your own words.** Summarize what the user/change is trying to achieve and what contract it promises, synthesized from the user's message, commits, and PR description. If the intent is ambiguous, ask the user before fanning out.
3. **Hold the read-only boundary.** Never modify the user's working tree or apply fixes. All reproduction scripts, tests, and probes live strictly inside temporary isolated worktrees.

## 2. Isolate & Fan Out

Create isolated detached `git worktree` checkouts under `/tmp/interrogate-<run_id>/` so parallel adversaries cannot collide with each other or dirty the user's working tree:

```bash
RUN_ID="$$_$(date +%s)"
BASE_DIR="/tmp/interrogate-${RUN_ID}"
mkdir -p "$BASE_DIR"

for i in 1 2 3; do
  WT="$BASE_DIR/adversary-$i"
  git worktree add --detach "$WT" HEAD
  # If the main checkout has uncommitted changes in scope, copy them into $WT
done
```

> **Tip for uncommitted changes:** Use `git diff` (for tracked changes) applied via `git -C "$WT" apply` and copy any untracked files in scope into `"$WT"`. Leave the main repo completely untouched.

Spawn **2–3 concurrent `self` subagents** via `invoke_subagent` (`TypeName: "self"`, `Model: "inherit"`, `Workspace: "inherit"`) in a single tool call, assigning each adversary its own worktree path and a distinct attack angle tailored to `tesla-apiscraper-rs` (omit angles that clearly do not apply to the scope, e.g., skip the Web SPA adversary for a pure Rust internal refactor):

1. **Adversary 1 — Backend, State Machine & InfluxDB Invariants**
   - Targets: `src/vehicles/`, `src/influxdb*.rs`, `src/streaming.rs`, `src/config_yaml.rs`, `src/encryption.rs`, `src/api/`.
   - Attack vectors: vehicle state-machine transitions, sleep/wake/streaming handoffs, stored-unit traps (`*_battery_range_km` stores **miles** and must be converted on read), InfluxQL quoting/escaping and update-on-close upserts (`drives`, `charging_sessions`), YAML config persistence/round-trips, token refresh/encryption.
2. **Adversary 2 — Frontend, API Contract & Full-Stack E2E**
   - Targets: `web/src/`, `src/api/`, `web/e2e/`.
   - Attack vectors: SolidJS reactivity/cleanup bugs, SSE `summary`/`state`/`resync` ordering and merge edge cases, JSON contract mismatches between `src/api/` and `web/src/lib/api.ts`, form validation (`NaN`/empty inputs), Playwright mocked E2E or live `verify-tesla-apiscraper` regressions.
3. **Adversary 3 — Auth, Concurrency & Failure Boundaries** *(include for multi-layer changes, routing/auth changes, or design/plan reviews)*
   - Targets: `src/api/require_auth.rs`, `src/api/mod.rs` (`with_spa` fallback), `src/main.rs`, async tasks/channels.
   - Attack vectors: auth middleware bypasses, SPA fallback shadowing `/api/*` or `/health*`, lock/channel deadlocks, graceful shutdown hangs, malformed upstream Tesla/Nominatim/InfluxDB responses or offline DB errors.

Give every adversary the scope, the restated intent, its assigned worktree path `$WT`, and this exact brief:

> Break this change from your assigned attack angle. Prove each finding with an actual run against the real code inside your worktree (`$WT`) — such as a failing `cargo test`, an `influxdb_integration` test, a Playwright repro (`npm --prefix web run test:e2e`), a `verify-app.mjs` drive (using a unique `--state` path), or a `curl`/probe script that demonstrates the wrong behavior. For a design or plan, run a prototype or probe against the current tree.
>
> - Keep all repro files strictly inside `$WT`; never modify the main repository checkout.
> - When running Cargo commands inside `$WT`, set `CARGO_TARGET_DIR=<repo_root>/target` so compiled dependencies are reused instead of triggering a cold build.
> - Report each finding with:
>   1. **Title**
>   2. **Exact command** (runnable from `$WT`)
>   3. **Observed output** (the exact failure printed)
>   4. **Required output** (what the intent requires)
> - **Leave out anything you did not run.** That excludes style, naming, structure, and speculative risks that require a future change to matter.
> - "No findings" is a valid result.

If an adversary drops out or returns no output, continue with the remaining adversaries and note the dropout in the final report.

## 3. Verify (Coordinator)

Before removing any worktree, independently verify every reported finding:

1. **Rerun the exact command** reported by the adversary (inside its `$WT` or against a clean worktree with the repro applied).
2. **Keep a finding only when** both conditions hold:
   - The command actually reproduces the failure.
   - The failure genuinely breaks the stated **Intent**.
3. **Drop everything else** — unexecuted claims, non-reproducing commands, environment flukes, or complaints that do not violate the intent. Record the dropped count and concise reason per adversary.
4. **Merge duplicate root causes** across adversaries, crediting every adversary that independently proved the same root cause.

## 4. Cleanup

Always clean up all temporary worktrees and scratch state before reporting, even if a run failed:

```bash
for i in 1 2 3; do
  git worktree remove --force "$BASE_DIR/adversary-$i" 2>/dev/null || true
done
rm -rf "$BASE_DIR"
git worktree prune
```

Confirm `git status` in the main repository is unchanged and no stray verification processes or InfluxDB test containers remain.

## 5. Output

Present a concise, evidence-only report:

- **Intent**: the restated goal and contract.
- **Findings**: for each verified finding — title, exact command, observed output, how it breaks the intent, and which adversary/adversaries found it. State **"No verified findings"** when none survive.
- **Dropped**: per adversary, the count of dropped findings and why (or `0 dropped`).
