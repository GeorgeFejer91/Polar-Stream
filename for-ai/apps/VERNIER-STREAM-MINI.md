# Vernier Stream Mini

Read this file when the user names Vernier Stream Mini, Vernier Mini, or the
Vernier mini node. Also read it for shared `stream-mini-runtime` work.

- Product: `Vernier Stream Mini`
- Source: `apps/vernier-stream-mini/`
- Shared one-session runtime: `crates/stream-mini-runtime/`
- Bundle identifier: `dev.georgefejer.vernierstreammini`
- Executable: `vernier-stream-mini`
- Release prefix: `Vernier-Stream-Mini_*`
- Windows install root: `%LOCALAPPDATA%\Vernier Stream Mini\`

This is one autonomous Vernier BLE-to-LSL process. It owns its own app-config
directory, `preferences.json`, BLE session, LSL outlets, UI state, executable,
icon, and installer. Launching another node starts another copy of this same
executable. It must not depend on the multi-streamer or Polar Mini process.
Its opt-in **Start with PC** switch owns only this product's operating-system
autostart entry. On every launch, the app restores its app-local stream name,
output mode, reconnect preference, and last successfully connected Go Direct
device; reconnect is enabled by default and may be disabled explicitly.
After unexpected BLE loss, keep the configured LSL outlets advertised, publish
a loss marker, and retry the saved device by ID then name on a bounded cadence.
Publish a restoration marker only on a successful reconnect; never generate
filler sensor rows. Single mode has sparse `signal_state` values 1=lost,
2=restored; separate mode owns a persistent `<base>-signalStatus` Markers
outlet with the same codes. Manual Disconnect cancels retry and closes the
outputs. Manual search shows all compatible devices and prefers the saved one.
The stream-mode switch and name field must replace active LSL outlets immediately
without restarting BLE acquisition; preserve the discovered channel schema
and keep the previous healthy outlets until replacements are ready. The
Outputs picker persists any nonempty subset of `rawVernier`, `rawForce`,
`vernierBreathing`, and `signalStatus`; all four are selected by default.
Separate mode opens only the selected outlets. Single mode opens one sparse
outlet containing only the selected columns. Apply changes live using the
same staged outlet replacement as the mode switch, without reconnecting BLE.
`rawVernier` means all numeric channels actually advertised by the belt plus
host recording diagnostics. The documented GDX-RB channels are Force,
Respiration Rate, Steps, and Step Rate; a raw accelerometer is not documented.
`rawForce` is an unfiltered compatibility copy of Force. `vernierBreathing`
is the app's 30-second relative force normalization, not lung volume or a
device respiration-rate channel. `signalStatus` is the loss/restoration marker.
Keep this distinction documented on the GitHub Pages Mini download surface.
The operating system is authoritative for autostart state, while
`preferences.json` is authoritative for the connection defaults.
The device-search dialog may query the system Bluetooth radio and request
radio-control permission only after the user operates its switch. The NSIS
installer cannot pregrant that permission; honor Windows denial, hardware
blocks, and the actual reported radio state. Do not mislabel a BLE scan error
as proof that the radio is off.

The visible **Mock** action launches the same executable as a separate
`--mock` process. That process must bypass Bluetooth, inherit a snapshot of the
current stream contract into memory only, clear remembered-device/autoconnect
state, add a PID suffix to the stream base, and immediately publish
deterministic 20 Hz Force data plus derived breathing through the production
output router. A mock process must never modify the physical app's
`preferences.json` or collide with another instance's LSL names.

Keep the UI compact, frameless, transparent outside its fully rounded node
shell, free of side port knobs, and draggable from every non-control descendant
surface. Buttons, inputs, selects, labels, dialogs, and menus must retain their
ordinary pointer behavior. Use one
accessible switch with app-local SVG miniatures for separate versus single LSL
output. Keep custom minimize and graceful-close controls keyboard accessible.
Keep the saved light/dark toggle and use the app-specific Vernier teal, orange,
and white visual identity in both modes across the UI, stream-mode miniatures,
desktop icon, shortcut, and installer assets. The icon must preserve the
original Polar Stream circular stream/keyhole geometry without an extra badge;
change only its colors to Vernier teal, orange, and white. Wrap the rounded
shell in three crisp 1 px lines ordered outside-in as teal, orange, and white.
Tint mock windows with the orange accent. Animate the three rings with a slow
breathing glow only after sample counters advance while LSL health reports
`Publishing`; connection alone is insufficient. Honor reduced-motion preference
with a static live glow. Publish the complete compatible raw channel set before
derived breathing or UI work.

Acquisition rate is not configurable. Request the GDX-RB's documented 50,000 us
period, or 20 Hz, by default. Keep the existing per-channel metadata validation
and use a slower common fallback when the connected schema cannot accept that
period; never synthesize a higher rate.
Low-latency behavior is internal: publish each decoded notification immediately,
preserve host receipt time and period-derived row timing, request
throughput-oriented link scheduling only where the backend supports it, and
report the negotiated MTU/interval rather than assuming the 50 ms sensor period
is the BLE delivery cadence. Windows leaves connection timing system-managed
and exposes its negotiated MTU read-only.

For a completed Vernier Mini edit, run its focused JS/Rust checks, build its own
installer, install it into its own product directory, and launch-test it beside
Polar Mini. Pages publication is not a Vernier Mini gate unless the download
catalog itself changed.
