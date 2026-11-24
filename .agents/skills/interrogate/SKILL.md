---
name: interrogate
description: "Use for \"interrogate\", \"adversarial review\", \"multi-model review\", \"challenge this\", \"stress test this code\", \"find blind spots\", or \"tear this apart\". Adversaries try to break a change or design, and only findings proven by a run survive."
---

# Interrogate

Adversaries try to break a change or a design. A finding survives only when a run proves it. Report the findings. Do not apply fixes.

## Frame

The scope is what the user names. Otherwise it is the branch diff from its merge base with `main`, through the working tree. State the intent with the **`in-your-own-words`** skill, from the user's message, the commits, and the PR description. Ask the user when the intent is unclear.

## Fan out

Run **Phase A steps 3 and 4** and **Phase B** of the **`arena`** skill, with four changes:

1. Pick 2–3 adversaries tailored to the touched surfaces of `tesla-apiscraper-rs` (e.g., Backend / Vehicle State Machine / InfluxDB invariants incl. `*_battery_range_km` miles storage & update-on-close; Frontend SolidJS / SSE / API contract / E2E; Auth / SPA fallback / Concurrency boundaries).
2. Copy the scope's uncommitted changes into each detached `git worktree` (and set `CARGO_TARGET_DIR=<repo_root>/target` for Rust runs).
3. Spawn each adversary as `self` (`invoke_subagent`, `TypeName: "self"`, `Model: "inherit"`, `Workspace: "inherit"`).
4. In place of the arena task and rationale, give each adversary the scope, the intent, its worktree path, and this brief:

> Break this change. Prove each finding with a run against the real code, such as a failing `cargo test`, an `influxdb_integration` test, a Playwright or `verify-app.mjs` repro, a repro script, or command output that shows the wrong behavior. For a design or plan, run a prototype or probe the current tree. Keep repros in your own worktree. Report each finding with a title, the exact command, the output it printed, and the output the intent requires. Leave out anything you did not run. That covers style, naming, structure, and risks that need a future change to matter. No findings is a valid result.

## Verify

Verify each finding per **`arena` Phase F** by rerunning its command. Keep a finding when the output shows the failure and the failure breaks the intent. Drop the rest. Merge findings that share a root cause.

Skip **`arena` Phases C, D, and E**. Adversaries find different failures, so the verdict keeps every verified finding, not one adversary's list.

Clean up all temporary worktrees (`git worktree remove --force ...` and `git worktree prune`) after verification, confirming the main working tree is untouched.

## Output

- **Intent**: the restatement.
- **Findings**: for each verified finding, the command, its output, how it breaks the intent, and the adversaries that found it. Say "no verified findings" when none survive.
- **Dropped**: for each adversary, the count of dropped findings and why.
