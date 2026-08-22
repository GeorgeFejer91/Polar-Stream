# ACC-derived breathing handoff

## Status in one paragraph

Polar Stream exposes an experimental one-dimensional respiratory-effort module
derived from the Polar H10 accelerometer. Its primary signals are the signed
chest-motion projection in g (`acc_breathing_magnitude`), its robustly normalized
0–1 form (`breathing_volume`, retained as a compatibility ID), and a three-state
classifier (`breathing_phase`: inhale `+1`, pause/not-ready `0`, exhale `-1`).
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
covered by deterministic tests, but the new browser PMD path has not yet been
verified against a physical H10.

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
   user-selected axis mask, time-window smoothing, causal baseline removal,
   adaptive robust bounds, explicit readiness/quality telemetry, the phase
   visualizer, and an equivalent browser processor.

The protocol path is cross-checked against Polar's official
[BLE SDK](https://github.com/polarofficial/polar-ble-sdk) and
[H10 product guide](https://github.com/polarofficial/polar-ble-sdk/blob/master/documentation/products/PolarH10.md).
Polar documents H10 ACC as three-axis acceleration in mg and supports 200 Hz,
16-bit, ±8 g streaming. This establishes the device data contract; it does not
validate the breathing inference.

## Active Polar Stream algorithm

### 1. Input and axis selection

The H10 supplies samples at a requested 200 Hz:

```text
a[n] = [x_mg, y_mg, z_mg] / 1000            (g)
```

Disabled axes are set to zero before any filtering or calibration. At least two
axes are required. X + Z is the current default because it reproduces the
upstream non-rotational/XZ mode, but this is an orientation-dependent starting
choice, not a universal anatomical rule. Including Y changes the learned PCA
axis and may either recover useful motion or admit additional rotation/artifact.

### 2. Sample smoothing

Each selected axis uses an exponential moving average:

```text
N_smooth = smoothing_window_seconds × 200
alpha    = clamp(2 / (N_smooth + 1), 0.001, 1)
f[n]     = f[n-1] + alpha × (a[n] - f[n-1])
```

The UI calls this a smoothing window for interpretability. It is an EMA time
constant proxy, not a rectangular moving-average window. A longer value removes
more rapid motion but adds lag and attenuates small/fast respiratory motion.

### 3. Quiet calibration and principal axis

The processor retains the latest
`calibration_window_seconds × 200` filtered samples. Every 0.5 seconds after the
window is full, it tries to calibrate:

```text
center = mean(f)
C      = mean((f - center)(f - center)^T)
axis   = dominant eigenvector(C)
```

The eigenvector is found with eight power iterations, initialized on the
dimension with the largest covariance diagonal. If `invert_direction` is true,
the vector sign is flipped. PCA itself cannot determine which sign is inhale;
manual inversion is therefore a necessary experimental control.

All calibration samples are projected onto that axis. The lower and upper
quantiles form robust provisional bounds. Calibration is rejected when the raw
quantile span is below `minimum_axis_range_g`. Accepted bounds are contracted by
3% of the raw span at both edges. A rejected attempt leaves the rolling window
active and tries again 0.5 seconds later.

### 4. Causal baseline, continuous projection, and normalized curve

For each subsequent filtered sample:

```text
beta        = 1 / (200 × 10)
baseline[n] = baseline[n-1] + beta × (f[n] - baseline[n-1])
p[n]        = dot(f[n] - baseline[n], axis)  (g)
v[n] = clamp((p[n] - lower) / (upper - lower), 0, 1)
```

`acc_breathing_magnitude` publishes `p[n]`. Despite the historical metric name,
this is a **signed PCA projection in g**, not the non-negative Euclidean vector
magnitude. The separate `acc_magnitude` metric is
`sqrt(x² + y² + z²)`. In the output module, the projection can remain in g or be
normalized to 0–1 for visualization/downstream use. `breathing_volume` publishes
`v[n]` as the closest Polar analogue to the one-dimensional belt waveform used
by Respyra. The ID is preserved for compatibility, but the UI labels it **ACC
breathing waveform** because it is not calibrated respiratory volume. The
10-second causal baseline reduces gravity and slow-posture drift without future
samples; it does not make the signal posture invariant.

The live visualizer presents this output as a preliminary one-dimensional trace:
the newest value is a moving dot and the preceding bounded display window forms
its leftward trail. A rising dot is labeled inhale and a falling dot exhale
according to the configured projection direction; belt placement can reverse
that polarity, so direction inversion must be checked against observed breaths.
The display introduces no additional filtering or respiratory estimate.

### 5. Adaptive bounds

When adaptive bounds are enabled, the processor samples the projection at no
more than 20 Hz, retains `adaptive_window_seconds`, and considers an update every
0.5 seconds after at least 80 retained values. New quantile bounds are accepted
only when their span:

- is at least `minimum_axis_range_g`;
- is at least 0.5 × the calibration span; and
- is no more than 2.0 × the calibration span.

Accepted bounds move 20% toward the new values. This limits abrupt visual jumps.
Together with causal baseline removal it reduces slow drift, but it still cannot
reliably distinguish posture changes from genuine changes in breathing effort.

### 6. Time-normalized three-state phase classifier

One phase decision is made per incoming ACC notification batch:

```text
reference_delta = 0.0005 + (1 - sensitivity)^2 × 0.015625
threshold_per_s = reference_delta / 0.05 s
batch_duration  = number_of_ACC_samples / 200 Hz
velocity        = (latest_v - previously_emitted_v) / batch_duration

velocity >  threshold_per_s  → +1 inhale
velocity < -threshold_per_s  → -1 exhale
otherwise                     →  0 pause/not-ready
```

The 50 ms reference preserves the previous sensitivity scale for a common
10-sample notification while expressing the actual decision in normalized
waveform units per second. A 30-sample/150 ms notification therefore needs
three times the absolute change of a 10-sample/50 ms notification to receive
the same phase class. Rust and Chromium use the same sample-count-derived
duration and deterministic invariance fixtures.

Before calibration, after a host-side notification gap longer than
`stale_timeout_seconds`, during excessive broadband motion, and on the first
accepted value, the numeric result is also `0`. Consequently, zero does **not**
uniquely mean a physiological pause. It may mean small motion, calibration,
stale data, or bad signal.

This removes the earlier direct dependence on BLE batch size. It still assumes
that accepted ACC samples represent the requested 200 Hz stream; lost samples,
sensor-rate changes, or malformed frame timing remain reasons to inspect raw
timestamps and quality telemetry rather than treating the class as ground
truth.

Every derived snapshot is emitted once per accepted ACC notification and is
stamped with that notification's newest H10 PMD sensor timestamp. Native LSL
maps this device time into the local LSL clock with the same first-frame offset
contract as raw ACC; native OSC and CSV retain the nanosecond value directly.
The Chromium event and browser CSV carry that same frame timestamp. This aligns
raw and derived streams without pretending that the calculation occurred at a
different physiological instant. It does not measure radio, operating-system,
or transport latency; those require host receipt timestamps and a physical
end-to-end gate.

### 7. Readiness and confidence

Readiness is true only after calibration, while notifications are fresh, and
while an all-axis motion score is at least 0.35. The motion path applies the
same EMA strength to all three raw axes before measuring successive filtered
vectors. It is independent of the user-selected projection axes: ordinary
high-rate sensor noise is attenuated, while broadband body motion still lowers
the score.

```text
motion_threshold = max(0.1 × minimum_axis_range_g, 0.001)
motion_score     = 1 / (1 + (filtered_motion_delta_ema_g / motion_threshold)^2)
ready            = calibrated AND fresh AND motion_score >= 0.35
```

The confidence index combines calibrated range, motion, history coverage, and
the strongest positive normalized autocorrelation over approximately 1.45–12.5
seconds in the no-more-than-20 Hz projection history:

```text
confidence = range_score × motion_score
             × (0.40 + 0.60 × coverage × periodicity)
```

Not-ready samples have confidence zero and force phase to zero. This is a causal,
bounded engineering heuristic for rejecting obviously poor intervals. It is not
a probability, does not establish that a periodic component is respiration, and
must be calibrated against synchronized reference data before thresholds are
used for study exclusion.

## Current configurable parameters

The add-output workflow exposes the common controls first and keeps the rest in
**Advanced experiment parameters**. The same parameters remain editable through
**Adjust** after the module is added; saving restarts calibration for both
breathing outputs.

| Parameter | Default | Accepted range | Effect and experimental note |
| --- | ---: | ---: | --- |
| `axes` | X + Z | any 2 or 3 | Axis mask applied before smoothing/PCA. Always log the mask and physical strap orientation. |
| `smoothing_window_seconds` | 0.75 s | 0.05–5 s | EMA strength. Longer is steadier/slower; shorter is more reactive/artifact-sensitive. |
| `sensitivity` | 0.60 | 0–1 | Phase-velocity threshold only. Higher classifies smaller normalized change per second. It does not improve waveform quality. |
| `invert_direction` | false | boolean | Flips the PCA axis sign. Use only after a predeclared reference check. |
| projection normalization | 0–1 | original g or 0–1 | Output/display transform for the continuous projection; does not change the classifier input. |
| `calibration_window_seconds` | 12 s | 1–60 s | Number of quiet samples used for PCA and initial bounds. |
| `minimum_axis_range_g` | 0.010 g | 0.001–0.250 g | Rejects calibration/adaptive windows with too little selected-axis travel. |
| `stale_timeout_seconds` | 3 s | 0.25–30 s | Host notification gap that forces class 0/not-ready. |
| `adaptive_bounds` | true | boolean | Permits recent accepted projection quantiles to shift normalization bounds. |
| `adaptive_window_seconds` | 20 s | 5–300 s | Recent history retained for adaptive quantiles. |
| `lower_quantile` | 0.05 | 0–0.40 | Robust low bound. Must stay at least 0.10 below the upper quantile. |
| `upper_quantile` | 0.95 | 0.60–1 | Robust high bound. Must stay at least 0.10 above the lower quantile. |

Fixed implementation constants that must also be versioned in a reproducible
study are: 200 Hz assumed ACC rate, eight PCA power iterations, 0.5-second
calibration retry, 3% edge contraction, adaptive sampling capped at 20 Hz,
minimum 80 adaptive values, 0.5–2.0 accepted span ratio, 20% bound update, and
the phase-threshold formula plus 50 ms compatibility reference above.

## Lessons incorporated from the Lalidis Mateo belt thesis

The reviewed 2026 thesis used a PLUX PZT respiration sensor through BITalino at
100 Hz with ten-sample device reads. It is not a Vernier protocol reference and
its PZT deformation signal is not physically equivalent to H10 acceleration.
Its useful contribution here is the separation of signal contracts and its
transparent treatment of cleaning failures:

| Thesis mode | Thesis output | Closest Polar Stream output | Non-equivalence |
| --- | --- | --- | --- |
| fixed control | fixed percentile-calibrated 0–1 belt controller | `breathing_volume` with adaptive bounds off | Polar still removes a moving 10-second baseline |
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
range during a manually selected five-second retention interval. Polar Stream's
10-second moving baseline likewise causes a sustained orientation/motion offset
to decay toward center. Consequently, neither `acc_breathing_magnitude` nor
`breathing_volume` is a defensible retained-lung-level signal. An interaction
that needs a held visual state must implement and label a separate hold-control
contract, then validate retention drift against a synchronized reference; the
continuous respiratory-motion waveform must not silently freeze itself.

## Upstream PolarH10 versus Polar Stream

The upstream tracker is useful reference code, but it is not byte-for-byte the
active algorithm.

| Concern | Upstream PolarH10 reference | Active Polar Stream |
| --- | --- | --- |
| Axis modes | Separate 3D PCA and XZ PCA; XZ is default | One PCA after a user-selectable 2/3-axis mask; X + Z default |
| Timing | Estimates sensor/host sample timing | Assumes requested 200 Hz for filtering/calibration |
| Raw smoothing | Fixed `SampleEmaAlpha = 0.10` | User-facing EMA window; default 0.75 s (`alpha ≈ 0.0132`) |
| Projection smoothing | Separate `ProjectionEmaAlpha = 0.10` | No additional projection EMA |
| Useful-signal gate | 4 s window, ≥80 samples, ≥20 Hz, ≥0.002 g per-axis range | Calibration/freshness/all-axis motion readiness plus range/coverage/periodicity confidence |
| Calibration | 12 s, ≥240 samples, ≥0.01 g travel, retry every 2 s | 12 s × 200 samples, ≥0.01 g quantile span, retry every 0.5 s |
| Bounds | 5/95% quantiles, 3% edge ease | Same default quantiles/ease, user configurable |
| Direction | Optional external direction reference plus inversion | Manual inversion only |
| Adaptation | Coverage/rate-limited expansion and contraction; default window 20 s | Simplified quantile span gate and fixed 20% blend |
| Phase | Upstream per-update delta threshold (`0.003`) | Sensitivity-derived normalized velocity per second |

Relevant upstream defaults not currently ported include its exact useful-signal
gate, minimum adaptive coverage 0.85, initial-span acceptance factors 0.75/1.35,
adaptive lerp speed 0.35, contraction multiplier 0.45, minimum 640 adaptive
samples, and an optional direction reference. Porting any of these changes the
algorithm and requires a new version plus comparison against retained raw data.

## How to tune this in an experiment

Do not search settings on the same recordings used to report performance. Use a
development set with a respiratory reference, freeze one configuration, then
evaluate it unchanged on held-out participants/conditions.

A compact, interpretable development grid is:

- axes: X + Z versus X + Y + Z;
- smoothing: 0.25, 0.75, and 1.50 seconds;
- phase sensitivity: 0.40, 0.60, and 0.80; and
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

## Validation plan

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
