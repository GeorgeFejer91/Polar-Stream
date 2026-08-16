# Optional Rusty LSL backend

Polar Stream's packaged and default source build continues to use liblsl. An
experimental, default-off Cargo feature can instead build the native desktop
output crate against Rusty LSL:

```bash
cargo run -p polar-stream --no-default-features --features rusty-lsl-backend
```

The dependency is commit-pinned to Rusty LSL
`74f7d0ea2cce9b3d049ea24602527a5f52360554`. This is the reviewed merge that
adds official-consumer initialization for one-channel ECG and three-channel
ACC without changing its one-channel behavior. Updating that revision requires
a new exact interoperability and physical-device review; a floating branch or
tag is not accepted.

## Transport contract

The optional backend retains one shared caller-polled discovery registry and
one independent persistent outlet per selected output. It publishes each
already-arrived BLE notification as one bounded chunk:

- raw ECG: Float32, 1 channel, nominal 130 Hz;
- raw ACC: Float32, 3 channels in X/Y/Z order, nominal 200 Hz;
- canonical name, type, source ID, channel labels, units, and Polar metadata;
- source timestamps in the local LSL clock domain, backfilled at the declared
  nominal rate from the notification's newest sample; and
- up to 256 records per chunk, 64 outlets, and exactly one admitted official
  consumer per outlet.

The one-consumer bound is an explicit Polar Stream deployment constraint, not
general multi-consumer conformance. A second concurrent connection is rejected
without disturbing the already admitted consumer. A product requirement for
multiple official consumers of the same outlet needs a separately scoped Rusty
LSL auxiliary-connection and fan-out qualification before this bound can move.

Polar's PMD sensor timestamp is retained separately in the native input event,
OSC, and physical evidence. It is not substituted directly into LSL because it
belongs to the H10 device-clock domain rather than the host LSL clock domain.

The advertised IPv4 interface is selected from the operating-system multicast
route without sending a probe datagram. Set
`POLAR_STREAM_RUSTY_LSL_IPV4=<concrete-unicast-IPv4>` to make that choice
explicit. Unspecified, multicast, broadcast, malformed, or unbindable values
fail visibly; the backend does not silently advertise an unrelated interface.
Windows can assign a TCP ephemeral port that is excluded or unavailable for
the paired UDP timedata socket. Setup therefore reserves an eligible UDP port
first, binds TCP to that same numeric port while the reservation remains held,
and releases the probe immediately before registry admission. Pair selection
is bounded to 16 attempts, and registry admission retries only the exact
`AddrInUse`/`PermissionDenied` release-to-bind race, at most four times, before
failing. Tests cover deterministic conflict, retry, release, and rebind without
a leaked listener.

The native coordinator advances Rusty LSL's synchronous caller-owned work on
Tokio's blocking pool. An explicit stop signal and two-second join deadline
bound shutdown without occupying an async runtime worker.

Discovery qualification deliberately calls official liblsl's broad stream
enumeration and then matches all of these fields client-side before opening:
`name`, `type`, `channel_count`, `nominal_srate`, `channel_format`, and
`source_id`. Zero, multiple, mismatched, or cross-stream candidates reject.
Rusty LSL does not currently claim conformance for liblsl's server-side
`resolve_byprop` predicate evaluation, and neither Polar Stream verifier uses
that API.

## Validation and limits

`scripts/verify_rusty_lsl_backend.py` runs two synthetic outlets from an exact
clean commit/tree against pinned pylsl 1.18.2/liblsl 1.17.7. It is host
interoperability evidence, not a device test.
`scripts/verify_rusty_lsl_h10.py` additionally drives the native Windows BLE
input from an exact clean commit/tree and requires one exact H10, advancing
ECG/ACC sensor timestamps, nonzero bounded X/Y/Z data, exact descriptors,
distinct official inlets, and count/rate/loss/reorder plus cleanup evidence.
Its output may contain a device identifier and must remain ignored/private; it
records bounded aggregates, not physiological samples. Official inlet
collection starts only after exact selection, the direct WinRT session, and
advancing ECG/ACC source frames. It runs in a daemon worker so a native liblsl
call cannot defeat the outer deadline; selection has a separate 30-second
fail-fast bound, post-selection source readiness has a 60-second bound, and
source/consumer collection retains its two-minute bound. The worker must close
before the source is stopped. Native session setup has its own 45-second total
budget, so a typed native stage exits before the outer source-readiness bound.
`POLAR_STREAM_H10_SESSION_DIAGNOSTICS` is enabled by this verifier and records
stage name, attempt, entry/exit, duration, and result class. It also records
identifier-free characteristic properties, requested and read-back CCCD state,
handler lifetime counts,
per-source callback/decode/enqueue counts, callback faults, and queue outcomes.
Link checkpoints record only connection state, negotiated PDU size, connection
interval units, and peripheral latency.
It never records addresses, names, payload bytes or sizes, manufacturer data, or
stable device identities. PMD settings and start responses are separate stages
from their GATT writes and first frames.

