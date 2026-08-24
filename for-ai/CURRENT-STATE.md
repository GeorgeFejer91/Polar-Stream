# Current state

Last verified: 2026-08-24

## Implemented

- Native desktop packages stage the official LabRecorder 1.17.0 from pinned,
  checksum-verified upstream release/source revisions together with its own
  Qt/liblsl runtime. The shared Output UI exposes **Open Lab Recorder**; native
  activation ensures automatic raw LSL and launches the separate process with
  a fixed remote-control-disabled profile, while Pages fails closed with the
  installed-app link. Users choose any discoverable LSL streams in LabRecorder
  and record them to XDF. The launcher accepts no renderer-supplied executable,
  path, arguments, or configuration and does not enter acquisition queues. The
  bundle also carries the Qt runtime notice and checksum-pinned LGPL/GPL texts.
- The existing `breathing_volume` compatibility stream now has a specialized
  preliminary 1D visualizer in the canonical Tauri/Pages UI. Its newest 0–1
  sample is a moving dot and its bounded recent history is a leftward trail;
  rising/falling labels follow configured inhale/exhale polarity without
  claiming lung volume. Deterministic desktop and responsive browser checks
  cover the shared display path.
- Native BLE scan, connection, PMD ECG/ACC streaming, and HR/RR ingestion.
- Windows uses a direct WinRT advertisement watcher for up to fifteen seconds,
  stopping early after the caller's exact H10 local-name target: one for the
  ordinary UI and two for the bounded two-session pool. This bound matches the
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
- The default reference-compatible Windows lifecycle owns the published
  reference's typed, cancellable 1.5-second settle after the validated ECG
  settings response and before ECG start. The closed diagnostic version of this
  timing edge completed one full physical source-to-official-inlet chain. The
  physical verifier clears any inherited diagnostic profile; the accepted
  same-epoch run therefore qualified only the ordinary product default.
- The temporary standalone WinRT probe used during diagnosis is not shipped.
  Physical qualification uses the bounded product verifier and exact default
  backend; identifier-free staged diagnostics remain opt-in.
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
- Native Windows BLE leaves connection parameters system-managed and reports
  the observed interval, peripheral latency, and negotiated MTU read-only. It
  does not mutate connection timing after sensor-frame qualification.
- A bounded native two-H10 qualification surface shares one discovery snapshot
  and admits exactly two distinct devices into non-identifying slots. Each slot
  owns an independent `InputManager`, Windows session owner, event receiver,
  and ECG/ACC outlet keys. One two-session output coordinator owns the single
  Rusty discovery registry and all four persistent outlets, so the fixed
  discovery port binds once. Session transitions serialize, a new scan rejects
  while any slot is active, and cleanup drains every slot. A same-epoch Windows
  run from exact source `2b7bdf0c8f0a567d8ad4a18dcbb24a78928f9197`
  passed repaired-reference discovery/acquisition, both Polar Stream sessions,
  four distinct pinned official inlets, 130/200 Hz bands, zero estimated
  loss/reorder, nonzero X/Y/Z, and exact cleanup. The ordinary desktop UI
  was the previous ordinary UI boundary.
- The ordinary application now admits up to eight mixed Polar H10 and Vernier
  Go Direct sources. Each source owns an independent input session, decoder,
  event receiver, metric engine, and output router; shared maps and mutexes are
  lifecycle-only. Stable source slots add unique stream-name suffixes and map
  to a fixed eight-color palette propagated through connected widgets, raw/output
  cards, and visualizers. UI circular buffers are per source, so selecting a
  device cannot mix samples from another device.
  Saved output configuration is transport-free: formula validation uses a
  destination-disabled router and only connected source routers may create
  LSL/OSC/CSV endpoints, preventing empty legacy unsuffixed streams.
  Polar and Go Direct advertisement scans run concurrently so one sensor family
  cannot add its complete scan window ahead of the other; connection remains a
  user selection. A saved-device-only scan is reserved for an unexpected active
  Vernier link drop; application startup waits for explicit Search.
  Once either family is active, the native scan action targets only the missing
  family and leaves the connected input session/output router live. With one
  Polar and one Vernier source active, source selection remains display-only;
  both routers continue publishing concurrently. A pinned official pylsl
  1.18.2/liblsl 1.17.7 synthetic gate resolves exact ECG, ACC, rawVernier, and
  vernierBreathing inlets, receives advancing rows from all four, and requires
  at least 1.5 seconds of overlapping LSL time.
