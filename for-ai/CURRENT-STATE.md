# Current state

Last verified: 2026-08-17

## Implemented

- Native BLE scan, connection, PMD ECG/ACC streaming, and HR/RR ingestion.
- Windows uses a direct WinRT advertisement watcher for up to fifteen seconds,
  stopping early after exact H10 local-name evidence. This bound matches the
  sparse advertisement cadence observed from a reference-positive physical
  H10 while retaining a fast path when an exact packet arrives immediately.
  Exact H10 local-name evidence is
  independent of service-UUID readability; unnamed PMD/heart-rate candidates
  require a bounded direct WinRT name confirmation, while weak/generic
  advertisements remain rejected. It then owns one persistent WinRT GATT
  session, requests PMD service access, and discovers
  uncached with bounded retry/cached fallback, enables each notification with
  one CCCD write immediately followed by one directly retained handler, and
  does not report success until both ECG and three-axis ACC have decoded. Each
  native async call has an identifier-free typed stage, its own
  deadline and cancel/close ownership inside a 45-second setup budget. Partial
  subscription rollback, internal queues, callback removal, and session cleanup
  are bounded and exactly-once. Startup requires exact successful PMD settings
  and start responses, qualifies ECG first, and starts ACC only after the first
  decoded ECG frame.
- The selected Windows device lifecycle is confined to one named OS thread with
  an explicitly initialized MTA. The default product profile drives bounded
  operations through a current-thread Tokio executor. The final closed
  verifier-only differential instead owns the complete GATT lifecycle
  synchronously on the plain MTA thread, matching the physically passing
  minimal probe's execution model; only decoded events cross to the application
  channel. A completion guard keeps disconnect signalling bounded if either
  owner unwinds.
- The current local verifier-only differential uses the physically passing
  minimal probe's `windows-future` `.when` completion callback for PMD
  CCCD/control operations, then crosses a bounded Tokio oneshot into the
  existing per-stage deadline owner. The preceding HR-free and unclosed-success
  physical runs both received zero PMD-data callbacks, so optional heart-rate
  setup and success-time explicit operation close are no longer candidate
  causes. Failure/timeout/cancellation cleanup and the default product lifecycle
  remain unchanged; physical acceptance remains open.
- An identifier-free input-only differential now drives the same product
  `InputManager`/WinRT session without constructing any output transport. It
  requires a connected session, two advancing ECG frames, two advancing ACC
  frames, and at least one nonzero sample on every ACC axis, then performs the
  normal bounded disconnect. This determines whether pre-connection Rusty
  outlet initialization influences the physical failure before another backend
  change is considered.
- That input-only differential reproduced the zero-PMD-data-callback failure,
  eliminating every output transport as a cause. The current verifier-only
  profile first applied the passing probe's `.when`/no-success-close operation
  policy across the entire selected-device setup chain as well as PMD
  CCCD/control operations. A reference-positive run still reached both ECG
  control responses with zero PMD-data callbacks, eliminating that projection.
  The current closed verifier-only profile additionally matches the probe's
  PMD service/characteristic/subscription sequence: one service access request,
  direct uncached exact characteristic lookups, explicit
  control-Indicate/data-Notify modes, no inter-subscription delay, and no
  pre-frame link-property reads. A reference-positive run still received both
  control responses with zero PMD-data callbacks, eliminating that setup shape.
  Keeping that setup while moving callback handoff to a bounded
  standard-library channel still produced zero data callbacks, eliminating the
  callback-to-queue transport. Moving the entire lifecycle to the synchronous
  MTA owner also timed out. A same-lease run then proved the full published
  doctor received ECG, ACC, and HR while the full Polar Stream profile did not;
  the minimal probe also failed in adjacent epochs. The current closed profile
  therefore restores only the full doctor's typed, cancellable 1.5-second
  post-settings dwell. Default product behavior, scanner confirmation,
  error/timeout cleanup, and battery-after-qualification remain unchanged.
- On Windows 11+, native BLE makes a best-effort throughput-optimized connection
  parameter request and reports the request status, observed interval,
  peripheral latency, and read-only negotiated MTU. Older Windows versions
  continue with system-managed timing.
- Immediate raw LSL/OSC publication with canonical names.
- A default-off, mutually exclusive Rusty LSL source backend is pinned to merge
  `8b6b2a6cd0c0e5147b7e1cc076a116ef226cddbd`. Pinned pylsl
  1.18.2/liblsl 1.17.7 broadly discovered and exactly matched independent
  1-channel/130 Hz ECG and 3-channel/200 Hz ACC outlets concurrently in the
  synthetic host gate. Exact full-info requests are routed independently from
  each outlet's data-consumer slot. Each outlet still admits one official data
  consumer; a second concurrent data consumer rejects without disturbing the
  admitted one. The synchronous poll loop runs on Tokio's blocking pool with
  explicit stop and bounded join. Default/package behavior remains liblsl.
