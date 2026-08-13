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
   └────────► Tauri channel ────────► bounded JS ring buffers
                                      │ requestAnimationFrame
                                      ▼
                                   Canvas 2D
```

Rules enforced by the crate graph:

- Input does not know whether the consumer is a UI, recorder, LSL, or OSC.
- Output does not know where samples came from.
- Protocol decoding can be unit-tested without an adapter or runtime.
- Every derived processor and its formula tests live in `polar-h10-metrics`.
- The HTML layer never republishes scientific data.
- Adding a custom metric means registering one `MetricDefinition` and feeding a
  `MetricSample`; it does not change BLE acquisition or output transports.
- UI preferences are isolated in `ui/preferences.js`; Bluetooth and output
  crates remain free of WebView storage concerns.

## Stable discovery names

`polar-h10-metrics` owns the metric/evidence/suffix catalog;
`polar-h10-output` owns the single canonical name function used by both
publishers. A normalized base such as `participant_07`
produces `participant_07_rawECG`, `participant_07_rawACC`, and one equivalent
name per optional metric. LSL uses that full string as its outlet name; OSC uses
the same string as its address with a leading slash.

The UI receives each suffix through the bootstrap catalog so its previews are
descriptions of the native contract, not an independent naming scheme.

## Latency policy

1. Decode each BLE notification once in Rust.
2. Publish LSL/OSC directly from the native coordinator.
3. Cross the WebView boundary in notification-sized batches, not per sample.
4. Store chart values in fixed-size typed ring buffers.
5. Repaint only on `requestAnimationFrame`; acquisition continues when rendering
   is paused or throttled.
6. Never block input on an animation frame.

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
the shipped app loads only that static JavaScript and has no Python, NumPy, or
NeuroKit runtime dependency. The UI draws static thumbnails for all rows and
animates only the selected detail preview to keep library rendering inexpensive.

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

The UI and output router consume the same bootstrap catalog, so every registered
metric automatically receives a stream name, metric-library entry, output card,
and visualizer choice. The library is deliberately a one-output transaction:
selecting a row reveals the catalog's scientific interpretation and citation,
while **Save output** adds the metric to the router configuration.

Raw input and output destinations remain unchanged.
