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
   - Keep recorded preview coverage for every catalog metric, but animate only
     the selected metric rather than every library row.
   - Keep selected-detail, Formula Lab, and recorded-input displays continuously
     looped without a boundary gap; preserve categorical outputs as stepped states.
   - Keep each selected metric focused on one SVG loop, a two- or three-sentence
     scientific summary, and two or three reviewed sources. Retain the
     Excite-O-Meter source and distinguish it from Polar Stream's experimental
     activation composite.
   - Keep Formula Lab source-clock constraints, automatic time axis, templates,
     explained insert keyboard, recorded before/after preview, native resource
     bounds, and per-formula fault isolation.
3. Keep repository context agent-ready.
   - Root `AGENTS.md` routes agents to `for-ai/`.
   - Context validation passes in local work and CI when adopted.
4. Keep native Vernier recording raw-first and immediately discoverable.
   - On a verified GDX-RB connection, enable every compatible metadata-exposed
     numeric channel and create one stable per-device `rawVernier` LSL outlet
     before the first measurement is published.
   - Preserve Float32/Int32 device values exactly in Double64, use `NaN` for
     sparse absent channels, publish labels/units/metadata, and expose timestamps,
     sequence, queue loss, device drop reports, latency, period, and encoding.
   - Publish a separate explicitly derived `vernierBreathing` Float32 waveform
     bounded to 0–1, with increasing force/inhalation moving upward. Never let
     this transformation delay or alter the raw recording stream.
   - Keep the force-only browser transport and lack of browser LSL explicit.
     Retain official liblsl-consumer coverage and physically requalify the
     revised all-channel mask/two-outlet path before broadening hardware claims.
5. Preserve device-first protocol modularity in the canonical interface.
   - Keep Output and Visualization empty when no source is connected.
   - Treat connection as the only trigger for a device profile's default
     outputs, and treat those outputs as the only trigger for applicable
     visualizations.
   - Keep Polar's ECG/ACC library, Vernier's raw/breathing protocol, and future
     device profiles independently declarative. Mixed connections must switch
     complete profiles by selected source without cross-family UI leakage.
   - Keep acquisition independent from the selected UI source. Refresh either
     protocol candidate registry without stopping active same- or mixed-family
     owners, exclude connected identities, and keep every source-suffixed LSL
     outlet advancing concurrently.
   - Preserve mapped timestamp/gap evidence in bounded temporal UI rings and
     retain the force+ACC and belt+ACC-breathing comparison presets without
     putting the WebView on the authoritative path.
   - Retain the exact four-inlet official pylsl gate for ECG, ACC, rawVernier,
     and vernierBreathing with an overlapping local-LSL-clock interval. A
     physical paired H10/GDX run remains required before hardware qualification.
   - Keep search results as classified, non-widget candidates. Promote only a
     successful connection into a device widget with source-owned controls and
     color identity; keep Vernier's reconnect control inside that widget and on
     by default for a new preference state.
   - Keep device raw measurements automatic, non-removable outputs that enable
     native LSL on physical connection. Keep processed metrics/formulas opt-in,
     and derive Visualization availability from automatic or selected outputs.
6. Ship a self-contained native XDF recording workflow.
   - Keep the official pinned LabRecorder and its Qt/liblsl runtime inside every
     desktop package; users must not need a separate recorder download.
   - Keep **Open Lab Recorder** in the shared Output UI. Native activation must
     ensure raw LSL is enabled, then open the separate recorder where the user
     selects any discoverable streams. Pages must fail closed with the installed-
     app release link.
   - Preserve one fixed no-argument native launcher boundary, the packaged
     remote-control-disabled profile, upstream notices, and exact-package launch
     gates. Never move XDF recording onto the acquisition/display hot path.

## Next

