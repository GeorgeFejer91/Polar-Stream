# Polar Stream Mini

Read this file when the user names Polar Stream Mini, Polar Mini, or the Polar
mini node. Also read it for shared `stream-mini-runtime` work.

- Product: `Polar Stream Mini`
- Source: `apps/polar-stream-mini/`
- Shared one-session runtime: `crates/stream-mini-runtime/`
- Bundle identifier: `dev.georgefejer.polarstreammini`
- Executable: `polar-stream-mini`
- Release prefix: `Polar-Stream-Mini_*`
- Windows install root: `%LOCALAPPDATA%\Polar Stream Mini\`

This is one autonomous Polar BLE-to-LSL process. It owns its own app-config
directory, `preferences.json`, BLE session, LSL outlets, UI state, executable,
icon, and installer. Launching another node starts another copy of this same
executable. It must not depend on the multi-streamer or Vernier Mini process.
Its opt-in **Start with PC** switch owns only this product's operating-system
autostart entry. On every launch, the app restores its app-local stream name,
output mode, metric selection, reconnect preference, and last successfully
connected H10; reconnect is enabled by default and may be disabled explicitly.
After unexpected BLE loss, keep active LSL outlets advertised, publish a loss
marker, and retry the saved H10 by ID then name on a bounded cadence. Publish a
restoration marker only on a successful reconnect. Never generate filler
ECG/ACC/HR measurements. Single mode uses sparse `event_kind` rows 20=lost,
21=restored; separate mode owns a persistent `<base>-signalStatus` Markers
outlet with 1=lost, 2=restored. Manual Disconnect cancels retry and closes the
outputs. Manual search shows all discovered H10s and prefers the saved one.
Its full bounded scan must not inherit the main controller's one-new-H10
early-stop behavior.
Changing the stream mode, name, or optional metrics during a live session must
replace the LSL outlets immediately without restarting BLE acquisition; keep
the previous healthy outlets until the replacements are ready.
The operating system is authoritative for autostart state, while
`preferences.json` is authoritative for the connection defaults.

The visible **Mock** action launches the same executable as a separate
`--mock` process. That process must bypass Bluetooth, inherit a snapshot of the
current stream contract into memory only, clear remembered-device/autoconnect
state, add a PID suffix to the stream base, and immediately publish
130 Hz ECG from the bundled 60-minute NeuroKit2 ECGSYN prerecording, plus
synthetic 200 Hz ACC, HR/RR, and selected derived metrics through the
production output router. Replay the ECG recording from its beginning and loop
after one hour; never synthesize an ECG fallback if the fixture is absent or
invalid. The generator is `scripts/generate_polar_mini_mock_ecg.py`; the
fixture belongs only in the Polar Mini installer, with no runtime Python or
NeuroKit dependency. A mock process must never modify the physical app's
`preferences.json` or collide with another instance's LSL names.

Keep the UI compact, frameless, transparent outside its fully rounded node
shell, free of side port knobs, and draggable from every non-control descendant
surface. Buttons, inputs, selects, labels, dialogs, and menus must retain their
ordinary pointer behavior. Use one
accessible switch with app-local SVG miniatures for separate versus single LSL
output. Keep custom minimize and graceful-close controls keyboard accessible.
The compact node labels its optional-output action **+ Realtime metrics**. Its
larger picker groups optional outputs into ECG/heart-derived and ACC-derived
families, gives each exact catalog metric name/ID a one-line explanation, and
links to the full metric library on the public Polar Stream Pages app. Keep
that picker descriptive only: the canonical metric catalog and processors stay
in Rust, and raw ECG/ACC/HR/RR remain always included.
Checking or unchecking an optional metric saves that selection immediately as
the next-launch default and applies it to any active LSL outlets. The picker
has a **Reset metrics** action that clears every optional selection; it never
removes the four direct outputs or resets stream name, mode, or device memory.
The optional `phan_breath_event` and `phan_breath_rate` outputs adapt Lyn
Phan's phone-on-abdomen ACC threshold protocol to H10 source-time samples.
They are mini-selectable but non-release in the main controller catalog. Keep
their short/long windows at 0.2/200 s, threshold slope at 0.0003 g/s, quiet
gate at 1 s, refractory delay at 0.25 s, and rate window at 60 s. Reset on
source gaps/clock changes; never claim chest-strap physiological validation.
See `docs/phan-acc-breathing.md` for LSL names and evidence limits.
Mini also exposes every catalogued ACC breathing and breathing-dynamics
compatibility output, while the main controller's release picker stays small.
The opt-in Flowborne phase/score adapt Viscereality's signed 24/180-frame
controller motion comparison to the H10's calibrated ACC projection using
source-time windows of 24/90 and 180/90 seconds. Phase codes are +1 inhale,
-1 exhale, 0 hold, and -2 bad signal. This is acceleration contrast, not
controller position; never describe its thresholds as metres or validated
respiratory phase. Keep source gaps, unready calibration, and quality loss
explicit. See `docs/acc-breathing-methods.md` for exact outputs and limits.
Keep the saved light/dark toggle and use the app-specific black, white, and
Polar red visual identity in both modes across the UI, stream-mode miniatures,
desktop icon, shortcut, and installer assets. The icon must preserve the
original Polar Stream circular stream/keyhole geometry without an extra badge;
change only its colors to Polar red, black, and white. Wrap the rounded shell
in three crisp 1 px lines ordered outside-in as Polar red, black, and white. Raw
publication stays ahead of derived metrics and UI work. Tint mock windows with
a lighter Polar accent. Animate the three rings with a slow breathing glow only
after sample counters advance while LSL health reports `Publishing`; connection
alone is insufficient. Honor reduced-motion preference with a static live glow.

Acquisition rate is not configurable. Request the H10's documented native
maxima by default: ECG at 130 Hz and accelerometer at 200 Hz. Heart rate/RR
remains the device's event-driven 1 Hz service rather than being resampled.
Low-latency behavior is also internal: publish every received PMD notification
immediately with device-derived sample timing, request throughput-oriented link
scheduling only on backends that expose it, and report the negotiated MTU rather
than assuming a request succeeded. Windows keeps its physically proven
system-managed connection timing and read-only MTU; do not restore the earlier
`ThroughputOptimized` request that stopped sustained H10 notifications.

For a completed Polar Mini edit, run its focused JS/Rust checks, build its own
installer, install it into its own product directory, and launch-test it beside
Vernier Mini. Pages publication is not a Polar Mini gate unless the download
catalog itself changed.
