# ACC-derived breathing handoff

## Status in one paragraph

Polar Stream exposes an experimental one-dimensional respiratory-effort module
derived from the Polar H10 accelerometer. The primary ACC library now presents
raw X/Y/Z, general 3D motion magnitude, and the robustly normalized 0–1 form
(`breathing_volume`, labeled **ACC breathing magnitude (0–1)** and retained as a
compatibility ID). The signed chest-motion projection in g
(`acc_breathing_magnitude`, labeled **ACC breathing projection (g)**), the
three-state classifier (`breathing_phase`: inhale `+1`, pause/not-ready `0`,
exhale `-1`), and specialist diagnostics/dynamics remain under Extra options.
Two companion outputs report computational readiness and an app-specific signal
quality index (`breathing_signal_ready` and `breathing_signal_confidence`). These
are not respiratory volume, airflow, a probability of correctness, or a
clinically validated respiratory-rate measurement. Body motion, strap
orientation, posture, heartbeat motion, fit, and notification timing can all
dominate the estimate. Every experiment should retain raw X/Y/Z ACC, record the
complete settings below, and compare the result with an independent respiratory
reference such as the verified GDX-RB Force (N) stream.

The active native implementation is
[`crates/polar-h10-metrics/src/breathing.rs`](../crates/polar-h10-metrics/src/breathing.rs).
The browser implementation in
[`apps/polar-stream/ui/polar-web-bluetooth.js`](../apps/polar-stream/ui/polar-web-bluetooth.js)
uses the same settings, formulas, and constants. Browser and native behavior are
covered by deterministic tests. The native product input plus timed estimator
passed the bounded physical H10 verifier on 2026-08-26; the direct browser PMD
path has not yet received its own physical acceptance run.

## Provenance

There are three distinct provenance layers; they should not be collapsed into
one claim.

1. The project owner describes an earlier collaborator contribution from
   Johannes. The available Git history does not identify a Johannes-authored
   breathing commit or mark exactly which equations came from that exchange.
   That contribution is therefore recorded as verbal project provenance, not a
   source-code attribution. Add the original notes or commit here when found.
