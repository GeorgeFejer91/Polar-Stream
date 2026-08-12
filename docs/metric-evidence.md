# Metric inventory and evidence notes

Polar Stream exposes descriptive research signals, not diagnoses. Every scalar output has a unique discoverable name in the form `<stream base>_<metric suffix>`, can be plotted in the visualizer, and—where mathematically meaningful—can be left in original units or min–max normalized to 0–1 over a sliding window or the whole running measurement.

The short definitions and citations in this document are also embedded in the metric-library UI. They deliberately distinguish established mathematical definitions from physiological interpretation and from app-specific experimental adaptations.

## Legacy coverage

The new engine retains every metric family exposed by the previous repository and expands the supporting telemetry that previously existed only in specialist windows:

| Previous output | New output IDs |
| --- | --- |
| Heart rate | `heart_rate`, `mean_heart_rate` |
| RR interval | `rr_interval`, `mean_nn` |
| ECG | `raw_ecg`, `ecg_mean`, `ecg_rms`, `ecg_peak_to_peak`, `ecg_sd` |
| Accelerometer X/Y/Z | `raw_acc`, `acc_magnitude` |
| Breathing waveform | `breathing_volume`, `breathing_axis_range`, `breathing_calibration` |
| Inhale/exhale classification | `breathing_phase` (+1 inhale, −1 exhale, 0 pause) |
| Short-term HRV | `rmssd`, `ln_rmssd`, `sdnn`, `pnn50`, `sd1` |
| Coherence | `coherence`, `heartmath_coherence`, `coherence_peak_frequency`, `coherence_peak_power`, `coherence_total_power` |
| Coherence readiness | `coherence_confidence` |
| Breathing readiness | `breathing_dynamics_confidence` |
| Breath-interval dynamics | `breath_interval_mean`, `_sd`, `_cv`, `_acw50`, `_psd_slope`, `_lzc`, `_sampen`, `_mse` |
| Breath-amplitude dynamics | `breath_amplitude_mean`, `_sd`, `_cv`, `_acw50`, `_psd_slope`, `_lzc`, `_sampen`, `_mse` |

`excitometer` is new. No implementation by that name was present anywhere in the previous repository or its Git history, so it is marked experimental instead of being presented as legacy-compatible or validated.

## Signal and ECG features

Raw ECG is the H10's single-lead voltage stream at 130 Hz. The five-second mean, RMS, peak-to-peak range and sample standard deviation are transparent waveform summaries intended for amplitude and signal-quality inspection; wearable ECG quality is a real concern, but these generic features are not morphology classifiers or clinical measurements ([Smital et al., 2020](https://consensus.app/papers/details/ad2a724fefd55d25baee823438fc672e/?utm_source=unknown)).

Raw ACC is the 200 Hz three-axis chest-acceleration stream. Magnitude is simply `sqrt(x² + y² + z²)` in g; it combines gravity, body motion and breathing-related motion rather than isolating any one of them.

## Heart rate and HRV

RR/NN intervals are the accepted beat-to-beat intervals used by the rolling processor. Mean NN and mean heart rate (`60000 / meanNN`) describe central tendency, whereas SDNN is the sample standard deviation of accepted NN intervals.

