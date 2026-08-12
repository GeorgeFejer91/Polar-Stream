# Polar Stream

Polar Stream is a compact, low-latency desktop bridge for Polar H10 raw ECG and
accelerometer data. Its interface has three focused areas: sensor input, stream
output, and live visualization. A searchable metric library adds modular ECG,
HRV, coherence, breathing, breathing-dynamics, and explicitly experimental
outputs without expanding the main three-panel surface. Selecting a metric opens
its scientific definition, interpretation limits, evidence level, cited source,
and exact stream-name preview; an explicit **Save output** action then adds that
single module to the enabled LSL and/or OSC publisher.

> Unofficial research software. Not affiliated with or endorsed by Polar
> Electro. This is not a medical device.

## Download

Use the repository's authenticated
[Polar Stream download page](https://github.com/GeorgeFejer91/Polar-Stream/releases/latest).
Because the source repository is private, GitHub asks authorized users to sign
in before serving an installer. A branded static download-page design is also
maintained in `download/`; its optional Pages workflow can be run if private
GitHub Pages is enabled for the account in the future.

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
- `apps/polar-stream`: thin Tauri coordinator and three-panel UI.

See [the architecture notes](apps/polar-stream/ARCHITECTURE.md) for the data path
and latency policy. Scientific definitions, formula caveats, and the full
legacy-output mapping are in [the metric evidence inventory](docs/metric-evidence.md).

## Develop

Prerequisites are Rust 1.88+ and the current
[Tauri system dependencies](https://v2.tauri.app/start/prerequisites/).

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p polar-stream
```

The browser-only frontend preview contains synthetic data and never runs inside
the native app:

```bash
npx browser-sync start --server apps/polar-stream/ui --no-open --no-ui
```

Release maintenance is documented in [RELEASING.md](RELEASING.md).

## License

MIT. Packaged third-party notices are in
`apps/polar-stream/resources/THIRD_PARTY_NOTICES.txt`.
