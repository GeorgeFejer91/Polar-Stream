# Orchestration

## Planning

- Translate a request into concrete outcomes, affected ownership boundaries,
  validation, and documentation impact.
- For data-path changes, write down the expected signal-integrity, latency,
  queue/overflow, failure, reset, and platform effects before implementation.
- Keep one active implementation step at a time unless independent work can be
  safely parallelized.
- Before external publication, re-check the exact diff, branch, remote, and PR.

## Delegation

Parallel agents are useful only for bounded, independent work. Give each agent:

- one outcome and an explicit file or subsystem scope;
- the required `for-ai/` reading list;
- constraints, checks, and a requested evidence-based handoff;
- non-overlapping write ownership whenever possible.

The coordinating agent remains responsible for reading applicable instructions,
integrating changes, resolving overlap, running final checks, and updating this
context. Never delegate responsibility for repository-wide correctness.

## Shared-worktree protocol

1. Inspect current changes and timestamps before editing.
2. Announce or record file ownership when multiple agents share a checkout.
3. Do not revert or reformat another worker's active files without coordination.
4. If files change concurrently, pause, identify the source, and integrate only
   after the write set stabilizes.
5. Take a final `git status` and diff snapshot immediately before committing.

## Handoff

Use `for-ai/templates/HANDOFF.md`. A useful handoff states the outcome, exact
files, decisions, checks and results, remaining risks, and the next safe action.
Do not report a test as passing unless it ran against the handed-off state.
Do not report cross-platform or hardware validation when only host-side checks
ran; name the untested platform or physical-device boundary explicitly.