RMSSD is the root mean square of successive NN differences; lnRMSSD is its natural logarithm. pNN50 is the percentage of successive pairs differing by more than 50 ms, and SD1 is `RMSSD / sqrt(2)`, so RMSSD and SD1 are mathematically redundant rather than independent evidence ([Shaffer & Ginsberg, 2017](https://www.frontiersin.org/journals/public-health/articles/10.3389/fpubh.2017.00258/full); [Ciccone et al., 2017](https://consensus.app/papers/details/7f1103a9f0ab504680182e73be66fa62/?utm_source=unknown)). RMSSD and related short-term indices are commonly used as correlates of vagally mediated cardiac modulation, but respiration, posture, artifacts, recording duration, age and context affect interpretation ([Laborde et al., 2017](https://www.frontiersin.org/journals/psychology/articles/10.3389/fpsyg.2017.00213/full)).

The default HRV solve follows the legacy five-minute window, requires at least 90 accepted RR samples and waits for 99% time-window coverage. Shorter samples are not silently labeled equivalent to conventional short-term HRV.

## Coherence and the HeartMath-style ratio

The coherence processor reproduces the legacy spectral constants. It linearly resamples the 64-second RR tachogram to 128 points, removes its mean, applies a Hann window, computes the spectrum, finds the dominant peak from 0.04–0.26 Hz, integrates a ±0.015 Hz peak window, and integrates total power from 0.0033–0.4 Hz.

- `coherence = clamp(peakBandPower / totalPower, 0, 1)`
- `heartmath_coherence = (peakBandPower / (totalPower - peakBandPower))²`

Slow paced breathing near an individual's cardiorespiratory resonance (often near 0.1 Hz) can concentrate RR spectral power, but the mechanism and response are more complex than a generic state of “relaxation” ([Sévoz-Couche & Laborde, 2022](https://consensus.app/papers/details/2db97cd4953751d6b7e682cae2d37420/?utm_source=unknown); [Shaffer et al., 2020](https://consensus.app/papers/details/5c1212d280615e05a0b04034626e7727/?utm_source=unknown)). The HeartMath-style value is therefore named for formula compatibility and described as spectral concentration, not as a clinical or emotional score. `coherence_confidence` reports sample/window readiness only.

Polar Stream intentionally does not expose LF/HF as “sympathovagal balance.” Reviews and validation work challenge both LF as a pure sympathetic measure and LF/HF as an accurate balance measure ([Billman, 2013](https://pubmed.ncbi.nlm.nih.gov/23431279/); [Thomas et al., 2019](https://consensus.app/papers/details/32ed9f9067f358eb988621adb60238f6/?utm_source=unknown)).

## ACC-derived breathing and phase

The breathing processor applies a 0.10 EMA to ACC, learns the dominant motion axis by PCA over a 12-second calibration window, projects samples onto that axis, uses the 5th and 95th percentile projections as robust bounds, and scales the waveform to 0–1. Rising batches are classified as inhale, falling batches as exhale, and changes inside the threshold as pause.

Chest-worn accelerometers can estimate respiratory motion and rate, but body movement degrades the signal and methods require validation against a respiratory reference ([Drummond et al., 2021](https://consensus.app/papers/details/9389301a991e52ee9210f51939d318d7/?utm_source=unknown); [Fazio et al., 2022](https://consensus.app/papers/details/7414c3f819b3558482e7001db50189cf/?utm_source=unknown)). The Polar Stream projection is therefore labeled an experimental chest-motion waveform—not lung volume, airflow or a clinically validated phase classifier.

## Breathing dynamics

Accepted alternating peaks/troughs create an amplitude series; accepted same-polarity extrema create a breath-interval series. Each series exposes mean, sample SD, coefficient of variation, the first autocorrelation lag below 0.5, low-frequency log–log PSD slope, normalized mean-binarized Lempel–Ziv complexity, sample entropy (`m=2`, delay 1, `r=0.2 SD`) and multiscale-entropy area across scales 1–5 (`m=3`). Basic statistics require eight accepted observations and entropy/complexity requires 24.

Entropy and complexity methods describe irregularity and structure across time; a higher number is not inherently “better,” “healthier” or “more relaxed,” and estimates depend on preprocessing, parameters and sample length ([Bará et al., 2024](https://consensus.app/papers/details/de562a29bb8454eda201852b544fd9d7/?utm_source=unknown); [Martins et al., 2020](https://consensus.app/papers/details/474b254cbdb556aa903883f4e838b3cb/?utm_source=unknown)). The feature family follows the previous app and the breathing-dynamics work of [Goheen et al. (2025)](https://pubmed.ncbi.nlm.nih.gov/40932176/), but this app substitutes an ACC-derived waveform for the study's respiration-belt input; it is an experimental adaptation.

## Excitometer

The experimental excitometer combines within-session standardized heart-rate elevation (65%) and standardized lnRMSSD reduction (35%), then maps the weighted sum through a logistic function to 0–1. It needs sufficient running baseline data and deliberately makes no claim to identify a particular emotion.

HRV often changes during stress, but findings and usable metrics depend on population, task, breathing, artifacts and method, and there is no accepted single-sensor standard for psychological stress ([Immanuel et al., 2023](https://consensus.app/papers/details/14838ea9a9045710b4a676dbb7d595aa/?utm_source=unknown); [Kim et al., 2018](https://consensus.app/papers/details/e365706a153354869319eb190770ea32/?utm_source=unknown)). Consequently the metric remains in an “Excitation (experimental)” filter and should be validated for any intended protocol before use.

## Normalization

Normalization is an output transform, not a new physiological measure. Sliding mode uses the minimum and maximum observed within the chosen 5–3600 second window; whole-run mode uses extrema since that processor/output configuration began. A zero-range series returns 0.5 until the minimum and maximum separate. Raw ECG/ACC and categorical phase are not normalized.
