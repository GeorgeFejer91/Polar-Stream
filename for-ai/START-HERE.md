# Start here

## Mandatory skill startup gate

Invoke the installed `tauri-rust developer` skill at the start of every task,
before substantive project analysis, planning, review, or editing. Apply its
Tauri and Rust guidance together with this repository context. Higher-priority
system, developer, and user instructions still win.

If the skill is not installed or cannot be loaded:

1. say so explicitly before claiming any skill-based review;
2. do not invent, approximate, or silently impersonate its instructions;
3. use `ENGINEERING-PRIORITIES.md`, `GLOBAL-INSTRUCTIONS.md`, the crate-local
   documentation, and repository checks as the transparent fallback; and
4. treat the missing skill as a blocker only when the user or governing
   instructions require that exact skill rather than an evidence-based fallback.

## Required sequence

1. Pass the mandatory skill startup gate above.
2. Read the root `AGENTS.md` and every document it lists.
3. Run `for-ai/scripts/repo-snapshot.sh` to see the current branch, worktree,
   recent commits, and available checks.
4. Inspect only the code and documentation relevant to the requested outcome.
5. Preserve uncommitted work. Never assume an unfamiliar change is disposable.
6. State the intended scope before making broad or externally visible changes.

This is the standalone canonical repository at
`/home/George/Documents/GitHub/Polar-Stream`. Do not redirect implementation to
the legacy `PolarH10`/Mesmerism fork; that checkout is quarantined at
`/home/George/Documents/GitHub/_quarantine/PolarH10-mesmerism-fork` and is not a
development target for Polar Stream.

## Repository map

- `apps/polar-stream/`: thin Tauri coordinator and HTML/CSS/JavaScript UI.
- `crates/polar-h10-core/`: Polar PMD/HR decoding and protocol primitives.
- `crates/polar-h10-input/`: Bluetooth discovery, connection, and input events.
- `crates/polar-h10-metrics/`: derived metric processors and evidence catalog.
- `crates/polar-h10-math/`: bounded custom formulas and stateful scalar DSP/HRV.
- `crates/polar-h10-output/`: canonical names, LSL/OSC publication, and bounded
  native CSV persistence.
- `scripts/`: build, release, preview, and interface-validation tooling.
- `docs/`: scientific evidence and architecture assessments.
- `.github/workflows/`: CI, release, and download-page automation.

## Definition of done

Work is not complete until behavior is implemented, proportionate checks pass,
the diff is reviewed for accidental scope, and this directory is updated when
one of the triggers in `SELF-UPDATE.md` applies. For every completed edit, the
source checkout, installed per-user desktop app, and public GitHub Pages app
must also be synchronized and verified under `GLOBAL-INSTRUCTIONS.md`; the final
handoff must include the verified live Pages URL.
