# Global repository instructions

## Startup requirement

- Invoke the `tauri-rust developer` skill as required by `START-HERE.md`.
- Use `ENGINEERING-PRIORITIES.md` as the repository-specific architecture and
  performance contract. The skill augments that contract; it does not replace
  verified project facts or higher-priority instructions.

## Product guardrails

- Polar Stream is an unofficial research signal bridge, not a medical device.
- Never present derived metrics as diagnoses, validated emotional states, or
  clinical measurements. Preserve evidence links and interpretation limits.
- Raw acquisition and LSL/OSC publication must remain independent of WebView
  rendering speed. A slow or hidden UI may drop display frames, never sensor or
  publication work.
- Keep canonical output names stable unless a migration is intentionally
  designed and documented.
- Raw ECG and raw X/Y/Z accelerometer values stay in native units at protocol
  boundaries. Visualization transforms must not silently alter raw outputs.

## Architecture rules

- Keep protocol decoding in `polar-h10-core`.
- Keep BLE/platform input in `polar-h10-input`.
- Keep derived calculations and their tests in `polar-h10-metrics`.
- Keep bounded custom-formula parsing/evaluation in `polar-h10-math`; treat
  formulas as untrusted configuration and retain AST, operation, state, and
  per-formula fault bounds.
- Keep LSL/OSC naming and transport in `polar-h10-output`.
- Keep `apps/polar-stream` a thin coordinator and presentation layer.
- Cross the Rust/WebView boundary in bounded batches or channels, never by
  invoking the frontend once per high-rate sample.
- Do not add a second source of truth for metric definitions or stream suffixes.
- Keep every queue bounded, define its overflow behavior, and keep raw sensor
  publication independent of display, persistence, logging, and derived work.

## Canonical frontend parity

- `apps/polar-stream/ui/` is the only frontend source for both the installed
  Tauri app and the GitHub Pages app. Never maintain, patch, or style a second
  browser-only interface copy.
- HTML, CSS, interaction behavior, metric-library flows, visualizations, and
  responsive formatting must remain identical across installed and Pages
  builds. Stage Pages from the canonical UI and keep asset-hash plus desktop,
  390px-touch, and 320px-touch checks passing.
- Runtime capabilities may differ behind the shared frontend adapter. Tauri may
  use Rust IPC and native LSL/OSC/CSV; Pages may use browser-local recorded preview input,
  experimental Web Bluetooth, bounded browser CSV, and browser audio. Express
  unavailable capabilities through runtime data and UI state, not a forked
  layout or divergent interaction implementation.
- Keep shared feature controls visible and operable in both targets even when a
  target lacks the backend capability. An unsupported attempt must fail closed,
  leave the feature off, and show a clear inline error naming the supported
  target and a useful next step. In particular, Pages must retain the LSL and
  OSC toggles and link rejected attempts to the latest installed-app release;
  never hide or disable those controls merely because a browser cannot publish
  the native protocols.
- Do not label a browser substitute as native LSL or OSC. A Pages limitation is
  an honest capability difference, not permission to let the two frontends
  drift.

## Mandatory three-surface synchronization

Every completed edit must end with all three user-visible surfaces on the same
accepted source state:

1. the canonical local checkout;
2. the resolved per-user installed Polar Stream desktop app; and
3. the public GitHub Pages application at
   `https://georgefejer91.github.io/Polar-Stream/`.

After applicable tests pass, build a production desktop artifact, preserve the
previous installed executable as a rollback copy, install the new artifact into
the existing resolved per-user location, and launch or smoke-test that installed
copy. Stage Pages from `apps/polar-stream/ui/`, publish the completed change to
`main` through the repository's normal reviewed Git workflow, wait for the Pages
deployment, and run the live manifest/hash verifier against the public site.
Do not call an edit complete when it exists only locally, only in a branch/PR,
only in an installed binary, or only in the staged Pages artifact.

The final response after every edit must state the installed path, commit/PR or
main delivery state, concrete checks, live deployment verification, and the
clickable Pages URL above. If build, install, publish, or live verification is
blocked, investigate safe in-scope alternatives, report the exact blocker, and
describe the work as incomplete. Never publish unrelated dirty files merely to
satisfy synchronization.

## Change discipline

- Inspect `git status` before edits and preserve unrelated work.
- Prefer small, reversible changes with tests close to the owning module.
- Do not commit generated build directories, local secrets, ad-hoc sensor
  recordings, or participant-identifying data. The sole preview-data exception
  is the reviewed canonical anonymized fixture at
  `apps/polar-stream/ui/data/preview-recording.json`.
- Update public documentation when behavior, setup, output contracts, or release
  expectations change.
- Add a `DECISIONS.md` entry for durable architectural or compatibility choices.

## Required validation

Run the narrowest relevant check during iteration and the applicable final set:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
node --check apps/polar-stream/ui/app.js
npm run validate:interface
npm run validate:browser-demo
for-ai/scripts/check-context.sh
```

Preview generation changes also require `npm run check:previews`. Release or
packaging changes require the checks documented in `RELEASING.md`. Before the
handoff for any completed edit, also run the applicable production desktop
build/install smoke test, `npm run build:browser-demo`, and after publication
`npm run verify:live-pages`; include the verified URL in the response.
