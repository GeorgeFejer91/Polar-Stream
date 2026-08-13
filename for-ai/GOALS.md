# Evolving goals

Update this file when priorities or completion criteria change. Move completed
items into the dated history instead of silently deleting them.

## Now

1. Finish and validate the production-hardening release path.
   - Native preference and IPC boundaries are covered by tests.
   - CI, release assets, and launch smoke checks pass on supported targets.
2. Make raw acceleration readable without axis switching.
   - One visualization choice represents raw acceleration. Implemented on the
     active branch with headless interface coverage.
   - X, Y, and Z appear as three stacked, labeled, color-distinct traces with
     independent zero-centered ranges. Implemented on the active branch.
   - Desktop and narrow layouts render without overflow. Desktop is covered;
     narrow-layout regression coverage remains to be added.
3. Keep repository context agent-ready.
   - Root `AGENTS.md` routes agents to `for-ai/`.
   - Context validation passes in local work and CI when adopted.

## Next

- Validate ECG/ACC throughput and reconnect behavior with physical H10 hardware
  on Linux, Windows, and macOS.
- Record measured renderer, queue, and output latency under representative MTUs.
- Measure the p99 runtime of each selected derived processor set against the BLE
  notification interval; isolate derived work if it can delay the next raw batch.
- Add non-blocking malformed-frame, UI-overflow, OSC/LSL failure, reconnect, and
  measurement-reset counters without delaying acquisition or publication.
- Complete signed/notarized distribution planning without weakening the current
  draft-release checks.
- Compare experimental breathing outputs against a reference respiratory signal.

## Later

- Complete Android platform glue and permissions while retaining the Rust crate
  boundaries.
- Add new metrics only with a formula owner, evidence entry, tests, stream
  metadata, preview coverage, and explicit interpretation limits.

## History

- 2026-08-13: simplified the output library around an ECG/ACC family selector,
  reduced new ACC breathing choices to magnitude and three-state phase, and
  exposed their shared experimental classifier controls before selection.
- 2026-08-13: established the dedicated agent control plane and prioritized the
  single stacked X/Y/Z accelerometer visualization.
