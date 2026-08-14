# Project context

## Purpose

Polar Stream connects to a Polar H10 over Bluetooth Low Energy, decodes raw ECG
and accelerometer notifications, calculates opt-in research metrics, and makes
selected signals available through Lab Streaming Layer and Open Sound Control.
Its primary UI deliberately stays focused on Input, Output, and Visualization.

## Data path

```text
Polar H10
  -> polar-h10-input
  -> thin Tauri coordinator
     -> polar-h10-metrics (only selected processors)
     -> polar-h10-output -> LSL / OSC / bounded native CSV
     -> bounded display queue -> JavaScript ring buffers -> Canvas 2D
```

Native publication is the authoritative real-time path. The WebView is a
display consumer and must not create backpressure.

The same files in `apps/polar-stream/ui/` also form the GitHub Pages browser
demo. Runtime behavior is selected once through the frontend runtime adapter:
Tauri can use native IPC for a real H10, while both Tauri and Pages can replay a
selectable, checked-in deterministic NeuroKit mock input through the same event
contract. Pages also offers an explicitly experimental Web Bluetooth adapter
for direct H10 ECG/ACC/HR/RR input in supported secure Chromium contexts. A
Pages session has no LSL/OSC publisher or paired mode: the runtime is
intentionally self-contained and does not call a localhost companion, native
wrapper, or remote relay. Native LSL/OSC remain separate installed-app features.
Pages can instead expose exact batches to same-tab/same-origin browser code and
record every received raw row plus produced metric events to a bounded local
CSV. These browser mechanisms are
never labeled LSL and are not discoverable by native LabRecorder.
This does not replace or enter the authoritative native H10 path.
Both runtimes also expose an explicitly experimental Web Audio data modem. It is
a cable/digital-loopback transport with CRC32, not encryption or an
authoritative replacement for native CSV/LSL/OSC.
The Pages artifact must be built from the canonical UI tree; a
separately maintained browser-interface copy is not allowed.

The project optimizes first for raw signal integrity, then continuous observable
publication, low latency/jitter, cross-platform correctness, derived metric
quality, and finally presentation. `ENGINEERING-PRIORITIES.md` defines the
operational contract for that ordering.

## Technology

- Rust workspace, minimum Rust version declared in the root `Cargo.toml`.
- Tauri 2 with the operating system WebView.
- Plain dependency-light HTML, CSS, and JavaScript frontend.
- `btleplug` for cross-platform BLE.
- Dynamically loaded liblsl and native UDP OSC output.
- Playwright for deterministic interface scenarios.
- Development-only Python/NeuroKit generation for checked-in synthetic metric
  preview and browser-demo assets; no Python runtime ships with the app or site.

## Important contracts

- One normalized base name produces stable per-output suffixes such as
  `participant_07_rawECG` and `participant_07_rawACC`.
- LSL and OSC share the same canonical discoverable output name.
- Configuration and remembered-device preferences have one schema-versioned
  native owner.
- Scientific definitions originate in the Rust metric catalog and are surfaced
  to the UI through bootstrap data.
- Browser NeuroKit input is synthetic/presentation-only. The separate browser
  H10 adapter is experimental, permission-gated, local to the tab, and limited
  to raw ECG/ACC, HR/RR, and the two unvalidated ACC breathing outputs. Browser
  LSL/OSC are unavailable and hidden because the self-contained Pages runtime
  cannot open the native sockets those protocols require. Browser recording
  must remain bounded and surface capacity/disconnect stops visibly.
- The NeuroKit mock input is also available offline in the installed app so the
  interface remains explorable without a Polar device. It must not enter or
  weaken the authoritative native acquisition/publication path.
- GitHub Pages must deploy the canonical UI after successful changes to `main`.
  Desktop and browser parity, plus narrow touch layouts down to 320 CSS pixels,
  are CI contracts rather than manual synchronization tasks.
- Chrome on Android may use the foreground Web Bluetooth path, with a
  best-effort visible-document screen wake lock. Pure Pages/PWA code cannot
  guarantee capture after a tab is hidden, the app is switched, or the screen
  locks because Web Bluetooth is unavailable to workers and mobile Chrome may
  freeze/discard the page.

## Repository and releases

- Canonical repository: `https://github.com/GeorgeFejer91/Polar-Stream`
- The repository is public as of the current state snapshot.
- Tagged releases use GitHub Actions to build platform packages and remain draft
  until the package matrix and launch checks are complete.
