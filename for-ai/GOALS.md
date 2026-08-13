# Evolving goals

Update this file when priorities or completion criteria change. Move completed
items into the dated history instead of silently deleting them.

## Now

1. Publish a continuously synchronized browser demo on GitHub Pages.
   - Build Pages from the exact canonical `apps/polar-stream/ui/` assets used by
     Tauri; do not maintain a second interface copy.
   - Offer an explicit selectable NeuroKit mock input in both Pages and the
     offline Tauri app, replaying deterministic ECG, ACC, respiration, and
     derived demo values through the shared runtime event contract.
   - Label synthetic input separately from the experimental Web Bluetooth H10
     adapter, and keep browser LSL/OSC behavior clearly unavailable.
   - Keep direct browser H10 support feature-detected and permission-gated;
     preserve mock fallback everywhere.
   - Deploy after successful changes to `main`, and fail CI if the staged Pages
     artifact or browser runtime drifts from the desktop UI.
   - Cover desktop Chromium and smartphone layouts down to 320 CSS pixels,
     including the output library and metric controls, without horizontal page
     overflow.
   - Implementation and automated coverage are present; retain these as ongoing
     parity requirements for every future interface change.
2. Keep repository context agent-ready.
   - Root `AGENTS.md` routes agents to `for-ai/`.
   - Context validation passes in local work and CI when adopted.

## Next

- Validate ECG/ACC throughput and reconnect behavior with physical H10 hardware
  on Linux, Windows, and macOS.
- Validate the Pages Web Bluetooth adapter with a physical H10 on supported
  desktop and Android Chromium, including PMD frame variants, MTU/batch cadence,
  disconnect/reconnect behavior, and long-run sample counts.
- Record measured renderer, queue, and output latency under representative MTUs.
- Measure the p99 runtime of each selected derived processor set against the BLE
  notification interval; isolate derived work if it can delay the next raw batch.
- Add non-blocking malformed-frame, UI-overflow, OSC/LSL failure, reconnect, and
  measurement-reset counters without delaying acquisition or publication.
- Complete signed/notarized distribution planning without weakening the current
  draft-release checks.
- Compare experimental breathing outputs against a reference respiratory signal.
- Make phase classification time-normalized rather than notification-batch
  dependent, version the behavior, and compare old/new output on retained raw
  ACC plus an independent respiratory reference.

## Later

- Complete Android platform glue and permissions while retaining the Rust crate
  boundaries.
- Add new metrics only with a formula owner, evidence entry, tests, stream
  metadata, preview coverage, and explicit interpretation limits.

## History

- 2026-08-13: established one canonical Tauri/Pages interface, a selectable
  offline NeuroKit mock input, automatic Pages staging/deployment, asset-parity
  checks, and desktop plus 390px/320px responsive browser coverage.
- 2026-08-13: added an experimental direct-H10 Web Bluetooth adapter to the
  canonical Pages UI, kept browser LSL/OSC explicitly disabled, mirrored the two
  ACC breathing outputs, exposed advanced experiment parameters, and added the
  algorithm/provenance handoff.
- 2026-08-13: production hardening, stacked raw X/Y/Z acceleration, and the
  ECG/ACC output-library redesign passed cross-platform CI and merged in PR #5.
- 2026-08-13: simplified the output library around an ECG/ACC family selector,
  reduced new ACC breathing choices to magnitude and three-state phase, and
  exposed their shared experimental classifier controls before selection.
- 2026-08-13: established the dedicated agent control plane and prioritized the
  single stacked X/Y/Z accelerometer visualization.