- Independent MIT `vernier-gdx-core` and `vernier-gdx-input` crates implement
  Go Direct command framing, decrementing counters/checksums, 20-byte writes
  without a GATT response when the characteristic advertises that mode (with a
  supported write-with-response fallback),
  correlated control responses, bounded fragmented/coalesced frame assembly,
  periodic/single float and integer measurement decoding, multi-session BLE,
  first-valid-measurement readiness, bounded queues, and latency/drop health.
  Cancellation covers setup as well as streaming, terminal events cannot block
  teardown, physical disconnect is bounded, and finished owners are pruned
  before new scan/connect admission so failed links do not strand capacity.
  Device and every advertised-sensor metadata record are parsed before
  streaming; the native product profile requires a confirmed GDX-RB and
  channel-1 periodic Force in N, then enables the complete compatible numeric
  periodic/aperiodic channel set. It rejects malformed, unsupported, duplicate,
  or mutually exclusive schemas instead of silently omitting raw values. It
  requests a common 10 Hz base period when metadata permits and otherwise uses
  a common plausible reported fallback. The initialization status response also reports the
  main firmware major/minor and battery percentage in native and Chromium
  connection metadata. A bounded native verifier exercises sustained samples,
  continuity/rate/health thresholds, explicit disconnect, and reconnect while
  retaining no Bluetooth identifier. A 2026-08-24 Windows/WinRT run against a
  physical GDX-RB on firmware 5.3 passed exact channel-1 Force (N) identity,
  70 primary samples at 10.01 Hz with zero drop/malformed/nonfinite counts,
  bounded disconnect, and a 20-sample reconnect stream at 10.05 Hz. The repair
  honors the characteristic's write mode and accepts the observed Go Direct
  response-header family while retaining bounded length and command/counter
  checks. Once connected, the 10 Hz measurement session remains active until
  explicit disconnect; no undocumented keep-awake command is sent, and a
  powered-down non-advertising belt cannot be awakened over BLE. These path
  dependencies are compiled into the desktop binary; Vernier support requires
  no Python process, browser shim, proprietary SDK, or separately installed
  runtime module. A default-on **Keep connected / awake** control inside the
  connected Vernier widget can
  retain that active measurement subscription and retry unexpected link drops
  with bounded exponential backoff. Deliberate disconnects never trigger a
  retry, and only an advertising sensor can reconnect.
- Native Go Direct raw data is a first-class per-device aggregate LSL output.
  `<base>_rawVernier` sorts the metadata-defined measurement channels by sensor
  number, losslessly widens device Float32/Int32 values into Double64, represents
  channels absent from a sparse native update as `NaN`, and appends sequence,
  input-drop, device-drop, period, decode-latency, host-receipt, and encoding
  diagnostics. `<base>_vernierBreathing` is a separate derived Float32 relative
  belt-force waveform, causally normalized over a bounded 30-second history and
  clamped to 0–1 with inhale increasing. Both are irregular-rate, explicitly
  timestamped liblsl outlets created before the first measurement is published.
  The existing channel-1 Force (N) LSL/OSC/CSV path remains compatible and raw
  aggregate publication still precedes derived/UI work. A pinned official pylsl
  1.18.2/liblsl 1.17.7 synthetic consumer gate verifies descriptors, metadata,
  sparse/exact values, formats, bounds, and advancing timestamps.
- Immediate raw LSL/OSC publication with canonical names.
- Raw ECG and ACC LSL chunks preserve H10 sensor-time spacing through separate
  first-frame offsets into the local LSL clock. This prevents setup-buffered
  notification bursts from overlapping while never publishing raw device-clock
  values directly. The liblsl and Rusty LSL backends share this contract.
- Each ACC-derived breathing snapshot uses the newest PMD sensor timestamp from
  its accepted notification. Native LSL maps it through the same ACC clock
  offset, native OSC/CSV retain the nanosecond value, and Chromium metric events
  and browser CSV carry it directly. HR-only notifications remain host-timed
  because the standard HR characteristic has no PMD device timestamp.
