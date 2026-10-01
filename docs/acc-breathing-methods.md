# ACC breathing methods in Polar Stream Mini

Polar Stream Mini makes every built-in ACC-derived breathing output individually
selectable under **+ Realtime metrics → ACC derived**. All are off by default.
The H10's raw X/Y/Z acceleration (mg, 200 Hz) remains a separate, unchanged
output. These methods infer breathing-related motion; none measures airflow or
lung volume, and none has a completed H10 versus respiratory-reference
validation. A source-clock reset or missing samples must not be filled with
invented acceleration.

## Output families

| Method | Input and transformation | Pickable outputs | Phase or event decision |
| --- | --- | --- | --- |
| Raw motion magnitude | Euclidean length of X/Y/Z acceleration, in g; retains gravity and unrelated motion | `acc_magnitude` | None |
| Calibrated PCA waveform | Source-time smoothing of selected ACC axes, principal-axis projection and robust 5th–95th percentile scaling | `acc_breathing_magnitude` (signed g), `breathing_volume` (0–1), `breathing_axis_range` (g), `breathing_calibration` (0–1), `breathing_signal_confidence` (0–1), `breathing_signal_ready` (0/1) | Supplies a common motion waveform and quality gates |
| Existing ACC phase | Smoothed source-time derivative of the fixed-calibration projection, with entry/hold hysteresis, confirmation and minimum dwell | `breathing_phase`: +1 inhale, −1 exhale, 0 pause **or bad signal** | Direction and persistence of motion; bad and pause share the public code 0 |
| Existing ACC cycle and dynamics | Like-polarity extrema on the waveform; accepted intervals and peak-to-trough amplitudes | `breathing_rate`, `breathing_dynamics_confidence`, eight `breath_interval_*` and eight `breath_amplitude_*` statistics | `breathing_rate` = 60 / mean accepted cycle interval; dynamics include mean, SD, CV, autocorrelation width, PSD slope, Lempel–Ziv complexity, sample entropy and multiscale entropy |
| Phan adaptation | Sum of absolute differences between short (0.2 s) and long (200 s) means on each raw ACC axis; adaptive threshold | `phan_breath_event`: 1 pulse; `phan_breath_rate`: detected pulses in trailing 60 s | Rising threshold crossing after 1 s quiet and 0.25 s refractory time; no inhale/exhale/hold classification |
| Flowborne adaptation | Short (24/90 s) minus long (180/90 s) means of the **signed PCA ACC projection**, divided by the current axis span | `flowborne_motion_score` (signed span fraction); `flowborne_phase`: +1 inhale, −1 exhale, 0 hold, −2 bad signal | +1 above +0.025 span, −1 below −0.075 span, 0 between; −2 while unready, warming up, after a gap or clock reset, or when the score exceeds one span |

All optional scalar metrics use the catalog's stable LSL suffix: for example,
`<base>_flowbornePhase`, `<base>_flowborneMotionScore`,
`<base>_phanBreathEvent`, and `<base>_breathingPhase`. Separate mode publishes
each selected output in its own outlet. Single mode gives each selected output
a sparse channel in the combined outlet. Flowborne phase is emitted on each
accepted ACC notification (including −2 during calibration); its score is
omitted when invalid. Phan event is emitted only on a counted crossing.

## What comes from the Unity project

[MesmerPrism's Viscereality `BreathDetection.cs`](https://github.com/MesmerPrism/Viscereality/blob/927beb17a1d6f90b5f1b3efdcd3d4eab656dfb71/Viscereality/Assets/Scripts/Breathing/BreathDetection.cs)
reads a tracked VR controller's world position and orientation every Unity
frame. It rotates a fixed local calibration direction into world space,
projects the position change onto that axis, integrates those signed frame
deltas, and computes a 24-frame mean minus a 180-frame mean of the integrated
series. The nominal 90 Hz windows are about 0.267 and 2 seconds. Positive and
negative scene thresholds classify inhale and exhale; values between them are
pausing. Missing controller pose, rotation above 0.5° per frame, or absolute
mean difference above 0.025 m are bad tracking. The questionnaire scene uses
+0.00025 m and −0.00075 m thresholds. Near a particle sphere's radius
endpoints, the Unity logic can multiply both thresholds by a scene-specific
retention factor; it can also reverse polarity for the left controller. Its
`StateChangeDelay` field is unused. Haptics and particle updates consume the
classification, but do not produce the motion score.

H10 acceleration has neither tracked world position nor controller orientation.
Flowborne therefore reuses the Unity **two-window signed contrast and
four-state decision pattern** on the existing calibrated H10 ACC projection.
It does not double-integrate acceleration into position, copy metre thresholds,
use the VR rotation guard, use particle-radius retention, or claim that the
H10's score is controller displacement. The questionnaire threshold asymmetry
(1:3) is preserved in dimensionless exploratory span fractions (+0.025,
−0.075); their absolute scale needs validation. `breathing_signal_ready`
provides the available calibration/motion/freshness gate. Invert direction in
the ACC breathing settings if a known mounting orientation reverses polarity.

The project's `UnifiedBreathingTracker` also has a separate 0–1 controller
volume driver calibrated over warmup/learning. Its phase path uses the axis
classifier. Flowborne adapts the phase classifier; `breathing_volume` remains
Polar Stream's independent H10 PCA waveform, not a port of Unity's controller
volume driver.

## Shared structure and consequential differences

Each detector extracts a slow movement pattern from noisy motion samples,
depends on mounting and polarity, and can confuse body movement with breathing.
They differ mainly in **what is compared** and **what event is declared**:

- Phan compares three absolute axis changes over very different time scales.
  Its score discards sign, so it can count candidate breaths but cannot assign
  inhale versus exhale. Its 200-second baseline adapts slowly to posture.
- The PCA path first learns a signed chest-motion axis. It preserves a waveform
  and can classify direction, but calibration and axis orientation matter. Its
  existing phase uses a derivative with hysteresis and dwell, so it reacts to
  *direction of change* rather than to being above or below a slow baseline.
- Flowborne compares the signed PCA projection with its 2-second baseline and
  uses asymmetric thresholds, like the controller classifier. It reacts to
  *short-term position of the motion signal relative to baseline*. On H10 that
  signal is acceleration-derived, so its timing can differ from physical chest
  displacement and from the existing derivative phase. Its separate −2 value
  distinguishes bad signal from a real hold.
- Rate and dynamics metrics require cycles, so they update more slowly and
  summarize interval/amplitude patterns instead of emitting instantaneous
  phase. Phan's rate instead counts threshold events in a rolling minute.

An eventual hybrid can be designed after paired reference recordings show
which detector fails where: use common source timestamps and calibrated
polarity; reject poor ACC quality first; compare directional phase and
Flowborne contrast only while ready; use Phan's event onset as a candidate
cycle marker; then require plausible phase order and timing before updating
rate/dynamics. Keep disagreement and coverage observable rather than forcing
a phase in uncertain windows. Tune thresholds and temporal windows on held-out
H10 plus respiratory-belt/airflow data across posture, strap placements,
movement, quiet breathing and breath holds. No hybrid is implemented here.
