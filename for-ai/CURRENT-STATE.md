# Current state

Last verified: 2026-08-13

## Implemented

- Native BLE scan, connection, PMD ECG/ACC streaming, and HR/RR ingestion.
- Immediate raw LSL/OSC publication with canonical names.
- Demand-driven ECG, HRV, coherence, breathing, breathing-dynamics, quality,
  and explicitly experimental metric modules.
- Three-panel Tauri UI with remembered native preferences.
- Deterministic metric previews and headless interface validation.
- Cross-platform release workflows and branded application icons.
- Low-latency renderer scheduling capped at 30 Hz and paused while hidden.
- One raw accelerometer visualizer with stacked, independently scaled X/Y/Z
  lanes on the active development branch.
- Public dedicated GitHub repository at `GeorgeFejer91/Polar-Stream`.

## Active branch context

The working branch builds on `codex/tauri-production-hardening`, whose draft PR
adds native preference ownership, stable IPC errors, bounded display delivery,
release hardening, and the rainbow application icon. Confirm the live Git/PR
state with `for-ai/scripts/repo-snapshot.sh`; do not treat this paragraph as a
substitute for Git.

## Known constraints

- Real BLE behavior and latency still depend on platform adapters, radio state,
  ATT MTU, and operating-system scheduling.
- liblsl availability and packaging differ by platform; follow `RELEASING.md`.
- ACC-derived respiration features are experimental and require comparison with
  a reference respiratory sensor before interpretation.
- Windows and macOS public packages are currently unsigned/ad-hoc unless release
  infrastructure states otherwise.
- Physical-device latency percentiles, queue high-water marks, and transport
  drop/error counters are not yet captured as a single end-to-end benchmark.
- Selected derived processors execute after raw publication in the coordinator
  loop. The input channel is bounded, but physical-device measurements have not
  yet proven every opt-in metric set stays inside the next-notification budget.
