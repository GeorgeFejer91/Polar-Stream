# Decision log

## 2026-08-24 — Separate discovery rows, connected widgets, and output-derived visuals

Model Input as two different lifecycles. Search produces temporary supported-
device rows classified by the typed Polar or Go Direct identification boundary;
names alone do not define the device role. Connect is explicit, and only a
successful connection promotes a candidate into a source widget. The widget
owns telemetry, disconnect, selection, and a live-session color picker. Apply
that color to the selected source's raw and processed Output cards and
Visualization surfaces. Put protocol-specific controls only in matching
widgets: Keep connected / awake exists only for native Vernier and defaults on
when no prior choice exists.

Treat measured device signals as mandatory raw ingredients. Polar raw ECG/ACC
and Vernier rawVernier remain automatic and non-removable; the previously
required automatic Vernier 0–1 breathing outlet remains paired with its raw
protocol. A successful native physical connection enables LSL and reapplies the
source-filtered configuration so raw outlets become discoverable without a
second setup step. Metrics and formulas remain processed, explicitly addable
outputs. Visualization choices are derived from automatic and selected outputs,
never from discovery rows. Do not auto-scan a remembered device at application
startup; user search owns initial discovery, while Vernier reconnect remains an
unexpected-drop policy for an already established session.

## 2026-08-24 — Discover the missing protocol without stopping the live one

Treat Polar and Go Direct discovery, acquisition, and output ownership as two
independent protocol families. When neither is connected, scan both families
concurrently. When one is connected, scan only the missing family because the
active family's session pool cannot be rescanned without interrupting its live
session. Do not disconnect, replace, or reconfigure the first source to admit
the second. Once both are active, disable redundant discovery and keep both
source-specific routers publishing; UI source selection changes presentation
only.

Retain a synthetic packaged-liblsl acceptance gate that opens four exact pinned
official pylsl inlets—Polar ECG and ACC plus Vernier raw and derived breathing—
and requires advancing timestamps with a shared overlapping interval. This
proves concurrent router/outlet behavior without claiming that a physical
H10/GDX dual-BLE run, synchronization accuracy, or respiratory agreement has
been qualified.

## 2026-08-24 — Make the interface device-first, then output-first

Represent no connected source as a real `none` profile: Output and Visualization
remain empty and expose no configuration or display choices. A first connection
activates only the selected device profile's defaults. Polar activates raw ECG
and raw accelerometer; Vernier presents the automatic complete `rawVernier` and
derived `vernierBreathing` protocol outputs, while the older rawForce route stays
an opt-in compatibility module.

Rebuild visualization choices from the selected source profile and its active or
automatic outputs. When Polar and Vernier are connected together, selecting a
source swaps the whole visible preset—including raw cards, output library,
Formula Lab availability, automatic protocol cards, and visualizations—without
mixing device families. Keep preferences transport-free, but do not instantiate
visible modules before a matching device connection.

## 2026-08-24 — Make the complete Vernier schema raw and the waveform derived

After confirming the GDX-RB and its required channel-1 Force (N) sensor, decode
and enable every compatible numeric sensor in the device's advertised metadata
mask. Reject malformed, unsupported, duplicate, or mutually exclusive schemas
instead of selecting only convenient channels and silently losing recordable
device output. Carry each native measurement frame through the input event as
one frame-wide value set before compatibility, derived, or UI work.

Create `<base>_rawVernier` automatically on the packaged liblsl path. Give it a
fixed channel order by sensor number, metadata-derived labels/units, Double64
storage that exactly represents device Float32 and Int32 values, `NaN` for a
channel absent from a sparse periodic/aperiodic update, and trailing sequence,
queue-loss, device-drop, period, decode-latency, host-receipt, and encoding
diagnostics. Keep nominal rate irregular and map explicit host receipt plus
configured periodic backfill into the local LSL clock because Go Direct exposes
no absolute sample clock here. Retain the older channel-1 rawForce output as a
compatibility surface, but make aggregate raw publication happen first.

Create `<base>_vernierBreathing` as a separate explicitly derived Float32
outlet. Map increasing GDX-RB force/inhalation upward using a causal bounded
30-second force history, warm-up min/max then 5th/95th-percentile bounds, and a
hard 0–1 clamp. Hold the last derived value on non-finite force while retaining
the exact non-finite input in raw LSL. This waveform is relative belt effort,
not lung volume, airflow, or a clinical signal. The browser remains force-only
and cannot publish LSL; the optional Rusty backend rejects this dynamic
Double64 schema instead of silently omitting it.

## 2026-08-24 — Honor physical Go Direct BLE write and response framing

Select the command characteristic's advertised GATT write mode, preferring
write without response as Vernier's native implementation does and falling
back to write with response only when that is the supported property. Treat
the low-five-bit `0x18` header family as bounded command responses while
retaining the distinct `0x20` measurement header, declared-length limits, and
command/counter correlation. Physical firmware 5.3 returned `0xb8` for
initialization and `0x98` for status rather than echoing outbound `0x58`.

This decision is backed by a passing identifier-free Windows/WinRT run through
the application's `InputSessionPool`: exact GDX-RB channel-1 Force (N), 70
primary samples, zero drop/malformed/nonfinite counts, explicit disconnect,
and a 20-sample reconnect stream. It closes only the single-device native gate;
browser, mixed-source, cross-platform, under-load, synchronized-reference, and
latency-percentile gates remain open.

Run Polar and Go Direct discovery concurrently rather than serializing their
complete scan windows. After a GDX-RB connects, keep its documented measurement
session active until explicit disconnect; do not invent a keep-awake command
that Vernier's public implementations do not expose. Software cannot wake a
device whose radio has been put to sleep, so saved-device reconnect minimizes
the pre-connection idle window by scanning only its remembered transport, but
still requires the belt to be advertising. Manual scans continue to discover
both sensor families. Expose an opt-in renderer setting that keeps the active
10 Hz measurement subscription open and retries an unexpected GDX link loss at
1.5-to-30-second bounded exponential intervals. Persist that UI lifecycle
choice locally, never retry a deliberate disconnect, and do not represent the
retry policy as a firmware wake command.

## 2026-08-22 — Visualize the canonical 1D waveform as a dot and trail

Specialize the existing `breathing_volume` visualizer instead of creating a
second display-only respiration algorithm or output ID. Draw the newest causal
0–1 sample as a moving dot and the selected display window as its leftward
trail in the canonical UI shared by Tauri and Pages. Label upward movement as
inhale and downward movement as exhale according to configured projection
polarity, while retaining the mounting/inversion caveat and experimental
respiratory-effort semantics.

## 2026-08-20 — Analyze reference agreement from raw recordings without tuning the product

Reconstruct native H10 ACC notification batches and replay the current Rust
`BreathingProcessor` rather than implementing an analyzer-only respiration
algorithm. Map H10 PMD sample spacing into host time with a robust
fifth-percentile host-minus-sensor offset; retain GDX-RB's explicit host-receipt
and configured-period timing because Go Direct exposes no comparable absolute
device clock.

Compare signed projection and normalized waveform separately after matched
causal baseline removal and a bounded plus/minus three-second lag search.
Anchor polarity at zero lag so a half-cycle oscillatory match cannot choose the
opposite mounting direction. Report quality, lag, polarity, normalized error,
respiratory-rate error, and window stability descriptively. Do not add a
correlation acceptance threshold or change production samples from one
recording; physiological acceptance requires predeclared repeated held-out
physical sessions.

## 2026-08-20 — Retain identifier-free GDX-RB physical evidence

Decode the stable main-firmware major/minor and battery fields from the Go
Direct status response in both native and Chromium paths. Qualify the native
product path through the same `InputSessionPool` used by the application,
requiring exact GDX-RB channel-1 Force (N) metadata, sustained periodic samples,
continuity and input-health thresholds, bounded disconnect, and reconnect.

Persist only explicit structured verifier markers under ignored `artifacts/`;
do not retain raw process output, chooser names, or Bluetooth identifiers. A
failed scan is evidence about that scan only, not evidence of incompatibility.
The first local attempt found no advertised candidate, so browser, mixed-source,
synchronized H10/reference, and latency qualification all remain open.

