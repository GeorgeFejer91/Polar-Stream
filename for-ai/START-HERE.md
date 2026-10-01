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

## Project routing

- "Main Polar Stream", "the controller", "the desktop app", "the browser demo",
  "multi-streamer", and unqualified "Polar Stream" UI work target
  `apps/polar-stream/` and the canonical `apps/polar-stream/ui/` Pages-shared
  interface. Read `apps/MULTI-STREAMER.md`.
- "Polar Stream Mini", "Polar Mini", or "Polar mini node" target
  `apps/polar-stream-mini/` plus shared mini behavior in
  `crates/stream-mini-runtime/`. Read `apps/POLAR-STREAM-MINI.md`.
- "Vernier Stream Mini", "Vernier Mini", or "Vernier mini node" target
  `apps/vernier-stream-mini/` plus shared mini behavior in
  `crates/stream-mini-runtime/`. Read `apps/VERNIER-STREAM-MINI.md`.
- Work in `crates/stream-mini-runtime/` affects both minis, so read both mini
  contracts and verify both packages. Do not read or apply another app's
  product-specific rules merely because it lives in the same Cargo workspace.
- The mini apps are standalone BLE-to-LSL applets with their own bundle
  identifiers, app-local UI assets, saved stream/device preferences, and
  installer packages. Do not fold them into the main controller, browser demo,
  LabRecorder workflow, or Pages parity contract unless the user explicitly
  asks for that integration.
- Mini UI should preserve the visual language of the main app's node view while
  remaining a compact transparent-shell app: the node card itself is the whole
  visible program outline, with no surrounding board or workspace background.

## Repository map

- `apps/polar-stream/`: thin Tauri coordinator and HTML/CSS/JavaScript UI.
- `apps/polar-stream-mini/`: standalone Polar H10 BLE-to-LSL mini node applet.
- `apps/vernier-stream-mini/`: standalone Vernier Go Direct BLE-to-LSL mini
  node applet.
- `crates/polar-h10-core/`: Polar PMD/HR decoding and protocol primitives.
- `crates/polar-h10-input/`: Bluetooth discovery, connection, and input events.
- `crates/polar-h10-metrics/`: derived metric processors and evidence catalog.
- `crates/polar-h10-math/`: bounded custom formulas and stateful scalar DSP/HRV.
- `crates/polar-h10-output/`: canonical names, LSL/OSC publication, and bounded
  native CSV persistence.
- `crates/stream-mini-runtime/`: shared one-session mini-app coordinator,
  preference memory, BLE-to-LSL routing, and new-instance launcher.
- `scripts/`: build, release, preview, and interface-validation tooling.
- `docs/`: scientific evidence and architecture assessments.
- `.github/workflows/`: CI, release, and download-page automation.

## Definition of done

Work is not complete until behavior is implemented, proportionate checks pass,
the diff is reviewed for accidental scope, and this directory is updated when
one of the triggers in `SELF-UPDATE.md` applies. Build, install, launch, and
report only the selected product set under `GLOBAL-INSTRUCTIONS.md`. Main-app
or download-surface work also carries the Pages gate; mini-only work does not.
