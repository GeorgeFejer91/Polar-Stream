# Reusable Rust crates

The new stream application is intentionally split at its data boundaries:

| Crate | Responsibility | Must not depend on |
| --- | --- | --- |
| `polar-h10-core` | PMD/HR decoding, commands, small rolling metrics | Bluetooth, UI, LSL, OSC |
| `polar-h10-input` | BLE scan, connect, subscribe, and typed input events | Tauri, visualization, outputs |
| `polar-h10-metrics` | Stateful ECG, HRV, coherence, breathing, dynamics, and experimental processors; metric metadata | Bluetooth, Tauri, LSL, OSC |
| `polar-h10-output` | Output selection and native LSL/OSC publishing | Bluetooth, Tauri, visualization |

`apps/polar-stream` is only an adapter between these crates and its HTML UI.
The boundaries allow acquisition, metrics, and output destinations to evolve
without making the interface or one transport responsible for the others.