- The physical Rusty LSL qualifier starts its two pinned official inlet workers
  as soon as the independent outlets report initialization, before BLE
  selection or source-frame qualification. Source readiness is a later gate;
  reaching it before official inlet startup fails closed. This receiver-first
  order now matches the passing synthetic official-consumer gate.
- A native **Save local CSV** destination records every received raw ECG/ACC
  sample, HR/RR, and every produced selected metric under `Downloads/Polar
  Stream` (app-data fallback). Its 128-notification writer queue is non-blocking
  and fail-stop so disk I/O cannot delay LSL/OSC or acquisition.
- Demand-driven ECG, HRV, coherence, breathing, breathing-dynamics, quality,
  and explicitly experimental metric modules.
- Three-panel Tauri UI with remembered native preferences.
- Deterministic metric previews derived from the canonical anonymized 60-second
  real H10 ECG/ACC recording, with its fingerprint checked in the asset. Compact,
  detail, and Formula Lab traces run as closed continuous loops; categorical
  outputs remain stepped.
- A one-at-a-time output picker exposes all 47 catalog metrics across ECG and
  ACC families. Every metric has recorded preview coverage, a concise scientific
  explainer, citation, and mathematical definition.
- Pre-save preview controls update the recorded outcome for display window,
  normalization, and the full experimental breathing configuration.
- Formula Lab maps source variables, keeps time as the automatic x-axis, offers
  metric templates plus explained calculator keys, validates formulas in Rust,
  and compares recorded input/output before saving.
- Enabled custom formulas run in the bounded `polar-h10-math` engine and publish
  independent LSL/OSC/CSV outputs with per-formula warm-up/fault health.
- Cross-platform release workflows and branded application icons.
- Low-latency renderer scheduling capped at 30 Hz and paused while hidden.
- One raw accelerometer visualizer with stacked, independently scaled X/Y/Z
  lanes on the active development branch.
- Red ECG / blue ACC output-library modes. ACC mode includes raw motion and the
  complete experimental breathing/breathing-dynamics catalog; every choice is
  still added individually rather than through a checkbox list.
- Shared pre-save ACC breathing controls for the two public breathing outputs:
  two or three axes (X + Z recommended), smoothing, phase sensitivity and
  direction; the magnitude output can optionally normalize to 0–1.
- A phase-only breathing circle with asymptotic size limits and pause inertia.
- Public dedicated GitHub repository at `GeorgeFejer91/Polar-Stream`.
- The legacy Mesmerism-derived `PolarH10` checkout is locally quarantined under
  `_quarantine/PolarH10-mesmerism-fork` with its Git history and untracked
  research PDF preserved; future Polar Stream work is routed here instead.
- The production-hardening and ECG/ACC output-library work is merged on `main`
  in PR #5.
- One canonical interface tree now targets Tauri and GitHub Pages. The staged
  Pages artifact records hashes of every shared asset, and browser validation
  covers desktop, 390px touch, and 320px touch layouts.
- On smartphones, the shared output library uses a full-viewport two-step flow:
  touch-sized family/search/filter/signal controls first, then a focused signal
  detail view with back navigation and a safe-area-aware save action.
- A selectable recorded-preview input replays the canonical anonymized ECG/ACC
  fixture and its derived preview values through the shared runtime event
  contract in both Pages and the offline Tauri app. Its bounded seam correction
  prevents an end-to-start jump without changing the source fixture or any live
  H10/native path. No generated-signal fallback remains in the shipped UI.
- GitHub Pages offers an experimental Web Bluetooth input in supported secure
  Chromium contexts. It requests a Polar H10, writes the canonical PMD ECG/ACC
  start commands, and decodes ECG, uncompressed/variable-delta ACC, HR/RR, and
  battery notifications into the shared UI event contract.
- A physical Motorola phone running Google Chrome selected and connected to an
  H10 from the public GitHub Pages site on 2026-08-14. This confirms the hosted
  chooser/GATT path, but not yet the timed CSV rates, loss, gaps, or reconnect
  acceptance criteria.
- The browser adapter includes a JavaScript mirror of the two retained ACC
  breathing outputs. Playwright validates the browser chooser/GATT contract,
  commands, protocol edge cases, breathing calibration, and responsive UI with
  an emulated device.
- The Pages runtime is self-contained: browser H10/mock acquisition, supported
  metrics, and visualization run without a localhost companion, installed
  helper, or remote relay. The shared LSL/OSC toggles remain visible in browser
  mode, but rejected attempts stay off and show an installed-app error with a
  latest-release download link. Native publication remains available only in
  the separately installed desktop app.
- Pages has a browser-native live/recording destination. Every un-decimated
  input batch is available through a same-tab event and same-origin
  `BroadcastChannel`; every received raw row and produced metric event can be
  captured to a timestamped CSV through the shared output toggle.
  The recorder stops visibly at 300,000 rows or input disconnect, never grows
  without a bound, and is automated against recorded preview input and CSV download.
