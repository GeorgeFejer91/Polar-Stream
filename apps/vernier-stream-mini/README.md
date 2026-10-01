# Vernier Stream Mini

Standalone one-device Vernier Go Direct BLE-to-LSL applet. The UI is
intentionally a compact transparent-shell window where the detached Polar
Stream node card is the visible program outline, not the main controller
workspace.

- Default outputs: all four independently selectable LSL outputs, `rawVernier`
  (every advertised numeric belt channel plus recording diagnostics),
  `rawForce` (unfiltered Force compatibility copy), `vernierBreathing` (our
  relative 0-1 force normalization), and `signalStatus` (loss/restoration
  markers). At least one output remains selected. Choices persist and replace
  active outlets without restarting Bluetooth.
- The documented GDX-RB device channels are Force, Respiration Rate, Steps,
  and Step Rate. Vernier does not document an exposed raw accelerometer channel
  on this belt. Its Force transducer measures tension in a short strap connected
  to the box, not air pressure or lung volume. See the
  [Vernier manual](https://www.vernier.com/manuals/GDX-RB) and the
  [Mini signal reference](https://georgefejer91.github.io/Polar-Stream/download/#vernier-signals).
- Modes: canonical separate LSL streams or one sparse fixed-channel LSL stream.
- For exact force/breathing sample timing, use separate streams. The single
  sparse mode preserves acquisition order and clamps overlapping channel
  timestamps to the next microsecond; it does not add full sample periods or
  drift into the future.
- Changing mode or stream name replaces active LSL outlets without disconnecting
  the sensor or pausing its sample worker. If replacement fails, the previous
  healthy outlets continue publishing.
- Startup: opt-in **Start with PC** registration plus default automatic
  reconnection to the last successfully connected Go Direct device.
- Device search shows the Windows system Bluetooth state and offers a radio
  switch. The app requests radio-control access when the switch is used;
  Windows may prompt or deny it. The NSIS installer cannot pregrant it, and
  hardware or policy blocks remain under Windows control.
- Memory: app-local stream name, output mode, output selection, reconnect
  preference, and last Vernier device.
- Multi-device use: launch multiple app instances.
- Mocking: **Mock** launches an independent, automatically streaming applet
  with deterministic 20 Hz force/breathing data and real LSL publication. Its
  PID-suffixed stream name and settings are session-only.
- Without a connected Go Direct sensor, the regular app does not publish
  invented measurements; use the clearly labeled Mock window for synthetic data.
- Live state: after samples are flowing through a healthy LSL outlet, the three
  outline rings emit a restrained breathing beacon; connected-only state does
  not animate.
- Logo: `icons/app-icon.svg`, preserving the original Polar Stream geometry and
  changing only its palette to Vernier teal, orange, and white.