## 2026-08-20 — Canonicalize Pages text bytes across host platforms

Stage and verify browser-demo text assets with CRLF normalized to LF, while
copying and hashing binary assets byte-for-byte. GitHub's Linux runner checks
out LF text, whereas the supported Windows development checkout may use
`core.autocrlf=true`; hashing raw worktree bytes made an otherwise exact live
deployment unverifiable from Windows. Canonical text bytes make the manifest
deterministic without changing JavaScript, HTML, CSS, JSON, or Markdown
semantics.

## 2026-08-20 — Gate motion after all-axis noise attenuation

Calculate the breathing motion-quality residual from successive vectors on a
dedicated all-axis EMA path, using the configured respiration smoothing
strength, rather than from raw 200 Hz sample deltas. The canonical recorded H10
fixture contains ordinary per-sample variation large enough to fail the raw
threshold despite a usable low-frequency component. Filtering all axes before
the residual keeps axis selection from hiding movement, attenuates sensor noise,
and still rejects deterministic broadband motion. Rust and Chromium own the
same update order and regression fixtures.

## 2026-08-20 — Timestamp ACC-derived respiration at the source frame

Publish each breathing snapshot once per accepted accelerometer notification at
that notification's newest H10 PMD sensor timestamp. Map it into local LSL time
through the existing ACC first-frame offset, retain the raw nanosecond timestamp
for OSC/CSV, and propagate it through the Chromium metric event and browser CSV.
This permits raw/derived alignment without conflating sensor time with host
calculation or arrival time. HR-only metrics remain host-timed because the
standard heart-rate characteristic supplies no comparable PMD timestamp.

## 2026-08-20 — Normalize breath phase by sensor time and keep holds separate

Classify ACC breathing phase from normalized waveform velocity per second, not
raw change per BLE notification. Derive batch duration from the accepted ACC
sample count at the requested 200 Hz. Divide the existing sensitivity-derived
delta threshold by a 50 ms compatibility reference, preserving its scale for a
common ten-sample notification while making equivalent slopes invariant across
MTU/browser/OS batch sizes. Rust and Chromium own the same constants and tests.

The Lalidis Mateo belt thesis reinforces that fixed normalized control, signed
movement, and adaptive normalized control are different contracts. Its tested
PZT belt drifted materially during breath retention. Polar Stream therefore
keeps the signed ACC projection and normalized waveform continuous and does not
silently freeze either as a retained-lung-level estimate. Any future hold-aware
interaction output must be separately named, quality-bearing, and validated
against synchronized respiratory reference data.

## 2026-08-20 — Treat respiration as a qualified waveform, not volume

Expose the Polar H10 ACC result as an experimental one-dimensional
respiratory-effort module analogous in shape—not measurement semantics—to a
respiration-belt curve. Preserve the signed PCA projection in g and the existing
`breathing_volume` compatibility ID for a robust 0–1 waveform, but label the
latter as an ACC breathing waveform because it is neither lung volume nor
airflow. Use only causal processing: selected-axis EMA, 12-second PCA
calibration, 10-second baseline removal, and bounded adaptive quantiles.

Add separate readiness and confidence streams instead of hiding questionable
samples. Readiness requires calibration, fresh notifications, and an all-axis
motion score; confidence combines calibrated range, motion, history coverage,
and positive autocorrelation. Confidence is an app-specific quality index, not
a probability. Phase becomes zero when not ready, while the continuous signal
and raw X/Y/Z remain publishable for audit and later reprocessing. Native Rust
and browser JavaScript must retain the same constants and deterministic tests.

Tighten Go Direct from a generic channel-1 assumption to a GDX-RB product
profile. Parse device and all advertised sensor metadata, confirm the GDX-RB
identity, and accept only periodic channel-1 Force in N. Prefer 100,000 µs when
the sensor reports that rate as valid; otherwise use its plausible typical
period. Reject unknown Go Direct products rather than presenting their channel
as a respiration belt. Respyra and the Lalidis Mateo thesis are behavioral and
methodological references only; no third-party implementation is copied into
the Rust or browser hot paths.

## 2026-08-20 — Add independent Go Direct and mixed-source routing

Implement Vernier Go Direct as an independent MIT protocol/input pair rather
than importing Respyra or GPL protocol code. The platform-neutral core owns
command bytes, counters, checksums, bounded frame assembly, and measurement
decoding; the BLE input owns discovery, GATT lifecycle, correlated setup,
first-frame readiness, bounded queues, and health. The initial product target is
channel 1 with a 10 Hz fallback period. A physical device gate remains required.
Session cancellation owns the complete negotiation/stream future, teardown
events are non-blocking, peripheral disconnect is bounded, and scan/connect
prune completed owners before applying capacity rules.

Generalize the ordinary application to at most eight simultaneous mixed inputs.
Every source receives an independent input owner, processing engine, output
router, stable slot, stream suffix, and fixed palette color. Shared locks are
lifecycle-only, display buffers are per source, and display loss cannot become
raw output backpressure. Filter output modules by input kind so sources never
advertise signal types they cannot produce.

Keep saved output configuration separate from transport ownership. Validate
formula state limits through a destination-disabled router, then create and
configure transports only inside connected source routers. Do not retain the
legacy global router because it would advertise empty unsuffixed outlets.

Publish Go Direct notification batches immediately and preserve the configured
intra-batch period with explicit backfilled timestamps. Advertise raw force as
irregular-rate LSL metadata because the implemented path has no equivalent to
the H10 absolute sensor timestamp. Do not convert receipt time into a false
device-clock claim.

Mirror the Go Direct protocol in the canonical browser UI because Chromium Web
Bluetooth can access the official GDX service. Keep this a separate adapter,
require HTTPS/localhost and a user-triggered chooser per device, and use only
browser-local CSV/live-channel outputs. Deterministic emulation proves the
software contract; physical browser compatibility and latency remain open.

The production/default liblsl backend is the multi-source native output path.
The optional Rusty LSL backend retains its compile/test coverage but cannot be
described as arbitrary mixed-source runtime support until its fixed discovery
registry is generalized beyond the older one-registry/two-H10 coordinator.

## 2026-08-18 — Qualify two H10s with independent native session owners

The published reference proved that two H10s can run concurrently when each
device owns a persistent Windows session. Polar Stream therefore adds a bounded
qualification pool, not a second UI workflow: one scan snapshot admits exactly
two distinct devices into non-identifying slots, and each slot gets its own
input manager, Windows owner, event receiver, and ECG/ACC outlet keys. One
two-session output coordinator owns the single Rusty discovery registry and all
four persistent outlets. This preserves the library's one-registry/many-outlet
composition and prevents a second bind of the fixed discovery port. Connection
and cleanup transitions serialize, rescanning while active rejects, and all
admitted sessions are drained even when one cleanup reports an error.
The ordinary scanner retains its one-device early-completion policy; the pool
requests two exact-name candidates before the same bounded watcher may stop.

The official verifier broadly enumerates and exactly matches four descriptors
client-side, opens one pinned pylsl 1.18.2/liblsl 1.17.7 inlet per outlet, and
requires advancing bounded sensor and LSL evidence before graceful two-session
shutdown. This does not change Rusty LSL, claim multiple consumers per outlet,
expose device identities, or grant browser code native BLE/LSL authority.

The exact source commit `2b7bdf0c8f0a567d8ad4a18dcbb24a78928f9197`
passed the same-epoch gate after a repaired-reference two-session pass. Four
distinct pinned official inlets measured device-slot 1 at 1,168 ECG samples /
130.01 Hz and 1,764 ACC records / 202.43 Hz, and device-slot 2 at 365 ECG
samples / 130.19 Hz and 468 ACC records / 202.47 Hz. Every stream advanced with
zero estimated loss/reorder, both ACC streams had nonzero X/Y/Z, all inlets
closed before source stop, both native sessions reported clean cleanup, and no
verifier process, listener, or Bluetooth lease remained. This accepts only the
bounded two-H10 Windows interoperability claim.

