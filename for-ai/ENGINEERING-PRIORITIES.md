# Engineering priorities

This is the repository-specific design contract for future implementation and
review. Optimize in this order:

1. preserve raw signal integrity and timing evidence;
2. keep acquisition and selected raw outputs continuous and observable;
3. minimize end-to-end latency, jitter, copying, and avoidable allocation;
4. retain correct behavior across supported operating systems and packaging;
5. calculate derived metrics accurately within explicit evidence limits;
6. render a responsive UI without making it part of the authoritative path.

A visually smoother interface or a new metric never justifies hidden sample
loss, changed units, reordered timestamps, transport backpressure, or reduced
platform support.

## Ownership and dependency direction

- `polar-h10-core` owns byte decoding, protocol units, and platform-neutral
  primitives. It must not depend on BLE, Tauri, transports, or the UI.
- `polar-h10-input` owns BLE adapters and emits typed notifications. Keep
  operating-system workarounds behind narrow adapters or `cfg` boundaries.
- `polar-h10-metrics` owns opt-in transformations, windows, formulas, quality
  gates, evidence metadata, and formula tests.
- `polar-h10-output` owns canonical discovery names, fail-soft LSL/OSC
  transport behavior, and bounded native CSV persistence. Transport code must
  not know about the WebView.
- `apps/polar-stream` coordinates lifecycle and IPC. It must not become a second
  protocol decoder, metric engine, catalog, or transport implementation.
- JavaScript consumes bounded display data. It may decimate or skip visual
  frames, but it must never publish authoritative research data.
- Browser-demo behavior belongs behind the shared frontend runtime API. Keep
  the HTML, CSS, application logic, metric previews, and visualizations shared
  with Tauri, and do not fork a second web-only interface implementation.

Dependencies flow from the thin app into domain crates, never from domain
crates back into Tauri. Platform-specific packaging stays outside reusable
signal and metric logic.

## Raw-path contract

For each BLE notification, preserve this order:

```text
receive -> validate/decode once -> publish selected raw output -> run selected
derived processors -> enqueue a best-effort display update
```

On the receive-to-raw-publish path:

- do not wait for the WebView, disk, logging, preferences, network retries, or
  derived metric completion;
- do not add timer-based batching beyond the sensor notification boundary;
- do not hold locks across `.await`, perform unbounded growth, sort windows,
  run transforms, or allocate per sample when reusable notification buffers or
  scratch storage can do the job;
- use bounded channels with a documented overflow policy; display data may
  coalesce or drop, while control-state delivery remains ordered;
- publish native units and original channel order unless an explicitly named,
  metadata-bearing derived output says otherwise; and
- treat sensor timestamps and channel mapping as evidence. Do not replace,
  synthesize, smooth, or reorder them silently.

Expensive work such as sorting, spectral analysis, resampling, or complexity
estimation belongs in bounded, opt-in processors with explicit cadence. A
raw-only configuration must not maintain their windows.

Today, selected derived processors run after raw publication in the coordinator
that receives the next input event. Keep their p99 runtime below the incoming
notification interval. If measurement shows that budget is exceeded, move the
derived work behind its own bounded, timestamp-preserving boundary with an
explicit coalescing/drop policy; never fix it by delaying or batching raw output.

## Signal quality and metric contract

- Raw ECG is microvolts; raw ACC axes are milligravity at protocol boundaries.
- Never silently filter, normalize, interpolate, clip, or impute raw output.
- A derived output must define source signal, units, window, cadence, reset
  behavior, rejection rules, quality limits, evidence, and interpretation.
- Non-finite or invalid samples must be rejected or surfaced deliberately, not
  allowed to poison a session window.
- Calibration, reconnect, source change, and measurement reset boundaries must
  be explicit and consistently reset all state that depends on them.
- Experimental outputs remain labeled experimental until validated against an
  appropriate reference. Synthetic previews demonstrate shape, not accuracy.
- Visualization scaling is presentation-only. It must not mutate native output
  values or create a competing metric calculation.

