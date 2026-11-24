---
name: arena
description: "Spawn N parallel candidates at the same task, pick a base, and graft the strongest parts of the losers into it. Use for \"arena this\", \"throw it in the arena\", or when one attempt at a non-trivial artifact would lock in the wrong shape."
---

# Arena

Track progress through all six phases (**Frame**, **Fan out**, **Cross-judge**, **Pick**, **Graft**, **Verify**) before launching anything.

## Phase A: Frame

Every candidate gets the same prompt, so the prompt is the contract.

1. State the artifact each candidate is producing.
2. Derive the rubric — 3–6 concrete, gradeable criteria for what success looks like on *this* task. The rubric is the picker's tool in Phase D; candidates only see the task.
3. Pick the runners: by default, spawn **3 parallel `self` subagents** via `invoke_subagent` (`TypeName: "self"`, `Model: "inherit"`, `Workspace: "inherit"`). Give each runner a distinct role or design angle when using the same model so candidates explore genuinely different directions (spawn more for broader design exploration, or fewer when scope is narrow).
4. Give each candidate its own isolated output location — a detached `git worktree` under `/tmp/arena-<slug>/candidate-<n>/` when modifying repository files, or a scratch directory `/tmp/arena-<slug>/candidate-<n>/` otherwise — so parallel candidates never collide on shared state. For Rust builds in `tesla-apiscraper-rs`, set `CARGO_TARGET_DIR=<repo_root>/target` so worktrees reuse cached dependencies instead of triggering cold builds.

## Phase B: Fan out

Spawn all N subagents concurrently in a single `invoke_subagent` call, each with:
- the task and shared grounding path,
- its own isolated output path / worktree,
- instructions to produce the artifact and a short rationale naming the alternatives it considered and what it rejected.

If a candidate produces no output, proceed with N-1 and note the dropout.

## Phase C: Cross-judge

After all Phase B candidates complete, spawn one read-only `research` subagent (`TypeName: "research"`, `Model: "inherit"`, `Workspace: "inherit"`). While you read in Phase D, the judge scores each candidate, by path label, on each rubric criterion and recommends a base.

## Phase D: Pick a base

Read every candidate end to end before picking. Score each against the rubric criterion by criterion, not on holistic feel, and compare with the cross-judge from Phase C. Agreement on the base confirms the pick; disagreement means one of you is biased or the rubric was ambiguous — read both rationales before deciding.

Pick the base a future maintainer can extend most easily without breaking invariants. When two feel tied, prefer the cleaner boundary or smaller API.

## Phase E: Graft

Walk each losing candidate once more for what is worth porting into the base — usually one or two things per candidate, not most of it. Fold each graft in by hand so the result stays coherent under one mental model; do not paste mechanically.

If the candidates converge on one shape, that is strong agreement — note it and ship the consensus shape without grafting. If they wildly diverge, Phase A was under-specified; reframe and re-run rather than averaging the divergence.

## Phase F: Verify

Verify the synthesized artifact against the real system — run the tests/commands, read the actual outputs, and inspect the diff (never trust "it compiles"). If verification finds a problem the arena missed, either Phase A was wrong (re-frame and re-run) or a candidate caught it and you missed the graft (go back to Phase E). Do not paper over it.

Clean up all temporary worktrees (`git worktree remove --force ...` and `git worktree prune`) and `/tmp/arena-<slug>/` once verification finishes.

## Outputs

One synthesized artifact, with a short synthesis note alongside naming:
- the base and why,
- the cross-judge's verdict (or that Phase C was skipped),
- each graft with its source candidate,
- what was rejected and why,
- any convergence or dropouts,
- the verification result.
