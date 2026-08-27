# Polar Stream

Polar Stream is a compact, low-latency desktop bridge for simultaneous Polar
H10 ECG/accelerometer and Vernier Go Direct respiration-belt inputs. Its interface has
three focused areas: sensor input, stream output, and live visualization. Output
and Visualization begin empty: the first connected device instantiates its own
preset, and selecting a source when Polar and Vernier are both active switches
the whole visible protocol without mixing cards or samples. Polar defaults to
raw ECG and accelerometer; Vernier defaults to its complete raw recording plus
the separate 0–1 breathing waveform. The Polar output library starts with a
prominent ECG / accelerometer selector. ECG mode contains the H10's core ECG,
heart-rate, HRV, coherence, Excite-O-Meter and experimental activation outputs;
ACC mode starts with exactly raw X/Y/Z, 3D motion magnitude, and **ACC breathing
magnitude (0–1)**. Signed projection, phase, quality/calibration, breathing rate,
and breathing-dynamics outputs remain available under **Extra options** with
their existing IDs and stream names.
Selecting a metric opens one focused preview window containing only a looping
SVG example of the output, a two- or three-sentence scientific summary, and two
or three of the most relevant sources. An explicit **Save output** action then
adds that single module to the enabled destinations. List rows stay static so
only the selected metric animates.

The loop is derived from the canonical anonymized 60-second real Polar H10
ECG/ACC recording. It illustrates output form—it is not a participant norm,
validation result, or diagnostic example. Display, normalization, and breathing
settings remain available through **Adjust** after the output is added.

Formula-compatible metrics can be explored separately in **Formula Lab**, which
explains the signal variables, keeps
time as the automatic x-axis, provides a calculator-style insert keyboard with
hover help, and compares recorded input with formula output. Custom formulas
are parsed into a bounded native scalar runtime and can publish their own LSL,
OSC, and CSV streams; specialized multi-stage metrics remain documented without
pretending that a misleading one-line scalar template is equivalent.

Every output module has a saved visualizer window and, where meaningful,
normalization controls. New ACC breathing outputs default to source-timed PCA
over X + Z with a 0.18-second filter time constant. The H10 normally delivers
roughly 37 internally 200 Hz samples per BLE notification; Polar Stream
uses nominal 5 ms timing for the first frame and interpolates later samples
between consecutive PMD newest-sample timestamps, so waveform and phase do not
inherit the slower notification cadence. The module exposes a signed chest-motion projection
in g, its robustly normalized 0–1 waveform, a hysteretic inhale/hold/exhale
classifier, and explicit readiness/confidence companions. Saved pre-v1 settings
retain the legacy estimator. Direction inversion, calibration, filter, state,
and optional adaptive-bound controls remain available through **Adjust**.

The waveform display separately offers responsive fresh smoothing or an
intentional 0.18-second timestamp-faithful delay; neither presentation mode
changes canonical LSL, OSC, CSV, or classifier values. Every output is an
unvalidated respiratory-motion/effort surrogate—not lung volume or airflow—and
should be compared with a synchronized respiratory reference. Mounting can
reverse polarity, so users must verify or invert direction. The classifier
circle expands or shrinks with phase alone, approaches its size limits
asymptotically, and eases its velocity toward rest during holds.

Raw acceleration is presented as one visualizer choice with X, Y, and Z in
three labeled, color-coded lanes, so comparing axes does not require switching
the visualizer source. When another connected source already has a compatible
active signal, **Add comparison source** overlays one comparator without
enabling any output. Polar's normalized `breathing_volume` and Vernier's
normalized breathing waveform share a fixed 0–1 host-monotonic view; raw force
cannot be compared with ACC and breathing cannot be compared with ECG.

> Unofficial research software. Not affiliated with or endorsed by Polar
> Electro. This is not a medical device.

## Browser demo

