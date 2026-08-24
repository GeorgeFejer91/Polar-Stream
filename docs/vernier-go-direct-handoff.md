# Vernier Go Direct and multi-source handoff

Last updated: 2026-08-24

## Status and scope

Polar Stream now has an independent Go Direct implementation in Rust and a
matching Chromium Web Bluetooth adapter. No Respyra source code or GPL Go Direct
implementation is imported. The protocol shape was checked against Vernier's
official [godirect-js](https://github.com/VernierST/godirect-js) and
[godirect-py](https://github.com/VernierST/godirect-py) implementations plus
the [Go Direct examples](https://github.com/VernierST/godirect-examples). The
product-specific channel and signal semantics are checked against Vernier's
[GDX-RB channel inventory](https://www.vernier.com/til/16315) and
[respiration-belt manual](https://www.vernier.com/manuals/GDX-RB/).
The two Rust crates are direct workspace dependencies compiled into the Tauri
desktop binary. Native Vernier support requires no Python helper, browser shim,
proprietary SDK, plugin, or separately installed protocol module.

The implemented product profile is specifically the GDX-RB respiration belt.
The native path parses device identity and every channel in the advertised
sensor mask, requires a confirmed GDX-RB model plus periodic channel-1 Force in
N, and enables the complete compatible numeric channel set. Setup fails instead
of silently omitting a channel when metadata is malformed, unsupported,
duplicated, mutually exclusive, or has no common periodic base interval. The
native path requests a 100,000 µs (10 Hz) base period when all periodic channels
accept it, otherwise it uses a common plausible metadata fallback. It reports
connected only after the start acknowledgement and first valid measurement.

Native LSL then creates two additional per-device outlets before forwarding
that first measurement: `<base>_rawVernier` and
`<base>_vernierBreathing`. The former is the authoritative metadata-driven raw
recording stream; the latter is an explicitly derived, relative 0–1 belt-force
waveform. The Chromium adapter remains the narrower channel-1 Force path and
cannot publish native LSL. Its limitation is explicit rather than presenting a
browser-local substitute as the all-channel stream.

Automated protocol, application, output, and emulated-browser tests pass. A
pinned official pylsl 1.18.2/liblsl 1.17.7 consumer gate exactly matches both
new descriptors, channel metadata, Double64/Float32 formats, sparse values,
exact widened Int32 data, bounded derived samples, and advancing timestamps. On
2026-08-24, the native Windows/WinRT verifier also passed against a physical
GDX-RB running main firmware 5.3: exact channel-1 periodic Force (N), 70 primary
samples at 10.01 Hz with zero drop/malformed/nonfinite counts, explicit
disconnect, and 20 reconnect samples at 10.05 Hz. This is single-device native
evidence, not browser, mixed-source, cross-platform, long-run, respiratory-
agreement, or end-to-end latency qualification. That physical evidence predates
the all-channel outlet change and therefore does not by itself requalify the new
multi-channel acquisition mask or the two live physical LSL outlets.

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
| Raw output | per-source aggregate raw LSL plus force-compatible LSL/OSC/CSV | channel-1 force in local CSV, same-tab event, and `BroadcastChannel`; no LSL |
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
5. Read the available-sensor mask, decode every advertised sensor's metadata,
   require channel 1 to be periodic Force in N, and reject any unsupported,
   duplicate, or mutually exclusive all-channel schema.
6. Request 100,000 µs when every periodic sensor accepts it, otherwise select a
   common plausible metadata period, then enable every compatible channel in
   one measurement mask.
7. Wait for the correlated start acknowledgement.
8. Report connected only after decoding the first non-empty schema-valid frame;
   the ordered connection event installs both LSL outlets before that frame is
   published.

Normal and wide periodic float frames, single/aperiodic float frames, and their
integer variants are decoded in Rust. Periodic frames are deinterleaved by
selected sensor; the packet count is interpreted as samples per selected
channel, matching the official JavaScript implementation rather than as total
float count. Device Float32 and Int32 values are widened losslessly to Rust
`f64` for the aggregate LSL path; no normalization, carry-forward, interpolation,
or unit conversion occurs in the raw stream.

## Parallel with the Polar H10 path

| Concern | Polar H10 | Vernier Go Direct | Shared Polar Stream rule |
| --- | --- | --- | --- |
| Discovery identity | exact H10 name/service evidence | Go Direct service plus parsed GDX-RB device identity | device ID is prefixed with input kind before UI routing |
| Setup | PMD settings/start responses | Go Direct command/counter responses | setup is bounded and fails closed |
| First-frame gate | valid ECG and ACC | valid frame inside complete negotiated schema | no premature `connected` state |
| Batch semantics | H10 sensor timestamps and sample spacing | host receipt plus configured period | raw batches publish before derived/UI work |
| Session ownership | one owner/manager per H10 | one independent task/peripheral/decoder per GDX | no shared hot-path device lock |
| Backpressure | bounded event/UI queues | bounded event/UI queues | UI may drop display batches; acquisition cannot wait for the WebView |
| Diagnostics | staged input and output health | malformed/drop/high-water/decode-latency counters | loss and latency remain observable |
| Output identity | source-suffixed ECG/ACC/metrics | source-suffixed rawVernier and vernierBreathing, plus compatible rawForce | streams from different bodies/devices cannot collide |
| Discovery while active | remains acquired while Go Direct scans | remains acquired while Polar scans | scan only the missing protocol family; never pause the live source |
| UI protocol | ECG/ACC defaults, metrics, and Formula Lab | automatic rawVernier/vernierBreathing, force/breathing visuals, optional rawForce | no modules before connection; selected source switches the complete profile |

The key parallel is architectural, not a claim that the wire protocols are the
same. Polar PMD carries device timestamps in measurement frames. Go Direct
periodic force frames do not provide an equivalent absolute timestamp in the
implemented measurement path. Polar Stream therefore records the host receipt
time once per Go Direct notification and backfills periodic rows at the
negotiated period. Both new outlets advertise irregular rate (`0`) so metadata
does not falsely promise a fixed device clock while explicit timestamps preserve
known intra-batch spacing.

`<base>_rawVernier` uses a stable channel order sorted by Go Direct sensor
number. Its first channels are the exact device measurements with labels, units,
sensor IDs, numeric types, sampling modes, ranges, uncertainty, and period
metadata copied into the LSL descriptor. Seven trailing diagnostics expose row
sequence, input-queue rows dropped before the row, device drop reports, base
period, decode latency, monotonic host-receipt time, and frame encoding. Because
periodic and aperiodic channels update at different cadences, a device channel
absent from a native update is `NaN`; it is never filled from an older value.
The outlet uses Double64 so every device Float32 and Int32 value is represented
exactly.

`<base>_vernierBreathing` contains one Float32 channel labeled as derived. Force
increases on inhalation for the GDX-RB, so the causal processor maps increasing
force upward. It uses a bounded 30-second force history, min/max bounds while
warming up and 5th/95th percentiles after 20 finite samples, clamps every value
to 0–1, and holds the previous derived value for a non-finite input while the
unaltered input remains visible in raw LSL. This is relative belt effort, not
lung volume, airflow, or a clinical measurement.

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
application scan. The two advertisement scans run concurrently to avoid adding
their full scan windows together; connection/setup remains serialized per
selected device. On startup, an exact saved GDX-RB preference scans the Go
Direct transport directly and reconnects without waiting for a Polar scan.
After either family connects, the scan action targets only the missing family,
so the first input session and its output router continue publishing throughout
discovery and connection. With both families active, the UI source selector is
display-only: the two native receivers, processors, and source-suffixed output
routers continue independently.

## Latency and reliability choices

- BLE notifications are consumed as delivered; no timer aggregation is added.
- Go Direct notification frames remain frame-wide through decoding and native
  raw routing; LSL rows are formed only at the final transport boundary.
- Native setup selects the command characteristic's advertised write mode,
  preferring write without response and falling back to write with response
  only when supported. Streaming itself is notification-driven.
- After connection, the requested periodic Force measurement remains active
  until explicit disconnect. Vernier's public protocol implementations expose
  no keep-awake or remote-wake command; a sleeping, non-advertising belt still
  requires its physical button.
- The default-off **Keep Vernier connected / awake** option leaves that
  measurement subscription active and, after an unexpected link loss, performs
  saved-device Go Direct-only discovery with exponential retry delays bounded
  from 1.5 to 30 seconds. A deliberate Disconnect cancels retry. This is a
  connection policy, not a firmware wake command, and it can reconnect only
  while the belt advertises.
- Each Go Direct session has its own decoder and bounded 256-event queue.
- Cancellation races the complete BLE setup/stream future, not only the steady
  notification loop. Teardown uses non-blocking terminal events and a bounded
  peripheral disconnect; completed owners are pruned before later scan/connect
  admission so an unexpected link end cannot strand a stale slot.
- Data enqueue uses non-blocking admission and increments explicit dropped-batch
  and dropped-row counters instead of delaying the BLE notification task.
- The UI bridge has its own capacity-8 best-effort queue. A hidden or slow
  WebView can lose display updates without delaying LSL, OSC, or CSV.
- Every five seconds the native input reports notification/scalar/row counts,
  malformed frames, dropped batches, device drop reports, queue high-water, and
  decode-latency p50/p95/p99/max.
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

The browser adapter mirrors the original force setup and decoding contract, including
20-byte command chunks, command/counter correlation, fragmented response
reassembly, availability gating, configured sample period, and first-valid-
measurement connection gating. It currently enables and exposes only channel 1
Force (N); it does not enumerate the native all-channel schema into an LSL
descriptor or generate the Vernier-derived 0–1 LSL stream. Its deterministic
Playwright fixture verifies
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
- physical Windows/WinRT GDX-RB native stream, disconnect, and reconnect
  qualification on 2026-08-24
- bundled liblsl outlet/sample smoke test including Vernier Double64 raw and
  Float32 derived pushes
- pinned official pylsl consumer verification through
  `scripts/verify_vernier_lsl.py`
- pinned official pylsl mixed-device verification through
  `scripts/verify_mixed_lsl.py`: four exact inlets receive advancing ECG, ACC,
  rawVernier, and vernierBreathing rows over an overlapping LSL interval

The physical result closes only the single-device native gate. The synthetic
mixed-device gate proves concurrent native outlet ownership and delivery, not a
physical dual-BLE run. The remaining software and emulated-browser results do
not prove physical browser, mixed-source, cross-platform, under-load,
respiratory-agreement, or latency compatibility.

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

The native verifier covers the single-GDX discovery, complete ordered metadata
schema plus exact GDX-RB channel-1 Force (N) contract, sustained primary stream,
health thresholds, explicit disconnect, and reconnect portions of this gate.
The retained physical run exercised the older force-only mask. A new physical
run must demonstrate the all-channel mask and both LSL outlets before that part
of the gate closes. It does not replace the mixed multi-device, browser,
under-load, independent-reference, or synchronized H10/GDX qualification runs.

For the synchronized portion, save isolated schema-2 native CSVs for H10 ACC
and GDX-RB Force (N), then use the repository's
`analyze_respiration_reference` Rust example. The analyzer reconstructs H10
notification batches, runs the production `BreathingProcessor`, aligns both
streams under the bounded host-time contract, and emits identifier-free
descriptive evidence. See `docs/acc-breathing-handoff.md` for the exact command,
conditions, metrics, and interpretation limits.

## Current limitations

- The native product profile intentionally supports the complete compatible
  numeric schema of a metadata-verified GDX-RB and requires channel-1 periodic
  Force (N). Generic Go Direct devices and arbitrary sensor selection remain
  future work and are rejected rather than mislabeled as respiration belts.
- The Chromium path remains force-only and browser-local. It cannot create the
  aggregate raw or derived native LSL outlets.
- One physical GDX-RB/native Windows run passed; physical browser,
  revised all-channel native/LSL, mixed-source, cross-platform, under-load, and
  long-run validation is pending.
- Browser source discovery is chooser-based by Web Bluetooth design.
- Go Direct absolute device-clock synchronization is not implemented; periodic
  samples use explicit host-receipt/backfilled timestamps.
- The optional default-off Rusty LSL backend is compile/test covered, but the
  ordinary multi-source app currently creates one router per source. Its older
  fixed-port one-registry/many-outlet qualification must be generalized before
  claiming multi-source Rusty-LSL runtime support. Production/default liblsl
  does not have that experimental registry constraint.
- The aggregate Double64 Vernier schema is supported only by the packaged
  liblsl backend. Enabling the optional Rusty backend with an installed Vernier
  schema fails explicitly instead of silently dropping either special outlet.