The synthetic host qualification passed. Earlier bounded physical Windows
testing found two straps with the separate native WinRT reference doctor, while
Polar Stream's old `btleplug` GATT connection path stopped at connect,
notification-receiver setup, or before the first PMD frame. A later bounded
pre-publication attempt initialized both Rusty outlets but the `btleplug`
scanner did not return before exact H10 selection. Polar Stream now has its own
direct safe-Rust WinRT advertisement and session backend with bounded discovery,
subscription, first-frame qualification, cancellation, queues, and cleanup.
An attended differential scan then proved the exact published reference
watcher could observe one H10 while the candidate received advertisements but
admitted none. Source diagnosis found that the candidate evaluated service-UUID
enumeration before the exact local-name route used by the reference. Those
predicates are now independent, and unnamed service candidates require bounded
direct WinRT exact-model confirmation. A subsequent same-lease physical run
was reference-positive and the repaired candidate selected one exact H10, so
discovery and predicate selection are no longer the blocker. A same-device
reference doctor then consumed ECG, ACC, and heart-rate notifications. The
response-gated candidate passed device/session acquisition, service and
characteristic discovery, all three CCCDs, and successful ECG settings/start
responses, but timed out before its first ECG data callback; ACC start was not
attempted. Rusty outlets and official physical inlets were not reached. Handler
order, a reference-style settings delay, and a pre-stream preferred-connection
request were separately eliminated as causes. The remaining gate is therefore
the WinRT PMD data-notification delivery/lifetime boundary, not scanner, GATT
setup, the physical H10, or Rusty LSL. An
earlier zero from the published reference CLI wrapper is invalid
because that wrapper returns immediately after starting its asynchronous
watcher and is not device-state evidence.

A later reference-positive differential run kept the candidate connected with a
232-byte negotiated PDU through the accepted ECG start response. PMD control and
heart-rate callbacks advanced, but PMD data delivered no WinRT event at all.
This rules out link readiness and MTU for that epoch. The next source checkpoint
adds fail-closed CCCD readback and explicit agile ownership of each registered
event delegate; it has deterministic host coverage but is not physical
acceptance until another attended same-epoch chain reaches both sensor streams
and the pinned official inlets.

The typed trace proved address/device acquisition, persistent session setup,
service/characteristic access, all three CCCDs, and the ECG settings/start
writes and responses succeeded. A GATT result does not prove the PMD protocol
admitted a stream, so
the host candidate now requests ECG settings and requires its exact successful
control response, then requires the ECG start response and first ECG frame
before issuing ACC start and requiring its response and first ACC frame.
Malformed, rejected, missing, duplicate/out-of-order control responses and
per-stage timeouts fail closed. Each WinRT async operation owns a bounded
cancel/close path; partial setup rolls back registered callbacks in reverse
order and closes the session once. This response-gated repair is host evidence
until another attended physical run. No identifiers or physiological
recordings are committed, and neither the reference-doctor runs nor compilation
may be substituted for the pending end-to-end acceptance.

The comparison is explicit: the published reference inserts a 500 ms delay and
configures optional heart rate before required PMD. Polar Stream now uses that
cancellable settle and heart-rate-first discovery order, and defers battery
discovery until both required sensor streams qualify. Their persistent-session,
uncached-service, service-access, retry/cached-fallback, and CCCD settings align.
The adopted lesson is the reference lifecycle and staged settings → ECG → ACC
control/data sequence. No implementation source is copied; the bounded settle
is an explicit, cancellable Windows compatibility stage rather than an
unobserved delay.

The browser application is outside this transport. Its same-origin
`BroadcastChannel`, event API, and CSV recorder are not LSL and remain unable
to provide native multicast discovery, TCP data transport, or LabRecorder
interoperability. Web Bluetooth remains foreground/chooser-dependent and does
not become a background/IWA transport through this feature.

Finally, Rusty LSL is AGPL-3.0-or-later while Polar Stream is MIT. No release
package enables this feature. Distributing an enabled combined binary requires
an explicit licensing decision and corresponding source/compliance process; the
optional integration must not be described as a drop-in MIT replacement.