## Latency and reliability contract

Measure latency by stage rather than relying on subjective UI smoothness:

1. BLE notification arrival to completed decode;
2. decode to raw LSL/OSC publication;
3. raw publication to derived emission;
4. UI queue entry to rendered frame.

Use representative ECG/ACC rates, notification sizes, ATT MTUs, enabled-output
sets, and stalled/hidden WebView cases. Record p50, p95, p99, maximum, queue
high-water marks, and sample/packet loss when adding performance telemetry.

Transport failures must remain fail-soft for acquisition, but not invisible.
Add non-blocking counters or health state for queue overflow, malformed frames,
OSC send failures, LSL failures, reconnects, and measurement resets. Reporting
must never introduce backpressure on the path it observes.

Prefer preallocated/reused buffers, typed ring buffers, notification-sized
publication, and dirty-driven rendering. Benchmark before accepting extra
copies, synchronization, serialization, or higher render cadence.

## Cross-platform contract

- Reusable Rust crates must use platform-neutral paths, time types, and APIs.
- Isolate Linux BlueZ, Windows WinRT/WebView2, macOS CoreBluetooth/WebKit, and
  Android permission/lifecycle differences behind adapters or build config.
- Do not introduce shell assumptions, case-sensitive path dependencies,
  hard-coded separators, architecture assumptions, or host-only build steps.
- Preserve the stable camelCase IPC payloads and schema-versioned native
  preferences across upgrades.
- Validate Linux, Windows, and macOS in CI. Changes touching Android-capable
  crates must compile independently of desktop-only Tauri glue where practical.
- Packaging is part of correctness: bundled liblsl, runtime dependencies,
  permissions, signing state, checksums, and exact-package smoke tests matter.
- Unsupported hardware or platform claims must be explicit; never infer
  physical BLE validation from a host-only unit or interface test.
- Treat the hosted browser demo as another presentation target: it must remain
  usable at 320 CSS pixels, with touch-sized controls, safe-area-aware chrome,
  no horizontal page overflow, and dialogs that fit the visual viewport.
- Browser fixtures must be deterministic, synthetic, generated from the pinned
  development NeuroKit workflow, and labeled as such. They demonstrate UI
  behavior only. The same mock module may run offline in Tauri, but must stay
  isolated from real native acquisition and publication code.
- Web Bluetooth is not a portability assumption. The browser H10 path is an
  explicit experimental adapter with secure-context, permission,
  browser-support, PMD protocol, and physical-device validation boundaries.
  It must stay feature-detected and self-contained. Browser LSL/OSC stay
  unavailable and hidden; neither browser input may weaken or enter the native
  authoritative data path.
- Do not add a localhost companion, installed helper, remote relay, or renamed
  WebSocket/HTTP protocol to make Pages appear to publish LSL. A genuine native
  LSL requirement belongs to the separately installed app.
- Browser-native capture may expose exact input batches through same-tab or
  same-origin browser APIs and may export all received raw data plus produced
  metric events locally, but its
  memory/storage queue must be bounded, overflow must stop or surface visibly,
  and neither mechanism may be named or documented as native LSL.
- A visible-document wake lock is foreground protection only. Never claim that
  Pages or an installed PWA provides guaranteed background/screen-off Web
  Bluetooth, CSV, or audio capture; that requires a native mobile lifecycle and
  foreground-service design.

## Review and definition of done

Every material change should answer:

- Which layer owns it, and does dependency direction remain clean?
- Can it delay, reorder, transform, copy, or drop raw data?
- What happens when its queue fills, transport fails, or the UI stalls?
- What state resets on disconnect/reconnect or configuration change?
- Which platforms and package forms are affected?
- What focused test proves the behavior, and what full checks ran?
- Does `for-ai/`, public documentation, the metric catalog, or the decision log
  need to change?

Do not describe a build as cross-platform, low-latency, reliable, or validated
without evidence proportional to that claim. Hardware-dependent gaps belong in
`CURRENT-STATE.md` or `GOALS.md`, not behind optimistic wording.
