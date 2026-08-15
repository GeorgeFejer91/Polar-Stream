# Polar Stream

Polar Stream is the condensed successor UI for this fork. It has exactly three
working areas:

1. **Input** — scan, connect, connection state, and battery.
2. **Output** — raw ECG/ACC readings, stream base name, LSL/OSC/CSV/experimental
   audio switches, and an extensible output list.
3. **Visualization** — one live canvas whose source is selected from the active
   outputs.

The frontend is plain HTML, CSS, and JavaScript. It has no framework runtime.
The native code is a Rust workspace split into protocol, input, output, and app
crates; see [ARCHITECTURE.md](ARCHITECTURE.md).

The one-at-a-time output library shows a recorded preview, concise scientific
context, citation and mathematical definition for every catalog metric. The
preview asset is derived from `ui/data/preview-recording.json`, an anonymized
60-second real Polar H10 ECG/ACC recording; output settings recompute or
transform that recording in the dialog before the output is saved.

Formula Lab turns formula-compatible metric definitions into editable native
custom outputs. It maps `ecg`, `x/y/z`, `hr`, and `rr`, retains sensor time as
the automatic x-axis, provides template and calculator keys with explanations,
and renders recorded before/after traces. The native runtime bounds expression,
AST, operation and retained-state costs and faults formulas independently.

## Validate the reusable crates

```bash
cargo test -p polar-h10-core -p polar-h10-input -p polar-h10-output
```

## Browser input and recorded preview input

The shared input adapter offers a clearly labeled **Recorded Polar H10 preview**
in both the installed Tauri app and browser demo. It replays the checked-in
`ui/data/preview-recording.json` entirely offline, so the complete workflow can
be explored without BLE hardware. It never enters the native BLE or LSL/OSC
path and is not device or algorithm validation.

The same canonical UI also exposes **Polar H10 via browser** on GitHub Pages.
Supported Chromium browsers use Web Bluetooth to request an H10, subscribe to
the PMD control/data characteristics, start ECG and 200 Hz ACC, and subscribe to
standard HR/RR notifications. This experimental frontend adapter mirrors the
native packet decoder and the two ACC breathing outputs, but it is not the
authoritative acquisition/publication path. A physical Motorola/Chrome smoke
test connected the public Pages site to an H10 on 2026-08-14; the full timed CSV
and reconnect acceptance run remains pending. Browser LSL and OSC destinations
are unavailable because a normal tab does not provide the native socket behavior
required by those protocols. The Pages workflow is entirely in-browser and never
connects to a localhost companion. Their shared toggles remain visible; attempts
stay off and surface an installed-app-only error with a latest-release download
link.

The browser adapter uses capability detection, preserves the initiating click
for the chooser, diagnoses Bluetooth Permissions Policy rejection, retries one
transient GATT connection failure before subscription, and falls back to the legacy
characteristic-write method. Upstream reports Web Bluetooth support in Chrome,
Edge, Samsung Internet, and Android Opera/Vivaldi, but those other browser/H10
combinations have not yet been physically validated here.

Brave disables Web Bluetooth on the currently tested Linux desktop and Android
installations. It may make `navigator.bluetooth` visible while still rejecting
the chooser. Use Google Chrome for the physical Android acceptance run, or
Chrome/Chromium with **Experimental Web Platform features** enabled for Linux
desktop hardware testing.

Chrome on Android can use this foreground path. The adapter requests a screen
wake lock while visible, but neither Pages nor an installed PWA can guarantee
capture after Android hides/freezes/discards the page or locks the screen; Web
Bluetooth is unavailable to service workers. The full Android CSV/rate/reconnect
acceptance run and a separate native foreground-service design remain required
boundaries.

The Pages output panel instead contains a browser-native session recorder. Its
**Save local CSV** toggle captures every incoming raw ECG/ACC row and every
browser-produced metric event before UI
decimation, retains raw units, reconstructs per-sample PMD timestamps from the
frame timestamp when available, and downloads a tidy CSV. Its 300,000-row cap
stops the recording visibly and requires export or discard before reuse. The
same incoming batches are exposed to same-tab code through the
`polar-stream-data` event and to other same-origin tabs through the
`polar-stream-live-v1` `BroadcastChannel`. Neither interface is described as
LSL and neither is discoverable by native LabRecorder.

The native version of the same toggle uses a bounded non-blocking Rust writer
under `Downloads/Polar Stream`. The experimental audio-data toggle emits a
CRC-checked 22.05 kbit/s stereo PCM modem waveform; see
[`docs/audio-data-output.md`](../../docs/audio-data-output.md) for its packet
format, AUX/digital recording constraints, and WAV-to-CSV decoder.