## 2026-08-17 — Remove the standalone WinRT probe from the publication surface

The temporary `polar-h10-winrt-probe` binary isolated Windows projection and
device behavior during diagnosis, but it is not part of the accepted Polar
Stream runtime or its physical qualification chain. Its target and source are
removed before publication so the product does not ship a second device owner
or an ad-hoc hardware executable. The accepted direct-WinRT backend, bounded
physical verifier, fake-backend lifecycle coverage, and identifier-free staged
diagnostics remain. Closed differential profiles remain only as deterministic
failure-regression evidence; the production-default verifier explicitly clears
their environment selector and the accepted run used the ordinary
reference-compatible lifecycle.

## 2026-08-17 — Accept the production-default physical H10 chain

An attended reference-first Windows run observed exactly one H10 at 100% battery.
With no session-profile override, the exact clean production-default candidate
then selected one session, advanced ECG and three-axis ACC sensor timestamps,
published two distinct Rusty outlets, and admitted one pinned pylsl
1.18.2/liblsl 1.17.7 consumer per outlet. Official collection measured 365 ECG
samples at 130.14 Hz and 432 ACC samples at 202.22 Hz with zero estimated loss,
zero reorder, advancing LSL timestamps, nonzero values on every ACC axis, and no
cross-stream match. Source evidence independently measured 584 ECG and 720 ACC
samples with zero loss/reorder.

Both official inlets closed before source stop, the source reported clean stop,
the process exited zero, and no worker, listener, or Bluetooth lease remained.
This accepts the bounded production-default Windows H10 → Rusty LSL → official
consumer chain. It is not a long-duration latency benchmark, generic
multi-consumer claim, predicate-filter conformance, browser transport proof, or
release authorization. The backend remains optional/default-off and enabled
distribution remains subject to the documented AGPL decision.

## 2026-08-17 — Promote the proven settings dwell into the Windows default

The closed `reference-settings-dwell` candidate completed one reference-positive
physical chain through exact H10 selection, both advancing sensor streams, both
Rusty outlets, pinned official consumers, zero loss/reorder, and exact cleanup.
That evidence proves the published-reference timing edge, but an environment
override is not production-default acceptance.

The ordinary reference-compatible Windows profile now owns the same typed,
cancellable 1.5-second settle after the validated ECG-settings response and
before ECG start. The physical verifier removes any inherited session-profile
override so it can qualify only the default lifecycle. Other diagnostic
profiles remain closed and unchanged. Host gates pass; publication remains held
until a fresh reference-positive run proves this exact default through both
official inlets and cleanup.

## 2026-08-17 — Map raw H10 sensor time into the local LSL clock

Leaving Windows connection timing system-managed restored sustained delivery:
the physical candidate reached both first frames, both independent Rusty
outlets, and both pinned official inlets. The official verifier then failed
closed on timestamp reorder (ECG 4, ACC 5). Setup had buffered several PMD
notifications, and the coordinator drained them faster than their device-time
spacing. Stamping each drained notification at the current local clock made
successive nominal-rate backfilled chunks overlap.

Each raw ECG and ACC outlet now establishes an independent offset from the
first nonzero H10 sensor timestamp to the backend's local LSL clock. Later
chunks map their newest sensor timestamp through that fixed offset, then retain
the existing nominal-rate backfill within the chunk. This preserves device-time
spacing while keeping published timestamps in the host LSL clock domain; the
raw device timestamp is never inserted directly. Zero timestamps and derived
scalar outputs retain the previous local-clock behavior. The same mapping is
owned by the liblsl and default-off Rusty LSL implementations. Host evidence is
green; physical acceptance and publication remain held for a fresh same-epoch
run.

## 2026-08-17 — Leave Windows connection timing system-managed

Moving optional battery access before subscriptions did not restore sustained
notifications, so that candidate was reverted. The resulting trace again kept
a connected 232-byte session, healthy queues, and admitted official consumers
while every native notification counter stopped after the initial sensor
frames. With battery access restored to its prior location, the only remaining
mutation between first-frame qualification and steady state was the
best-effort `ThroughputOptimized` preferred-connection request.

Polar Stream now follows the published reference boundary and leaves connection
parameters under Windows ownership. It retains read-only connection interval,
peripheral latency, and MTU diagnostics, but no longer creates or retains a
preferred-parameter request after the sensor streams start. This is a bounded
Windows acquisition change; PMD commands, response gates, callbacks, battery,
Rusty LSL, default liblsl, and cleanup remain unchanged. Physical acceptance
remains held for a fresh same-epoch run.

## 2026-08-17 — Observe the first-frame-to-steady-state handoff directly

The first receiver-first physical run was reference-positive, reached both
first sensor frames, opened both pinned official inlets, and reported two
admitted Rusty consumers at source readiness. Neither the source frame
thresholds nor official sample thresholds then completed. This proves the
consumer-start correction while locating the remaining uncertainty after the
WinRT first-frame gate and before sustained application delivery; it does not
attribute the stop to Rusty LSL because the source threshold also failed.

The existing WinRT session behavior remains unchanged. While session
diagnostics are explicitly enabled, the steady-state owner now reports its
identifier-free connection snapshot and accumulated per-source
callback/decode/enqueue/fault/queue counters at entry and every five seconds.
The next attended run can therefore separate absent native notifications from
a callback-to-application handoff defect without another speculative transport
change. Physical acceptance and publication remain held.

## 2026-08-17 — Physical qualification is receiver-first at outlet readiness

The latest attended physical run reached both advancing H10 sensor streams,
initialized independent ECG and ACC Rusty outlets, and let pinned official
pylsl 1.18.2/liblsl 1.17.7 consumers resolve and open both descriptors. It did
not complete the source/consumer thresholds before the outer deadline. The
synthetic gate, which passes the same producer lifecycle and official consumers,
starts those consumers immediately after outlet initialization; the physical
verifier instead waited for BLE source readiness first.

The physical verifier now starts its official inlet worker on
`POLAR_H10_LSL_INITIALIZED`, before selection/session/source readiness, and then
independently awaits `POLAR_H10_SOURCE_READY`. Reaching source readiness before
official startup fails closed. Deterministic tests bind this order. This is a
qualification-orchestration correction only: BLE, Rusty LSL, outlet data,
product behavior, and the default liblsl package path are unchanged. Physical
acceptance and publication remain held until a fresh same-epoch run completes
both official streams and exact cleanup.

## 2026-08-17 — Bind physical qualification to the landed two-inlet merge

The default-off Rusty LSL backend is now pinned to reviewed upstream merge
`8b6b2a6cd0c0e5147b7e1cc076a116ef226cddbd`. Its exact reviewed tree preserves
the one-channel initialization contract, admits the three-channel ACC shape,
and routes simultaneous official ECG/ACC full-info requests independently from
their data-consumer slots. Polar Stream retains its stricter deployment bound
of one official data consumer per outlet and continues to use broad discovery
plus exact client-side descriptor matching; predicate-filter conformance is not
claimed.

This dependency advancement prepares the existing synthetic and physical H10
verifiers against the only landed Rusty LSL authority. It is not physical H10,
browser, LabRecorder, package, licensing, or release evidence. Default and
packaged builds continue to use liblsl, and the physical publication hold
remains until one same-epoch ECG/ACC official-inlet chain passes cleanly.

## 2026-08-17 — Reopen the reference settings dwell after a full-doctor pass

In one exact lease the published PolarH10 doctor reached PMD ready, a control
response, ECG frames, ACC frames, and heart-rate notification with no failure.
Polar Stream's PMD-only synchronous-owner profile then timed out with zero PMD
data callbacks, and its full reference-compatible profile did the same. In the
immediately preceding epochs, the minimal Rust PMD probe also timed out after
accepted settings/start responses despite having passed once earlier. That
single minimal-probe pass is therefore not stable authority for removing the
published reference's setup dwell.

