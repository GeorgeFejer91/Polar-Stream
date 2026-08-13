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
- The HTML layer never republishes scientific data.
- Adding a custom metric means registering one `MetricDefinition` and feeding a
  `MetricSample`; it does not change BLE acquisition or output transports.
- Native preferences have one typed, schema-versioned Rust owner in
  `apps/polar-stream/src/preferences.rs`; `ui/preferences.js` is used only by
  the browser renderer. Bluetooth and output crates remain storage-agnostic.
- All native calls are isolated behind `ui/runtime-api.js`. Command failures
  cross IPC as stable code/message/retryable objects rather than Rust strings.

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
the router. The two public ACC breathing outputs share one native axis/smoothing
configuration; their independent scalar streams and visualizers do not move
signal processing into JavaScript.

Raw input and output destinations remain unchanged.
