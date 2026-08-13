# Polar Stream

Polar Stream is a compact, low-latency desktop bridge for Polar H10 raw ECG and
accelerometer data. Its interface has three focused areas: sensor input, stream
output, and live visualization. The output library starts with a prominent
red ECG / blue accelerometer selector. ECG mode contains the H10's core ECG,
heart-rate, HRV and related outputs; ACC mode intentionally stays small, with
raw ACC, 3D motion magnitude, and only two experimental breathing outputs.
Selecting a metric opens
its scientific definition, interpretation limits, evidence level, cited source,
and exact stream-name preview; an explicit **Save output** action then adds that
single module to the enabled LSL and/or OSC publisher.

When the output library is open, every row contains an SVG thumbnail of its
expected output shape. Preview data and SVG nodes are kept out of normal startup
and unloaded from the document when the library closes.
Selecting the row opens a larger, smoothly looped preview generated at development
time from deterministic NeuroKit2 ECGSYN ECG and respiratory signals. These are
illustrative synthetic traces, not example participant norms, algorithm validation,
or diagnostic data; the app labels that limitation beside every expanded preview.

Every output module has a saved visualizer window and, where meaningful,
normalization controls. Before either ACC breathing output is added, the user
chooses two or three axes (X + Z recommended) and a smoothing window. The
three-state phase classifier also exposes sensitivity and direction inversion;
the continuous magnitude estimate can be published in g or normalized to 0–1.
Both are explicitly unvalidated and should be compared with a respiratory
reference. The classifier circle expands or shrinks with phase alone, approaches
its size limits asymptotically, and eases its velocity toward rest during pauses.

Raw acceleration is presented as one visualizer choice with X, Y, and Z in
three labeled, color-coded lanes, so comparing axes does not require switching
the visualizer source.

> Unofficial research software. Not affiliated with or endorsed by Polar
> Electro. This is not a medical device.

## Browser demo

The [live Polar Stream browser demo](https://georgefejer91.github.io/Polar-Stream/)
is deployed from the same `apps/polar-stream/ui/` HTML, CSS, and JavaScript used
by the Tauri application. On supported Chrome/Edge browsers, choose **Polar H10
via browser** to grant Web Bluetooth permission and stream ECG, ACC, HR, and RR
directly in the tab. This path is experimental and still requires validation on
physical H10 hardware. Web Bluetooth requires HTTPS (or localhost), an explicit
user chooser, and browser/OS support; unsupported phones and browsers keep the
input visible with a compatibility explanation.

Choose **NeuroKit simulated input** to replay deterministic ECGSYN ECG and
respiration-derived accelerometer data without Bluetooth hardware. The same
synthetic input is available offline in the installed desktop app for interface
exploration without a Polar strap.

Synthetic input is labeled throughout the interface. It demonstrates layout,
configuration, and visualization behavior only; it is not recorded Polar data,
device or algorithm validation. By itself it does not open BLE, LSL, or OSC
connections.

The browser demo is deliberately self-contained: Bluetooth acquisition, mock
replay, selected browser-supported metrics, and visualization all run in the
tab without Tauri, Python, a localhost service, or another installed process.
After connecting either input, **Start recording** captures the selected raw and
derived outputs before chart decimation. **Download CSV** saves timestamped ECG,
ACC, and selected metric rows locally from the browser. Files are bounded to
300,000 rows (about 15 minutes with the default raw ECG + ACC outputs); reaching
the limit stops visibly instead of dropping rows or growing memory without a
bound. Export or discard that file before starting the next segment, and always
download it before closing the tab.

The site also publishes the same event batches as a `polar-stream-data` browser
event and on the same-origin `polar-stream-live-v1` `BroadcastChannel`. This is
a browser-native integration surface for another page or script in the same
browser profile. It is deliberately called a live channel, not LSL.

LSL and OSC are not shown as browser destinations because ordinary web pages
cannot open the raw UDP discovery/multicast and TCP/UDP sockets those native
protocols require. Native LSL/OSC remain features of the separately installed
desktop app; the website does not relay data to it and does not label a
WebSocket or HTTP transport as LSL.

GitHub Pages is rebuilt from the canonical UI after changes land on `main`, and
CI checks byte-for-byte asset parity plus desktop, 390px, and 320px layouts.

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

The packages include a checksum-pinned liblsl runtime. No separate LSL install
is required. Windows packages include the Microsoft runtime needed by liblsl,
and the Windows installer carries WebView2's offline installer.

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
- `polar-h10-input`: BLE discovery, connection, and typed sensor events.
- `polar-h10-metrics`: independent ECG, HRV, coherence, breathing, complexity,
  and experimental processors plus the evidence-backed metric catalog.
- `polar-h10-output`: LSL/OSC naming and publication.
- `apps/polar-stream`: thin Tauri coordinator and shared three-panel UI.

Derived processors are demand-driven by the selected output set. Raw batches are
published immediately in Rust; no timer-based batching is added, and a bounded
display-only queue prevents a slow WebView from delaying LSL or OSC.
Native preferences are stored by one schema-versioned Rust service, and the
frontend reaches the seven-command IPC surface only through a small runtime
adapter with stable error objects.

See [the architecture notes](apps/polar-stream/ARCHITECTURE.md) for the data path
and latency policy, and [the Tauri assessment](docs/tauri-assessment.md) for the
evidence-based Rust/Tauri decision and its limits. Scientific definitions,
formula caveats, and the full
legacy-output mapping are in [the metric evidence inventory](docs/metric-evidence.md).
The exact ACC breathing lineage, formulas, settings, known batch-timing
limitation, and proposed reference-validation protocol are in the
[ACC-derived breathing handoff](docs/acc-breathing-handoff.md).

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
preview coverage, Pages parity, NeuroKit replay, bounded browser recording and
CSV download, the tab-local live event contract, and responsive overflow:

```bash
npm ci
npx playwright install chromium
npm run validate:interface
npm run validate:browser-demo
```

NeuroKit is a development-only fixture generator and is not shipped in the app
or website. Regenerate and reproducibly verify the checked-in, dependency-free
SVG previews and mock input with:

```bash
python3 -m venv .venv-previews
.venv-previews/bin/python -m pip install -r requirements-previews.txt
.venv-previews/bin/python scripts/generate_metric_previews.py
.venv-previews/bin/python scripts/generate_demo_data.py
PATH="$PWD/.venv-previews/bin:$PATH" npm run check:previews
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