The next closed verifier value is
`POLAR_STREAM_H10_SESSION_PROFILE=reference-settings-dwell`. It preserves the
full default reference-compatible lifecycle and changes only one timing edge:
after validating the exact ECG-settings response, it holds a typed,
cancellable 1.5-second settle before issuing the ECG start command, matching
the physically passing full doctor. The default product path, scanner,
subscription modes, response/frame validation, commands, output transports,
and publication hold remain unchanged.

## 2026-08-17 — Give the passing lifecycle one synchronous MTA owner

The reference watcher observed exactly one H10 immediately before the
`pmd-only-probe-std-handoff` input-only run. That run retained the passing
probe's PMD setup order and used the same bounded standard-library callback
channel, but still received both ECG control responses and zero PMD-data
callbacks. Callback capture and the callback-to-queue transport are therefore
eliminated.

The final closed verifier value is
`POLAR_STREAM_H10_SESSION_PROFILE=pmd-only-probe-synchronous-owner`. It keeps
the scanner and exact PMD sequence unchanged, but moves the complete selected
device lifecycle—WinRT operations, response/frame waits, steady-state receive,
stop, callback removal, CCCD rollback, and handle closure—onto the dedicated
plain MTA thread. Native completions and notifications use bounded
standard-library channels; only decoded `InputEvent` values cross to the
existing application channel. Every native operation retains a deadline and
cancellation route, callback/channel faults fail closed, and active-session
cleanup remains generation-safe. This profile is diagnostic-only until the
same-epoch physical input and official-inlet gates pass; default product
behavior and the publication hold are unchanged.

## 2026-08-17 — Isolate the callback handoff from Tokio

The reference watcher observed exactly one H10 immediately before the
`pmd-only-probe-equivalent-sequence` input-only run. That run used one PMD
service-access request, direct uncached exact characteristic lookups, explicit
control-Indicate/data-Notify CCCDs, no inter-subscription delay, and no
pre-frame link-property reads. It still received both ECG control responses
while PMD-data callbacks remained zero through the first-frame deadline. Those
setup-order and property-read differences are therefore eliminated.

The next closed verifier value is
`POLAR_STREAM_H10_SESSION_PROFILE=pmd-only-probe-std-handoff`. It preserves the
failed candidate's exact WinRT setup and changes only the native callback
handoff: callbacks send bounded messages through the same standard-library
channel family as the physically passing probe, and one owned bridge forwards
them into the existing bounded Tokio queue. The bridge is stopped and joined
after reverse-order handler/CCCD cleanup, with deterministic forwarding,
fault, and no-orphan coverage. The default product path, response/frame gates,
commands, scanner, output transports, and publication hold remain unchanged.

## 2026-08-17 — Match the passing probe's PMD setup sequence

The reference-positive input-only
`pmd-only-winrt-when-all-setup` run used the passing probe's `.when`
completion/no-success-close policy throughout device, session, service,
characteristic, CCCD, and control operations. It still completed both ECG
control responses with zero PMD-data callbacks through the first-frame
deadline. The selected-device async projection and operation lifetime are
therefore eliminated; no output transport was constructed.

The next closed verifier value is
`POLAR_STREAM_H10_SESSION_PROFILE=pmd-only-probe-equivalent-sequence`. It
retains the existing bounded owner and changes only remaining setup-sequence
differences relative to the physically passing minimal probe: one PMD
service-access request, one direct uncached exact lookup for each PMD
characteristic, explicit control-Indicate and data-Notify CCCD values without
reading characteristic properties, no inter-subscription delay, and no
pre-first-frame link-property reads. Identifier-free diagnostics explicitly
mark suppressed reads and selected modes. Error/timeout cancellation, partial
rollback, final cleanup, scanner confirmation, response/frame gates, and the
default product profile remain unchanged. Publication remains held.

## 2026-08-16 — Apply probe completion policy to the full selected-device setup

The reference-positive input-only differential selected one exact H10 and,
without constructing any output transport, reproduced the same failure: device
and session setup, 232-byte PDU, PMD CCCDs/handlers, and both ECG control
responses succeeded while PMD-data callbacks remained zero through the
first-frame timeout. Rusty LSL, OSC, CSV, and output initialization are therefore
eliminated as causes; the defect remains inside the product WinRT session path.

The next closed verifier value is
`POLAR_STREAM_H10_SESSION_PROFILE=pmd-only-winrt-when-all-setup`. Relative to
the failed PMD-only `.when` candidate, it changes one remaining projection and
lifetime boundary: selected-device acquisition, GATT-session creation, PMD
service discovery/access, and characteristic discovery now also use the
physically passing probe's `windows-future` `.when` completion path without an
explicit success-time `Close()`. PMD CCCD/control operations retain that same
policy. Timeouts, cancellation, errors, rollback, and final cleanup remain under
the existing bounded owner. Scanner confirmation, battery-after-qualification,
and the default product profile remain unchanged. Publication remains held.

## 2026-08-16 — Split the product input backend from output initialization

The attended `pmd-only-winrt-when-completion` run was reference-positive and
again reached both control responses with zero PMD-data callbacks. PMD-only
`windows-future` completion projection is therefore eliminated. The link was
connected with a 232-byte PDU after the start response but reported disconnected
at the first-frame timeout; this single checkpoint does not establish whether
the disconnect caused or followed missing data delivery.

Before changing the backend again, an identifier-free input-only differential
now drives the real `InputManager` scanner/session and requires two advancing
ECG and ACC frames with nonzero samples on every ACC axis, but deliberately
constructs no LSL/OSC/CSV output owner. This separates the product WinRT backend
from the full verifier's pre-connection Rusty outlet initialization. It retains
the exact session profile and cleanup owner and emits no name, address, payload,
or stable identity. If it passes, output initialization/lifecycle is the next
boundary; if it fails, the defect remains inside the product input backend.
Publication remains held.

## 2026-08-16 — Match the passing probe's WinRT completion projection

The attended `pmd-only-retain-successful-gatt-operations` run was
reference-positive with one exact H10 and a 100% battery reading. The candidate
again completed device/session/PMD discovery, negotiated a 232-byte PDU,
committed both PMD CCCDs, attached both handlers, and received both ECG control
responses, but PMD-data callbacks remained zero through the first-frame
timeout. Explicit success-time operation `Close()` is therefore eliminated.

The next closed verifier value is
`POLAR_STREAM_H10_SESSION_PROFILE=pmd-only-winrt-when-completion`. Relative to
the failed unclosed baseline, PMD CCCD/control operations use the physically
passing probe's `windows-future` `.when` completion callback rather than the
`IntoFuture` projection. The result still crosses a bounded Tokio oneshot and
the existing per-stage owner retains timeout cancellation, failure cleanup, and
final teardown. The default product path and every non-PMD operation remain
unchanged. Publication and physical acceptance remain held.

## 2026-08-16 — Leave successful PMD GATT operations unclosed as the next differential

The attended `pmd-only-differential` run was reference-positive, selected the
exact H10, completed PMD discovery/subscriptions and both ECG control responses,
but still recorded zero PMD-data callbacks through the first-frame timeout.
Optional heart-rate discovery/subscription is therefore eliminated.

The next closed verifier value is
`POLAR_STREAM_H10_SESSION_PROFILE=pmd-only-retain-successful-gatt-operations`.
Relative to the failed PMD-only baseline it changes one behavior: successful
PMD CCCD and control-write WinRT operations are not explicitly closed
immediately after completion, matching their lifetime in the physically passing
minimal probe. Native error, timeout, cancellation, rollback, and final session
cleanup keep their cancel/close behavior. The default product profile still
closes successful operations, and unknown values reject before device setup.
This remains a local diagnostic candidate; publication and physical acceptance
remain held.

## 2026-08-16 — PMD-only production differential changes one optional branch

The next full-product candidate uses the exact closed environment value
`POLAR_STREAM_H10_SESSION_PROFILE=pmd-only-differential`. It skips optional
heart-rate service/characteristic discovery and notification subscription, then
uses the unchanged production scanner, address/device/session acquisition, PMD
service/characteristics, control/data subscriptions, response and first-frame
gates, callback queue, cleanup, Rusty outlets, and official-inlet verifier. The
physical verifier sets the value explicitly; absence preserves the normal
reference-compatible product lifecycle, and any other value rejects before
device setup.

