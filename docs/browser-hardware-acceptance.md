# GitHub Pages H10 Acceptance

This procedure is the release-blocking browser test for Polar Stream. A pass
must use the exact public site at
<https://georgefejer91.github.io/Polar-Stream/> in an ordinary Brave tab on the
Motorola phone. Localhost, Tauri, ADB, remote debugging, fake GATT, and the
NeuroKit input cannot produce a hardware pass.

Record the exact deployment before touching the phone:

```bash
npm run verify:live-pages
```

The command downloads the live manifest and every declared asset, checks each
SHA-256 against both the manifest and the canonical UI in this checkout, rejects
remote acquisition primitives, records GitHub's response headers and
Last-Modified values, and writes ignored JSON evidence under
`artifacts/real-world-pages/`.

## Isolation

1. Stop every Polar Stream preview, Tauri process, and companion service.
2. Do not run an ADB session, USB connection, remote-debugging port, WebSocket
   server, or private-network relay during acquisition.
3. Disconnect the phone from USB. Disable Wi-Fi and VPN and use cellular data.
4. Close Polar Beat, Polar Flow, and every other possible H10 Bluetooth consumer.
5. Clear the GitHub Pages site's Brave storage so the page and chooser permission
   are fresh.

Other unrelated computer processes do not participate in the test, but the
public page must not make any request to them. The provenance verifier confirms
the deployed JavaScript has no HTTP, WebSocket, SSE, or private-address
acquisition path.

## Physical Run

1. Wear and moisten the H10. Start a phone screen recording that shows the Brave
   address bar and exact public URL.
2. Confirm LSL and OSC are absent. Press **Choose Polar H10** and select the
   physical strap in Android's chooser.
3. Confirm connected state, ECG and ACC activity within five seconds, finite HR
   and RR values, and an increasing sample counter. Battery is optional.
4. Enable **Save local CSV** and keep Brave visible in the foreground for at
   least two uninterrupted minutes.
5. Disable **Save local CSV** to download the recording. Disconnect, reconnect
   through Android's chooser, and observe another 30 seconds of data. First data
   must arrive within five seconds and the sample counter must not advance at a
   doubled rate.
6. End the browser session before transferring evidence. Keep the recording
   local; do not commit the CSV, screenshot, or screen recording.

If the chooser is empty, reactivate the worn strap, close competing apps, toggle
phone Bluetooth once, and retry. Cancelling the chooser must return quietly to
ready state and is not a pass. Preserve exact screenshots for GATT, malformed
frame, disconnect, or recorder failures. NeuroKit may be run from the public
site only to separate a UI/recorder defect from Bluetooth; it never counts as
physical evidence.

## Offline Analysis

After the browser session has ended, attach the primary CSV and final connected
screenshot to the Codex task or analyze the file directly:

```bash
python3 scripts/analyze_browser_recording.py /absolute/path/to/recording.csv \
  --json-output artifacts/real-world-pages/h10-csv-report.json
```

The analyzer requires `input_kind=web-bluetooth`, a Polar H10 source, positive
strictly increasing sensor timestamps, raw ECG and three-axis ACC, at least 118
seconds of sensor-time coverage for each raw stream, 129-131 Hz ECG, 199-201 Hz
ACC, estimated loss below 0.1%, no sensor or foreground host gap above one
second, finite raw values, plausible HR/RR, and a normal user/export stop.

A CSV pass is not the complete hardware pass. The screen recording or connected
screenshot and manual reconnect observation still establish chooser identity,
the ordinary public Brave context, absence of runtime errors, and clean
resubscription.