- An identifier-free offline Rust qualification tool accepts isolated schema-2
  native H10 ACC and GDX-RB Force (N) CSVs, reconstructs H10 notification
  batches, and replays them through the current `BreathingProcessor`. It maps
  PMD sensor spacing into host time with a fifth-percentile receipt-offset
  anchor, resamples both streams at 10 Hz, applies matched causal baseline
  removal, and reports bounded lag, mounting polarity, waveform/rate error,
  window stability, readiness/confidence, robust spans, and timing quality.
  Recording-quality status remains separate from physiological acceptance,
  which is always false until repeated held-out physical evidence exists.
- Breathing readiness measures successive vectors on an independent all-axis
  EMA path before applying its motion-quality threshold. This avoids treating
  ordinary 200 Hz H10 sample noise as broadband body motion while retaining a
  deterministic high-amplitude motion rejection gate; Rust and Chromium share
  the implementation and noisy-clean/motion fixtures.
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
- Output and Visualization remain empty until at least one source connects.
  The first source activates its device profile and default outputs; active
  outputs then gate that profile's visualization choices. Polar activates raw
  ECG/ACC and its ECG/ACC output library. Vernier exposes only raw force plus
  the 0–1 breathing display and automatically presents the two native protocol
  outputs (`rawVernier` Double64 and `vernierBreathing` Float32); rawForce is an
  opt-in compatibility module. With mixed sources, source selection swaps the
  complete preset, and deterministic/browser checks reject cross-family cards,
  visualizations, and formulas.
- Input discovery now renders supported candidates as non-widget rows with an
  explicit Connect action. Only a successful connection creates a persistent
  source widget. Each widget owns telemetry, source selection, disconnect, and
  a color picker; the chosen live-session color outlines that source's widget,
  raw/processed output cards, and visualization surfaces. The native Vernier
  widget alone exposes Keep connected / awake, default-on for a new preference
  state while preserving a previously saved off choice.
- Polar raw ECG/ACC outputs are always activated and cannot be removed; Vernier
  rawVernier plus vernierBreathing remain automatic protocol outputs. A native
  physical connection enables LSL and reapplies source-filtered output config
  automatically. Processed catalog metrics and formulas remain add/remove
  outputs, and only automatic or selected outputs appear in Visualization.
- Native startup no longer performs saved-device Bluetooth discovery. Search is
  user-triggered, avoiding a startup scan that obscures the device-list model;
  default-on Vernier reconnect remains limited to an unexpected drop from an
  already connected session.
- The shared destination controls use a compact adaptive grid: four 44px
  LSL/OSC/CSV/audio toggle targets form a 2-by-2 block when the Output panel is
  wide enough and collapse to one column without horizontal overflow when it
  is narrow. Browser-only capability and recorder messages remain full width.
- Deterministic metric previews derived from the canonical anonymized 60-second
  real H10 ECG/ACC recording, with its fingerprint checked in the asset. Only the
  selected metric renders an SVG loop; list rows remain static. Formula Lab
  traces also run as closed continuous loops, and categorical outputs remain
  stepped.
- A one-at-a-time output picker exposes all 50 catalog metrics across ECG and
  ACC families. Each selected metric shows exactly one SVG output loop, a
  two- or three-sentence scientific explainer, and two or three reviewed sources.
  Formula definitions and output settings remain available through Formula Lab
  and post-add **Adjust** controls instead of crowding this preview.
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
- Shared post-add ACC breathing controls cover the signed projection, normalized
  0–1 waveform, phase, calibration/range, readiness, and confidence outputs:
  two or three axes (X + Z recommended), smoothing, phase sensitivity and
  direction; the signed projection can also use the output normalization layer.
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
  Staging and live verification canonicalize text newlines to LF while
  retaining binary bytes exactly, so Windows `core.autocrlf` checkouts and the
  Linux deployment runner produce and verify the same manifest hashes.
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
- The same Pages runtime now offers an independent Vernier Go Direct Web
  Bluetooth chooser. It uses the official GDX service/characteristics, mirrors
  native command/counter response correlation and frame assembly, gates
  connection on the first channel-1 force batch, supports up to eight
  user-selected Go Direct sessions, and routes each into its own colored source
  buffers. Deterministic Playwright coverage validates setup, a fragmented
  force packet, source identity/color, and cleanup; no physical browser GDX run
  has yet been completed.
