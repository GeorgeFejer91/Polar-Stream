# Evolving goals

Update this file when priorities or completion criteria change. Move completed
items into the dated history instead of silently deleting them.

## Now

1. Publish a continuously synchronized browser demo on GitHub Pages.
   - Build Pages from the exact canonical `apps/polar-stream/ui/` assets used by
     Tauri; do not maintain a second interface copy.
   - Offer the canonical anonymized 60-second H10 recording as an explicit
     selectable preview input in both Pages and the offline Tauri app; never use
     generated signals as an app fallback.
   - Label recorded preview input separately from the experimental Web Bluetooth H10
     adapter. Keep the Pages runtime entirely in-browser: retain the shared
     native LSL/OSC controls, reject browser attempts with an installed-app
     error and release link, and do not depend on a companion, wrapper, or
     relay.
   - Keep direct browser H10 support feature-detected and permission-gated;
     preserve recorded hardware-free preview input everywhere.
   - Keep the browser live channel and CSV recorder self-contained, bounded,
     visibly stopped on capacity/disconnect, record all incoming raw rows plus
     produced metrics, and remain explicit that they are not discoverable
     native LSL outlets.
   - Deploy after successful changes to `main`, and fail CI if the staged Pages
     artifact or browser runtime drifts from the desktop UI.
   - Cover desktop Chromium and smartphone layouts down to 320 CSS pixels,
     including the output library and metric controls, without horizontal page
     overflow.
   - Implementation and automated coverage are present; retain these as ongoing
     parity requirements for every future interface change.
   - Retain the shared CSV and experimental PCM-audio destination controls in
     desktop and Pages layouts. Keep native CSV off the hot path, keep browser
     storage/audio bounded, and preserve the reference WAV decoder contract.
   - Treat Android Chrome Web Bluetooth as foreground-only until physical-device
     evidence says more. Never turn a screen wake lock into a background-capture
     claim; a guaranteed locked-screen workflow requires native Android work.
   - After every completed edit, production-build/install the desktop app,
     publish the accepted canonical UI to `main`, verify the live Pages manifest
     against the checkout, and return the live Pages URL in the handoff.
2. Keep the metric picker and Formula Lab evidence-first and approachable.
   - Preserve one-at-a-time output addition and the responsive ECG/ACC filter
     layout.
   - Keep recorded previews for every catalog metric and update them live for
     settings that affect display or published values.
   - Keep compact, detail, Formula Lab, and recorded-input displays continuously
     looped without a boundary gap; preserve categorical outputs as stepped states.
   - Keep concise scientific context, citations, and mathematical definitions
     on every metric; retain the Excite-O-Meter source link and distinguish it
     from Polar Stream's experimental activation composite.
   - Keep Formula Lab source-clock constraints, automatic time axis, templates,
     explained insert keyboard, recorded before/after preview, native resource
     bounds, and per-formula fault isolation.
3. Keep repository context agent-ready.
   - Root `AGENTS.md` routes agents to `for-ai/`.
   - Context validation passes in local work and CI when adopted.

## Next

- Complete the bounded native H10 acceptance for the optional Rusty LSL backend:
  one exact session, advancing sensor timestamps, ECG 1ch/130 Hz and ACC
  3ch/200 Hz, nonzero bounded axes, two exact official pylsl/liblsl inlets,
  loss/reorder/rate evidence, and clean process/socket shutdown. The direct
  Windows WinRT scanner/session now has physical success through both advancing
  first sensor frames, and both Rusty outlets were resolved and opened by the
  pinned official consumers. The incomplete edge was verifier sequencing: the
  official inlet workers started after source readiness instead of immediately
  after outlet initialization as in the passing synthetic gate. Run one fresh
  attended same-epoch qualification with those receivers waiting first, then
  require the complete counts/rates/loss/reorder and cleanup evidence. Rusty
  physical transport remains unaccepted until that run passes.