```bash
npm run build:browser-demo
python3 -m http.server 8000 --directory artifacts/browser-demo
```

Open `http://127.0.0.1:8000` and select the recorded H10 preview. The deployed
version is at `https://georgefejer91.github.io/Polar-Stream/`.

See [`docs/acc-breathing-handoff.md`](../../docs/acc-breathing-handoff.md) for
the complete accelerometer-breathing provenance, formulas, parameters, and
validation plan.

To rebuild or verify the metric-library previews, install the repository's
`requirements-previews.txt` in a Python 3.13 virtual environment and run
`scripts/generate_metric_previews.py` (or pass `--check`). NeuroKit is used only
to clean the real ECG during offline generation; there is no simulated-signal
fallback. These traces explain output shape and UI behavior, not validation.

## Run the native desktop app

Install the current [Tauri system prerequisites](https://v2.tauri.app/start/prerequisites/).
On Debian/Ubuntu this includes WebKitGTK 4.1 and libsoup 3 development packages.
Then run:

```bash
cargo run -p polar-stream
```

Linux Bluetooth access uses BlueZ/D-Bus. macOS packages must include a Bluetooth
usage description. Windows uses `btleplug` for scanning, then owns the selected
H10 through a direct persistent WinRT GATT session. It requests service access,
discovers PMD uncached with bounded retry, subscribes directly, and waits for
both decoded ECG and three-axis ACC before reporting a successful connection.
Setup, queues, cancellation, and cleanup are bounded. The best-effort
throughput-optimized connection request and observed interval/latency are
reported in the activity log. Windows still owns MTU negotiation and exposes
the negotiated value read-only.

## Outputs

Every selected output has one discoverable name shared across protocols. For a
base name of `participant_07`, raw ECG is `participant_07_rawECG` and raw
accelerometer is `participant_07_rawACC`. Additional metrics follow the same
rule, for example `participant_07_heartRate`. Spaces and protocol-unsafe
characters in the user-entered base are collapsed to underscores.

- **LSL:** packaged/default builds load `liblsl` dynamically. The app still
  starts if it is absent; enabling LSL reports the missing library beside the
  switch. ECG, ACC, and each asynchronous metric get separate outlets so
  differing sample rates are not mixed into an invalid fixed-rate stream. A
  mutually exclusive `rusty-lsl-backend` Cargo feature is available for
  source-only evaluation and remains default-off, unreleased, physically
  unaccepted, and subject to the documented AGPL boundary. Its current
  deployment contract admits one official consumer per outlet; multiple
  consumers of one outlet remain a future Rusty LSL qualification requirement.
- **OSC:** UDP packets go to `127.0.0.1:9000`. The address is the same
  discoverable name with a leading slash, such as
  `/participant_07_rawECG`. Every message starts with the sensor timestamp as an
  OSC `int64`, followed by float samples.
- **CSV:** all received raw ECG/ACC, HR/RR, and produced selected metrics are
  written by a bounded dedicated thread. Queue or disk failure stops CSV rather
  than slowing sensor/LSL/OSC work.
- **Audio data:** shared Web Audio emits an experimental stereo Manchester/CRC32
  link. In Tauri it is fed by the display channel and is therefore not an
  authoritative replacement for LSL, OSC, or native CSV.

The fixed OSC destination is deliberate: changing it is an integration concern,
not a lever needed in the primary UI. It can later be injected through app
configuration without touching acquisition or visualization code.

## Remembered preferences

The native Rust application owns a typed, schema-versioned preferences file in
the operating system's application-config directory. Each accepted output
configuration is flushed through a sibling temporary file before replacement;
the last successfully connected sensor ID and name are saved independently.
On the next launch, Polar Stream applies the name immediately and scans
automatically for that sensor. It prefers an exact device-ID match, falls back
to a single unambiguous name match, and otherwise leaves all scan results
available for manual selection. A failed connection never replaces the
remembered sensor. The browser demo uses local storage and never writes the
native file; the recorded preview input is not remembered as a physical sensor. On the
first upgraded launch only, the native app accepts
the previous version's local preferences through a bounded Rust migration
command, then uses the native file exclusively.

## Android

Tauri 2 provides the Android shell and the Rust crates are Android-compatible.
`btleplug` 0.12 supports Android GATT operations, but its Java/JNI module and
Android 12+ Bluetooth permission flow must be included when generating the
Android Studio project. Keep that platform glue inside the input crate or a
Tauri input plugin; do not move BLE packets through the HTML layer.

This is an unofficial research tool and is not a medical device.