This is a diagnostic candidate, not a supported user preference and not a
conclusion that heart rate caused the failure. It is intentionally the smallest
full-product difference from the physically passing minimal PMD probe. Host
tests bind the closed vocabulary, default behavior, verifier routing, and
identifier-free profile diagnostic. Publication remains held until the attended
source-to-official-inlet gate passes.

## 2026-08-16 — Minimal Windows PMD projection passes on the physical H10

With the Bluetooth headset and nearby 2.4 GHz mouse receiver disconnected, one
attended lease produced a reference-positive H10 observation followed by a
complete standalone-probe pass. The probe received all three exact control
responses, one 73-sample ECG frame, and one 36-sample three-axis ACC frame. This
physically proves that the H10, negotiated link, PMD commands, direct
`TypedEventHandler` projection, `windows-future` completion callbacks, and
standard-library handoff can deliver both streams on this Windows host. The RF
condition improved this epoch but one comparison does not establish either
disconnected peripheral as the sole cause of earlier advertisement misses.

The full Polar Stream verifier then ran in the same lease. It selected the exact
H10, retained a connected 232-byte-PDU session, initialized both Rusty outlets,
completed service/characteristic discovery and all three subscriptions, and
received the exact ECG settings/start responses. Its PMD-data callback count
nevertheless remained zero through the first-ECG timeout, so official inlets
were never opened. This is a production input-session failure before Rusty LSL,
not a device, scanner, protocol-command, or generic `windows-rs` callback
failure.

The next source work must compare the passing probe and product path one
behavior at a time. The bounded candidates are heart-rate exclusion, successful
WinRT-operation lifetime/projection, and the PMD callback-to-queue handoff; do
not change discovery or Rusty LSL. Publication remains held until the full
source-to-official-inlet chain passes.

## 2026-08-16 — Isolate the remaining PMD event boundary from the application

The directly retained one-write/one-handler candidate was physically
reference-positive, selected and connected the exact H10 with a 232-byte PDU,
and received both successful PMD control responses, but still received zero PMD
data callbacks. Descriptor readback and handler-wrapper ownership are therefore
no longer active hypotheses.

The next differential is a standalone Windows-only PMD probe in
`polar-h10-input`. It accepts a private Bluetooth address without scanning or
emitting it, initializes one MTA, uses `windows-future` completion callbacks and
standard-library channels, resolves only PMD control/data, requests ECG at
130 Hz and ACC at 200 Hz, and requires exact control responses plus one decoded
frame from each stream. It deliberately excludes Tauri, Tokio, LSL, heart rate,
UI state, and application queues. Every operation and frame wait is bounded;
timeouts request cancellation, event handlers and CCCDs are removed in reverse
order, and cleanup is guarded exactly once.

The behavioral comparison remains the public `MesmerPrism/PolarH10` Windows
path at `3777ccf6970d2a0457d0a4be99e6c15645818db0`; no implementation source was
copied. Host tests prove parsing, response validation, completion outcomes,
cancellation, and cleanup gating only. An attended same-lease reference-positive
device run is required before this probe can diagnose or clear the remaining
physical projection boundary, and it does not change the publication hold.

## 2026-08-16 — Subscription projection matches the passing Windows reference

The attended dedicated-apartment candidate was reference-positive, connected
with a negotiated 232-byte PDU, received heart-rate and both expected PMD
control responses, and still received zero PMD data callbacks before the first
ECG-frame deadline. Confining windows-rs, its current-thread executor, every
GATT object, and teardown to one explicitly initialized MTA therefore did not
fix the physical boundary and is no longer the active hypothesis.

The passing `MesmerPrism/PolarH10` projection performs one successful CCCD
write and immediately registers a directly retained `TypedEventHandler`. Polar
Stream had inserted a descriptor readback between those operations and retained
an `AgileReference` rather than the handler itself. The next candidate removes
only those extra projection operations while preserving the proven
write-then-handler order, direct characteristic ownership, response gates,
deadlines, queues, and exactly-once cleanup. A typed activation guard rejects
handler-before-write and duplicate-write/handler states; deterministic tests
cover the valid sequence and rollback boundary. Prior exact CCCD-readback
evidence remains valid historical evidence, but readback is no longer part of
the acquisition path. This remains host evidence until an attended run reaches
both sensor streams and the pinned official consumers.

## 2026-08-16 — One explicit Windows apartment owns the selected H10

The reference-aligned physical run kept a healthy 232-byte connection, confirmed
all three CCCDs, and delivered heart-rate plus PMD-control events, but PMD data
still entered zero callbacks. Link state, command admission, subscription state,
delegate retention, service order, and reference timing are therefore exhausted
as candidate causes. Polar Stream previously created and awaited WinRT objects
on a multithreaded application runtime whose worker could change across awaits.

The complete selected-device lifecycle now runs on one named OS thread with an
explicitly initialized multithreaded Windows Runtime apartment and a
current-thread Tokio executor. The same owner performs setup, receives callbacks,
runs steady state, sends stop commands, removes handlers, closes handles, and
balances `RoUninitialize`. Only the required OS apartment initialization and
teardown calls are isolated `unsafe` boundaries; all GATT work remains through
windows-rs. The caller still awaits setup and disconnect asynchronously, while a
drop guard guarantees completion signalling on unwind. Host tests prove thread
identity across async yields and guard behavior. Physical acceptance remains
held for the next attended run.

## 2026-08-16 — Windows qualification follows the proven reference lifecycle

The exact CCCD-readback physical run confirmed PMD data was set to Notify and
its delegate remained owned, yet no PMD data event entered while the link stayed
connected at a 232-byte PDU and PMD control responses advanced. Repeating the
same ordering cannot add evidence. The remaining black-box difference from the
published, physically passing Windows reference is setup sequencing.

After creating its persistent `GattSession`, Polar Stream now uses the
reference's bounded 500 ms connection settle, discovers optional heart rate
before required PMD, and defers optional battery discovery/read until both ECG
and ACC have qualified. The settle is cancellable and typed; all service and
characteristic calls retain their individual deadlines and cleanup. This is a
behavioral lifecycle contract derived from the published reference, not copied
implementation. It remains host evidence until a fresh attended run passes both
sensor streams and the pinned official-consumer chain.

## 2026-08-16 — Notification admission verifies CCCD state and owns delegates

A reference-positive physical epoch proved the selected H10 remained connected
with a negotiated 232-byte PDU before and after its accepted ECG start response.
PMD control indications and heart-rate notifications entered, decoded, and
queued through the same WinRT callback bridge, while PMD data entered zero
callbacks. The failure is therefore narrower than link readiness, MTU, generic
event dispatch, buffer conversion, queueing, or Rusty LSL.

Every Windows notification subscription now reads the CCCD back after a
successful write and requires the exact requested Notify/Indicate value. A
failed or mismatched readback disables that CCCD before setup returns. Each
registered `TypedEventHandler` also has an explicit agile owner retained beside
its removal token until exactly-once teardown. These changes make OS-side
subscription state and delegate lifetime explicit; they do not claim to have
fixed the physical PMD data boundary. Another attended same-epoch run must still
reach advancing ECG and ACC, both Rusty outlets, and both pinned official inlets
before publication.

## 2026-08-16 — PMD startup is response-gated and qualifies ECG before ACC

The first typed physical trace proved the selected H10 reached a persistent
session, uncached PMD discovery, control/data notification subscription, and
successful GATT writes for both start commands. Neither first ECG nor first ACC
frame arrived. A successful WinRT write is transport evidence only; it does not
prove that PMD accepted the command or that two commands were safely sequenced.

The known-working published reference doctor requests ECG settings, observes
the ECG control/data phase, and starts ACC afterward. Polar Stream adopts that
behavioral contract without copying its implementation or blind fixed delays:
request ECG settings → validate the exact successful response → start ECG →
validate its response → decode the first ECG frame → start ACC → validate its
response → decode the first ACC frame. Malformed, rejected, missing, duplicate
or out-of-order control responses fail closed under individual deadlines.
Early HR or sensor events remain bounded and buffered; callback/session cleanup
remains exactly-once.

