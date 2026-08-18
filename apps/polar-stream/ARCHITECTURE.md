# Architecture

## Decision

Use a **Rust core with a Tauri 2 system-WebView shell**.

Rust and C++ can both meet the sensor rates involved here. Rust is the better
repository choice because it combines native performance with memory safety,
has a current cross-platform BLE library, and lets the same domain crates build
for desktop and Android. Tauri supplies the requested HTML interface without
shipping a second browser engine.

Primary evidence:

- [Tauri supports HTML/CSS/JS frontends and Rust/Swift/Kotlin backend logic](https://v2.tauri.app/start/).
- [Tauri uses the platform WebView rather than bundling a browser](https://v2.tauri.app/reference/webview-versions/).
- [`btleplug` 0.12 supplies the non-Windows scanning and GATT adapters](https://docs.rs/crate/btleplug/latest); Windows uses the direct WinRT path described below.
- [liblsl supports Windows, Linux, macOS, Android, and iOS](https://labstreaminglayer.readthedocs.io/info/intro.html).
- [Tauri channels are intended for ordered, high-throughput native-to-frontend data](https://v2.tauri.app/develop/calling-rust/).

Qt/C++ remains a credible native-UI alternative, especially for an application
that must avoid a WebView entirely. It is not the preferred fit here because the
requested frontend is explicitly HTML-driven and C++ would add manual lifetime
and FFI complexity without reducing the dominant BLE/radio latency.

## Module boundaries

```text
Polar H10
   │ BLE notifications
   ▼
polar-h10-input
   │ typed InputEvent values
   ▼
apps/polar-stream (thin coordinator)
   ├────────► polar-h10-metrics
   │                    │ scalar MetricSample values
   │                    ▼
   ├────────► polar-h10-output ─────► LSL outlets
   │                    └───────────► OSC/UDP
   │                    └───────────► bounded CSV writer thread
   │                    ▲
   ├────────► polar-h10-math ───────┘ bounded custom scalar formulas
   │
   └────────► bounded display queue ─► Tauri channel ─► typed JS ring buffers
                                                        │ data-triggered frame
                                      ▼
                                   Canvas 2D
```

Rules enforced by the crate graph:

- Input does not know whether the consumer is a UI, recorder, LSL, or OSC.
- Output does not know where samples came from.
- Protocol decoding can be unit-tested without an adapter or runtime.
- Every derived processor and its formula tests live in `polar-h10-metrics`.
- Only processors required by selected outputs are active; a raw-only setup does
  not maintain ECG, HRV, coherence, breathing, or excitation windows.
- In the native acquisition path, the HTML layer never republishes scientific
  data. The browser-input path remains entirely inside its tab and never enters
  or delays native H10 acquisition or native output publication.
- Adding a built-in metric means registering one `MetricDefinition` and feeding
  a `MetricSample`; it does not change BLE acquisition or output transports.
- User formulas run only in `polar-h10-math`. The parser has no statements,
  assignment, loops, strings, or user-defined functions, and compilation,
  operation, depth, state, and formula-count budgets are enforced before a
  formula can join the native output path.
- Native preferences have one typed, schema-versioned Rust owner in
  `apps/polar-stream/src/preferences.rs`; `ui/preferences.js` is used only by
  the browser renderer. Bluetooth and output crates remain storage-agnostic.
- All native calls are isolated behind `ui/runtime-api.js`. Command failures
  cross IPC as stable code/message/retryable objects rather than Rust strings.

## Shared desktop and browser interface

`apps/polar-stream/ui/` is the only interface source. Tauri packages that
directory directly, while `scripts/build_browser_demo.py` copies every regular
file in that canonical tree into the GitHub Pages artifact and records SHA-256
hashes. It does not transform or fork the application UI.
`scripts/validate_browser_demo.mjs`
checks the staged hashes against the canonical files before exercising the site
at desktop, 390px touch, and 320px touch viewports.

`ui/runtime-api.js` selects the runtime adapter once. Real H10 devices use the
typed Tauri IPC/channel path. A selectable `recorded-h10-preview` module is
available in both Tauri and Pages and loops the anonymized 60-second real H10
fixture in `ui/data/preview-recording.json` through the same connection, ECG,
accelerometer, HR/RR, and derived-output event shapes. Recorded preview data
uses a bounded 1.2-second endpoint correction to form a continuous circular
presentation, never crosses into the Rust acquisition path, and is visibly
labeled recorded. Live H10 input and native raw publication are never seam-conditioned.

On GitHub Pages only, `ui/polar-web-bluetooth.js` adds a second real-input
adapter. A user gesture opens the browser chooser filtered to `Polar H10`, then
the adapter serially discovers GATT services, subscribes to PMD data/control and
standard HR notifications, and writes the same ECG/ACC start commands as
`polar-h10-core`. Its decoder covers signed 24-bit ECG, uncompressed ACC, and
Polar's variable-bit-width delta-compressed ACC blocks. It emits the same UI
event shapes and runs a JavaScript port of the two experimental ACC breathing
outputs. Deterministic Playwright GATT emulation verifies chooser options,
commands, decoders, HR/RR, battery, breathing calibration, and responsive UI.
Physical-H10 browser validation remains an explicit release boundary.

`ui/browser-session.js` owns the Pages-only live/recording destination. The
runtime publishes each un-decimated browser input event to it before the chart
callback. It then exposes the typed batch on the same-tab `polar-stream-data`
event and same-origin `polar-stream-live-v1` `BroadcastChannel`. Recording is
demand-driven, includes every received raw ECG/ACC row plus every metric event
the browser produces, preserves raw ECG microvolts and ACC milligravity, and
reconstructs timestamps backwards from a PMD frame's final
sensor timestamp. CSV rows and their in-memory chunks have a hard 300,000-row
limit; the recorder stops and reports `FULL` at the boundary rather than
discarding data or growing indefinitely. A disconnect also stops an active
recording. This browser export is an experimental Pages acquisition artifact,
not the authoritative native output path.

Web Bluetooth requires a secure context, an explicit browser permission
chooser, and a compatible browser/OS. The adapter is visibly disabled with a
reason when any prerequisite is absent. It is experimental and not the
authoritative native acquisition path. Pages hides LSL and OSC because browser
tabs cannot open their native sockets. Browser acquisition, processing, and
visualization are self-contained and do not call a localhost companion, native
wrapper, or remote relay. Native Tauri acquisition remains isolated in Rust and
unchanged by this browser adapter.

The browser adapter makes a best-effort screen wake-lock request while an H10 is
connected and reacquires it when a visible document resumes. This improves an
Android Chrome foreground session but is not background execution. Web
Bluetooth is unavailable in workers/service workers, and the browser may freeze
or discard a hidden mobile page. The UI warns after a hidden browser-BLE session;
timestamps remain the evidence for detecting gaps. Guaranteed phone capture
with another app foregrounded or the screen locked requires native Android
platform work and a foreground service.

`ui/audio-data-link.js` is a shared, experimental Web Audio destination. It
encodes 125 ms batches as bounded 22.05 kbit/s stereo Manchester PCM frames with
a preamble, sequence number, compact raw values, and CRC32. The checked-in
`scripts/decode_audio_data.py` reference decoder reconstructs CSV from a clean
stereo PCM WAV, and browser validation exercises the production encoder through
that decoder. In Tauri this still consumes the bounded WebView display channel,
so it can miss display events and is not an authoritative native output. The
native CSV writer, LSL, and OSC remain the research-data paths.

An ordinary GitHub Pages tab still cannot implement native LSL discovery and
transport: it has no general UDP multicast or raw TCP/UDP socket API. The live
channel above is intentionally browser-scoped and must never be presented as an
LSL outlet. A native LabRecorder requirement continues to use the installed
Tauri application's liblsl outlets.

## Native LSL backend boundary

The installed application selects exactly one compile-time LSL backend.
`liblsl-backend` remains the default and the only packaged path.
`rusty-lsl-backend` is an experimental, default-off source feature pinned to
one exact upstream merge. Both implementations stay inside
`polar-h10-output`, reuse the canonical metric catalog and names, and remain
independent of the WebView.

The Rusty implementation owns a shared bounded discovery registry and
independent persistent outlets. The native coordinator advances its
caller-owned discovery/timedata/consumer work on Tokio's blocking pool only
while a sensor session exists. An explicit stop signal and bounded join keep
that synchronous loop from occupying an async runtime worker during operation
or shutdown. Each outlet admits one official consumer; a second concurrent
connection rejects without disturbing the admitted consumer. This is a Polar
Stream deployment bound, not general Rusty LSL multi-consumer conformance.
Official qualification enumerates broadly and matches the complete descriptor
client-side; server-side property predicate conformance is not claimed. See
`docs/rusty-lsl-backend.md` for the exact revision, shapes, limits, evidence,
and licensing/release holds.

The physical qualifier starts official inlet resolution when the independent
outlets initialize, before BLE selection or source-frame readiness. It then
awaits source and consumer thresholds as separate gates. This receiver-first
order is verifier orchestration, not an application buffering or publication
policy.
The two-H10 qualifier uses one bounded `InputSessionPool`: a single shared scan
snapshot admits exactly two distinct devices into stable, non-identifying
slots, while each slot owns a separate `InputManager`, Windows session owner,
event receiver, output router, and ECG/ACC outlet pair. Connection and cleanup
transitions are serialized, scanning is rejected while a session is active,
and `disconnect_all` drains every admitted slot before returning. This is a
native qualification surface; it does not change the single-sensor desktop UI
or give the browser runtime native BLE or LSL authority.
Opt-in session diagnostics continue across the first-frame-to-steady-state
handoff, reporting only aggregate link and callback/queue counters every five
seconds so native notification loss and internal forwarding stalls remain
distinguishable without exposing a device identity.

## Windows BLE link policy

Windows uses an active `BluetoothLEAdvertisementWatcher` for a bounded scan and
exact device selection, then opens the selected Bluetooth address directly with
WinRT. `polar-h10-input` owns one persistent `GattSession`,
uncached PMD service discovery, `RequestAccessAsync`, bounded uncached/cached
characteristic discovery, direct WinRT notification handlers, PMD start/stop,
and handle closure. Other operating systems retain the cross-platform
`btleplug` scan and connection path.

The selected device's complete WinRT lifecycle runs on one named OS thread with
an explicitly initialized multithreaded Windows Runtime apartment and one
current-thread Tokio executor. Setup, callbacks, steady-state reads, stop
commands, handler removal, handle closure, and apartment uninitialization cannot
migrate between runtime workers. A completion guard signals disconnect even if
that owner unwinds; the caller remains asynchronous while awaiting setup and
bounded shutdown.

Setup does not emit `Connected` until it has decoded both an ECG frame and a
three-axis ACC frame. After persistent-session creation, a cancellable 500 ms
settle matches the proven Windows reference before heart-rate-first discovery.
Battery discovery is deferred until both required sensor streams qualify.
Every native async setup operation has its own deadline,
cancel/close ownership, typed result class, and a shared 45-second setup budget
that expires before the physical verifier's outer readiness deadline.
Cancellation is checked during each operation, the raw-notification and
first-frame queues have fixed capacities, and overflow stops acquisition rather
than hiding loss. Shutdown gives every best-effort GATT cleanup operation a
500 ms bound before synchronous handler removal and WinRT handle closure;
callbacks and session closure are claimed exactly once, and
`GattSession.MaintainConnection` is always cleared. The scanner coalesces
advertisements by address for up to fifteen seconds, stops early after exact
H10 local-name evidence, and admits at most 256 strong candidates. The ceiling
matches a physical reference run that observed only four exact H10 packets in
fifteen seconds; an exact `Polar H10` local-name packet is
sufficient even when that packet's service-UUID collection is unavailable. A
PMD/heart-rate service packet without an exact name remains provisional and is
returned only after a bounded, eight-way, six-second WinRT
`BluetoothLEDevice.Name` confirmation resolves the exact H10 model.
Missing-name repeats may update an already admitted address, while generic BLE
presence, manufacturer data, and a non-H10 Polar name never admit a device by
themselves. The callback is removed before property confirmation or return.
Cleanup calls `Stop` only while the watcher is actively started.

Setting `POLAR_STREAM_H10_SCAN_DIAGNOSTICS` emits opt-in aggregate predicate
counters for advertisement shape, exact-name/service routes, duplicates,
property confirmation, rejection, and overflow. Those diagnostics contain no
address, name, payload bytes, manufacturer value, or stable device identity.
`POLAR_STREAM_H10_SESSION_DIAGNOSTICS` separately emits ordered setup stage
entry/exit records with attempt, duration, and result class. At subscription
 and qualification checkpoints it also emits aggregate, identifier-free
 characteristic properties, selected CCCD mode and successful commit count,
 callback entry/decode/enqueue counts,
handler attachment/removal counts, callback faults, and bounded-queue outcomes.
Identifier-free link checkpoints additionally record connection state,
`GattSession.MaxPduSize`, connection-interval units, and peripheral latency
after discovery, before PMD setup, after the ECG start response, and on setup
failure.
It covers device acquisition, persistent session creation, PMD
service/characteristics, CCCD subscriptions, ECG settings/start responses, ACC
start responses, and first ECG/ACC frames without emitting the selected
name/address, payload bytes, payload size, or stable device identity.

[WinRT negotiates MTU automatically and exposes `GattSession.MaxPduSize` as a
read-only observation](https://learn.microsoft.com/en-us/uwp/api/windows.devices.bluetooth.genericattributeprofile.gattsession.maxpdusize).
The adapter leaves connection parameters under Windows ownership and reports
the observed interval, latency, and MTU read-only. It neither forces MTU nor
mutates connection timing after sensor-frame qualification.

The original safe-Rust implementation uses the public MIT-licensed
[`MesmerPrism/PolarH10`](https://github.com/MesmerPrism/PolarH10) transport at
commit `3777ccf6970d2a0457d0a4be99e6c15645818db0` as a behavioral reference for
persistent session ownership, active advertisement discovery, Windows service
access, retry, subscription, and cleanup. No C# source is copied. Cross-platform compilation and deterministic
tests remain host evidence only; the physical Windows gate requires advancing
130 Hz ECG and 200 Hz three-axis ACC evidence from Polar Stream itself.

The published reference opens the device and persistent session, waits 500 ms,
optionally configures heart rate, then resolves PMD control/data and enables
their notifications. Its known-working doctor requests ECG settings, observes
the control/data phase for ECG, and only then starts ACC. Polar Stream now
matches that settle and heart-rate-first discovery order while keeping optional
battery discovery outside required sensor qualification. It gates startup on
the exact PMD settings response, ECG start response, first decoded ECG frame,
ACC start response, and first decoded ACC frame. Malformed, rejected, missing, or
out-of-order control responses fail closed. Both routes use uncached service
discovery, `RequestAccessAsync` before characteristic lookup, bounded retries
with cached fallback, and direct CCCD writes.

The discovery predicate repair additionally uses a black-box, identifier-free
observation of that exact published watcher: one physical H10 was admitted by
an exact local-name shape without requiring advertised service UUIDs. The
published CLI `scan` wrapper returns after starting its asynchronous watcher,
so an earlier zero-result invocation was discarded and is not device-state
evidence. The candidate borrows only the observed predicate contract; it does
not copy the reference implementation or treat generic advertisements as H10s.
A later same-device differential run proved the reference doctor consumed ECG,
ACC, and heart-rate notifications while the candidate selected that H10 and
passed device/session acquisition, services, characteristics, and all three
CCCDs. Under the response-gated sequence, the candidate received successful ECG
settings and ECG start responses, then timed out before its first ECG data
callback; ACC start was intentionally not attempted. Handler-order, a
reference-style settings delay, and a pre-stream preferred-connection request
were each eliminated as causes. Identifier-free callback/property/lifetime
counters now distinguish a missing WinRT event from buffer conversion, queue,
or frame qualification failure. A subsequent reference-positive run retained a
connected session and negotiated 232-byte PDU before and after the accepted ECG
start response, while PMD control and heart-rate callbacks advanced and PMD
 data callbacks remained at zero. Link readiness and MTU are therefore no longer
 candidate causes. Exact CCCD readback and explicit agile delegate ownership
 were then proven on hardware without producing a PMD data event. The
 reference-aligned settle/discovery/deferred-battery sequence also produced zero
 PMD-data events while heart-rate and PMD control callbacks advanced. Confining
 the entire WinRT lifecycle to the explicit apartment owner above produced the
 same physical outcome, ruling out runtime-worker migration.

The direct-handler candidate and a standalone minimal PMD probe closed the
generic projection-level question. The probe physically received ECG and ACC;
the full product verifier in the same lease still received both ECG control
responses but zero PMD-data callbacks. The closed verifier-only session profile
`POLAR_STREAM_H10_SESSION_PROFILE=pmd-only-differential` then skipped optional
heart-rate discovery/subscription. The full product still received both ECG
control responses but zero PMD-data callbacks, eliminating the optional heart
rate branch. The next exact candidate is
`pmd-only-retain-successful-gatt-operations`: relative to that failed baseline,
it additionally leaves successful PMD CCCD/control-write WinRT operations
without an explicit `Close()` immediately after completion. Timeout,
cancellation, native-error,
rollback, and final session cleanup remain unchanged. That candidate also
received both control responses but zero PMD-data callbacks, eliminating
success-time explicit close. The next exact candidate is
`pmd-only-winrt-when-completion`: relative to the failed unclosed baseline it
uses the passing probe's `windows-future` `.when` completion callback for PMD
CCCD/control operations while retaining the same Tokio deadlines and cleanup
owner. The normal path remains unchanged when the variable is absent, and every
other value rejects. Physical product and official-inlet acceptance remain open
until the next attended run.

An input-only differential complements that full verifier. It uses the same
product `InputManager`, scanner, selected WinRT session, PMD response gates, and
cleanup owner, but constructs no LSL/OSC/CSV transport. It admits success only
after both sensor timestamps advance and every ACC axis has a nonzero sample,
and emits no device identity. This diagnostic separates the product input
backend from output initialization; it is not a reduced acceptance claim.
The reference-positive input-only run reproduced the same zero-data-callback
timeout, so output initialization is eliminated. The next closed profile,
`pmd-only-winrt-when-all-setup`, extends the passing probe's `.when` completion
and no-success-close policy to device acquisition, GATT-session creation, PMD
service access/discovery, and characteristic discovery. It retains the same
deadline/cancellation/cleanup owner and leaves the default product path
unchanged. That profile also reached both ECG control responses with zero PMD
data callbacks, eliminating the selected-device async completion projection as
the remaining difference. The next closed verifier profile,
`pmd-only-probe-equivalent-sequence`, additionally matches the passing probe's
PMD-only setup shape: one service-access request, one uncached exact
characteristic lookup per PMD characteristic, explicit control-Indicate and
data-Notify CCCD values, no inter-subscription delay, and no pre-frame link
property reads. Production deadlines, cancellation, rollback, final cleanup,
and the default profile remain unchanged. A reference-positive physical run
still received both ECG control responses and zero PMD-data callbacks, so those
setup-order differences are eliminated. The next closed verifier profile,
`pmd-only-probe-std-handoff`, keeps that exact setup but gives both native
callbacks the passing probe's bounded standard-library channel handoff. One
owned bridge forwards into the existing bounded Tokio queue and is stopped and
joined during the same reverse-order cleanup. This remains diagnostic-only.
That handoff also produced both control responses and zero data callbacks in a
reference-positive run. The final diagnostic profile,
`pmd-only-probe-synchronous-owner`, therefore matches the probe's remaining
architectural property: one plain MTA thread synchronously owns every GATT
operation, response/frame wait, steady receive, and reverse cleanup. Bounded
native channels remain inside that owner; only decoded events cross into the
application. It does not alter the default profile or weaken physical
acceptance.
That synchronous-owner differential also timed out. A later same-lease full
published-doctor run received ECG, ACC, and HR, while both Polar Stream's
PMD-only synchronous owner and full reference-compatible profile received no
PMD data. Because the minimal probe failed in adjacent epochs, its earlier
single pass no longer justifies removing the full doctor's timing edge. The
closed `reference-settings-dwell` profile therefore changes only one edge in
the full product lifecycle: a typed, cancellable 1.5-second settle follows the
validated ECG-settings response before ECG start. Default behavior remains
unchanged pending physical qualification.

## Stable discovery names

`polar-h10-metrics` owns the metric/evidence/suffix catalog;
`polar-h10-output` owns the single canonical name function used by both
publishers. A normalized base such as `participant_07`
produces `participant_07_rawECG`, `participant_07_rawACC`, and one equivalent
name per optional metric. LSL uses that full string as its outlet name; OSC uses
the same string as its address with a leading slash.

The UI receives each suffix through the bootstrap catalog and the Pages build
uses the browser catalog generated from that same Rust source, so previews are
descriptions of the native contract rather than an independent naming scheme.
Its library presentation groups every catalog output by sensor family: the ECG
view contains raw ECG, ECG features, HR/HRV, coherence, and excitation metrics;
the ACC view contains raw/magnitude, breathing, phase, and breathing-dynamics
metrics. Validated custom formulas use the same base plus their normalized
formula name and therefore remain separately discoverable on LSL, OSC, and CSV.

## Latency policy

1. Decode each BLE notification once in Rust.
2. Publish LSL/OSC directly from the native coordinator.
3. If CSV is enabled, copy the decoded notification into a bounded 128-batch
   non-blocking queue. Formatting and file I/O happen on a dedicated thread;
   overflow or writer failure stops CSV visibly without delaying acquisition.
4. Forward each already-arrived BLE notification immediately. LSL uses one
   immediate chunk call for that notification (`pushthrough = true`), while OSC
   uses one immediate UDP packet; there is no timer or additional accumulation.
5. Cross the WebView boundary through a small bounded display-only queue. If the
   renderer stalls, it may skip visual frames instead of applying backpressure
   to acquisition or publication.
6. Store chart values in fixed-size typed ring buffers.
7. Request a frame only when data, layout, or controls change, capped at 30 Hz;
   stop requesting frames when idle or hidden.
8. Never block input on an animation frame.

Output reconfiguration is separately serialized so overlapping UI changes
cannot let an older asynchronous OSC setup overwrite a newer configuration.
That lifecycle lock is never taken by the publication hot path.

This keeps visualization work proportional to display pixels and refresh rate,
not to the lifetime of the recording.

## Background interface renderer

Visual validation uses `scripts/render_interface.mjs`. It serves the production
HTML, CSS, and JavaScript to headless Chromium and injects deterministic named
scenarios through `window.PolarInterfaceRenderer`. Classifier labels, circle
colors and geometry, module controls, Save behavior, and all catalog preview
SVG paths are checked without
opening the installed desktop app, scanning Bluetooth, or enabling LSL/OSC. CI
uploads all rendered classifier targets and the metric library as review artifacts.

## Recorded metric previews

`ui/data/preview-recording.json` is the sole hardware-free app-preview source:
an anonymized 60-second real Polar H10 recording containing 130 Hz ECG, 200 Hz
three-axis ACC, and HR/RR events. `scripts/generate_metric_previews.py` reads
that fixture, derives every catalog metric, and writes the checked-in numeric
series and compact SVG paths in `ui/metric-previews.js`. NeuroKit remains an
offline ECG-cleaning/method-provenance dependency; it does not generate or
replace the preview signal, and the packaged app has no Python, NumPy, or
NeuroKit runtime dependency.

The preview generator parses the native Rust catalog and fails on missing or
extra IDs. CI checks fixture privacy fields and hash, regenerates the preview
asset with pinned tooling, evaluates every executable metric template on the
fixture, and verifies that pre-save window/normalization controls visibly alter
the selected preview. These previews explain output shape and settings behavior;
one recording is not evidence of accuracy, expected population ranges, or
clinical validity.

The recorded runtime adapter loops the same raw fixture and replays its checked-in
derived series. The runtime conditions only the recorded loop boundary, while
metric and Formula Lab charts tile closed paths; discrete classes remain stepped.
This demonstrates the shared runtime event contract and lets
users inspect the app without hardware while keeping recorded input visibly
distinct from a currently connected physical sensor.

## Formula Lab

The Rust metric catalog owns both a human-readable mathematical definition for
every built-in metric and, where the bounded scalar model can express it without
distortion, an executable formula template. The generated `ui/metric-catalog.js`
keeps Pages synchronized with this source. Specialized multi-stage processors
remain fully documented but are not mislabeled as one-line executable formulas.

Formula Lab presents one editable output at a time. Sensor/event time is always
the x-axis; the selected expression is the y value. The source clock determines
the only available variables (`ecg`, `x/y/z`, `hr`, or `rr`), while the keypad,
variable map, metric templates, and recorded before/after plot make the system
usable without requiring users to memorize the grammar. JavaScript evaluates
only the preview; Tauri validates and compiles the same submission in
`polar-h10-math` before it can publish.

Each enabled formula has independent DSP state and health. It evaluates after
raw publication on its selected source clock, publishes one scalar LSL outlet
and OSC path, and writes its values to native CSV. Invalid or repeatedly
non-finite formulas fail locally and visibly without stopping raw or built-in
output publication.

## Adding outputs

1. Add one evidence-backed `MetricDefinition` in
   `crates/polar-h10-metrics/src/catalog.rs`.
2. Produce a `MetricSample` in an appropriate processor module.
3. Add the mathematical definition, executable scalar template when honest, and
   source citation in the catalog; update formula tests and the evidence inventory.
4. Regenerate `ui/metric-catalog.js` and `ui/metric-previews.js`; both generators
   enforce catalog coverage.

The UI and output router consume the same bootstrap catalog, so registered
metrics share native definitions, stream names, output cards, formula metadata,
and visualizer metadata. The library is a one-output transaction: selecting a
row reveals its recorded preview, mathematical definition, concise scientific
interpretation, citation, and any live pre-save processing controls before
**Save output** adds it to the router. The two public ACC breathing outputs share one
axis/smoothing/calibration configuration. Their independent scalar streams
remain native in Tauri; the Pages-only Web Bluetooth adapter mirrors that
processor locally so the live browser input can demonstrate the same two
outputs without pretending to be a native LSL/OSC source. See the
[ACC-derived breathing handoff](../../docs/acc-breathing-handoff.md) for exact
formulas and known differences from the upstream PolarH10 tracker.

Raw input units and canonical LSL/OSC names remain unchanged.