- Extend the physically passing native timed ACC breathing gate to the direct
  browser hardware path and to a synchronized independent respiration reference.
  Exercise Lost/recovery and both presentation modes in the full UI without
  changing raw LSL/OSC/CSV behavior. Do not turn one H10 run into physiological
  acceptance.
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
- Run the checked-in offline Rust analyzer on synchronized native H10 ACC and
  GDX-RB Force (N) recordings. The production-processor replay, bounded
  host-time alignment, polarity/lag analysis, and identifier-free evidence
  schema are implemented; physical paired recordings and held-out acceptance
  evidence remain pending.
- Retain the passing 2026-08-24 single-device native GDX-RB gate, then qualify
  the revised all-channel acquisition plus raw/derived LSL outlets, browser
  streaming, mixed multi-device load, cross-platform/under-load runs, and a
  synchronized H10-ACC/GDX-RB reference recording before making broader
  compatibility, respiratory-agreement, or latency-percentile claims.

## Later

- Complete Android platform glue and permissions while retaining the Rust crate
  boundaries.
- Add new metrics only with a formula owner, evidence entry, tests, stream
  metadata, preview coverage, and explicit interpretation limits.

## History

- 2026-08-26: physically qualified the native timed ACC breathing estimator on
  Windows through the production H10 input owner: 6,120/6,120 samples in 170
  36-sample frames over 30.035968212 source seconds, 202.547103 Hz effective
  interpolated cadence, zero late/gap/order/cadence errors, calibration/readiness,
  all three phase values, and 0.903095 waveform span. No output transport was
  initialized and physiological acceptance remains false.
- 2026-08-26: implemented and host-qualified a timed ACC breathing
  candidate with source-time reconstruction, versioned legacy migration,
  fixed-span hysteretic state, bounded diagnostics/presentation points, and
  matching native/browser damage and invariance tests. Real-H10 validation is
  deliberately required before publication.
- 2026-08-24: added the pinned official LabRecorder as a self-contained native
  package resource with an **Open Lab Recorder** UI action. The separate process
  discovers user-selected LSL outlets and records XDF; the launcher accepts no
  frontend path or arguments, uses a remote-control-disabled profile, and adds
  exact-package startup gates for Windows, macOS, and Linux package classes.
- 2026-08-24: repaired native Go Direct interoperability by selecting the
  command characteristic's advertised write mode and accepting the bounded
  response-header family observed from the belt. A Windows/WinRT physical
  GDX-RB run passed channel-1 Force (N), 70 samples at 10.01 Hz with zero
  drop/malformed/nonfinite counts, disconnect, and a 20-sample reconnect at
  10.05 Hz. The desktop UI now offers an opt-in keep-connected policy that
  retains the live measurement session and retries unexpected drops without
  inventing an unsupported remote-wake command.
- 2026-08-20: added a bounded offline H10/GDX-RB respiration-reference
  analyzer that reconstructs native ACC batches, replays the production Rust
  breathing processor, aligns PMD sensor spacing with host-timed force, and
  reports separate signed/normalized waveform agreement without turning one
  descriptive recording into physiological acceptance.
- 2026-08-20: added native/browser GDX status metadata and an identifier-free
  native product-path verifier for exact Force (N), stream health, disconnect,
  and reconnect. The first physical scan found no advertised GDX-RB, so the
  hardware gate remains open.
- 2026-08-20: made ACC breath-phase sensitivity time-normalized using accepted
  sample count at 200 Hz, with a 50 ms compatibility reference and matching
  Rust/Chromium invariance tests. Reference-respiration validation remains open.
- 2026-08-18: added a bounded native two-H10 qualification pool with one shared
  exact discovery snapshot, two independent Windows session owners, one Rusty
  discovery registry, and four persistent ECG/ACC outlets. A same-epoch
  repaired-reference-first physical run passed four pinned official inlets,
  130/200 Hz bands, nonzero X/Y/Z, zero estimated loss/reorder, and exact
  cleanup. The installed UI remains deliberately single-sensor.
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