This is host-tested repair evidence, not physical acceptance. The next attended
run must pass both first-frame stages before Rusty outlets and pinned official
inlets are admitted. Scanner admission and the already-passing GATT setup stages
are not reopened by this change.

## 2026-08-16 — WinRT session setup owns typed stages before physical acceptance

A same-lease differential physical run observed one exact H10 through the
published `MesmerPrism/PolarH10` watcher and the repaired Polar Stream watcher.
Polar Stream selected the candidate and entered WinRT session setup, but did not
qualify both first sensor frames before its outer readiness deadline. This is
positive evidence for discovery and exact-model selection only. It is not ECG,
ACC, Rusty outlet, official-inlet, or complete device-acceptance evidence.

At that checkpoint the existing session order remained unchanged while the
first failing stage was diagnosed. Address-to-device acquisition, `GattSession` creation,
maintain-connection, uncached service discovery, per-characteristic access and
uncached/cached resolution, CCCD subscription, ECG/ACC start commands, and each
first frame now have separate typed observations. Every WinRT async operation
has its own deadline and cancel/close owner inside a 45-second setup budget, so
the verifier's 60-second outer readiness deadline cannot be the first failure
classification. Partial subscriptions roll back in reverse order; callback
tokens and session cleanup are claimed once.

`POLAR_STREAM_H10_SESSION_DIAGNOSTICS` emits stage, attempt, transition,
duration, and result class plus aggregate notification diagnostics. The latter
records each characteristic's Notify/Indicate/read/write property shape, the
selected CCCD mode, handler attachment/removal counts, and per-source callback,
decode, enqueue, fault, and queue outcomes. It never emits an address, name,
payload, manufacturer value, payload size, or stable device identity.
Identifier-free link checkpoints also expose only connection state, negotiated
PDU size, connection-interval units, and peripheral latency. The
subsequent physical run
stopped at the first typed non-success stage and compared that ordering with the
exact published reference behavior before the response-gated change above. The
reference uses persistent `GattSession`, uncached service discovery,
`RequestAccessAsync`, bounded uncached/cached characteristic lookup, direct
CCCD subscription, and explicit cleanup; no reference source is copied.

## 2026-08-16 — H10 advertisement evidence is route-independent and confirmed

The Windows watcher keeps its bounded start/stop/callback lifecycle, but
local-name and advertised-service evidence are evaluated independently. An
exact `Polar H10` name is strong evidence even if WinRT cannot enumerate that
packet's service UUIDs. A PMD/heart-rate service without an exact name is only
a provisional candidate; a globally bounded, eight-way WinRT device-name
sweep must resolve the exact H10 model before it is returned. Generic BLE
presence, manufacturer data, and other Polar model names remain insufficient.
The later persistent session must still expose PMD and deliver both ECG and
three-axis ACC before connection success.

This corrects a predicate-order mismatch found by differential observation.
An identifier-free harness around the exact published `MesmerPrism/PolarH10`
watcher at `3777ccf6970d2a0457d0a4be99e6c15645818db0` observed one physical H10
through its exact local-name shape. The reference path did not require service
UUID evidence. The published reference CLI `scan` command returns immediately
after starting its asynchronous watcher; its earlier zero result was therefore
invalid and is not device-state evidence. No reference implementation source,
identifier, or advertisement payload is incorporated.

`POLAR_STREAM_H10_SCAN_DIAGNOSTICS` reports only aggregate counts for readable
advertisement fields, exact-name/service admission, missing names, duplicates,
rejection, overflow, property confirmation, and returned candidates. It never
reports names, addresses, manufacturer values, payload bytes, or stable device
identity. Deterministic fixtures cover the accepted shape and damaged,
ambiguous, duplicate, missing-name, non-H10, and capacity cases. Physical
selection and the complete Rusty LSL chain remain held for a separately
authorized attended run.

## 2026-08-15 — Windows owns one direct WinRT Bluetooth session

Windows uses an active WinRT advertisement watcher for up to fifteen seconds
and coalesces at most 256 unique matching addresses before exact device
selection. It stops early after observing exact H10 local-name evidence. The
longer ceiling is bound to a same-lease differential run where the known
reference observed only four exact H10 packets over fifteen seconds while the
candidate's prior four-second window received 41 unrelated advertisements and
no H10 packet.
The callback is removed before return, cleanup calls `Stop` only while the
watcher is actively started, and no per-device property request can stall the
scan. After selection, `polar-h10-input` opens the Bluetooth address
with WinRT and owns one persistent `GattSession`, uncached PMD service
discovery, access requests, characteristic discovery, notification handlers,
start commands, and teardown. Linux, macOS, iOS, and Android retain the existing
`btleplug` connection path.

The Windows session does not announce connection success until it has decoded
both an ECG frame and a three-axis ACC frame. Setup stages have explicit
deadlines and observe cancellation at stage boundaries. Notification and
first-frame buffers are bounded and fail closed rather than lose data silently;
shutdown has one global deadline followed by synchronous handler and WinRT
handle release. A throughput-optimized connection request remains best effort,
and negotiated MTU remains a read-only observation.

