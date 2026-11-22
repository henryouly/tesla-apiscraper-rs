---
name: tdd-workflow
description: General test-driven development (TDD) workflow for adding features, making improvements, fixing bugs, or addressing PR review comments — write a failing test first to reproduce or specify the behavior (Red), implement the minimal change (Green), verify with the test suite and linters, and commit cleanly.
---

# Test-Driven Development (TDD) Workflow

Use this skill whenever adding a feature, improving existing behavior, fixing a bug, or resolving a PR review comment. Never edit production code before observing a failing test that proves the gap or defect.

## Core Loop (Red → Green → Refactor)

### 1. Understand & Verify First
- Read the relevant code and existing tests to confirm the exact current behavior.
- For bug reports or PR review comments (human or automated): verify whether the claim is factually accurate before changing anything. Do not blindly apply suggestions or silently dismiss them without checking.

### 2. Red — Write a Failing Test First
- Add the smallest, most deterministic test that:
  - **Bug / review fix**: reproduces the exact defect against the unmodified production code.
  - **Feature / improvement**: specifies the desired behavior and edge cases.
- Prefer the lowest test level that faithfully reproduces the real behavior (unit test for pure logic/protocol edge cases; real-service integration test when the behavior depends on an external engine or database).
- Run the new test **before** touching production code and confirm it **fails for the expected reason**. If it passes immediately, the test is not exercising the bug or new requirement — tighten it until it fails.

### 3. Green — Implement the Minimal Fix or Feature
- Make the smallest, cleanest production change needed to make the failing test pass.
- Preserve backwards compatibility and avoid unrelated refactors or breaking schema/API changes unless explicitly requested.
- Re-run the new test and confirm it now passes.

### 4. Refactor & Full Verification
- Clean up any duplication or test boilerplate without changing behavior.
- Run the project's full local verification suite:
  - Formatter check
  - Linter / static analysis (including test targets)
  - Full unit test suite
  - All integration tests
- Confirm zero regressions and zero new warnings.

### 5. Commit, CI Gate & Merge Readiness
- Keep commits atomic and clear (one logical change per commit; separate bug fixes from unrelated refactors).
- When iterating on a PR:
  1. Fetch all feedback (reviews, top-level conversation comments, inline review comments, and CI checks).
  2. Run the Red → Green → Refactor loop for each valid issue.
  3. Push and wait for **every CI check** — including **all unit and integration test jobs** — to pass completely before considering the PR ready to merge.
