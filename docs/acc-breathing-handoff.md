# ACC-derived breathing handoff

## Status in one paragraph

Polar Stream exposes only two breathing-facing accelerometer outputs: a continuous
signed chest-motion projection (`acc_breathing_magnitude`) and a three-state
classifier (`breathing_phase`: inhale `+1`, pause/not-ready `0`, exhale `-1`).
Both are experimental research signals. They are not respiratory volume,
airflow, or a clinically validated respiratory-rate measurement. Body motion,
strap orientation, posture, heartbeat motion, fit, and notification timing can
all dominate the estimate. Every experiment should retain raw X/Y/Z ACC, record
the complete settings below, and compare the result with an independent
respiratory reference.

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
   user-selected axis mask, time-window smoothing, a shared configuration for
   the two retained outputs, adaptive robust bounds, the phase visualizer, and
   now an equivalent browser processor.

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

### 4. Continuous projection and normalized curve

For each subsequent filtered sample:

```text
p[n] = dot(f[n] - center, axis)              (g)
v[n] = clamp((p[n] - lower) / (upper - lower), 0, 1)
```

`acc_breathing_magnitude` publishes `p[n]`. Despite the historical metric name,
this is a **signed PCA projection in g**, not the non-negative Euclidean vector
magnitude. The separate `acc_magnitude` metric is
`sqrt(x² + y² + z²)`. In the output module, the projection can remain in g or be
normalized to 0–1 for visualization/downstream use. `breathing_volume` is an
internal/legacy alias for `v[n]`; it is deliberately not offered as a third
breathing metric because the signal is not calibrated respiratory volume.

### 5. Adaptive bounds

When adaptive bounds are enabled, the processor samples the projection at no
more than 20 Hz, retains `adaptive_window_seconds`, and considers an update every
0.5 seconds after at least 80 retained values. New quantile bounds are accepted
only when their span:

- is at least `minimum_axis_range_g`;
- is at least 0.5 × the calibration span; and
- is no more than 2.0 × the calibration span.

Accepted bounds move 20% toward the new values. This limits abrupt visual jumps,
but it does not distinguish slow posture drift from real changes in breathing
depth.

### 6. Three-state phase classifier

One phase decision is made per incoming ACC notification batch:

```text
threshold = 0.0005 + (1 - sensitivity)^2 × 0.015625
delta     = latest_v - previously_emitted_v

delta >  threshold  → +1 inhale
delta < -threshold  → -1 exhale
otherwise           →  0 pause/not-ready
```

Before calibration, after a host-side notification gap longer than
`stale_timeout_seconds`, and on the first accepted value, the numeric result is
also `0`. Consequently, zero does **not** uniquely mean a physiological pause.
It may mean small motion, calibration, stale data, or bad signal.

The threshold is currently applied per BLE notification batch rather than per
second. A different browser, OS, MTU, or connection interval can therefore
change effective phase sensitivity even when the sensor sampling rate remains
200 Hz. This is a known limitation to fix before claiming cross-platform phase
equivalence. A future revision should divide by elapsed batch time or classify a
fixed-rate derivative and version the output contract.

## Current configurable parameters

The add-output workflow exposes the common controls first and keeps the rest in
**Advanced experiment parameters**. The same parameters remain editable through
**Adjust** after the module is added; saving restarts calibration for both
breathing outputs.

| Parameter | Default | Accepted range | Effect and experimental note |
| --- | ---: | ---: | --- |
| `axes` | X + Z | any 2 or 3 | Axis mask applied before smoothing/PCA. Always log the mask and physical strap orientation. |
| `smoothing_window_seconds` | 0.75 s | 0.05–5 s | EMA strength. Longer is steadier/slower; shorter is more reactive/artifact-sensitive. |
| `sensitivity` | 0.60 | 0–1 | Phase threshold only. Higher classifies smaller batch-to-batch changes. It does not improve waveform quality. |
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
the phase-threshold formula above.

## Upstream PolarH10 versus Polar Stream

The upstream tracker is useful reference code, but it is not byte-for-byte the
active algorithm.

| Concern | Upstream PolarH10 reference | Active Polar Stream |
| --- | --- | --- |
| Axis modes | Separate 3D PCA and XZ PCA; XZ is default | One PCA after a user-selectable 2/3-axis mask; X + Z default |
| Timing | Estimates sensor/host sample timing | Assumes requested 200 Hz for filtering/calibration |
| Raw smoothing | Fixed `SampleEmaAlpha = 0.10` | User-facing EMA window; default 0.75 s (`alpha ≈ 0.0132`) |
| Projection smoothing | Separate `ProjectionEmaAlpha = 0.10` | No additional projection EMA |
| Useful-signal gate | 4 s window, ≥80 samples, ≥20 Hz, ≥0.002 g per-axis range | No separate warm-up quality gate; calibration range gate only |
| Calibration | 12 s, ≥240 samples, ≥0.01 g travel, retry every 2 s | 12 s × 200 samples, ≥0.01 g quantile span, retry every 0.5 s |
| Bounds | 5/95% quantiles, 3% edge ease | Same default quantiles/ease, user configurable |
| Direction | Optional external direction reference plus inversion | Manual inversion only |
| Adaptation | Coverage/rate-limited expansion and contraction; default window 20 s | Simplified quantile span gate and fixed 20% blend |
| Phase | Upstream per-update delta threshold (`0.003`) | Sensitivity-derived threshold per ACC batch |

Relevant upstream defaults not currently ported include useful-signal gating,
minimum adaptive coverage 0.85, initial-span acceptance factors 0.75/1.35,
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

## Validation plan

Use a respiratory inductance plethysmography belt, airflow/capnography, or
another justified reference sampled on a synchronizable clock. Evaluate quiet
and movement conditions separately. At minimum report:

- calibration acceptance/failure rate and time to calibration;
- analyzable coverage, rather than silently dropping difficult intervals;
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