This is an original safe-Rust adapter using the repository's existing
`windows` crate. Its behavioral reference is the public MIT-licensed
[`MesmerPrism/PolarH10`](https://github.com/MesmerPrism/PolarH10) Windows
transport at commit `3777ccf6970d2a0457d0a4be99e6c15645818db0`: active
advertisement discovery, persistent session ownership, `RequestAccessAsync`,
uncached discovery/retry, direct CCCD subscription, and explicit close. No C#
source or local experimental evidence is incorporated. Compilation and synthetic tests do not replace a physical H10
acceptance run.

## 2026-08-15 — Rusty outlets admit one official consumer

Polar Stream bounds every optional Rusty LSL outlet to one official consumer.
The synchronous caller-polled work runs on Tokio's blocking pool with an
explicit stop signal and bounded join, and Windows outlet setup reserves one
numeric port across TCP and UDP before registry admission. Deterministic tests
require a second concurrent consumer to reject without disturbing the admitted
consumer and require conflicted port candidates to retry and release cleanly.

This is a bounded Polar Stream deployment workaround, not generic Rusty LSL
multi-consumer or auxiliary-connection conformance. If the product later needs
multiple official consumers of one outlet, that is a separate Rusty LSL source
unit. Two physical straps passed the independent native WinRT doctor, while
Polar Stream's Windows `btleplug` path stopped at connect, notification setup,
or first PMD frame. Therefore the draft retains no physical-acceptance claim;
a native WinRT backend is separately scoped.

## 2026-08-14 — Rusty LSL is an exact-pinned, default-off native backend

Packaged and default builds retain liblsl. Source developers may select exactly
one mutually exclusive `rusty-lsl-backend`, pinned to reviewed Rusty LSL merge
`8b6b2a6cd0c0e5147b7e1cc076a116ef226cddbd`. It stays inside
`polar-h10-output`, preserves the canonical names/metadata and independent
ECG/ACC outlets, and never enters the browser runtime.

Qualification uses pinned official pylsl 1.18.2/liblsl 1.17.7 broad
enumeration, followed by exact client-side matching of name, type, channel
count, nominal rate, channel format, and source ID. Zero, multiple, mismatched,
or cross-stream candidates reject. Server-side `resolve_byprop` predicate
conformance is intentionally unsupported and cannot be implied by discovery
success.

Host interoperability passed for 1-channel/130 Hz ECG and 3-channel/200 Hz ACC.
The backend remains device-unaccepted. No package may enable it until physical
and cross-platform gates pass. Rusty LSL is
AGPL-3.0-or-later while Polar Stream is MIT; enabled distribution additionally
requires an explicit licensing/compliance decision.

## 2026-08-15 — Recorded previews form continuous circular presentation signals

The hardware-free 60-second recording repeats without an empty interval or a
visible end-to-start jump. Recorded-input playback applies a smooth, bounded
1.2-second endpoint correction to ECG, all three ACC axes, HR and RR; the
checked-in fixture remains unchanged. This conditioning is identified as
recorded-preview behavior and cannot enter live H10 acquisition, native raw
publication, or physical-device claims.

The selected metric SVG tiles a closed path, and Formula Lab continuously
scrolls two closed copies of its recorded before/after trace. Metric list rows
remain static so the library never runs dozens of concurrent decorative
animations. Categorical outputs such as breathing phase remain integer-stepped
rather than acquiring invented intermediate values. Reduced-motion users receive
the same closed trace as a static view instead of forced animation.

## 2026-08-22 — Metric detail is a concise preview, summary, and reviewed sources

Clicking a metric opens one focused window with exactly three content sections:
one looping SVG output example, a two- or three-sentence scientific summary, and
two or three reviewed sources. Formula definitions, stream metadata, evidence
badges, and pre-add settings do not appear in this window; Formula Lab and the
post-add **Adjust** flow retain those capabilities.

The Rust catalog is authoritative for each metric's source set, and the browser
asset is generated from it. Native citation opening accepts only an exact HTTPS
URL from the selected metric's reviewed set, preventing arbitrary shell-open
requests from a modified frontend.

## 2026-08-15 — Completed edits synchronize source, desktop install, and live Pages

The user-visible application has three delivery surfaces: canonical source,
the resolved per-user desktop installation, and the public Pages app at
`https://georgefejer91.github.io/Polar-Stream/`. A completed edit must leave all
three on the same accepted UI state. The delivery workflow therefore includes
production build, rollback-preserving local install and smoke test, reviewed
publication to `main`, Pages deployment, and live manifest/hash verification.
Every edit handoff includes the public URL. A branch-only, staged-only, or
locally installed change is incomplete; unrelated dirty work is never swept
into a deployment to meet this rule.

## 2026-08-15 — Previews come from one real recording; formulas are bounded native outputs

`apps/polar-stream/ui/data/preview-recording.json` is the sole hardware-free
preview signal: an anonymized 60-second real H10 ECG/ACC recording. Browser
replay and every metric preview derive from it, and the generated preview asset
records its hash. NeuroKit may be used for offline cleaning/method provenance,
but no simulated signal is a shipped fallback.

The metric catalog remains the Rust source of truth for labels, scientific
context, citations, formulas, and executable templates; Pages consumes a
generated catalog asset. Formula Lab retains sensor/event time automatically
and evaluates a scalar y-value from exactly one source clock (`ecg`, `x/y/z`,
`hr`, or `rr`). Expressions have no assignment, loops, strings, or user-defined
functions and are bounded by formula count, expression/AST depth, operations,
and retained state. Specialized spectral or multi-stage metrics receive honest
mathematical definitions without a fake one-line executable template.

## 2026-08-14 — Frontend controls do not disappear with backend capability

Tauri and GitHub Pages use one canonical frontend, including the same control
placement, responsive formatting, and interaction flows. Runtime adapters may
produce different results only where platform capability genuinely differs.
The missing capability must not be handled by removing or disabling the shared
control: an attempted unsupported action fails closed and presents a specific
error plus a useful next step.

Accordingly, Pages continues to display interactive LSL and OSC toggles. A
browser attempt leaves the toggle off and shows that native publication requires
the installed app, with a link to the latest GitHub release. Pages still does
not publish, relay, or rename another transport as LSL/OSC. This supersedes only
the control-hiding portion of the 2026-08-13 self-contained-browser decision;
the no-companion and native-protocol boundaries remain in force.

## 2026-08-14 - Browser Bluetooth compatibility is capability-based

The Pages adapter does not admit or reject browsers by brand. It checks the
secure context and `navigator.bluetooth.requestDevice`, then lets the chooser
remain authoritative and diagnoses `bluetooth` Permissions Policy rejection if
it occurs. Browser names remain guidance only because Chromium forks can
independently ship, gate, or remove Web Bluetooth. A browser-level or
administrator-level block cannot be repaired by site code.

The H10 chooser must be the first awaited browser operation in the click/touch
connection path so transient user activation survives stricter Chromium
implementations. After selection, the adapter may retry one transient
`NetworkError`/`AbortError` from the initial GATT connection before any service
subscription exists. PMD control writes prefer `writeValueWithResponse` and
retain the older `writeValue` fallback. These changes improve compatibility
without retrying the chooser, duplicating subscriptions, or weakening the
foreground-only and physical-validation boundaries.

## 2026-08-14 — Local CSV is bounded and independent of native publication

The shared Output panel exposes one **Save local CSV** toggle in Tauri and
Pages. Native H10 recording belongs to `polar-h10-output`: immediate selected
LSL/OSC publication happens first, then a decoded notification is copied with a
non-blocking `try_send` into a dedicated 128-batch CSV writer. Formatting and
filesystem I/O never enter the sensor path. A full/disconnected queue or writer
error stops CSV and reports a best-effort UI warning; it may not grow, block, or
silently discard accepted rows. Native files include all received raw ECG/ACC,
HR/RR, and every derived metric produced by active processors.

This supersedes the selected-output CSV scope recorded in the 2026-08-13 Pages
decision below; the row and memory bounds remain in force.

Pages retains a separate 300,000-row in-tab recorder because a hosted page does
not have Rust filesystem authority. It now records every received raw row and
every metric event the browser produces, independent of which outputs are
selected for visualization. Turning the toggle off uses the initiating gesture
to download the file. Browser capture remains vulnerable to tab lifecycle and
must be downloaded before the page is lost.

## 2026-08-14 — Audio data is an experimental cable modem, not encryption

The shared frontend may emit compact raw/metric batches as 22.05 kbit/s stereo
Manchester PCM with a preamble, sequence number, and CRC32. This rate is chosen
to fit nominal 130 Hz signed-24-bit ECG plus 200 Hz three-axis signed-16-bit ACC
and framing over clean stereo AUX/digital loopback. CRC detects corruption; the
format has no forward error correction, confidentiality, authentication, or
tamper protection and must never be described as encryption.

The implementation remains in the shared Web Audio layer and is bounded to a
1.25-second scheduled horizon. It is not authoritative in Tauri because the
bounded display channel may omit events. Native CSV/LSL/OSC remain the reliable
paths. `scripts/decode_audio_data.py` is the public stereo-PCM-WAV reference
decoder and browser validation must round-trip the production encoder through
it. Speaker/microphone and mono/lossy paths remain outside the claim.

## 2026-08-14 — Browser H10 is foreground-only despite wake-lock protection

Chrome on Android can grant a visible secure page direct BLE GATT access, so
Pages requests a best-effort screen wake lock after H10 connection and
reacquires it when the document becomes visible again. This does not authorize
or technically provide background capture. Web Bluetooth is unavailable in Web
Workers/service workers, active screen locks are released for inactive/hidden
documents, and mobile Chrome may freeze or discard a page without a final
callback. Switching apps/tabs or locking the screen can therefore interrupt
Bluetooth, CSV, and audio.

The UI warns after returning from a hidden browser-BLE session and sensor
timestamps remain the gap evidence. A guaranteed smartphone locked-screen or
other-app-foreground workflow is a separate native Android foreground-service
feature, not something GitHub Pages or PWA installation can promise.

## 2026-08-13 — Pages records and shares browser events without claiming LSL

The self-contained Pages path exposes every exact browser input batch through a
same-tab `polar-stream-data` event and same-origin `polar-stream-live-v1`
`BroadcastChannel`. It also records the currently selected outputs to a local
CSV before visualization decimation. Raw units are retained and device sample
times are reconstructed from PMD frame timestamps when present.

This path is implemented from scratch in the canonical UI and requires no
localhost service, remote relay, browser extension, or installed wrapper. Its
in-memory file is capped at 300,000 rows, stops visibly at capacity or input
disconnect, and must be downloaded before the tab closes. The browser event,
BroadcastChannel, and CSV are not LSL: native discovery and LabRecorder
interoperability remain installed-app capabilities because a normal Pages tab
cannot open LSL's multicast UDP and general TCP/UDP sockets.

## 2026-08-13 — GitHub Pages is a self-contained browser application

This supersedes the paired browser-to-native LSL decision below. The browser
workflow must require only the loaded Pages application and the browser's own
Web Bluetooth permission. H10/mock acquisition, browser-supported processing,
and visualization stay in the tab. Pages must not call a localhost companion,
installed wrapper, or remote relay, and it must not rename WebSocket/HTTP output
as LSL.

Ordinary hosted pages cannot open the raw UDP discovery/multicast and TCP/UDP
sockets required by native LSL and OSC. Their destination controls are therefore
hidden in browser mode rather than shown disabled or conditionally paired.
Native LSL/OSC remain available in the separately installed Tauri app. The
removed companion remains documented below only as superseded history.

## 2026-08-13 — Paired browser sessions may publish through native LSL

This supersedes the earlier blanket rule that Pages must always disable LSL.
An ordinary page still disables LSL and OSC because browser tabs cannot open
LSL discovery/data or UDP OSC sockets. The installed app may now explicitly
start an ephemeral IPv4-loopback companion and open the canonical Pages URL
with a random 128-bit bearer token in the fragment. The page removes the
fragment immediately, keeps the token in memory, and must pass exact
origin/Host/token checks plus Chromium Local Network Access before a dedicated
native `OutputRouter` accepts bounded ECG, ACC, or metric batches. Each desktop
launch replaces the prior pairing, and the first authenticated random browser
client identifier exclusively owns the new session. Browser OSC remains
unavailable.

The bridge is a separate browser acquisition route, never a detour for native
H10 data. Its page queue is bounded and observable, the server enforces request
and concurrency limits, and errors disable publication rather than grow or retry
without limit. It adds serialization, a 20 ms flush window, HTTP scheduling, and
receipt-time LSL stamping, so direct native acquisition remains preferred. The
current companion is same-computer only; a phone cannot reach a desktop's
loopback listener.

## 2026-08-13 — Port MesmerPrism Windows GATT access, not a forced MTU

MesmerPrism's relevant durable Windows fix is in
`WindowsGattServiceHandle.GetCharacteristicAsync`: call
`GattDeviceService.RequestAccessAsync`, discover uncached, and retry short-lived
`AccessDenied`/`Unreachable` states. Polar Stream primes the PMD service and its
control/data characteristics with the same bounded three-attempt pattern before
ordinary `btleplug` discovery. The Windows 11 `ThroughputOptimized` request is
also moved immediately after connection.

This does not supersede the MTU decision below. MesmerPrism's Windows
`RequestMtuAsync` only returns read-only `GattSession.MaxPduSize`; neither repo
can force a smaller ATT MTU through WinRT. Physical Windows H10 evidence is still
required before claiming an applied connection interval or loss improvement.

## 2026-08-13 — Windows MTU is observed; connection timing is requested

Do not describe Windows as allowing Polar Stream to force an ATT MTU. WinRT
owns MTU negotiation and exposes `GattSession.MaxPduSize` read-only. The native
Windows 11+ path instead makes a best-effort `ThroughputOptimized` preferred
connection-parameter request through `btleplug`, then reports the observed
connection interval, peripheral latency, and negotiated MTU. The request may be
unavailable or rejected, especially on older Windows versions, and must fail
soft without stopping ECG/ACC acquisition. Cross-platform CI is not physical
H10 validation; preserve device-run diagnostics before claiming the shorter
interval was applied.

## 2026-08-13 — Desktop and browser use one canonical interface

GitHub Pages is a presentation demo of the Tauri application, not a separately
designed website. Pages artifacts are staged from `apps/polar-stream/ui/`, and
the same HTML, CSS, application logic, metric library, and visualizations ship
in both targets. At the time, a runtime adapter selected native IPC or a local
NeuroKit mock. The 2026-08-15 recorded-preview decision supersedes that mock
signal with the canonical real fixture while preserving isolation from native
acquisition/publication. CI must exercise the
staged Pages artifact at desktop and smartphone widths so visual feedback on
the hosted version remains applicable to the desktop interface.

The direct-H10 adapter remains experimental. Web Bluetooth is available only in
compatible secure Chromium contexts and requires an explicit chooser. Recorded
preview replay is the hardware-free path and claims neither native timing,
LSL/OSC transport, nor scientific validation.

## 2026-08-13 — Browser H10 input is experimental and local to the tab

GitHub Pages may acquire H10 ECG/ACC and standard HR/RR directly through Web
Bluetooth. The adapter mirrors Polar PMD commands/decoding and only the two
retained ACC breathing processors so the canonical interface can receive live
data without a local app. It is feature-detected, visibly permission-gated, and
must retain the hardware-free recorded preview. Physical-H10 verification is required before a
release claim.

Ordinary browser tabs cannot provide the native socket behavior used by LSL
discovery/data or UDP OSC. Pages therefore disables both destinations instead
of simulating publication. Tauri remains the authoritative acquisition and
publication path. Browser code must not be imported into the Rust hot path.

## 2026-08-13 — ACC breathing provenance and parameters are explicit

`docs/acc-breathing-handoff.md` is the canonical handoff for the two
experimental ACC breathing outputs. It distinguishes verbal Johannes
provenance, public MesmerPrism/PolarH10 code history, and Polar Stream's Rust and
browser adaptations. The add/adjust UI exposes all current experiment controls,
while fixed constants and the known notification-batch-dependent phase
threshold are documented. No tuning setting may be described as validated
until comparison with an independent respiratory reference supports it.

## 2026-08-13 — New ACC breathing selection is deliberately limited

The output library separates red ECG-derived outputs from blue accelerometer
outputs. New ACC breathing selection exposes only two independent scalar
streams: a continuous selected-axis projection in g (optionally normalized by
the output layer) and a three-state phase class (`+1`, `0`, `-1`). They share
native axis and smoothing settings, default to X + Z without rotational Y, and
remain labeled unvalidated. Legacy breathing telemetry stays catalogued only so
saved configurations can migrate safely; it is not offered as a new library
choice. The phase circle is presentation-only and derives its inertial,
asymptotically bounded motion from phase values rather than a hidden volume
stream.

## 2026-08-13 — The dedicated repository is public

`GeorgeFejer91/Polar-Stream` is publicly readable. Keep credentials,
participant-identifying data, recordings, local preferences, signing material,
and unpublished research data out of Git history; public visibility makes the
existing data-minimization and secret-scanning rules release-critical.

## 2026-08-13 — Startup requires the Tauri-Rust developer skill

Every future repository agent must invoke the installed `tauri-rust developer`
skill before substantive work. If unavailable, the agent must disclose that
fact and use the explicit repository fallback without pretending the skill ran.
This keeps specialized Tauri/Rust review routine while leaving an auditable path
for environments where the personal skill is not installed.

## 2026-08-13 — Signal integrity precedes presentation

The engineering priority order is raw signal integrity, observable publication,
latency and jitter, cross-platform correctness, derived metric quality, then UI
presentation. Raw publication therefore precedes optional derived processing,
and bounded display loss may never become sensor or output backpressure.

## 2026-08-13 — Canonical AI context lives in `for-ai/`

The root `AGENTS.md` is a short bootstrap that requires future repository-aware
agents to read `for-ai/`. Detailed context lives in one discoverable folder so
instructions, goals, current state, and handoff practices can evolve without a
large root file.

## 2026-08-13 — Raw ACC uses one stacked visualization

Raw acceleration is one three-channel sensor output. The UI therefore exposes a
single raw-ACC visualizer and shows X, Y, and Z in stacked lanes with distinct
colors and independent zero-centered display ranges. This removes unnecessary
axis switching while retaining sign and per-axis readability.

## 2026-08-12 — Rust core with a Tauri system-WebView shell

Rust owns protocol, input, metrics, output, and application coordination. Tauri
provides an HTML-driven UI without bundling another browser engine. The WebView
is never placed on the authoritative acquisition/publication path.

## 2026-08-12 — One canonical output name across transports

Metric metadata defines one stable suffix. LSL uses the resulting full name and
OSC uses that same name with a leading slash. Transport-specific naming drift is
not allowed.
