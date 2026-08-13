# Decision log

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