- Decide whether and how an AGPL-enabled source build may be distributed before
  enabling Rusty LSL in any package. Keep liblsl as the default until both that
  decision and physical/cross-platform gates close.
- Validate ECG/ACC throughput and reconnect behavior with physical H10 hardware
  on Linux, Windows, and macOS. On Windows, retain the requested/observed link
  diagnostics and sample/drop counts so low-interval behavior is evidence-based.
- Validate the Pages Web Bluetooth adapter with a physical H10 on supported
  desktop and Android Chromium, including PMD frame variants, MTU/batch cadence,
  disconnect/reconnect behavior, and long-run sample counts. Use the checked-in
  live Pages provenance verifier and strict offline CSV analyzer for the full
  Motorola Google Chrome foreground run; do not substitute localhost or mock
  evidence. The public-site chooser and connection smoke test passed on
  2026-08-14, while the timed CSV and reconnect evidence remains pending.
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

- 2026-08-14: hardened browser-H10 compatibility around capability and
  Permissions Policy error diagnosis, immediate user-gesture chooser invocation,
  one transient GATT retry, and legacy characteristic writes. A physical
  Motorola/Chrome public-Pages connection smoke test passed; complete
  CSV/rate/reconnect acceptance remains pending.
- 2026-08-14: added a public-Pages-only Motorola Brave hardware acceptance
  procedure, complete deployed-asset/header provenance capture, and strict
  offline H10 CSV analysis for source, sensor time, rates, loss, gaps, raw
  values, HR/RR, and recorder termination. Physical evidence remains pending.
- 2026-08-14: added shared local-CSV and experimental audio-data toggles, a
  bounded native CSV writer, all-input browser CSV capture, a 22.05 kbit/s
  stereo Manchester/CRC32 format with a WAV-to-CSV decoder, Android-touch GATT
  coverage, and explicit browser-background lifecycle limits.
- 2026-08-13: established one canonical Tauri/Pages interface, a selectable
  offline NeuroKit mock input, automatic Pages staging/deployment, asset-parity
  checks, and desktop plus 390px/320px responsive browser coverage.
- 2026-08-13: added an experimental direct-H10 Web Bluetooth adapter to the
  canonical Pages UI, kept browser LSL/OSC explicitly disabled, mirrored the two
  ACC breathing outputs, exposed advanced experiment parameters, and added the
  algorithm/provenance handoff.
- 2026-08-13: added an authenticated, loopback-only native LSL companion for
  explicitly paired Pages sessions and ported MesmerPrism's bounded WinRT PMD
  access/retry pattern without misrepresenting read-only Windows MTU.
- 2026-08-13: superseded and removed the browser-to-LSL companion so the Pages
  workflow is fully self-contained; native LSL/OSC were hidden in browser mode
  and remained installed-app features. The 2026-08-14 frontend-parity decision
  later restored the visible controls without restoring browser publication.
- 2026-08-13: added a from-scratch browser session outlet: exact incoming event
  batches are available within the browser, while selected ECG/ACC/metric rows
  can be recorded and downloaded as bounded timestamped CSV without a server,
  helper, or native wrapper.
- 2026-08-13: production hardening, stacked raw X/Y/Z acceleration, and the
  ECG/ACC output-library redesign passed cross-platform CI and merged in PR #5.
- 2026-08-13: simplified the output library around an ECG/ACC family selector,
  reduced new ACC breathing choices to magnitude and three-state phase, and
  exposed their shared experimental classifier controls before selection.
- 2026-08-13: established the dedicated agent control plane and prioritized the
  single stacked X/Y/Z accelerometer visualization.
- 2026-08-15: replaced synthetic app previews with the canonical anonymized
  H10 recording, expanded recorded preview coverage to all 47 metrics, added
  live pre-save setting previews and a bounded native Formula Lab, and made
  local/install/live-Pages synchronization plus URL verification mandatory for
  every completed edit.
