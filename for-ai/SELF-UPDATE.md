# Self-update protocol

This folder evolves with the repository; it is not a one-time prompt dump.

## Update triggers

Update the relevant files in the same change whenever any of these occur:

- product purpose, user workflow, or safety framing changes;
- crate/module ownership or data flow changes;
- a stable external contract, output name, unit, or configuration schema changes;
- a goal is added, completed, deferred, or reprioritized;
- release status, supported platform, or known constraint changes;
- latency budgets, measured performance, queue capacities/overflow policy,
  transport health, or signal-quality rejection/reset behavior changes;
- a durable architecture decision is made or superseded;
- agents repeatedly rediscover the same non-obvious fact or failure mode.

## Procedure

1. Verify the fact from code, tests, Git, or authoritative documentation.
2. Edit the narrowest canonical file; avoid copying the same volatile detail to
   several documents.
3. Add a dated decision entry when the reasoning should survive implementation.
4. Move completed goals to history and write the next measurable outcome.
5. Run `for-ai/scripts/check-context.sh`.
6. Review the context diff for secrets, participant data, stale branch claims,
   contradictory instructions, and unsupported scientific statements.

## Rules for self-modification

- Agents may update this folder only as part of authorized repository work.
- Never weaken user, system, security, privacy, validation, or scientific-safety
  requirements to make a task easier.
- Do not invent current status. Use explicit dates and label uncertain material.
- Preserve useful decision history; supersede it with a new entry instead of
  rewriting the past.
- Keep documents readable without proprietary agent tooling.
