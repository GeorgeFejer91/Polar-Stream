# Decision log

## 2026-08-15 — Windows owns one direct WinRT Bluetooth session

Windows uses an active WinRT advertisement watcher for four seconds and
coalesces at most 256 unique matching addresses before exact device selection.
The callback is removed before return, cleanup calls `Stop` only while the
watcher is actively started, and no per-device property request can stall the
scan. After selection, `polar-h10-input` opens the Bluetooth address
with WinRT and owns one persistent `GattSession`, uncached PMD service
discovery, access requests, characteristic discovery, notification handlers,
start commands, and teardown. Linux, macOS, iOS, and Android retain the existing
`btleplug` connection path.

The Windows session does not announce connection success until it has decoded
both an ECG frame and a three-axis ACC frame. Setup stages have explicit
deadlines and observe cancellation at stage boundaries. Notification and
first-frame buffers are bounded and fail closed rather than lose data silently;
shutdown has one global deadline followed by synchronous handler and WinRT
handle release. A throughput-optimized connection request remains best effort,
and negotiated MTU remains a read-only observation.

This is an original safe-Rust adapter using the repository's existing
`windows` crate. Its behavioral reference is the public MIT-licensed
[`MesmerPrism/PolarH10`](https://github.com/MesmerPrism/PolarH10) Windows
transport at commit `3777ccf6970d2a0457d0a4be99e6c15645818db0`: active
advertisement discovery, persistent session ownership, `RequestAccessAsync`,
uncached discovery/retry, direct CCCD subscription, and explicit close. No C#
source or local experimental evidence is incorporated. Compilation and synthetic tests do not replace a physical H10
acceptance run.

## 2026-08-15 — Rusty outlets admit one official consumer

Polar Stream bounds every optional Rusty LSL outlet to one official consumer.
The synchronous caller-polled work runs on Tokio's blocking pool with an
explicit stop signal and bounded join, and Windows outlet setup reserves one
numeric port across TCP and UDP before registry admission. Deterministic tests
require a second concurrent consumer to reject without disturbing the admitted
consumer and require conflicted port candidates to retry and release cleanly.

This is a bounded Polar Stream deployment workaround, not generic Rusty LSL
multi-consumer or auxiliary-connection conformance. If the product later needs
multiple official consumers of one outlet, that is a separate Rusty LSL source
unit. Two physical straps passed the independent native WinRT doctor, while
Polar Stream's Windows `btleplug` path stopped at connect, notification setup,
or first PMD frame. Therefore the draft retains no physical-acceptance claim;
a native WinRT backend is separately scoped.

## 2026-08-14 — Rusty LSL is an exact-pinned, default-off native backend

Packaged and default builds retain liblsl. Source developers may select exactly
one mutually exclusive `rusty-lsl-backend`, pinned to reviewed Rusty LSL merge
`74f7d0ea2cce9b3d049ea24602527a5f52360554`. It stays inside
`polar-h10-output`, preserves the canonical names/metadata and independent
ECG/ACC outlets, and never enters the browser runtime.

Qualification uses pinned official pylsl 1.18.2/liblsl 1.17.7 broad
enumeration, followed by exact client-side matching of name, type, channel
count, nominal rate, channel format, and source ID. Zero, multiple, mismatched,
or cross-stream candidates reject. Server-side `resolve_byprop` predicate
conformance is intentionally unsupported and cannot be implied by discovery
success.

Host interoperability passed for 1-channel/130 Hz ECG and 3-channel/200 Hz ACC.
The backend remains device-unaccepted. No package may enable it until physical
and cross-platform gates pass. Rusty LSL is
AGPL-3.0-or-later while Polar Stream is MIT; enabled distribution additionally
requires an explicit licensing/compliance decision.

## 2026-08-15 — Recorded previews form continuous circular presentation signals

The hardware-free 60-second recording repeats without an empty interval or a
visible end-to-start jump. Recorded-input playback applies a smooth, bounded
1.2-second endpoint correction to ECG, all three ACC axes, HR and RR; the
checked-in fixture remains unchanged. This conditioning is identified as
recorded-preview behavior and cannot enter live H10 acquisition, native raw
publication, or physical-device claims.

Compact and detail metric SVGs tile closed paths, and Formula Lab continuously
scrolls two closed copies of its recorded before/after trace. Categorical
outputs such as breathing phase remain integer-stepped rather than acquiring
invented intermediate values. Reduced-motion users receive the same closed
trace as a static view instead of forced animation.

## 2026-08-15 — Completed edits synchronize source, desktop install, and live Pages

The user-visible application has three delivery surfaces: canonical source,
the resolved per-user desktop installation, and the public Pages app at
`https://georgefejer91.github.io/Polar-Stream/`. A completed edit must leave all
three on the same accepted UI state. The delivery workflow therefore includes
production build, rollback-preserving local install and smoke test, reviewed
publication to `main`, Pages deployment, and live manifest/hash verification.
Every edit handoff includes the public URL. A branch-only, staged-only, or
locally installed change is incomplete; unrelated dirty work is never swept
into a deployment to meet this rule.

## 2026-08-15 — Previews come from one real recording; formulas are bounded native outputs

`apps/polar-stream/ui/data/preview-recording.json` is the sole hardware-free
preview signal: an anonymized 60-second real H10 ECG/ACC recording. Browser
replay and every metric preview derive from it, and the generated preview asset
records its hash. NeuroKit may be used for offline cleaning/method provenance,
but no simulated signal is a shipped fallback.

The metric catalog remains the Rust source of truth for labels, scientific
context, citations, formulas, and executable templates; Pages consumes a
generated catalog asset. Formula Lab retains sensor/event time automatically
and evaluates a scalar y-value from exactly one source clock (`ecg`, `x/y/z`,
`hr`, or `rr`). Expressions have no assignment, loops, strings, or user-defined
functions and are bounded by formula count, expression/AST depth, operations,
and retained state. Specialized spectral or multi-stage metrics receive honest
mathematical definitions without a fake one-line executable template.

## 2026-08-14 — Frontend controls do not disappear with backend capability

Tauri and GitHub Pages use one canonical frontend, including the same control
placement, responsive formatting, and interaction flows. Runtime adapters may
produce different results only where platform capability genuinely differs.
The missing capability must not be handled by removing or disabling the shared
control: an attempted unsupported action fails closed and presents a specific
error plus a useful next step.

Accordingly, Pages continues to display interactive LSL and OSC toggles. A
browser attempt leaves the toggle off and shows that native publication requires
the installed app, with a link to the latest GitHub release. Pages still does
not publish, relay, or rename another transport as LSL/OSC. This supersedes only
the control-hiding portion of the 2026-08-13 self-contained-browser decision;
the no-companion and native-protocol boundaries remain in force.

## 2026-08-14 - Browser Bluetooth compatibility is capability-based

The Pages adapter does not admit or reject browsers by brand. It checks the
secure context and `navigator.bluetooth.requestDevice`, then lets the chooser
remain authoritative and diagnoses `bluetooth` Permissions Policy rejection if
it occurs. Browser names remain guidance only because Chromium forks can
independently ship, gate, or remove Web Bluetooth. A browser-level or
administrator-level block cannot be repaired by site code.

The H10 chooser must be the first awaited browser operation in the click/touch
connection path so transient user activation survives stricter Chromium
implementations. After selection, the adapter may retry one transient
`NetworkError`/`AbortError` from the initial GATT connection before any service
subscription exists. PMD control writes prefer `writeValueWithResponse` and
retain the older `writeValue` fallback. These changes improve compatibility
without retrying the chooser, duplicating subscriptions, or weakening the
foreground-only and physical-validation boundaries.

## 2026-08-14 — Local CSV is bounded and independent of native publication

The shared Output panel exposes one **Save local CSV** toggle in Tauri and
Pages. Native H10 recording belongs to `polar-h10-output`: immediate selected
LSL/OSC publication happens first, then a decoded notification is copied with a
non-blocking `try_send` into a dedicated 128-batch CSV writer. Formatting and
filesystem I/O never enter the sensor path. A full/disconnected queue or writer
error stops CSV and reports a best-effort UI warning; it may not grow, block, or
silently discard accepted rows. Native files include all received raw ECG/ACC,
HR/RR, and every derived metric produced by active processors.

This supersedes the selected-output CSV scope recorded in the 2026-08-13 Pages
decision below; the row and memory bounds remain in force.

Pages retains a separate 300,000-row in-tab recorder because a hosted page does
not have Rust filesystem authority. It now records every received raw row and
every metric event the browser produces, independent of which outputs are
selected for visualization. Turning the toggle off uses the initiating gesture
to download the file. Browser capture remains vulnerable to tab lifecycle and
must be downloaded before the page is lost.

## 2026-08-14 — Audio data is an experimental cable modem, not encryption

The shared frontend may emit compact raw/metric batches as 22.05 kbit/s stereo
Manchester PCM with a preamble, sequence number, and CRC32. This rate is chosen
to fit nominal 130 Hz signed-24-bit ECG plus 200 Hz three-axis signed-16-bit ACC
and framing over clean stereo AUX/digital loopback. CRC detects corruption; the
format has no forward error correction, confidentiality, authentication, or
tamper protection and must never be described as encryption.

The implementation remains in the shared Web Audio layer and is bounded to a
1.25-second scheduled horizon. It is not authoritative in Tauri because the
bounded display channel may omit events. Native CSV/LSL/OSC remain the reliable
paths. `scripts/decode_audio_data.py` is the public stereo-PCM-WAV reference
decoder and browser validation must round-trip the production encoder through
it. Speaker/microphone and mono/lossy paths remain outside the claim.

## 2026-08-14 — Browser H10 is foreground-only despite wake-lock protection

Chrome on Android can grant a visible secure page direct BLE GATT access, so
Pages requests a best-effort screen wake lock after H10 connection and
reacquires it when the document becomes visible again. This does not authorize
or technically provide background capture. Web Bluetooth is unavailable in Web
Workers/service workers, active screen locks are released for inactive/hidden
documents, and mobile Chrome may freeze or discard a page without a final
callback. Switching apps/tabs or locking the screen can therefore interrupt
Bluetooth, CSV, and audio.

The UI warns after returning from a hidden browser-BLE session and sensor
timestamps remain the gap evidence. A guaranteed smartphone locked-screen or
other-app-foreground workflow is a separate native Android foreground-service
feature, not something GitHub Pages or PWA installation can promise.

## 2026-08-13 — Pages records and shares browser events without claiming LSL

The self-contained Pages path exposes every exact browser input batch through a
same-tab `polar-stream-data` event and same-origin `polar-stream-live-v1`
`BroadcastChannel`. It also records the currently selected outputs to a local
CSV before visualization decimation. Raw units are retained and device sample
times are reconstructed from PMD frame timestamps when present.

This path is implemented from scratch in the canonical UI and requires no
localhost service, remote relay, browser extension, or installed wrapper. Its
in-memory file is capped at 300,000 rows, stops visibly at capacity or input
disconnect, and must be downloaded before the tab closes. The browser event,
BroadcastChannel, and CSV are not LSL: native discovery and LabRecorder
interoperability remain installed-app capabilities because a normal Pages tab
cannot open LSL's multicast UDP and general TCP/UDP sockets.

## 2026-08-13 — GitHub Pages is a self-contained browser application

This supersedes the paired browser-to-native LSL decision below. The browser
workflow must require only the loaded Pages application and the browser's own
Web Bluetooth permission. H10/mock acquisition, browser-supported processing,
and visualization stay in the tab. Pages must not call a localhost companion,
installed wrapper, or remote relay, and it must not rename WebSocket/HTTP output
as LSL.

Ordinary hosted pages cannot open the raw UDP discovery/multicast and TCP/UDP
sockets required by native LSL and OSC. Their destination controls are therefore
hidden in browser mode rather than shown disabled or conditionally paired.
Native LSL/OSC remain available in the separately installed Tauri app. The
removed companion remains documented below only as superseded history.

## 2026-08-13 — Paired browser sessions may publish through native LSL

This supersedes the earlier blanket rule that Pages must always disable LSL.
An ordinary page still disables LSL and OSC because browser tabs cannot open
LSL discovery/data or UDP OSC sockets. The installed app may now explicitly
start an ephemeral IPv4-loopback companion and open the canonical Pages URL
with a random 128-bit bearer token in the fragment. The page removes the
fragment immediately, keeps the token in memory, and must pass exact
origin/Host/token checks plus Chromium Local Network Access before a dedicated
native `OutputRouter` accepts bounded ECG, ACC, or metric batches. Each desktop
launch replaces the prior pairing, and the first authenticated random browser
client identifier exclusively owns the new session. Browser OSC remains
unavailable.

The bridge is a separate browser acquisition route, never a detour for native
H10 data. Its page queue is bounded and observable, the server enforces request
and concurrency limits, and errors disable publication rather than grow or retry
without limit. It adds serialization, a 20 ms flush window, HTTP scheduling, and
receipt-time LSL stamping, so direct native acquisition remains preferred. The
current companion is same-computer only; a phone cannot reach a desktop's
loopback listener.

## 2026-08-13 — Port MesmerPrism Windows GATT access, not a forced MTU

MesmerPrism's relevant durable Windows fix is in
`WindowsGattServiceHandle.GetCharacteristicAsync`: call
`GattDeviceService.RequestAccessAsync`, discover uncached, and retry short-lived
`AccessDenied`/`Unreachable` states. Polar Stream primes the PMD service and its
control/data characteristics with the same bounded three-attempt pattern before
ordinary `btleplug` discovery. The Windows 11 `ThroughputOptimized` request is
also moved immediately after connection.

This does not supersede the MTU decision below. MesmerPrism's Windows
`RequestMtuAsync` only returns read-only `GattSession.MaxPduSize`; neither repo
can force a smaller ATT MTU through WinRT. Physical Windows H10 evidence is still
required before claiming an applied connection interval or loss improvement.

## 2026-08-13 — Windows MTU is observed; connection timing is requested

Do not describe Windows as allowing Polar Stream to force an ATT MTU. WinRT
owns MTU negotiation and exposes `GattSession.MaxPduSize` read-only. The native
Windows 11+ path instead makes a best-effort `ThroughputOptimized` preferred
connection-parameter request through `btleplug`, then reports the observed
connection interval, peripheral latency, and negotiated MTU. The request may be
unavailable or rejected, especially on older Windows versions, and must fail
soft without stopping ECG/ACC acquisition. Cross-platform CI is not physical
H10 validation; preserve device-run diagnostics before claiming the shorter
interval was applied.

## 2026-08-13 — Desktop and browser use one canonical interface

GitHub Pages is a presentation demo of the Tauri application, not a separately
designed website. Pages artifacts are staged from `apps/polar-stream/ui/`, and
the same HTML, CSS, application logic, metric library, and visualizations ship
in both targets. At the time, a runtime adapter selected native IPC or a local
NeuroKit mock. The 2026-08-15 recorded-preview decision supersedes that mock
signal with the canonical real fixture while preserving isolation from native
acquisition/publication. CI must exercise the
staged Pages artifact at desktop and smartphone widths so visual feedback on
the hosted version remains applicable to the desktop interface.

The direct-H10 adapter remains experimental. Web Bluetooth is available only in
compatible secure Chromium contexts and requires an explicit chooser. Recorded
preview replay is the hardware-free path and claims neither native timing,
LSL/OSC transport, nor scientific validation.

## 2026-08-13 — Browser H10 input is experimental and local to the tab

GitHub Pages may acquire H10 ECG/ACC and standard HR/RR directly through Web
Bluetooth. The adapter mirrors Polar PMD commands/decoding and only the two
retained ACC breathing processors so the canonical interface can receive live
data without a local app. It is feature-detected, visibly permission-gated, and
must retain the hardware-free recorded preview. Physical-H10 verification is required before a
release claim.

Ordinary browser tabs cannot provide the native socket behavior used by LSL
discovery/data or UDP OSC. Pages therefore disables both destinations instead
of simulating publication. Tauri remains the authoritative acquisition and
publication path. Browser code must not be imported into the Rust hot path.

## 2026-08-13 — ACC breathing provenance and parameters are explicit

`docs/acc-breathing-handoff.md` is the canonical handoff for the two
experimental ACC breathing outputs. It distinguishes verbal Johannes
provenance, public MesmerPrism/PolarH10 code history, and Polar Stream's Rust and
browser adaptations. The add/adjust UI exposes all current experiment controls,
while fixed constants and the known notification-batch-dependent phase
threshold are documented. No tuning setting may be described as validated
until comparison with an independent respiratory reference supports it.

## 2026-08-13 — New ACC breathing selection is deliberately limited

The output library separates red ECG-derived outputs from blue accelerometer
outputs. New ACC breathing selection exposes only two independent scalar
streams: a continuous selected-axis projection in g (optionally normalized by
the output layer) and a three-state phase class (`+1`, `0`, `-1`). They share
native axis and smoothing settings, default to X + Z without rotational Y, and
remain labeled unvalidated. Legacy breathing telemetry stays catalogued only so
saved configurations can migrate safely; it is not offered as a new library
choice. The phase circle is presentation-only and derives its inertial,
asymptotically bounded motion from phase values rather than a hidden volume
stream.

## 2026-08-13 — The dedicated repository is public

`GeorgeFejer91/Polar-Stream` is publicly readable. Keep credentials,
participant-identifying data, recordings, local preferences, signing material,
and unpublished research data out of Git history; public visibility makes the
existing data-minimization and secret-scanning rules release-critical.

## 2026-08-13 — Startup requires the Tauri-Rust developer skill

Every future repository agent must invoke the installed `tauri-rust developer`
skill before substantive work. If unavailable, the agent must disclose that
fact and use the explicit repository fallback without pretending the skill ran.
This keeps specialized Tauri/Rust review routine while leaving an auditable path
for environments where the personal skill is not installed.

## 2026-08-13 — Signal integrity precedes presentation

The engineering priority order is raw signal integrity, observable publication,
latency and jitter, cross-platform correctness, derived metric quality, then UI
presentation. Raw publication therefore precedes optional derived processing,
and bounded display loss may never become sensor or output backpressure.

## 2026-08-13 — Canonical AI context lives in `for-ai/`

The root `AGENTS.md` is a short bootstrap that requires future repository-aware
agents to read `for-ai/`. Detailed context lives in one discoverable folder so
instructions, goals, current state, and handoff practices can evolve without a
large root file.

## 2026-08-13 — Raw ACC uses one stacked visualization

Raw acceleration is one three-channel sensor output. The UI therefore exposes a
single raw-ACC visualizer and shows X, Y, and Z in stacked lanes with distinct
colors and independent zero-centered display ranges. This removes unnecessary
axis switching while retaining sign and per-axis readability.

## 2026-08-12 — Rust core with a Tauri system-WebView shell

Rust owns protocol, input, metrics, output, and application coordination. Tauri
provides an HTML-driven UI without bundling another browser engine. The WebView
is never placed on the authoritative acquisition/publication path.

## 2026-08-12 — One canonical output name across transports

Metric metadata defines one stable suffix. LSL uses the resulting full name and
OSC uses that same name with a leading slash. Transport-specific naming drift is
not allowed.