2. The concrete upstream implementation is the public
   [MesmerPrism/PolarH10 repository](https://github.com/MesmerPrism/PolarH10).
   The initial tracker/settings documentation is visible in commit
   [`3b97f4f`](https://github.com/MesmerPrism/PolarH10/commit/3b97f4f93a4515cddaf8456adf555432b1229ae6),
   attributed in Git to Till Holzapfel. A later signed-waveform change is visible
   in George Fejer's commit
   [`4759259`](https://github.com/MesmerPrism/PolarH10/commit/475925967678d0a17415440a4e76cebcd96fcf63).
   The upstream [developer reference](https://mesmerprism.com/PolarH10/reference/index.html),
   [breathing workflow](https://mesmerprism.com/PolarH10/reference/breathing-workflow.html),
   and [formula sheet](https://mesmerprism.com/PolarH10/reference/breathing-formulas.html)
   are the original handoff surfaces.
3. Polar Stream reimplemented a smaller, explicit variant in Rust, added the
   user-selected axis mask and explicit readiness/quality telemetry. The current
   timed-v1 upgrade adds per-sample PMD source-time reconstruction, dt-aware
   filtering, fixed-coordinate phase hysteresis, bounded optional output-range
   adaptation, separate renderer presentation, and a matching browser processor.

The protocol path is cross-checked against Polar's official
[BLE SDK](https://github.com/polarofficial/polar-ble-sdk) and
[H10 product guide](https://github.com/polarofficial/polar-ble-sdk/blob/master/documentation/products/PolarH10.md).
Polar documents H10 ACC as three-axis acceleration in mg and supports 200 Hz,
16-bit, ±8 g streaming. This establishes the device data contract; it does not
validate the breathing inference.

## Active Polar Stream algorithm

The new default contract is `timed-pca-v1` for the continuous waveform and
`hysteresis-v1` for phase. The pre-upgrade implementation remains available as
`legacy-v0`; saved settings with either version field missing deserialize that
field as legacy so an old configuration is not silently reinterpreted.

### 1. Source-time input and ordering

The H10 records ACC at a nominal requested 200 Hz but normally delivers many samples
in one BLE notification. The PMD timestamp identifies the **newest** sample in
that frame. The first frame uses the nominal 5 ms period. Each ordinary later
frame distributes the measured interval between consecutive newest-sample
anchors across all samples, with the final sample landing exactly on the new
anchor:

```text
a[i] = [x_mg, y_mg, z_mg] / 1000                         (g)
t_first[i] = newest - (frame_count - 1 - i) × 5 ms
t_later[i] = previous_newest
             + floor((newest - previous_newest) × (i + 1) / frame_count)
```

Disabled axes are zeroed before filtering and PCA; at least two axes are
required. X + Z remains the orientation-dependent default. Raw ACC publication
stays first and unchanged. The estimator immediately discards samples at or
behind its source-time watermark and counts them; it does not yet maintain a
live reorder buffer. A real forward source gap beyond `stale_timeout_seconds`
or an explicit source-clock reset produces Lost/not-ready evidence and retains
nominal newest-anchored backfill instead of stretching across missing time. A
clock reset also restarts calibration. Notification arrival time and
notification size never determine waveform or state dynamics.

### 2. Source-time smoothing and PCA calibration

Each selected axis uses a first-order low-pass with the actual accepted source
time step:

```text
alpha = dt / (volume_filter_tau_seconds + dt)
f[i]  = f[i-1] + alpha × (a[i] - f[i-1])
```

The default time constant is 0.18 s. Calibration retains a complete
`calibration_window_seconds` interval of filtered source-time samples, then
computes their mean, covariance, and dominant PCA eigenvector. Power iteration
runs 32 times from the dimension with the largest covariance diagonal. The
largest absolute axis component is made positive for deterministic polarity;
`invert_direction` is applied after that convention because PCA cannot know
which physical direction is inhale.

Calibration fails closed unless it has at least eight samples, PCA dominance is
at least 0.05, and the 5th-to-95th percentile span (configurable) meets
`minimum_axis_range_g`. The accepted mean, axis, lower bound, upper bound, span,
and PCA dominance become the fixed calibration reference.

### 3. Continuous projection and normalized waveform

For each accepted filtered sample:

```text
p[i] = dot(f[i] - calibration_center, axis)                  (g)
v[i] = clamp((p[i] - output_lower) / (output_upper-output_lower), 0, 1)
```

`acc_breathing_magnitude` publishes the signed projection `p`; the historical
name does not mean Euclidean magnitude. `breathing_volume` publishes `v` for
compatibility, while the UI labels it **ACC breathing waveform**. It is a
relative respiratory-motion/effort proxy, not retained lung volume, airflow, or
a calibrated physiological volume.

Adaptive bounds are off by default. When enabled, projection points are sampled
at no more than 20 Hz, retained for `adaptive_window_seconds`, and reconsidered
at most every 0.5 s after 80 points. A quantile update is admitted only when its
span is at least the configured minimum and between 0.5 and 2.0 times the fixed
calibration span. Accepted output bounds move with
`alpha = 1 - exp(-0.5 × dt)`. Adaptation affects only the displayed/published
0–1 range; it cannot manufacture phase transitions because phase uses the fixed
calibration coordinate below.

### 4. Fixed-coordinate phase with hysteresis

The canonical state coordinate and derivative are calculated for every accepted
source sample, independently of BLE batching:

```text
q[i]        = (p[i] - fixed_lower) / fixed_calibration_span
raw_dq_dt   = (q[i] - q[i-1]) / dt
alpha_state = dt / (phase_derivative_tau_seconds + dt)
d[i]        = d[i-1] + alpha_state × (raw_dq_dt - d[i-1])
```

With the defaults, `d >= 0.030/s` requests Inhale, `d <= -0.030/s` requests
Exhale, and `|d| <= 0.025/s` requests Hold. Values in the hysteresis band retain
the active direction. A different request must persist for the 0.40 s
confirmation time, and the current active state must independently satisfy its
0.40 s minimum dwell before transition. A gap/reset clears derivative and state
history; the lost batch is not allowed to change hidden phase state.

The transport value remains `+1` inhale, `0` hold/not-ready, and `-1` exhale for
compatibility. Consumers must use `breathing_signal_ready` to distinguish a
valid Hold from calibration, Lost, or rejected signal. `legacy-v0` retains the
older sensitivity-derived per-batch velocity classifier for saved experiments.

### 5. Readiness, confidence, and diagnostics

Readiness is true only after calibration, outside Lost, and while the all-axis
motion score is at least 0.35. All-axis vectors receive the same source-time
volume filter before their successive-vector magnitude is smoothed with a 0.50
s time constant:

```text
motion_threshold = max(0.1 × minimum_axis_range_g, 0.001)
motion_score     = 1 / (1 + (motion_delta_ema_g / motion_threshold)^2)
range_quality    = clamp(calibration_span / (2 × minimum_axis_range_g), 0, 1)
confidence       = range_quality × motion_score × PCA_dominance
```

Not-ready samples have confidence zero and publish phase zero. Confidence is an
app-specific quality index, not a probability. Native diagnostics retain the
configuration generation, source time, clock revision, accepted and late-drop
counts, gap/reset/Lost counts, transition count, fixed span, PCA axis and
dominance, and filtered derivative.

### 6. Canonical output versus presentation

One canonical derived snapshot is emitted per accepted ACC notification and is
stamped with that notification's newest PMD time; its internal computation still
used all reconstructed/interpolated sample times. Native LSL maps source time through
the same clock contract as raw ACC, while OSC/CSV preserve their existing
timestamp contract. The browser adapter mirrors the estimator from PMD source
time.

Separately, the core exposes at most 512 ordered `(source_timestamp_ns,
volume_01)` points for the renderer. They never enter LSL, OSC, CSV, calibration,
or classification. **Fresh + smoothing** follows the newest available point
with a 0.12 s render-time smoothing constant. **Timestamp-faithful** interpolates
the trail at an intentional 0.18 s source-time delay, matching the usual roughly
37-sample notification span more closely. This separation provides a responsive
display without falsifying the canonical sensor-time stream.

## Current configurable parameters

New outputs use the versioned defaults below. All parameters are editable
through **Adjust** after the module is added and are shared by the breathing
output family. A waveform/PCA change restarts calibration; a state-only change
resets only the state machine in the native core. Older saved objects with no
mode fields retain legacy behavior.

| Parameter | Default | Accepted range | Effect and experimental note |
| --- | ---: | ---: | --- |
| `volume_mode` | `timed-pca-v1` | timed or legacy | Selects the source-time waveform estimator. Missing saved values mean `legacy-v0`. |
| `state_mode` | `hysteresis-v1` | hysteresis or legacy | Selects the phase state machine. Hysteresis requires timed PCA; legacy volume is clamped to legacy state. Missing saved values mean `legacy-v0`. |
| `axes` | X + Z | any 2 or 3 | Axis mask applied before smoothing/PCA. Always log the mask and physical strap orientation. |
| `volume_filter_tau_seconds` | 0.18 s | 0.01–5 s | Source-time low-pass time constant for timed PCA. |
| `smoothing_window_seconds` | 0.75 s | 0.05–5 s | Legacy-v0 sample-count EMA control only. |
| `sensitivity` | 0.60 | 0–1 | Legacy-v0 phase threshold only. |
| `invert_direction` | false | boolean | Flips the PCA axis sign. Use only after a predeclared reference check. |
| projection normalization | 0–1 | original g or 0–1 | Output/display transform for the continuous projection; does not change the classifier input. |
| `calibration_window_seconds` | 12 s | 1–60 s | Complete source-time interval used for PCA and initial bounds. |
| `minimum_axis_range_g` | 0.010 g | 0.001–0.250 g | Rejects calibration/adaptive windows with too little selected-axis travel. |
| `stale_timeout_seconds` | 0.50 s | 0.25–30 s | Forward source-time gap that marks the current batch Lost/not-ready. Legacy saved settings default to 3 s. |
| `phase_derivative_tau_seconds` | 0.40 s | 0.01–5 s | Source-time low-pass time constant for fixed-coordinate velocity. |
| `phase_enter_threshold_per_second` | 0.030/s | 0.001–5/s | Absolute velocity required to request inhale/exhale. |
| `phase_hold_threshold_per_second` | 0.025/s | 0–enter threshold | Absolute velocity at or below which Hold is requested. |
| `phase_confirmation_seconds` | 0.40 s | 0–5 s | Continuous request time before a different phase may activate. |
| `phase_minimum_dwell_seconds` | 0.40 s | 0–5 s | Minimum active-state duration; evaluated independently of confirmation. |
| `adaptive_bounds` | false | boolean | Allows only the 0–1 output bounds to follow recent accepted quantiles. Legacy saved settings default to true. |
| `adaptive_window_seconds` | 20 s | 5–300 s | Recent history retained for adaptive quantiles. |
| `lower_quantile` | 0.05 | 0–0.40 | Robust low bound. Must stay at least 0.10 below the upper quantile. |
| `upper_quantile` | 0.95 | 0.60–1 | Robust high bound. Must stay at least 0.10 above the lower quantile. |
| presentation mode | fresh + smoothing | fresh or timestamp-faithful | Renderer only; never changes canonical outputs. |
| presentation smoothing tau | 0.12 s | 0.01–2 s | Render-time smoothing for fresh mode. |
| presentation delay | 0.18 s | 0–1 s | Intentional source-time delay for faithful interpolation. |

Fixed implementation constants that must also be versioned in a reproducible
study are: 5 ms default ACC sample period, 32 PCA power iterations, eight-sample
minimum, 0.05 PCA-dominance floor, 0.50 s motion-quality time constant, 0.35
readiness threshold, immediate watermark late-drop policy, adaptive sampling
capped at 20 Hz, minimum 80 adaptive values, 0.5 s update cadence, 0.5–2.0
accepted span ratio, adaptation rate 0.5/s, and the 512-point renderer bound.

## Lessons incorporated from the Lalidis Mateo belt thesis

The reviewed 2026 thesis used a PLUX PZT respiration sensor through BITalino at
100 Hz with ten-sample device reads. It is not a Vernier protocol reference and
its PZT deformation signal is not physically equivalent to H10 acceleration.
Its useful contribution here is the separation of signal contracts and its
transparent treatment of cleaning failures:

| Thesis mode | Thesis output | Closest Polar Stream output | Non-equivalence |
| --- | --- | --- | --- |
| fixed control | fixed percentile-calibrated 0–1 belt controller | timed `breathing_volume` with adaptive bounds off | Polar measures chest acceleration projected on a learned axis, not belt deformation |
| movement proxy | median-centered signed belt movement | `acc_breathing_magnitude` | Polar is a signed PCA projection in g |
| adaptive control | slowly adapting center/amplitude 0–1 controller | `breathing_volume` with adaptive bounds on | Polar uses bounded rolling quantiles rather than the thesis model |

The thesis preserved raw, filtered, and emitted values separately; logged
flatline, saturation, and baseline-shift QC without silently rewriting samples;
used percentile rather than extrema calibration; kept processing causal;
bounded its acquisition queue; preserved source gaps; and separated continuous
values from unvalidated turning-point events. Those principles align with Polar
Stream's raw-ACC retention, causal percentile/PCA module, explicit readiness and
confidence, bounded queues, and conservative phase language.

Its strongest negative result is also an acceptance requirement: the tested PZT
setup drifted by 0.33 (fixed mode) and 0.40 (adaptive mode) of the normalized
range during a manually selected five-second retention interval. Timed Polar
Stream no longer removes a moving baseline, but strap acceleration still does
not encode an absolute retained-lung level and its fixed calibration coordinate
can shift with posture or mounting. Consequently, neither
`acc_breathing_magnitude` nor `breathing_volume` is a defensible retained-volume
signal. An interaction that needs a held visual state must implement and label a
separate hold-control contract, then validate retention drift against a
synchronized reference.

## Upstream PolarH10 versus Polar Stream

The upstream tracker is useful reference code, but it is not byte-for-byte the
active algorithm.

| Concern | Upstream PolarH10 reference | Active Polar Stream |
| --- | --- | --- |
| Axis modes | Separate 3D PCA and XZ PCA; XZ is default | One PCA after a user-selectable 2/3-axis mask; X + Z default |
| Timing | Estimates sensor/host sample timing | Uses nominal 5 ms first/gap backfill and interpolates ordinary batches between PMD newest-sample anchors; batch arrival cadence is irrelevant |
| Raw smoothing | Fixed `SampleEmaAlpha = 0.10` | dt-aware first-order low-pass; 0.18 s default tau |
| Projection smoothing | Separate `ProjectionEmaAlpha = 0.10` | Axis filtering occurs before projection; no batch-level projection filter |
| Useful-signal gate | 4 s window, ≥80 samples, ≥20 Hz, ≥0.002 g per-axis range | Calibration/PCA dominance, source freshness, and all-axis motion readiness |
| Calibration | 12 s, ≥240 samples, ≥0.01 g travel, retry every 2 s | Complete 12 s source-time window, 32-iteration PCA, quantile span and dominance gates |
| Bounds | 5/95% quantiles, 3% edge ease | User quantiles on the fixed calibration projection; no edge contraction |
| Direction | Optional external direction reference plus inversion | Manual inversion only |
| Adaptation | Coverage/rate-limited expansion and contraction; default window 20 s | Off by default; bounded rolling quantiles and dt-aware convergence affect output bounds only |
| Phase | Upstream per-update delta threshold (`0.003`) | Fixed-calibration velocity, dt-aware derivative smoothing, hysteresis, confirmation, dwell, and stale/Lost reset |

Relevant upstream defaults not currently ported include its exact useful-signal
gate, minimum adaptive coverage 0.85, initial-span acceptance factors 0.75/1.35,
adaptive lerp speed 0.35, contraction multiplier 0.45, minimum 640 adaptive
samples, and an optional direction reference. Porting any of these changes the
algorithm and requires a new version plus comparison against retained raw data.

## How to tune this in an experiment

Do not search settings on the same recordings used to report performance. Use a
development set with a respiratory reference, freeze one configuration, then
evaluate it unchanged on held-out participants/conditions.

A compact, interpretable timed-v1 development grid is:

- axes: X + Z versus X + Y + Z;
- volume filter tau: 0.10, 0.18, and 0.30 seconds;
- derivative filter tau: 0.25, 0.40, and 0.60 seconds;
- enter/hold threshold pairs around the 0.030/0.025 per-second defaults; and
- confirmation/dwell pairs around the 0.40/0.40-second defaults; and
- adaptive bounds: off versus on with the 20-second default window.

These are test levels, not validated presets. Change one factor at a time before
trying a factorial grid. Keep the 12-second calibration and 5/95% bounds fixed
initially. If movement is substantial, reject or label the interval rather than
assuming stronger smoothing has recovered respiration.

For each run, save:

- participant/session pseudonym and condition;
- H10 identifier/firmware if available, host OS, app commit, and native/browser route;
- physical strap orientation, posture, activity, and calibration instructions;
- all parameters in the table plus fixed algorithm version;
- raw X/Y/Z ACC with sensor timestamps, all BLE gap/error events, projection,
  normalized curve, classifier, and reference-respiration signal; and
- manual events such as posture change, speech, cough, strap adjustment, or
  deliberate breath hold.

## Synchronized H10/GDX-RB qualification

The repository includes an offline native-CSV analyzer that replays H10 ACC
through the current Rust `BreathingProcessor`; it does not maintain a second
approximation of the product algorithm. Record the H10 and a Vernier GDX-RB
respiration belt as distinct sources, then run:

```text
cargo run -p polar-stream --example analyze_respiration_reference -- <h10-native.csv> <gdx-native.csv> --output <new-evidence.json>
```

Use at least three quiet seated minutes so the 12-second H10 calibration and
the analyzer's 120-second overlap gate leave useful data. Record separate,
predeclared sessions for quiet spontaneous breathing, paced breathing, breath
holds, and movement; do not concatenate conditions or tune settings on the
reported sessions. Preserve raw files outside the repository and commit only
reviewed, identifier-free evidence when appropriate.

The timing contract is deliberately conservative. H10 PMD sensor spacing is
mapped into host time using the fifth percentile of per-notification
host-receipt-minus-sensor-time offsets, which reduces queue-drain bias without
claiming clock synchronization. GDX-RB has no exposed absolute device clock,
so its periodic samples retain the host-receipt/backfill timing used by the
product. The report therefore cannot separate filter delay, Bluetooth/OS
delivery delay, and true physiological lag.

Both signals are resampled at 10 Hz with interpolation gaps capped at 0.5
seconds, then receive the same causal 10-second baseline removal. Agreement is
reported separately for the signed PCA projection and the normalized 0–1
waveform. A bounded plus/minus three-second lag search uses polarity anchored
at zero lag so an oscillatory half-cycle peak cannot silently reverse the
mounting direction. Positive lag means H10 follows the earlier GDX force.

Evidence includes zero-lag and best-lag correlation, polarity-adjusted
correlation, normalized RMSE, dominant-rate error over 3–42 breaths/minute,
30-second window stability, ready/confidence coverage, robust signal spans,
clock/rate diagnostics, and quality failures. A quality pass only means the
recording met predeclared analysis gates. The analyzer always leaves
`physiologicalAcceptanceEstablished` false: acceptance limits require repeated
held-out participants and conditions, not a favorable correlation in one run.

## Physical engineering validation and remaining validation plan

To repeat the bounded timed-v1 engineering gate, wake and wear exactly one H10,
remain still while breathing naturally through calibration, then deliberately
include at least one complete inhale/hold/exhale sequence and run:

```text
cargo run -p polar-stream --example verify_h10_timed_breathing
```

The bounded 90-second verifier uses the production Windows input owner and the
same `MetricEngine` timed API as the app. It retains no identifier or raw
physiological values. Passing requires at least 30 source-time seconds, complete
calibration, ready output, a non-flat waveform, all three phase values, at least
1,000 strictly ordered renderer points within a bounded 4.5–5.5 ms nominal-200 Hz
interval, and zero estimator late/gap damage. It also reports samples per BLE
notification and effective source cadence so batching is evidence rather than an
assumption.

The 2026-08-26 Windows run passed with 170 frames and 6,120 accepted samples over
30.035968212 source seconds: every frame contained 36 samples, 169 batches were
anchor-interpolated, effective presentation cadence was 202.547103 Hz, and all
3,688 measured presentation intervals were 4.934591–4.939678 ms. It reported zero
late drops, gaps, order errors, or cadence errors; calibration completed, 103
frames were ready, all three phase values appeared, confidence peaked at 0.968760,
and normalized waveform span was 0.903095. No output transport was initialized.
This is an engineering acceptance run only and does not establish physiological
validity.

Use a respiratory inductance plethysmography belt, airflow/capnography, or
another justified reference sampled on a synchronizable clock. Evaluate quiet
and movement conditions separately. At minimum report:

- calibration acceptance/failure rate and time to calibration;
- readiness/confidence coverage and receiver-operating thresholds against
  manually labeled reference-quality intervals, rather than silently dropping
  difficult intervals;
- waveform agreement after a predeclared lag/sign procedure;
- inhale/exhale transition timing and three-state confusion against labeled
  reference phases;
- respiratory-rate error derived from the continuous curve, including bias and
  limits of agreement, stratified by posture/activity;
- latency and sensitivity to BLE batch cadence; and
- failures caused by lost packets, stale input, repositioning, speech, cough,
  gross movement, and cardiac-motion contamination.

Published work supports the feasibility of chest/torso accelerometry while also
showing why quality rejection, posture, position, and multi-axis processing
matter. Schipper et al. used constrained recursive PCA and explicitly traded
coverage against rate error; Drummond et al. rejected unreliable periods;
Hostrup et al. evaluated PCA plus autocorrelation in healthy participants; Ryser
et al. evaluated sleep recordings. None validates Polar Stream's exact H10
placement, parameterization, phase labels, or browser implementation.

## References

- Schipper F, van Sloun RJG, Grassi A, et al. (2021).
  [Estimation of respiratory rate and effort from a chest-worn accelerometer using constrained and recursive principal component analysis](https://doi.org/10.1088/1361-6579/abf01f).
- Drummond GB, Fischer D, Lees M, Bates A. (2021).
  [Classifying signals from a wearable accelerometer device to measure respiratory rate](https://doi.org/10.1183/23120541.00681-2020).
- Hostrup MCF, Nielsen AS, Sørensen FE, et al. (2025).
  [Accelerometer-based estimation of respiratory rate using principal component analysis and autocorrelation](https://doi.org/10.1088/1361-6579/adbe23).
- Ryser F, Hanassab S, Lambercy O, et al. (2022).
  [Respiratory analysis during sleep using a chest-worn accelerometer: A machine learning approach](https://doi.org/10.1016/j.bspc.2022.104014).

This handoff documents an experimental signal-processing pipeline. It is not a
medical-device specification and should not be used for diagnosis or safety
monitoring.
