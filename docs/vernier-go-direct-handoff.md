# Vernier Go Direct and multi-source handoff

Last updated: 2026-08-20

## Status and scope

Polar Stream now has an independent Go Direct implementation in Rust and a
matching Chromium Web Bluetooth adapter. No Respyra source code or GPL Go Direct
implementation is imported. The protocol shape was checked against Vernier's
official [godirect-js](https://github.com/VernierST/godirect-js) and
[Go Direct examples](https://github.com/VernierST/godirect-examples).

The implemented product profile is specifically the GDX-RB respiration belt.
Native and browser paths parse device identity, enumerate the advertised sensor
mask and metadata, require a confirmed GDX-RB model, and accept only channel 1
when it is a periodic Force sensor reporting N. They request a 100,000 µs (10
Hz) period when the metadata says it is valid, otherwise they use the sensor's
plausible typical period. Both paths wait for the start acknowledgement and a
valid force measurement before reporting the source as streaming.

Automated protocol, application, output, and emulated-browser tests pass. No
physical Go Direct device was available for this handoff, so radio behavior,
actual channel metadata, sustained loss, and end-to-end latency remain hardware
validation gates rather than completed claims.

The bounded native verifier exercises the shipped Rust input pool directly:

```powershell
python scripts/verify_gdx_rb.py
```

If several nearby GDX-RB belts make selection ambiguous, set
`POLAR_GDX_TARGET_NAME` to the exact chooser name for this one run. The wrapper
retains only structured, identifier-free JSON under
`artifacts/physical-gdx/<UTC>/native-gdx-verification.json`; the ignored
artifact records model/channel/unit, main firmware major/minor, battery,
configured period, startup and cleanup timing, stream continuity/rate, input
health, and reconnect evidence. It deliberately does not retain the device
name or Bluetooth identifier.

## Component boundaries

| Layer | Native desktop | Chromium browser |
| --- | --- | --- |
| Protocol | `crates/vernier-gdx-core` | `ui/vernier-web-bluetooth.js` |
| BLE ownership | `crates/vernier-gdx-input` using `btleplug` | Web Bluetooth GATT objects owned by the tab |
| Application routing | Tauri coordinator and per-source task | shared runtime event adapter |
| Raw output | per-source LSL, OSC, and bounded CSV | local CSV, same-tab event, and `BroadcastChannel` |
| Presentation | shared source descriptors, buffers, colors, and visualizers | same canonical UI |

The Rust protocol crate is platform-neutral and has no BLE dependency. The
input crate owns BLE lifecycle and translates decoded measurements into typed
events. The application remains responsible for source naming and output
fan-out. Browser JavaScript does not enter the native hot path.

## Go Direct protocol contract

The implementation uses the official Go Direct BLE UUIDs:

| Purpose | UUID |
| --- | --- |
| Service | `d91714ef-28b9-4f91-ba16-f0d9a604f112` |
| Command characteristic | `f4bf14a6-c7d5-4b6d-8aa8-df1a7c83adcb` |
| Response/measurement characteristic | `b41e6675-a329-40e0-aa01-44d2f444babe` |

Commands use the `0x58` frame marker, a decrementing rolling counter, the Go
Direct checksum byte, and 20-byte writes. Control responses are correlated by
both echoed command ID and rolling counter. A same-ID response with the wrong
counter cannot complete the waiter. Notifications may contain a fragment, one
frame, or several frames; the bounded accumulator reconstructs complete frames
and rejects invalid lengths.

The startup order is:

1. Discover the Go Direct service and both characteristics.
2. Subscribe to responses before sending commands.
3. Send initialization, status, and device-information commands.
4. Parse device metadata and require the GDX-RB model identifier/description.
5. Read the available-sensor mask, enumerate its sensor metadata, and require
   channel 1 to be periodic Force in N.
6. Request 100,000 µs when valid for that sensor, otherwise use its plausible
   typical period, then enable channel 1.
7. Wait for the correlated start acknowledgement.
8. Report connected only after decoding the first non-empty channel-1 batch.

Normal and wide periodic float frames, single/aperiodic float frames, and their
integer variants are decoded in Rust. Periodic frames are deinterleaved by
selected sensor; the packet count is interpreted as samples per selected
channel, matching the official JavaScript implementation rather than as total
float count.

## Parallel with the Polar H10 path

| Concern | Polar H10 | Vernier Go Direct | Shared Polar Stream rule |
| --- | --- | --- | --- |
| Discovery identity | exact H10 name/service evidence | Go Direct service plus parsed GDX-RB device identity | device ID is prefixed with input kind before UI routing |
| Setup | PMD settings/start responses | Go Direct command/counter responses | setup is bounded and fails closed |
| First-frame gate | valid ECG and ACC | valid selected force batch | no premature `connected` state |
| Batch semantics | H10 sensor timestamps and sample spacing | host receipt plus configured period | raw batches publish before derived/UI work |
| Session ownership | one owner/manager per H10 | one independent task/peripheral/decoder per GDX | no shared hot-path device lock |
| Backpressure | bounded event/UI queues | bounded event/UI queues | UI may drop display batches; acquisition cannot wait for the WebView |
| Diagnostics | staged input and output health | malformed/drop/high-water/decode-latency counters | loss and latency remain observable |
| Output identity | source-suffixed ECG/ACC/metrics | source-suffixed raw force | streams from different bodies/devices cannot collide |

The key parallel is architectural, not a claim that the wire protocols are the
same. Polar PMD carries device timestamps in measurement frames. Go Direct
periodic force frames do not provide an equivalent absolute timestamp in the
implemented measurement path. Polar Stream therefore records the host receipt
time once per Go Direct notification and backfills samples at the negotiated
period. The force LSL outlet advertises irregular rate (`0`) so metadata does
not falsely promise a fixed clock while explicit timestamps preserve the known
intra-batch spacing.

## Multi-device and color routing

The application admits at most eight simultaneous sources. A source receives a
stable slot (`source-1` through `source-8`) and the corresponding palette color:

`#00c2ff`, `#ffb000`, `#ff5c8a`, `#7bd88f`, `#b392f0`, `#ff7b54`,
`#58d6c7`, `#e5d85c`.

Each native source owns its input receiver, protocol state, metric engine, and
output router. The shared maps and configuration mutex are lifecycle-only; raw
publication does not take them. Stream bases gain the stable source suffix, for
example `participant_07_source-2_rawForce`. Source-specific output filtering
prevents an H10 from advertising an empty force outlet and prevents a Go Direct
device from advertising empty ECG/ACC or H10-derived outlets.

The UI keeps a separate circular buffer bank per source. Selecting a source
switches the values and visualizer without mixing samples. Its color marks the
source chip, raw cards, output cards, and visualizer frame/trace. ACC retains its
X/Y/Z axis colors inside the source-colored visualizer boundary.

Chromium can hold several Go Direct sessions, up to the same eight-source
bound. Web Bluetooth requires a fresh user-triggered chooser for each new
device, so a website cannot silently enumerate all nearby GDX sensors. Native
desktop discovery can return multiple Polar and Vernier candidates in one
application scan, although its two transport scans are sequential to avoid
radio/setup contention.

## Latency and reliability choices

- BLE notifications are consumed as delivered; no timer aggregation is added.
- Go Direct notification batches remain batches through decoding and native
  output, avoiding per-value channel overhead.
- Reliable setup uses characteristic writes with response. Streaming itself is
  notification-driven.
- Each Go Direct session has its own decoder and bounded 256-event queue.
- Cancellation races the complete BLE setup/stream future, not only the steady
  notification loop. Teardown uses non-blocking terminal events and a bounded
  peripheral disconnect; completed owners are pruned before later scan/connect
  admission so an unexpected link end cannot strand a stale slot.
- Data enqueue uses non-blocking admission and increments an explicit dropped
  batch counter instead of delaying the BLE notification task.
- The UI bridge has its own capacity-8 best-effort queue. A hidden or slow
  WebView can lose display updates without delaying LSL, OSC, or CSV.
- Every five seconds the native input reports notification/sample counts,
  malformed frames, dropped batches, queue high-water, and decode-latency
  p50/p95/p99/max.
- LSL, OSC, and CSV receive explicitly backfilled timestamps derived from the
  newest host receipt and the configured sample period.

The preferred 100,000 microsecond period is 10 samples/s. It is requested when
the sensor metadata declares it valid; otherwise the reported plausible typical
period is used. “Best possible latency” here means no avoidable application
batching or UI backpressure; Bluetooth connection
interval, controller scheduling, device firmware batching, and browser tab
scheduling remain outside application control.

## Chromium behavior and limits

Go Direct browser input is possible in Chromium through Web Bluetooth. It must
run in a [secure context](https://developer.mozilla.org/en-US/docs/Web/API/Web_Bluetooth_API),
normally HTTPS or localhost, and
[`requestDevice()`](https://developer.mozilla.org/en-US/docs/Web/API/Bluetooth/requestDevice)
must be initiated by a user gesture. The browser owns the chooser and grants
only the device selected by the user. Passive background detection and silent
multi-device enumeration are intentionally unavailable to page JavaScript.

The browser adapter mirrors the native setup and decoding contract, including
20-byte command chunks, command/counter correlation, fragmented response
reassembly, availability gating, configured sample period, and first-valid-
measurement connection gating. Its deterministic Playwright fixture verifies
the service filter, complete startup sequence, fragmented normal-force packet,
source color, live force display, and disconnect cleanup.

The coordinator keeps the saved output configuration in transport-free state.
It validates formulas with a destination-disabled router, then configures only
routers owned by connected sources. There is no legacy unsuffixed/global router
advertising empty LSL outlets alongside the source-suffixed streams.

A normal page cannot expose native LSL/OSC sockets. Browser force data therefore
uses the existing bounded local CSV and same-origin live-channel surfaces. Use
the installed desktop application when LabRecorder discovery or OSC is needed.
Foreground tab suspension, screen lock, and unsupported browser/OS combinations
can interrupt Web Bluetooth; the browser implementation is not a substitute for
a native background service.

## Validation completed

- `cargo test --workspace`
- `cargo test -p polar-h10-output --no-default-features --features rusty-lsl-backend`
- `cargo test -p polar-stream --no-default-features --features rusty-lsl-backend`
- JavaScript syntax checks and recorded-preview tests
- Rust-to-browser metric catalog parity
- deterministic interface validation including two colored sources
- staged browser-demo parity and Polar PMD + Vernier Go Direct Web Bluetooth
  acceptance with desktop/mobile responsive checks

These are software and emulated-device results. They do not prove physical Go
Direct compatibility or latency.

## Required physical gate

For each supported operating system and browser/native path:

1. Record device model, firmware, host adapter, OS/browser/app revision, channel
   number, and selected/configured period.
2. Connect at least two sources, including one Go Direct device, and confirm
   distinct source colors and source-suffixed outputs.
3. Save the native/browser raw recording while an independent reference client
   observes the device when a mutually exclusive BLE lease permits it.
4. Measure startup time, notification/sample counts, queue high-water, drop and
   malformed counters, decode latency, output timestamp monotonicity, gap size,
   and reconnect cleanup.
5. Repeat with the intended maximum concurrent source count and with the UI
   hidden/under load.
6. Do not publish a physical compatibility or latency percentile claim until
   the evidence artifact is retained and reviewed.

The native verifier covers the single-GDX discovery, exact GDX-RB channel-1
Force (N) contract, sustained primary stream, health thresholds, explicit
disconnect, and reconnect portions of this gate. It does not replace the mixed
multi-device, browser, under-load, independent-reference, or synchronized
H10/GDX qualification runs.

## Current limitations

- The product profile intentionally supports only a metadata-verified GDX-RB
  channel-1 periodic Force (N) sensor. Generic Go Direct devices and an
  arbitrary sensor picker remain future work and are rejected rather than
  mislabeled as respiration belts.
- Physical GDX-RB/native/browser validation is still pending.
- Browser source discovery is chooser-based by Web Bluetooth design.
- Go Direct absolute device-clock synchronization is not implemented; periodic
  samples use explicit host-receipt/backfilled timestamps.
- The optional default-off Rusty LSL backend is compile/test covered, but the
  ordinary multi-source app currently creates one router per source. Its older
  fixed-port one-registry/many-outlet qualification must be generalized before
  claiming multi-source Rusty-LSL runtime support. Production/default liblsl
  does not have that experimental registry constraint.
