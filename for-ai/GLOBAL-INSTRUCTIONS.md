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
- Keep `crates/stream-mini-runtime` as the shared one-session coordinator for
  standalone mini applets. It may use input, metrics, output, and Tauri types,
  but it must not become a second protocol decoder or metric catalog.
- Keep `apps/polar-stream-mini` and `apps/vernier-stream-mini` independent
  Tauri packages with app-local node-style UIs. They are not the main
  controller and are not part of the Pages frontend parity surface.
- Cross the Rust/WebView boundary in bounded batches or channels, never by
  invoking the frontend once per high-rate sample.
- Do not add a second source of truth for metric definitions or stream suffixes.
- Keep every queue bounded, define its overflow behavior, and keep raw sensor
  publication independent of display, persistence, logging, and derived work.

## Canonical frontend parity

- `apps/polar-stream/ui/` is the only frontend source for both the installed
  Tauri app and the GitHub Pages app. Never maintain, patch, or style a second
  browser-only interface copy.
- The mini app UIs under `apps/polar-stream-mini/ui/` and
  `apps/vernier-stream-mini/ui/` are intentional standalone desktop surfaces.
  Keep them as compact transparent-shell apps whose node card is the visible
  program outline, visually aligned with the main node view but without a
  surrounding board. Do not publish them as the Pages demo or require them to
  share the main controller's browser runtime adapter.
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

## Mandatory product-scoped synchronization

Treat the repository as three separately installed products. Select the
contract or contracts in `for-ai/apps/` from the user's wording before editing.

- Main multi-streamer work synchronizes the checkout, installed Polar Stream
  app, and, when the canonical UI or download surface changes, the public Pages
  app at `https://georgefejer91.github.io/Polar-Stream/`.
- Polar Mini work builds, installs, and launch-tests only Polar Stream Mini.
- Vernier Mini work builds, installs, and launch-tests only Vernier Stream Mini.
- Shared mini-runtime work builds, installs, and launch-tests both minis side by
  side. It does not trigger a main-app or Pages release by itself.

Each product keeps its own bundle identifier, executable, application/config
directory, preferences, runtime state, icon, process, and installer. No product
may overwrite another product's install or preference path, require another
product to be running, or use another product as a helper process.

After applicable tests pass, build the selected production artifact, preserve
the previous installed executable as a rollback copy, install into that
product's resolved location, and launch or smoke-test the installed copy. Never
publish unrelated dirty files to satisfy a delivery gate. The final response
must state each selected product's installed path, package path, checks, and
delivery state. Include the clickable Pages URL and live hash result only when
the selected scope includes the main or download surface.

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

Run the narrowest relevant check during iteration, then the selected app
contract's final set. Shared Rust changes retain workspace gates; app-local UI
changes do not need unrelated browser-demo validation.

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
packaging changes require the applicable checks documented in `RELEASING.md`.
Main UI/download work also requires `npm run build:browser-demo` and, after
publication, `npm run verify:live-pages`. Mini-only UI/package work requires
the two mini JS syntax checks, focused Rust gates, and the selected mini's exact
installer build/install/parallel-launch smoke test.
