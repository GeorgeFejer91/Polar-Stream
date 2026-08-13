# Decision log

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
in both targets. A runtime adapter selects native IPC or a deterministic local
NeuroKit mock input. That mock input is also selectable in the offline Tauri
app, allowing interface exploration without a Polar strap while remaining
isolated from real native acquisition and publication. CI must exercise the
staged Pages artifact at desktop and smartphone widths so visual feedback on
the hosted version remains applicable to the desktop interface.

The browser labels NeuroKit input synthetic and the direct-H10 adapter
experimental. Web Bluetooth is available only in compatible secure Chromium
contexts and requires an explicit chooser. It is not the portable fallback and
does not claim native timing, LSL/OSC transport, or scientific validation.

## 2026-08-13 — Browser H10 input is experimental and local to the tab

GitHub Pages may acquire H10 ECG/ACC and standard HR/RR directly through Web
Bluetooth. The adapter mirrors Polar PMD commands/decoding and only the two
retained ACC breathing processors so the canonical interface can receive live
data without a local app. It is feature-detected, visibly permission-gated, and
must retain NeuroKit fallback. Physical-H10 verification is required before a
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
