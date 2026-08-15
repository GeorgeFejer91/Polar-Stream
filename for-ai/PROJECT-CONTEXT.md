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
     -> polar-h10-math (only enabled bounded custom formulas)
     -> polar-h10-output -> LSL / OSC / bounded native CSV
     -> bounded display queue -> JavaScript ring buffers -> Canvas 2D
```

Native publication is the authoritative real-time path. The WebView is a
display consumer and must not create backpressure.

The same files in `apps/polar-stream/ui/` also form the GitHub Pages browser
demo. Runtime behavior is selected once through the frontend runtime adapter:
Tauri can use native IPC for a real H10, while both Tauri and Pages can replay
the selectable canonical anonymized 60-second H10 ECG/ACC recording through the
same event contract. Pages also offers an explicitly experimental Web Bluetooth adapter
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
- Development-only Python/NeuroKit cleaning and metric derivation from the
  checked-in real preview fixture; no Python runtime or generated-signal
  fallback ships with the app or site.

## Important contracts

- One normalized base name produces stable per-output suffixes such as
  `participant_07_rawECG` and `participant_07_rawACC`.
- LSL and OSC share the same canonical discoverable output name.
- Configuration and remembered-device preferences have one schema-versioned
  native owner.
- Scientific definitions originate in the Rust metric catalog and are surfaced
  to the UI through bootstrap data and the generated Pages catalog asset.
- Formula-compatible catalog entries expose editable templates through a
  bounded native formula runtime; sensor time is the automatic x-axis and each
  formula produces a scalar y-value from one source clock.
- Browser recorded preview input is presentation-only. The separate browser
  H10 adapter is experimental, permission-gated, local to the tab, and limited
  to raw ECG/ACC, HR/RR, and the two unvalidated ACC breathing outputs. Browser
  LSL/OSC are unavailable because the self-contained Pages runtime cannot open
  the native sockets those protocols require. Their canonical toggles remain
  visible and interactive; an attempted enable must stay off and surface an
  installed-app-only error with the latest-release download link. Browser
  recording must remain bounded and surface capacity/disconnect stops visibly.
- The canonical recorded preview input is also available offline in the installed app so the
  interface remains explorable without a Polar device. It must not enter or
  weaken the authoritative native acquisition/publication path.
- GitHub Pages must deploy the canonical UI after successful changes to `main`.
  Desktop and browser parity, plus narrow touch layouts down to 320 CSS pixels,
  are CI contracts rather than manual synchronization tasks.
- Every completed edit must also production-build/install the same source into
  the resolved per-user desktop location, verify the public Pages manifest
  against the checkout, and return the live Pages URL to the user.
- Chrome on Android may use the foreground Web Bluetooth path, with a
  best-effort visible-document screen wake lock. Pure Pages/PWA code cannot
  guarantee capture after a tab is hidden, the app is switched, or the screen
  locks because Web Bluetooth is unavailable to workers and mobile Chrome may
  freeze/discard the page.

## Repository and releases

- Canonical repository: `https://github.com/GeorgeFejer91/Polar-Stream`
- Canonical local checkout: `/home/George/Documents/GitHub/Polar-Stream`; the
  `PolarH10` Mesmerism-derived checkout is quarantined at
  `/home/George/Documents/GitHub/_quarantine/PolarH10-mesmerism-fork`.
- The repository is public as of the current state snapshot.
- Tagged releases use GitHub Actions to build platform packages and remain draft
  until the package matrix and launch checks are complete.
