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

The output library also shows a looped SVG preview for every catalog metric.
Those paths are produced at development time from a fixed NeuroKit2 ECGSYN and
respiration simulation; the generated asset is offline, deterministic, and adds
no Python or scientific-computing dependency to the installed application.

## Validate the reusable crates

```bash
cargo test -p polar-h10-core -p polar-h10-input -p polar-h10-output
```

## Browser input and offline mock input

The shared input adapter offers a clearly labeled **NeuroKit simulated input**
in both the installed Tauri app and browser demo. It replays the generated,
checked-in `ui/demo-data.js` fixture entirely offline, so the complete workflow
can be explored without BLE hardware. It never enters the native BLE or LSL/OSC
path and is not device or algorithm validation.

The same canonical UI also exposes **Polar H10 via browser** on GitHub Pages.
Supported Chromium browsers use Web Bluetooth to request an H10, subscribe to
the PMD control/data characteristics, start ECG and 200 Hz ACC, and subscribe to
standard HR/RR notifications. This experimental frontend adapter mirrors the
native packet decoder and the two ACC breathing outputs, but it is not the
authoritative acquisition/publication path and still needs physical-H10
validation. Browser LSL and OSC destinations are unavailable because a normal
tab does not provide the native socket behavior required by those protocols.
The Pages workflow is entirely in-browser and never connects to a localhost
companion; native destinations are hidden instead of presented as unavailable
switches.

Desktop Brave on Linux disables Web Bluetooth. Its Chromium feature switch may
make `navigator.bluetooth` visible while the chooser remains blocked, so Linux
desktop hardware tests require Chrome or Chromium with **Experimental Web
Platform features** enabled. Brave on Android is a separate implementation and
must be evaluated through the physical-phone acceptance procedure.

Chrome on Android can use this foreground path. The adapter requests a screen
wake lock while visible, but neither Pages nor an installed PWA can guarantee
capture after Android hides/freezes/discards the page or locks the screen; Web
Bluetooth is unavailable to service workers. Physical Android/H10 testing and
a separate native foreground-service design remain required boundaries.

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

Open `http://127.0.0.1:8000` and select the NeuroKit mock module. The deployed
version is at `https://georgefejer91.github.io/Polar-Stream/`.

See [`docs/acc-breathing-handoff.md`](../../docs/acc-breathing-handoff.md) for
the complete accelerometer-breathing provenance, formulas, parameters, and
validation plan.

To rebuild or verify the metric-library previews, install the repository's
`requirements-previews.txt` in a Python 3.13 virtual environment and run
`scripts/generate_metric_previews.py` and `scripts/generate_demo_data.py` (or
pass `--check`). These traces explain output shape and UI behavior only; they
are not recordings or validation evidence.

## Run the native desktop app

Install the current [Tauri system prerequisites](https://v2.tauri.app/start/prerequisites/).
On Debian/Ubuntu this includes WebKitGTK 4.1 and libsoup 3 development packages.
Then run:

```bash
cargo run -p polar-stream
```

Linux Bluetooth access uses BlueZ/D-Bus. macOS packages must include a Bluetooth
usage description. Windows uses the system WinRT BLE implementation through
`btleplug`. On Windows 11+, the app makes a best-effort request for WinRT's
throughput-optimized (low-interval) connection parameters before starting the
streams, then reports the observed connection interval, peripheral latency, and
negotiated MTU in the activity log. Windows owns MTU negotiation and exposes it
read-only; on older Windows versions the optimization request fails soft and
streaming continues with system-managed timing.

## Outputs

Every selected output has one discoverable name shared across protocols. For a
base name of `participant_07`, raw ECG is `participant_07_rawECG` and raw
accelerometer is `participant_07_rawACC`. Additional metrics follow the same
rule, for example `participant_07_heartRate`. Spaces and protocol-unsafe
characters in the user-entered base are collapsed to underscores.

- **LSL:** `liblsl` is loaded dynamically. The app still starts if it is absent;
  enabling LSL reports the missing library beside the switch. ECG, ACC, and each
  asynchronous metric get separate outlets so differing sample rates are not
  mixed into an invalid fixed-rate stream. The outlet name is the discoverable
  name above.
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
native file; the synthetic input is not remembered as a physical sensor. On the
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
