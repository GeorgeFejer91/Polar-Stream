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
- Red ECG / blue ACC output-library modes. ACC mode offers only raw ACC, 3D
  motion magnitude, a continuous experimental breathing projection, and a
  three-state experimental phase classifier.
- Shared pre-save ACC breathing controls for the two public breathing outputs:
  two or three axes (X + Z recommended), smoothing, phase sensitivity and
  direction; the magnitude output can optionally normalize to 0–1.
- A phase-only breathing circle with asymptotic size limits and pause inertia.
- Public dedicated GitHub repository at `GeorgeFejer91/Polar-Stream`.
- The production-hardening and ECG/ACC output-library work is merged on `main`
  in PR #5.
- One canonical interface tree now targets Tauri and GitHub Pages. The staged
  Pages artifact records hashes of every shared asset, and browser validation
  covers desktop, 390px touch, and 320px touch layouts.
- A selectable, deterministic NeuroKit mock input replays synthetic ECGSYN ECG,
  respiration-derived X/Y/Z motion, and illustrative metrics through the shared
  runtime event contract in both Pages and the offline Tauri app.
- GitHub Pages offers an experimental Web Bluetooth input in supported secure
  Chromium contexts. It requests a Polar H10, writes the canonical PMD ECG/ACC
  start commands, and decodes ECG, uncompressed/variable-delta ACC, HR/RR, and
  battery notifications into the shared UI event contract.
- The browser adapter includes a JavaScript mirror of the two retained ACC
  breathing outputs. Playwright validates the browser chooser/GATT contract,
  commands, protocol edge cases, breathing calibration, and responsive UI with
  an emulated device. Browser LSL and OSC controls are disabled.
- The ACC breathing add/adjust workflows expose axes, smoothing, sensitivity,
  direction, calibration window/range, stale timeout, adaptive bounds/window,
  and robust quantiles. `docs/acc-breathing-handoff.md` documents provenance,
  exact formulas, current/upstream differences, parameters, and validation.

## Active branch context

`main` is the merged baseline. Confirm live Git and PR state with
`for-ai/scripts/repo-snapshot.sh`; do not infer it from this snapshot.

## Known constraints

- Real BLE behavior and latency still depend on platform adapters, radio state,
  ATT MTU, and operating-system scheduling.
- liblsl availability and packaging differ by platform; follow `RELEASING.md`.
- Both public ACC-derived respiration outputs are unvalidated and require
  comparison with a reference respiratory sensor before interpretation.
- Windows and macOS public packages are currently unsigned/ad-hoc unless release
  infrastructure states otherwise.
- Physical-device latency percentiles, queue high-water marks, and transport
  drop/error counters are not yet captured as a single end-to-end benchmark.
- Selected derived processors execute after raw publication in the coordinator
  loop. The input channel is bounded, but physical-device measurements have not
  yet proven every opt-in metric set stays inside the next-notification budget.
- The mock input demonstrates interface behavior only. It does not validate H10
  fidelity, timing, ACC respiration, or output transports, and it never opens
  native BLE, LSL, or OSC connections.
- The Web Bluetooth adapter has not yet been exercised against a physical H10.
  Browser/OS support remains non-portable, and BLE throughput, reconnect,
  compressed-frame behavior, and timing must be recorded on real hardware.
- Ordinary browser tabs do not publish LSL or OSC; those transports require the
  desktop app. The UI must never suggest that a disabled browser switch opens a
  native outlet.
- Breathing phase sensitivity is currently applied to normalized change per ACC
  notification batch, not change per second. BLE batch cadence can therefore
  affect classification and must be corrected/versioned before cross-platform
  phase equivalence is claimed.
