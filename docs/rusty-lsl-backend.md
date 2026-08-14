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
- up to 256 records per chunk, 64 outlets, and 4 consumers per outlet.

Polar's PMD sensor timestamp is retained separately in the native input event,
OSC, and physical evidence. It is not substituted directly into LSL because it
belongs to the H10 device-clock domain rather than the host LSL clock domain.

The advertised IPv4 interface is selected from the operating-system multicast
route without sending a probe datagram. Set
`POLAR_STREAM_RUSTY_LSL_IPV4=<concrete-unicast-IPv4>` to make that choice
explicit. Unspecified, multicast, broadcast, malformed, or unbindable values
fail visibly; the backend does not silently advertise an unrelated interface.
Windows can occasionally assign a TCP ephemeral port that its paired UDP
timedata socket cannot bind. Setup retries only that exact
`AddrInUse`/`PermissionDenied` race, at most four times, before failing.

Discovery qualification deliberately calls official liblsl's broad stream
enumeration and then matches all of these fields client-side before opening:
`name`, `type`, `channel_count`, `nominal_srate`, `channel_format`, and
`source_id`. Zero, multiple, mismatched, or cross-stream candidates reject.
Rusty LSL does not currently claim conformance for liblsl's server-side
`resolve_byprop` predicate evaluation, and neither Polar Stream verifier uses
that API.

## Validation and limits

`scripts/verify_rusty_lsl_backend.py` runs two synthetic outlets against
pinned pylsl 1.18.2/liblsl 1.17.7. It is host interoperability evidence, not a
device test. `scripts/verify_rusty_lsl_h10.py` additionally drives the native
Windows BLE input and requires one exact H10, advancing ECG/ACC sensor
timestamps, nonzero bounded X/Y/Z data, exact descriptors, distinct official
inlets, and count/rate/loss/reorder plus cleanup evidence. Its output may
contain a device identifier and must remain ignored/private; it records bounded
aggregates, not physiological samples.

The 2026-08-14 host qualification passed. The first bounded physical run stopped
before connection because the native scan found no H10; it produced no chooser
or pairing prompt and makes no device-acceptance claim. A ready change or
release using this backend still requires a passing physical run.

The browser application is outside this transport. Its same-origin
`BroadcastChannel`, event API, and CSV recorder are not LSL and remain unable
to provide native multicast discovery, TCP data transport, or LabRecorder
interoperability. Web Bluetooth remains foreground/chooser-dependent and does
not become a background/IWA transport through this feature.

Finally, Rusty LSL is AGPL-3.0-or-later while Polar Stream is MIT. No release
package enables this feature. Distributing an enabled combined binary requires
an explicit licensing decision and corresponding source/compliance process; the
optional integration must not be described as a drop-in MIT replacement.
