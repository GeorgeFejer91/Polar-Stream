# Skill routing and YAGNI

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
