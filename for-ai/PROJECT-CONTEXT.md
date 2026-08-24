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

Vernier GDX-RB
  -> vernier-gdx-input (complete compatible metadata-defined channel set)
  -> thin Tauri coordinator
     -> polar-h10-output -> exact sparse Double64 rawVernier LSL
     -> polar-h10-metrics -> bounded derived 0-1 belt waveform
        -> polar-h10-output -> vernierBreathing LSL
     -> compatible channel-1 Force LSL / OSC / bounded native CSV
     -> bounded display queue -> JavaScript force buffer -> Canvas 2D

Bundled LabRecorder (separate process)
  <- discovers any native LSL outlets selected by the user
  -> records the selected stream set to XDF
```

Native publication is the authoritative real-time path. The WebView is a
display consumer and must not create backpressure.
Each connected native source owns an independent input session and output
router. Connecting or selecting another source never replaces the first: while
one sensor family is active, discovery scans only the missing family, and both
families publish source-suffixed outlets concurrently once connected. The UI
source selector affects presentation only.

The same files in `apps/polar-stream/ui/` also form the GitHub Pages browser
demo. Runtime behavior is selected once through the frontend runtime adapter:
Tauri can use native IPC for a real H10, while both Tauri and Pages can replay
the selectable canonical anonymized 60-second H10 ECG/ACC recording through the
same event contract as a seamless circular presentation. A bounded endpoint
correction is confined to recorded-preview playback; it never alters live H10
input or authoritative native output. Pages also offers an explicitly experimental Web Bluetooth adapter
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
- Dynamically loaded liblsl and native UDP OSC output. A mutually exclusive,
  exact-revision Rusty LSL backend exists only as a default-off source feature;
  packages continue to use liblsl.
- Native packages include the pinned official LabRecorder and its own Qt/liblsl
  runtime as an independently launched resource application. Polar Stream does
  not embed recording in its acquisition or publication path.
- Playwright for deterministic interface scenarios.
- Development-only Python/NeuroKit cleaning and metric derivation from the
  checked-in real preview fixture; no Python runtime or generated-signal
  fallback ships with the app or site.

## Important contracts

- One normalized base name produces stable per-output suffixes such as
  `participant_07_rawECG` and `participant_07_rawACC`.
- A native GDX-RB automatically adds per-source `rawVernier` and
  `vernierBreathing` suffixes. The raw outlet is a metadata-driven sparse
  Double64 recording contract; the derived outlet is an explicitly labeled
  Float32 0–1 relative belt-force waveform. Browser Go Direct remains
  channel-1 force-only and has no native LSL capability.
- Output and Visualization have a real no-device state. A source connection
  activates only that device profile's defaults, and the selected profile's
  active outputs determine the available visualizations. Polar defaults to raw
  ECG/ACC; Vernier defaults to automatic `rawVernier` plus
  `vernierBreathing`, while rawForce remains an optional compatibility module.
  When both families are connected, selecting a source switches the entire UI
  profile and never combines Polar cards/formulas with Vernier breathing cards.
- The native Add-device action scans only an inactive protocol family. Existing
  Polar or Go Direct acquisition and publication continue during that scan, and
  the default liblsl path can expose Polar ECG/ACC plus Vernier raw/breathing
  outlets simultaneously on one local LSL clock.
- Input has two distinct UI states: classified discovery rows and connected
  source widgets. Only a successful connection creates a widget. Device-profile
  attributes come from the typed Polar/Go Direct identification boundary, not a
  renderer name guess. Widgets own source telemetry, disconnect, session color,
  and applicable controls; keep-connected is Vernier-only and defaults on for a
  new preference state.
- Raw source outputs are automatic and non-removable. A native physical
  connection automatically enables LSL and applies the source-owned raw output
  configuration. Processed metrics/formulas remain explicit additions, and
  Visualization choices are rebuilt only from automatic or selected outputs.
- LSL and OSC share the same canonical discoverable output name.
- The native LabRecorder launcher accepts no frontend-supplied executable,
  path, arguments, or configuration. It resolves only the complete fixed
  platform resource, enables raw LSL before launch when needed, and passes the
  packaged profile with remote control disabled. Stream selection, XDF naming,
  and recording remain user-controlled in the separate LabRecorder window.
- Official Rusty-backend qualification must enumerate broadly and apply exact
  client-side name/type/channel/rate/format/source-ID matching before opening.
  Predicate-filter conformance is intentionally unsupported.
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
  interface remains explorable without a Polar device. Its visual and browser-local
  preview streams loop continuously, with categorical classes retained as steps.
  It must not enter or
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
  until the package matrix and launch checks are complete. Those checks include
  starting the pinned bundled LabRecorder from every exact installer payload.