- A physical Motorola phone running Google Chrome selected and connected to an
  H10 from the public GitHub Pages site on 2026-08-14. This confirms the hosted
  chooser/GATT path, but not yet the timed CSV rates, loss, gaps, or reconnect
  acceptance criteria.
- The browser adapter includes a JavaScript mirror of the complete ACC
  breathing module, including causal baseline removal, normalized waveform,
  phase, readiness, and confidence. Playwright validates the browser
  chooser/GATT contract, commands, protocol edge cases, breathing calibration,
  quality gating, and responsive UI with an emulated device.
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
- ACC breathing phase now thresholds normalized waveform velocity per second,
  using notification sample count at the requested 200 Hz. A 50 ms reference
  preserves the prior sensitivity scale for ten-sample batches while Rust and
  Chromium invariance tests cover equivalent 50/150 ms slopes.

## Active branch context

`main` is the merged baseline. Confirm live Git and PR state with
`for-ai/scripts/repo-snapshot.sh`; do not infer it from this snapshot.

## Known constraints

- Real BLE behavior and latency still depend on platform adapters, radio state,
  ATT MTU, and operating-system scheduling.
- Native Go Direct intentionally supports the complete compatible numeric schema
  of a metadata-verified GDX-RB, requires channel-1 periodic Force (N), and
  rejects generic sensors. Chromium remains channel-1 force-only and cannot
  publish native LSL. One physical native Windows force-only-mask
  stream/disconnect/reconnect qualification passed on 2026-08-24; the revised
  all-channel mask/two-outlet path, Chromium, mixed-source, under-load,
  cross-platform, long-run loss, and latency-percentile qualifications remain
  physically open.
- Chromium can connect to Go Direct only in a secure context and only after a
  user-triggered device chooser for each device. A page cannot passively scan
  or silently enumerate all nearby sensors, and hidden-tab/screen-lock
  continuity is not guaranteed.
- The default liblsl path supports per-source outlets. The optional default-off
  Rusty LSL backend compiles and passes its existing single/two-H10 tests, but
  its fixed-port registry must be generalized before claiming runtime support
  for the new ordinary app's arbitrary mixed-source routers.
- The dynamic Double64 aggregate Vernier outlet is packaged-liblsl-only. The
  optional Rusty backend rejects an active Vernier schema explicitly instead of
  silently omitting the raw or derived special outlet.
- Windows CI validates the WinRT integration and the non-Windows `btleplug` path at compile and unit-test
  level only. Retain the system-managed link values and sample counts from a
  physical Windows run before making timing claims.
- liblsl availability and packaging differ by platform; follow `RELEASING.md`.
- The bundled LabRecorder preparation and exact-package smoke gates cover all
  release classes. On 2026-08-24 the Windows x64 NSIS install passed the real
  WebView button -> native IPC -> installed LabRecorder process chain, and the
  administratively extracted MSI launched both packaged processes. Linux,
  macOS universal, and native ARM64 package evidence is produced by the next
  tagged release matrix rather than inferred locally.
- Both public ACC-derived respiration outputs are unvalidated and require
  comparison with a reference respiratory sensor before interpretation.
- Windows and macOS public packages are currently unsigned/ad-hoc unless release
  infrastructure states otherwise.
- Physical-device latency percentiles, queue high-water marks, and transport
  drop/error counters are not yet captured as a single end-to-end benchmark.
- The optional Rusty LSL backend has bounded production-default Windows H10
  acceptance. One reference-first run with no diagnostic session override
  advanced ECG and all three ACC axes, published two distinct outlets, and let
  pinned pylsl 1.18.2/liblsl 1.17.7 consume 365 ECG samples at 130.14 Hz and
  432 ACC samples at 202.22 Hz with zero estimated loss/reorder, advancing LSL
  timestamps, no cross-stream match, and exact cleanup. This is not a
  long-duration latency benchmark or release authorization; the backend remains
  optional/default-off and no package enables it.
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
