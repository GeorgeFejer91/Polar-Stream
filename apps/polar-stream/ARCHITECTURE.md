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
- [`btleplug` 0.12 implements scan, GATT connect, write, subscribe, and notifications on Windows, macOS/iOS, Linux, and Android](https://docs.rs/crate/btleplug/latest).
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
- Adding a custom metric means registering one `MetricDefinition` and feeding a
  `MetricSample`; it does not change BLE acquisition or output transports.
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
typed Tauri IPC/channel path. A selectable `neurokit-mock` module is available
in both Tauri and Pages and replays `ui/demo-data.js` through the same connection,
ECG, accelerometer, and metric event shapes. Mock data never crosses into the
Rust acquisition path and is visibly labeled synthetic.

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

Web Bluetooth requires a secure context, an explicit browser permission
chooser, and a compatible browser/OS. The adapter is visibly disabled with a
reason when any prerequisite is absent. It is experimental and not the
authoritative native acquisition path. Pages hides LSL and OSC because browser
tabs cannot open their native sockets. Browser acquisition, processing, and
visualization are self-contained and do not call a localhost companion, native
wrapper, or remote relay. Native Tauri acquisition remains isolated in Rust and
unchanged by this browser adapter.

## Windows BLE link policy

The Windows native adapter distinguishes ATT MTU from BLE connection timing.
[WinRT negotiates MTU automatically and exposes `GattSession.MaxPduSize` as a
read-only observation](https://learn.microsoft.com/en-us/uwp/api/windows.devices.bluetooth.genericattributeprofile.gattsession.maxpdusize).
On Windows 11+, `polar-h10-input` asks `btleplug` for the
[`ThroughputOptimized` preferred connection parameters](https://learn.microsoft.com/en-us/uwp/api/windows.devices.bluetooth.bluetoothledevice.requestpreferredconnectionparameters)
before subscriptions and PMD start commands. This is a request for a shorter
connection interval, not a forced MTU, and the controller or peripheral can
reject it. The activity log therefore records both the request result and the
observed interval, latency, and negotiated MTU. Unsupported Windows versions
and adapters remain fail-soft; they continue with operating-system-managed
timing.

Immediately after connection, before ordinary `btleplug` service discovery,
the Windows path also ports MesmerPrism's WinRT access pattern: open the PMD
service uncached, call `GattDeviceService.RequestAccessAsync`, discover PMD
characteristics uncached, and retry three times with 200/400 ms backoff when
Windows has not made the service reachable yet. This primes access without
claiming control of ATT MTU and fails soft into ordinary discovery when the
preflight cannot confirm access.

Cross-platform compilation proves API integration only. A release claim about
the applied interval, sustained ECG/ACC throughput, or packet loss still
requires a physical H10 run on the target Windows machine with the activity-log
values and sample/drop counts retained.

## Stable discovery names

`polar-h10-metrics` owns the metric/evidence/suffix catalog;
`polar-h10-output` owns the single canonical name function used by both
publishers. A normalized base such as `participant_07`
produces `participant_07_rawECG`, `participant_07_rawACC`, and one equivalent
name per optional metric. LSL uses that full string as its outlet name; OSC uses
the same string as its address with a leading slash.

The UI receives each suffix through the bootstrap catalog so its previews are
descriptions of the native contract, not an independent naming scheme. Its
library presentation groups outputs by sensor family: ECG-derived outputs use
the red ECG view, while the blue ACC view intentionally exposes only raw ACC,
3D motion magnitude, the continuous experimental breathing projection and the
three-state experimental phase classifier. Legacy breathing IDs remain in the
catalog for saved-config migration but are not offered as new library choices.

## Latency policy

1. Decode each BLE notification once in Rust.
2. Publish LSL/OSC directly from the native coordinator.
3. Forward each already-arrived BLE notification immediately. LSL uses one
   immediate chunk call for that notification (`pushthrough = true`), while OSC
   uses one immediate UDP packet; there is no timer or additional accumulation.
4. Cross the WebView boundary through a small bounded display-only queue. If the
   renderer stalls, it may skip visual frames instead of applying backpressure
   to acquisition or publication.
5. Store chart values in fixed-size typed ring buffers.
6. Request a frame only when data, layout, or controls change, capped at 30 Hz;
   stop requesting frames when idle or hidden.
7. Never block input on an animation frame.

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

## Synthetic metric previews

`scripts/generate_metric_previews.py` uses pinned NeuroKit2 development tooling
to create deterministic ECGSYN ECG and respiratory signals, processes their
peaks and rates, derives illustrative metric series, and converts each series to
a compact SVG path. `ui/metric-previews.js` is the generated, checked-in bundle;
the shipped app has no Python, NumPy, or NeuroKit runtime dependency. The static
preview bundle is lazy-loaded only when the output library opens; closing the
dialog removes all preview SVG nodes. The UI draws static thumbnails for rows
and animates only the selected detail preview.

The preview generator parses the native Rust catalog and fails on missing or
extra IDs. CI regenerates it with the pinned dependency and compares bytes before
the headless renderer checks all paths and representative animated previews.
Synthetic previews explain output shape only and are not evidence of accuracy,
real-world signal quality, expected ranges, or clinical validity.

`scripts/generate_demo_data.py` separately creates a 30-second deterministic
offline input fixture from NeuroKit2 ECGSYN ECG and RSP signals. The fixture
contains 130 Hz ECG, respiration-derived 200 Hz three-axis motion, and selected
20 Hz illustrative metric values. Python and NeuroKit are generation-time only;
the app and Pages site replay the checked-in JavaScript arrays. This fixture
demonstrates the shared runtime event contract and UI behavior, not Polar H10
fidelity or breathing-classifier validity.

## Adding outputs

1. Add one evidence-backed `MetricDefinition` in
   `crates/polar-h10-metrics/src/catalog.rs`.
2. Produce a `MetricSample` in an appropriate processor module.
3. Add or update formula tests in that module and the evidence inventory.
4. Regenerate `ui/metric-previews.js`; the generator enforces catalog coverage.

The UI and output router consume the same bootstrap catalog, so registered
metrics share native definitions, stream names, output cards, and visualizer
metadata. A small presentation allowlist keeps retired ACC breathing telemetry
out of new selections without breaking saved configurations. The library is a
one-output transaction: selecting a row reveals the scientific interpretation,
citation, and any pre-save processing controls before **Save output** adds it to
the router. The two public ACC breathing outputs share one
axis/smoothing/calibration configuration. Their independent scalar streams
remain native in Tauri; the Pages-only Web Bluetooth adapter mirrors that
processor locally so the live browser input can demonstrate the same two
outputs without pretending to be a native LSL/OSC source. See the
[ACC-derived breathing handoff](../../docs/acc-breathing-handoff.md) for exact
formulas and known differences from the upstream PolarH10 tracker.

Raw input and output destinations remain unchanged.
