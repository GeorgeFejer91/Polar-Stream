# Latency and multi-source architecture

This document describes the implemented native data path and the remaining
physical qualification boundary. It is an engineering contract, not a claim of
hard real-time behavior or cross-device hardware synchronization.

## Implemented topology

Each connected physical source owns an independent acquisition and processing
lane:

```text
BLE callback / notification stream
  -> capture process-monotonic host receipt time
  -> bounded platform input owner and one decode
  -> source-owned OutputRouter (raw LSL/OSC/CSV first)
  -> selected source-owned derived processors
  -> bounded best-effort display queue
  -> replaceable Tauri Channel endpoint
  -> timestamped typed JavaScript rings
```

The maps shared by sources contain lifecycle handles, descriptors, display
endpoints, and output routers. They are not a shared sample queue. A slow source,
renderer, or output reconfiguration therefore does not intentionally serialize
the notification paths of unrelated sources.

The application admits at most eight concurrent source instances. A presentation
slot such as `source-1` may be reused after disconnect, but the internal source
identity is a new UUID for every connection. LSL names retain the readable slot
suffix while in-process caches and renderer events use the non-reused instance
identity.

## Discovery while streaming

Scanning is allowed while one or more sensors remain connected. The Polar and
Go Direct pools retain active owners and maintain incremental candidate caches
with a 45-second TTL. Active devices are excluded from available results.
Naturally completed sessions are pruned before scan, connect, and capacity
queries.

On Windows, an H10 refresh stops after finding one exact-name candidate that is
not already active. This avoids waiting for the maximum configured source count
on every Add-device action. Go Direct retains its bounded scan window because
device qualification requires service and metadata inspection after discovery.

Concurrent discovery is still a radio/driver scheduling load. It may change
connection-event timing on a particular adapter even though the software keeps
the live owners intact. The release qualification matrix must therefore measure
inter-arrival gaps, queue watermarks, loss, and latency during scans on every
supported adapter/driver family. The UI states that streams remain active; it
does not claim scanning is physically free.

## Clock model

`polar-stream-time` owns one process-wide monotonic host epoch. H10 callbacks
capture host receipt at the earliest safe WinRT or stream boundary. Go Direct
captures the same clock immediately before frame assembly/decode. Wall clock is
never substituted for acquisition timing.

Every emitted native data batch carries a timing envelope:

- raw device timestamp when the protocol supplies one;
- host receipt time;
- mapped common-host time;
- sample period when known;
- clock-map revision, quality, and uncertainty;
- a reset/gap flag.

Nanosecond fields cross Tauri as decimal strings so JavaScript cannot silently
lose integer precision. H10 uses one affine mapper per physical source, shared
by ECG and ACC because those PMD timestamps are in the same device clock. The
mapper uses a bounded 128-observation window, a drift-constrained fit, a
low-delay offset estimate, and a new segment after a material device-clock
regression. Go Direct has no equivalent device clock, so it remains explicitly
arrival-timed and backfills a batch only from the negotiated sample period.

Derived values inherit the causal batch timing. Reconnect starts a new mapper
because it starts a new source instance.

## LSL and unequal sample rates

LSL does not require producers to share a sampling rate or sample index. Each
raw/derived outlet keeps its native cadence and metadata. A source-owned LSL
publisher shares one device-to-LSL clock mapper across all of that source's
outlets; individual outlets retain only their last published time so consecutive
chunks cannot overlap or regress. Both the packaged liblsl backend and the
experimental Rusty backend use this rule.

The common time domain enables downstream comparison, but it does not prove
simultaneous physical capture. H10 device-time mapping has an estimated
uncertainty; Go Direct arrival timing also contains firmware, controller,
driver, and notification batching delay that cannot be removed in software.
Recorder receipt and durable XDF flush are later boundaries and must be measured
separately from producer publication.

## Display path and composite panels

The WebView is a disposable observer. Closing or reloading it detaches only the
display channel. Acquisition and native outputs continue, and the new renderer
calls `attach_active_sources` to replace each display endpoint and replay the
latest connection snapshot.

Per-source rings store values, mapped timestamps, and gap flags in typed arrays.
Capacity is rate-aware, bounded to 131,072 samples per signal, and supports up
to a ten-minute requested chart window without allocating every catalog metric
eagerly. Canvas drawing uses mapped time on the horizontal axis and reduces each
pixel column to temporal extrema. Paths break at declared gaps.

When at least one Polar and one Vernier source are active, Visualization adds:

- **Compare · belt force + Polar ACC**: force in N plus X/Y/Z in mg, rendered
  as four independent lanes with one host-time axis;
- **Compare · belt + ACC breathing**: the native Rust-derived Vernier 0–1
  waveform and the selected Polar ACC 0–1 estimate on one comparable scale.

Force and acceleration never share a vertical scale. The composite is a view
over source-owned raw buffers; it does not merge, resample, or republish raw
scientific data.

## Reliability and overload rules

- Input and authoritative output precede derived processing and display.
- The display queue is bounded to eight events. Full display queues drop the
  attempted display event without stopping acquisition or native output.
- Renderer channel failure detaches the renderer instead of ending a source.
- Native CSV retains its independent bounded fail-stop writer policy.
- Output reconfiguration is serialized and transactional across active source
  routers. On a router failure, already-updated routers are restored before the
  global configuration can commit.
- Connection reads/configures/inserts its source router under the same output
  lifecycle lock, so it cannot start from a stale configuration revision.
- Source coordinator handles are retained. Disconnect waits up to three seconds;
  application exit disconnects all sources and joins coordinators within one
  four-second global deadline before forcing remaining tasks to stop.

## PsychoPy boundary

Polar Stream now follows the parts of a PsychoPy-style acquisition architecture
that belong in this product: native device ownership, independent clock domains,
bounded queues, renderer isolation, explicit session lifecycle, and downstream
LSL interoperability. It does not turn a WebView canvas into a frame-accurate
psychophysics renderer.

For experiments requiring deterministic stimulus onset, use the hybrid layout:

```text
Polar Stream native acquisition -> LSL/XDF
PsychoPy stimulus process        -> frame-locked presentation + event markers
physical timing validation       -> photodiode/audio/loopback evidence
```

PsychoPy can own the presentation window and frame-flip timing while Polar
Stream owns BLE, clock evidence, raw publication, and recording streams. A
future native presentation sidecar or GPU window would need a separate measured
contract, supervised lifecycle, present timestamps, and physical onset tests.
Elevating the whole Tauri/WebView process or using Windows real-time priority is
not an acceptable substitute.

## Qualification still required

Deterministic tests prove mapping behavior, lifecycle semantics, frontend
alignment, and bounded data structures. They do not qualify radio or display
timing. Before making release latency/reliability claims, run release builds
with physical H10 and GDX-RB hardware and record:

- 1, 2, 4, and maximum-admitted source loads;
- scan-during-stream, reconnect, renderer reload/stall, configuration update,
  recording, and shutdown cases;
- callback-to-decode, decode-to-raw-publish, derived, IPC, and paint data age;
- p50/p95/p99/max, queue age/high-water, sequence gaps, mapping uncertainty,
  LSL inlet receipt, and recorder continuity;
- representative Bluetooth adapters, drivers, power modes, interference, CPU,
  disk, and network load; and
- multi-hour soak behavior plus physical stimulus-onset instrumentation when
  PsychoPy or another presentation process is used.

Zero unreported loss is the integrity requirement. Priority/QoS, affinity, and
timer-resolution changes remain opt-in experiments until measurements show a
repeatable benefit without starving Bluetooth, networking, disk, or UI work.