- Both runtimes expose an experimental 22.05 kbit/s stereo Manchester PCM data
  modem toggle for clean cable/digital-loopback recording. Frames use compact
  ECG/ACC/metric sections, sequence numbers, and CRC32. Browser validation sends
  the production waveform through `scripts/decode_audio_data.py` and checks the
  recovered CSV rows.
- Browser H10 sessions request a best-effort screen wake lock while the page is
  visible, and emulated Android-touch coverage exercises the permission/GATT
  and wake-lock contract.
- The public Pages hardware run now has a strict evidence path: a live verifier
  records GitHub response headers and validates every manifest SHA-256 against
  the served asset and canonical checkout, while an offline CSV analyzer checks
  Web Bluetooth provenance, positive monotonic H10 time, raw streams, rates,
  loss, gaps, values, HR/RR, and recorder stop state. These tools prepare and
  evaluate a physical run; they do not replace one.
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
- Windows CI validates the WinRT integration and the non-Windows `btleplug` path at compile and unit-test
  level only. It does not prove that a particular adapter/H10 accepted the
  preferred connection parameters; retain the reported link values and sample
  counts from a physical Windows run before making that claim.
- liblsl availability and packaging differ by platform; follow `RELEASING.md`.
- Both public ACC-derived respiration outputs are unvalidated and require
  comparison with a reference respiratory sensor before interpretation.
- Windows and macOS public packages are currently unsigned/ad-hoc unless release
  infrastructure states otherwise.
- Physical-device latency percentiles, queue high-water marks, and transport
  drop/error counters are not yet captured as a single end-to-end benchmark.
- The optional Rusty LSL backend is host-qualified but not yet physically
  accepted. The latest attended run selected an exact H10 and physically passed
  the complete direct-WinRT session boundary: service/characteristic discovery,
  notification subscriptions, PMD responses, and advancing first ECG and ACC
  frames. Both independent Rusty outlets initialized and pinned official
  pylsl/liblsl consumers resolved and opened them, but the physical verifier
  did not complete its source/consumer thresholds before its outer deadline.
  Review found a verifier-only ordering mismatch: unlike the passing synthetic
  gate, it did not start official consumers until after BLE source readiness.
  The prepared correction starts them at outlet initialization and leaves BLE,
  output, product, and default-package behavior unchanged. Its first attended
  run proved both consumers were admitted before source readiness, but neither
  source nor official thresholds completed after the setup-to-steady-state
  handoff. Identifier-free five-second link and callback/queue checkpoints now
  distinguish native notification loss from an internal forwarding stall.
  Retain the
  publication and release hold until a fresh same-epoch run proves bounded ECG
  and ACC counts/rates/order plus exact cleanup through that corrected chain.
- Rusty LSL does not claim `resolve_byprop` predicate-filter conformance.
  Consumers must enumerate broadly and exactly match the six documented
  descriptor fields client-side. Rusty LSL's AGPL-3.0-or-later license also
  requires an explicit distribution/compliance decision before any enabled
  package.
- The one-official-consumer-per-outlet bound is a Polar Stream deployment
  constraint. Generic Rusty LSL auxiliary-connection and multi-consumer
  conformance remains separately scoped if the product later requires it.
- Selected derived processors execute after raw publication in the coordinator
  loop. The input channel is bounded, but physical-device measurements have not
  yet proven every opt-in metric set stays inside the next-notification budget.
- The recorded preview demonstrates interface behavior only. It does not validate H10
  fidelity, timing, ACC respiration, or output transports, and it never opens
  native BLE, LSL, or OSC connections.
- The Web Bluetooth adapter has a physical public-Pages connection smoke test on
  Android Chrome, but browser/OS support remains non-portable. BLE throughput,
  reconnect, compressed-frame behavior, and timing still need a complete saved
  hardware run. Follow `docs/browser-hardware-acceptance.md`; only the public
  Pages site in an ordinary browser tab can satisfy that acceptance run.
- Web Bluetooth is unavailable to workers/service workers, and mobile Chrome
  may freeze or discard a hidden page. The screen wake lock is released when
  the document becomes inactive. Pages therefore cannot guarantee Bluetooth,
  CSV, or audio continuity while another app/tab is foreground or the screen is
  locked; a native Android foreground service is still required for that claim.
- Ordinary browser tabs do not publish LSL or OSC because they cannot open the
  raw UDP discovery/multicast and TCP/UDP sockets those protocols require. The
  browser workflow deliberately does not emulate them through another transport.
- Browser CSV and the same-origin live channel are not discoverable by native
  LabRecorder and do not provide cross-device clock synchronization. Use the
  installed app when native LSL is a requirement.
- The audio modem has CRC-based error detection but no forward error correction,
  encryption, or physical AUX/device validation. It requires clean stereo PCM;
  mono phone microphone inputs, lossy codecs, speaker/microphone paths, and
  native WebView event loss remain unsupported/unvalidated boundaries.
- Breathing phase sensitivity is currently applied to normalized change per ACC
  notification batch, not change per second. BLE batch cadence can therefore
  affect classification and must be corrected/versioned before cross-platform
  phase equivalence is claimed.
