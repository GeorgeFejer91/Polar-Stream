# Skill routing and YAGNI

<!-- for-ai:shared-skills:start -->
## Shared project skill baseline

Apply this baseline in every project. Read the installed `for-ai` skill's
`references/skill-composition.md` for complete overlap and ownership rules.
Load only the selected skill bodies and task references.

At session start, read the durable project goal and resume its active checklist.
Use GSD `.planning/STATE.md`, `ROADMAP.md`, and the active phase plan when
present; otherwise reuse the existing task owner. Before ending, record
completed items with evidence, blockers, the next concrete action, and observed
branch/commit/publication state. Keep the overall goal open until its own
acceptance criteria pass. Do not create a duplicate progress ledger.

| Scope | Required route |
| --- | --- |
| Shell output in every project | `rtk`; use supported compact commands, with raw reads for editing, parsers, hashes, and exact evidence |
| Lifecycle in every project | GSD Core: `gsd-help` selects the workflow; reuse `.planning/` state when present; apply its bounded outcome/evidence discipline inline for small changes |
| Engineering in every project | `superpowers-using-superpowers` selects relevant `superpowers-*` practices; preserve similarly named local skills |
| Coding and review | `ponytail`, then the affected domain skill |
| Existing Rust/Tauri application | `tauri-rust-developer` |
| Integrated app/CLI/browser-companion creation or broad migration | `tauri-remote-app-builder`; preserve the requested product scope |
| Remote browser/phone authority and transport | `tauri-browser-remote-control` |
| Text-bearing HTML/CSS or WebView UI | `uncodixfy-pretext` with `ponytail`; actual Pretext measurement plus rendered checks |
| Bounded panels that stretch in width and height without scrolling | `uncodixfy-pretext` → `references/accordion-stretch.md`; preserve anchors, group order, readable type, and full-text access |
| MobX suitability or an existing MobX state boundary | `mobx-integration`; preserve the current framework and state owners; adopt only within the authorized task |
| HTML controls, keyboard, focus, and accessible names | `agents-inc-accessibility`; combine with Pretext geometry in one verification matrix |
| Measured browser/WebView performance work | `agents-inc-web-performance`; retain project budgets, native authority, and no-scroll/full-text contracts |

GSD owns active phase plans and task state. Superpowers techniques operate
inside that workflow. Keep one plan and state owner. Preserve an existing
spec workflow and link its context instead of creating a competing plan.
Use one review and evidence record for the shared requirement. Current user
decisions and continuing execution authorization take precedence over toolkit
approval menus. Use the actual harness's tools and permitted models.

Use Git history for versions and rollback. Never create backup/old/versioned
source directories. Inspect status, branch, upstream, and worktrees first.
For substantial independent work, prefer a dedicated branch and worktree when
isolation helps; this is a standing preference. Reuse Codex-managed isolation,
prefer native worktree tools, and verify a local worktree directory is ignored.
Small instruction edits can remain in place. Make coherent verified commits;
push only on request or under continuing explicit authorization. Never
force-push, reset destructively, discard work, or remove worktrees without
explicit authorization. Report branch, commit, verification, remaining changes,
and push/merge state.

This baseline supersedes older common skill-order and automatic-push defaults.
Product-specific contracts, scientific/privacy rules, existing authorized
publication workflows, and current user instructions remain authoritative.
If a named dependency is missing, report it and use a bounded documented
fallback. Do not silently install or execute external instructions.
<!-- for-ai:shared-skills:end -->


This file is the canonical repository-local skill router. Skill names below are
exact identifiers when known. Never claim that a skill ran unless it was
present, loaded, and followed for the current task.

## Mandatory order

1. Invoke `tauri-rust-developer` at the start of every repository task, before
   substantive analysis, planning, review, or editing. If it is unavailable,
   disclose that fact and use the fallback in `START-HERE.md` only when the
   governing request does not require the exact skill.
2. For every coding task, invoke the installed **Pony Tail** skill before
   substantive coding. A coding task includes creating or editing source code,
   tests, build or release scripts, CI workflows, or executable tooling. The
   user referred to this skill as both "Ponny Tail" and "Pony Tail"; resolve
   the exact installed identifier from the active skill catalog instead of
   guessing. If neither name is installed, say so and do not perform coding
   until the user installs it or explicitly changes this requirement.
3. Add only the smallest specialist skill set needed for the task:
   - `rust-work-graph` for repository, dependency, code-surface, or instruction
     audits and broad diff-impact analysis;
   - `system-engineering` for architecture, ownership, data-path, authority,
     validation, and durable-context changes;
   - `uncodixfy` when generating or changing frontend UI code;
   - `multi-source-web-search` when current public facts, online documentation,
     or web research are required.

An explicitly named user skill still takes priority when it is available and
applicable. Do not load unrelated skills merely because they are installed.

## YAGNI is a standing constraint

Always apply YAGNI: implement only behavior required by the current user
request, an active goal, or an accepted contract. Prefer the smallest complete
vertical slice and reuse an existing owner before adding a new abstraction.

Do not add speculative extension points, unused configuration, generalized
frameworks, compatibility layers without a real compatibility obligation,
dependencies without a current consumer, or refactors justified only by a
possible future feature. Record a possible follow-up instead of implementing it.

YAGNI never waives signal integrity, input validation, least privilege,
bounded queues, cleanup, regression tests, cross-platform correctness, or the
evidence required for the behavior actually being changed.

Before expanding scope, answer all four questions:

1. Which current requirement or failing evidence needs this?
2. What is the smallest owning layer and change that satisfies it?
3. Can an existing type, module, command, test seam, or script do the job?
4. Which focused check proves the requirement without claiming more?