The [live Polar Stream browser demo](https://georgefejer91.github.io/Polar-Stream/)
is deployed from the same `apps/polar-stream/ui/` HTML, CSS, and JavaScript used
by the Tauri application. On supported Chrome/Edge browsers, choose **Polar H10
via browser** for ECG, ACC, HR, and RR, or **Vernier Go Direct via browser** for
a metadata-verified GDX-RB channel-1 Force (N) stream. Each browser device is granted through its own
Web Bluetooth chooser; Go Direct sessions receive stable source colors and
separate visual buffers. A physical Motorola/Chrome smoke test selected and connected
to an H10 from the public Pages site on 2026-08-14. The full two-minute CSV,
sample-rate, loss, and reconnect acceptance run is still pending, and Go Direct
has not yet received a physical browser run. Web Bluetooth
requires HTTPS (or localhost), an explicit user chooser, and browser/OS support.
Unsupported phones and browsers keep the input visible with a compatibility
explanation.

The installed desktop app uses the wider native Vernier path. A verified
GDX-RB connection automatically creates `<base>_rawVernier`, a Double64,
irregular-rate LSL stream containing every metadata-exposed device channel plus
timing/loss diagnostics, and `<base>_vernierBreathing`, a separately identified
derived 0–1 relative belt-force waveform. Missing slow/aperiodic raw values are
`NaN`, never carried forward; the compatible channel-1 rawForce output remains
available as an opt-in compatibility module rather than a third default stream.
The Vernier preset exposes only force/breathing cards and visualizations; Polar
ECG/ACC and Formula Lab controls are not shown for that source. Browser Web
Bluetooth remains channel-1 Force only and cannot create
native LSL outlets.

In the installed app, connecting one sensor changes the discovery action to
**Add Vernier** or **Add Polar H10**. The connected source keeps acquiring and
publishing while either protocol candidate cache is refreshed. Once both are
connected, **Add another sensor** can find another non-active Polar or Vernier
device without replacing either live owner. Their source-specific output
routers remain live together: Polar ECG/ACC and Vernier raw/breathing outlets
can be recorded on the same LSL clock. Selecting a source changes ordinary
controls and charts only. A compatible active signal can be added explicitly as
one comparator without stopping, merging, or republishing either source.

Input discovery is deliberately separate from connection state. **Search
devices** lists supported candidates as classified rows—Polar H10 for ECG or a
metadata-verified GDX-RB for breathing—and **Connect** promotes a row to a
persistent device widget only after the link succeeds. Connected widgets own
their metadata, disconnect action, and two-color palette picker; the Vernier widget also
owns its keep-connected/reconnect switch, which defaults on for a new
preference state. Eight unique remembered palette pairs provide light and dark
variants for each source and thread through Input, Output, Visualization,
legends, LSL descriptors, and CSV metadata. The app-level sun/moon control
follows the OS initially and persists an explicit choice without restarting
outputs.

Raw device measurements are non-removable automatic outputs. A native physical
connection also enables LSL automatically so the corresponding source-suffixed
raw outlets become discoverable without an extra setup step. Metrics and custom
formulas are processed outputs: they join Output only when added (or restored
from saved output preferences), and only active outputs contribute choices to
Visualization. The browser keeps the same structure but cannot publish native
LSL.

Native packages also include the official LabRecorder. After Polar Stream has
created the LSL outlets, **Open Lab Recorder** starts its separate GUI without a
second download. The user can then select any Polar Stream or other discoverable
LSL streams and record them together as XDF. The Pages button remains visible
for interface parity but directs the user to the installed app because a web
page cannot launch LabRecorder or discover native LSL. See the
[bundled LabRecorder guide](docs/bundled-lab-recorder.md).

Compatibility is capability-based rather than tied to a browser-name allowlist.
The adapter checks the secure context and `navigator.bluetooth.requestDevice`,
then reports Permissions Policy rejection from the chooser. It opens the chooser
at the first asynchronous boundary, retries one transient pre-subscription GATT
connection, and supports both current and legacy characteristic-write methods.
Upstream browser/platform coverage is tracked in the
[Web Bluetooth implementation table](https://github.com/WebBluetoothCG/web-bluetooth/blob/main/implementation-status.md).
It reports support in Chrome, Edge 79+, Samsung Internet, and Android Opera/Vivaldi;
only Android Chrome has been physically smoke-tested with Polar Stream so far.
Linux Chrome/Chromium may require **Experimental Web Platform features**.

Brave disables Web Bluetooth and cannot acquire an H10 from the Pages site on
the currently tested Linux desktop and Android installations. Its Chromium API
may be visible while Brave still rejects the chooser. Android WebView, iOS
Chromium browsers, and desktop Opera/Vivaldi are also listed upstream as lacking
a working Web Bluetooth implementation. Website code cannot enable an API that
the browser or its administrator has disabled.

While browser Bluetooth is connected, Polar Stream makes a best-effort screen
wake-lock request. This can keep a visible foreground session awake, but a pure
website cannot guarantee recording after the user changes tabs/apps or locks
the screen: Android may freeze or discard the page, and Web Bluetooth is not
available to a service worker. The desktop app avoids this tab lifecycle; a
reliable smartphone background/screen-off workflow requires a future native
Android build with a foreground service, not the GitHub Pages site.

Choose **Recorded Polar H10 preview** to replay the checked-in, anonymized
60-second real ECG/ACC recording without Bluetooth hardware. Its last 1.2 seconds
receive a smooth endpoint correction so the presentation and browser-local
preview outputs repeat without a false gap or sudden end-to-start jump; the
checked-in recording itself is unchanged. The same input is
available offline in the installed desktop app for interface exploration. It is
labeled recorded throughout, is not algorithm validation, and by itself does
not open BLE, LSL, or OSC connections.

The browser demo is deliberately self-contained: Bluetooth acquisition, mock
replay, selected browser-supported metrics, and visualization all run in the
tab without Tauri, Python, a localhost service, or another installed process.
Turn on **Save local CSV** to capture every incoming raw ECG/ACC row and every
metric event produced in the browser before chart decimation. Turning it off
downloads the timestamped CSV immediately. Files are bounded to 300,000 rows
(about 15 minutes at the raw ECG + ACC rates); reaching the limit stops visibly
instead of dropping rows or growing memory without a bound. Download or discard
that stopped file before starting the next segment, and always download it
before closing the tab.

Browser recording schema 3 adds `source_id` and `source_palette_id` columns plus
one complete palette-definition header per encountered source. Native CSV schema
3 keeps its existing sample rows and adds the same palette definition to header
comments. Changing a palette while recording closes the current segment and
starts a new one so one file never silently changes source-color identity.

The installed app exposes the same **Save local CSV** toggle. Native recordings
are written under `Downloads/Polar Stream` (with an app-data fallback) by a
dedicated bounded writer thread. Raw LSL/OSC publication happens first and never
waits for disk I/O; if the 128-notification CSV queue fills or writing fails,
CSV stops and the interface reports the error.

The **Audio data output** toggle emits an experimental 22.05 kbit/s stereo PCM
modem signal for a digital recorder or line-level AUX/USB audio cable. It is
modulation, not encryption, and currently uses Manchester coding plus CRC32
without forward error correction. A checked-in decoder converts a stereo PCM
WAV back into CSV:

```bash
python3 scripts/decode_audio_data.py recording.wav
```

See [the audio data-output design and recording guide](docs/audio-data-output.md)
for the researched alternatives, packet format, hardware requirements, and
reliability limits.

The site also publishes the same event batches as a `polar-stream-data` browser
event and on the same-origin `polar-stream-live-v1` `BroadcastChannel`. This is
a browser-native integration surface for another page or script in the same
browser profile. It is deliberately called a live channel, not LSL.

LSL and OSC remain visible in the shared browser interface, but ordinary web
pages cannot open the raw UDP discovery/multicast and TCP/UDP sockets those
native protocols require. Trying either toggle in Pages leaves it off and shows
an installed-app-only error with a link to the latest release. Native LSL/OSC
remain features of the separately installed desktop app; the website does not
relay data to it and does not label a WebSocket or HTTP transport as LSL.

GitHub Pages is rebuilt from the canonical UI after changes land on `main`, and
CI checks byte-for-byte asset parity plus desktop, 390px, and 320px layouts.
The release-blocking physical procedure is documented in the
[GitHub Pages H10 acceptance guide](docs/browser-hardware-acceptance.md). It
verifies the public deployment and analyzes the resulting CSV without allowing
localhost, Tauri, a relay, or synthetic data to count as hardware evidence.

## Download

Use the repository's
[Polar Stream download page](https://github.com/GeorgeFejer91/Polar-Stream/releases/latest).

Every published release is held as a draft until CI has built and launch-tested
the complete package set:

| Platform | CPU | Packages |
| --- | --- | --- |
| Windows 10/11 | x64 | NSIS `.exe`, `.msi` |
| Windows 11 | ARM64 | NSIS `.exe`, `.msi` |
| macOS 10.15+ | Intel and Apple Silicon | Universal `.dmg` |
| Linux | x64 | `.AppImage`, `.deb` |
| Linux | ARM64 | `.AppImage`, `.deb` |

The packages include a checksum-pinned liblsl runtime and a self-contained,
pinned official LabRecorder. No separate LSL, Qt, or LabRecorder install is
required. Windows packages include the Microsoft runtime needed by liblsl, and
the Windows installer carries WebView2's offline installer.

Source developers may instead compile the experimental, default-off Rusty LSL
backend. It is pinned to one reviewed upstream commit and is not enabled in any
package. Its official-consumer host qualification, exact descriptor-selection
rule, physical-device gate, predicate-filter exclusion, and AGPL licensing
boundary are documented in the
[optional Rusty LSL backend guide](docs/rusty-lsl-backend.md).

Current packages are ad-hoc/unsigned. Windows SmartScreen and macOS Gatekeeper
may therefore require an explicit confirmation. See the download page for the
short platform-specific instructions.

## Stream names

The user enters one base name. Polar Stream normalizes protocol-unsafe
characters and applies stable suffixes to every LSL outlet and OSC address:

```text
participant 07  →  participant_07_rawECG
                →  participant_07_rawACC
                →  participant_07_heartRate
                →  participant_07_excitementScore
```

OSC uses the identical name with a leading slash, for example
`/participant_07_rawECG`. The last accepted base name and the last successfully
connected H10 are remembered between launches.

## Architecture

The frontend is plain HTML, CSS, and JavaScript inside a Tauri 2 system WebView.
Acquisition and publication stay native in Rust:

- `polar-h10-core`: PMD and heart-rate decoding.
- `polar-h10-input`: bounded multi-H10 BLE discovery, connection, and typed events.
- `vernier-gdx-core`: independent Go Direct command/framing and measurement decoding.
- `vernier-gdx-input`: bounded multi-device native BLE sessions and latency health.
- `polar-h10-metrics`: independent ECG, HRV, coherence, breathing, complexity,
  and experimental processors plus the evidence-backed metric catalog.
- `polar-h10-math`: bounded custom scalar formulas, stateful DSP/HRV functions,
  source-clock validation, and per-formula fault isolation.
- `polar-h10-output`: LSL/OSC naming and publication plus bounded native CSV.
- `apps/polar-stream`: thin Tauri coordinator and shared three-panel UI.

Derived processors are demand-driven by the selected output set. Raw batches are
published immediately in Rust; Go Direct frames retain every negotiated device
channel and their configured intra-batch sample spacing, no timer-based batching
is added, and a bounded display-only queue prevents a slow WebView from delaying
LSL or OSC. The separate Vernier 0–1 waveform is calculated only after the exact
aggregate raw frame has been handed to LSL.
Native preferences are stored by one schema-versioned Rust service, and the
frontend reaches the eight-command IPC surface only through a small runtime
adapter with stable error objects.

See [the architecture notes](apps/polar-stream/ARCHITECTURE.md) for the data path
and latency policy, and [the Tauri assessment](docs/tauri-assessment.md) for the
evidence-based Rust/Tauri decision and its limits. Scientific definitions,
formula caveats, and the full
legacy-output mapping are in [the metric evidence inventory](docs/metric-evidence.md).
The exact ACC breathing lineage, source-time/batch contract, versioned formulas,
settings, presentation boundary, and proposed reference-validation protocol are in the
[ACC-derived breathing handoff](docs/acc-breathing-handoff.md).
The [Go Direct and multi-source handoff](docs/vernier-go-direct-handoff.md)
documents the native/browser protocol paths, timing contract, source identity,
and remaining hardware gates.
The [latency and multi-source architecture](docs/latency-multi-source-architecture.md)
documents shared clock mapping, scanning during live sessions, renderer
reattachment, composite panels, graceful shutdown, PsychoPy integration, and
the physical qualification matrix.
The [optional Rusty LSL backend guide](docs/rusty-lsl-backend.md) records the
separate default-off transport, interoperability evidence, and remaining
device/licensing gates.

## Develop

Prerequisites are Rust 1.88+ and the current
[Tauri system dependencies](https://v2.tauri.app/start/prerequisites/).

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p polar-stream
```

Visual validation uses deterministic Playwright renderers, never a connected
sensor or the running desktop app. They render every breath-class target, the
settings workflow, all metric-library SVGs, the staged Pages application, and
touch layouts. Checks cover canvas geometry, color, labels, saved controls,
preview coverage, Pages parity, recorded H10 replay, Formula Lab behavior, bounded browser recording and
CSV download, audio packet/WAV decode, the tab-local live event contract,
emulated Android-style Polar and Go Direct Web Bluetooth, and responsive overflow:

```bash
npm ci
npx playwright install chromium
npm run validate:interface
npm run validate:browser-demo
npm run test:browser-acceptance
npm run verify:live-pages
```

The canonical recording is checked in at
`apps/polar-stream/ui/data/preview-recording.json`. NeuroKit is used only for
offline cleaning/method provenance while deriving the checked-in preview asset;
it never generates or replaces the app preview signal. Regenerate and verify
the recorded previews and Rust-exported browser catalog with:

```bash
python3 -m venv .venv-previews
.venv-previews/bin/python -m pip install -r requirements-previews.txt
.venv-previews/bin/python scripts/generate_metric_previews.py
PATH="$PWD/.venv-previews/bin:$PATH" npm run check:previews
npm run generate:catalog
npm run check:catalog
```

Stage the exact GitHub Pages artifact locally with:

```bash
npm run build:browser-demo
python3 -m http.server 8000 --directory artifacts/browser-demo
```

Release maintenance is documented in [RELEASING.md](RELEASING.md).

## License

MIT. Packaged third-party notices are in
`apps/polar-stream/resources/THIRD_PARTY_NOTICES.txt`.
