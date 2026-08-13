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
- Keep LSL/OSC naming and transport in `polar-h10-output`.
- Keep `apps/polar-stream` a thin coordinator and presentation layer.
- Cross the Rust/WebView boundary in bounded batches or channels, never by
  invoking the frontend once per high-rate sample.
- Do not add a second source of truth for metric definitions or stream suffixes.
- Keep every queue bounded, define its overflow behavior, and keep raw sensor
  publication independent of display, persistence, logging, and derived work.

## Change discipline

- Inspect `git status` before edits and preserve unrelated work.
- Prefer small, reversible changes with tests close to the owning module.
- Do not commit generated build directories, local secrets, sensor recordings,
  or participant-identifying data.
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
for-ai/scripts/check-context.sh
```

Preview generation changes also require `npm run check:previews`. Release or
packaging changes require the checks documented in `RELEASING.md`.
