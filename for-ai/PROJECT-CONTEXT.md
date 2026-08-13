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
     -> polar-h10-output -> LSL / OSC
     -> bounded display queue -> JavaScript ring buffers -> Canvas 2D
```

Native publication is the authoritative real-time path. The WebView is a
display consumer and must not create backpressure.

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
  preview assets; no Python runtime ships with the app.

## Important contracts

- One normalized base name produces stable per-output suffixes such as
  `participant_07_rawECG` and `participant_07_rawACC`.
- LSL and OSC share the same canonical discoverable output name.
- Configuration and remembered-device preferences have one schema-versioned
  native owner.
- Scientific definitions originate in the Rust metric catalog and are surfaced
  to the UI through bootstrap data.

## Repository and releases

- Canonical repository: `https://github.com/GeorgeFejer91/Polar-Stream`
- The repository is public as of the current state snapshot.
- Tagged releases use GitHub Actions to build platform packages and remain draft
  until the package matrix and launch checks are complete.
