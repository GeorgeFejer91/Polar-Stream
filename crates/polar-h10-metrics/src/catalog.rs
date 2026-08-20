use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricDefinition {
    pub id: &'static str,
    pub stream_suffix: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
    pub unit: &'static str,
    pub category: &'static str,
    pub explainer: &'static str,
    pub evidence: &'static str,
    pub citation_label: &'static str,
    pub citation_url: &'static str,
    pub keywords: &'static str,
    pub raw: bool,
    pub normalizable: bool,
    pub channels: i32,
    pub rate_hz: f64,
    pub stream_type: &'static str,
}

/// Mathematical context kept alongside the scientific metric catalog. This is
/// resolved at runtime because stable Rust cannot match string IDs in a const
/// initializer.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricFormulaDefinition {
    /// Human-readable definition shown in the metric library.
    pub formula: &'static str,
    /// Executable Formula Lab expression when the bounded scalar runtime can
    /// reproduce the metric directly.
    pub formula_template: Option<&'static str>,
    /// Sensor channel used by the executable template.
    pub formula_source: &'static str,
}

impl MetricDefinition {
    pub fn for_id(id: &str) -> Option<Self> {
        metric_definition(id)
    }

    pub fn suffix(self) -> &'static str {
        self.stream_suffix
    }
}

const HRV_REVIEW: &str = "Shaffer & Ginsberg (2017)";
const HRV_URL: &str =
    "https://www.frontiersin.org/journals/public-health/articles/10.3389/fpubh.2017.00258/full";
const HRV_METHODS: &str = "Laborde, Mosley & Thayer (2017)";
const HRV_METHODS_URL: &str =
    "https://www.frontiersin.org/journals/psychology/articles/10.3389/fpsyg.2017.00213/full";
const LFHF_REVIEW: &str = "Billman (2013)";
const LFHF_URL: &str = "https://pubmed.ncbi.nlm.nih.gov/23431279/";
const RESONANCE_REVIEW: &str = "Sévoz-Couche & Laborde (2022)";
const RESONANCE_URL: &str =
    "https://www.sciencedirect.com/science/article/abs/pii/S0149763422000653";
const BREATH_ACC: &str = "Schipper et al. (2021)";
const BREATH_ACC_URL: &str = "https://pubmed.ncbi.nlm.nih.gov/33739305/";
const BREATH_COMPLEXITY: &str = "Bará et al. (2024)";
const BREATH_COMPLEXITY_URL: &str =
    "https://consensus.app/papers/details/de562a29bb8454eda201852b544fd9d7/?utm_source=unknown";
const STRESS_REVIEW: &str = "Immanuel et al. (2023)";
const STRESS_URL: &str =
    "https://consensus.app/papers/details/14838ea9a9045710b4a676dbb7d595aa/?utm_source=unknown";
const EXCITEOMETER_SOURCE: &str = "Excite-O-Meter source implementation";
const EXCITEOMETER_SOURCE_URL: &str =
    "https://github.com/luisqtr/exciteometer/blob/main/docs/1_UserManual.md#scientific-disclaimer";
const ECG_QUALITY: &str = "Smital et al. (2020)";
const ECG_QUALITY_URL: &str =
    "https://consensus.app/papers/details/ad2a724fefd55d25baee823438fc672e/?utm_source=unknown";
const LEGACY_FORMULAS: &str = "Polar Stream legacy formula inventory";
const LEGACY_FORMULAS_URL: &str =
    "https://github.com/GeorgeFejer91/Polar-Stream/blob/main/docs/metric-evidence.md";

macro_rules! metric {
    ($id:literal, $suffix:literal, $label:literal, $detail:literal, $unit:literal,
     $category:literal, $explainer:expr, $evidence:literal, $citation:expr, $url:expr,
     $keywords:literal, $raw:literal, $normalizable:literal, $channels:literal, $rate:literal, $stream:literal) => {
        MetricDefinition {
            id: $id,
            stream_suffix: $suffix,
            label: $label,
            detail: $detail,
            unit: $unit,
            category: $category,
            explainer: $explainer,
            evidence: $evidence,
            citation_label: $citation,
            citation_url: $url,
            keywords: $keywords,
            raw: $raw,
            normalizable: $normalizable,
            channels: $channels,
            rate_hz: $rate,
            stream_type: $stream,
        }
    };
}

fn formula_for(id: &str) -> &'static str {
    match id {
        "raw_ecg" => "y(t) = ECG(t) [µV]; x-axis = sensor time t",
        "raw_acc" => "a(t) = [x(t), y(t), z(t)] [mg]",
        "raw_force" => "F(t) = Go Direct force measurement [N]",
        "acc_magnitude" => "|a(t)| = √(x(t)² + y(t)² + z(t)²) / 1000",
        "ecg_mean" => "μECG(t) = (1/N) Σ ECGᵢ over the preceding 5 s",
        "ecg_rms" => "RMS(t) = √[(1/N) Σ ECGᵢ²] over the preceding 5 s",
        "ecg_peak_to_peak" => "P2P(t) = max(ECGᵢ) − min(ECGᵢ) over the preceding 5 s",
        "ecg_sd" => "sECG(t) = √[Σ(ECGᵢ − μECG)² / (N − 1)] over 5 s",
        "heart_rate" => "HR(t) = device-reported beats per minute",
        "rr_interval" => "RRᵢ = t(Rᵢ) − t(Rᵢ₋₁)",
        "mean_nn" => "meanNN = (1/N) Σ NNᵢ over the accepted 5 min window",
        "mean_heart_rate" => "meanHR = 60,000 / meanNN",
        "rmssd" => "RMSSD = √[(1/(N−1)) Σ(NNᵢ₊₁ − NNᵢ)²]",
        "ln_rmssd" => "lnRMSSD = ln(RMSSD)",
        "sdnn" => "SDNN = √[Σ(NNᵢ − meanNN)² / (N−1)]",
        "pnn50" => "pNN50 = 100 · count(|NNᵢ₊₁ − NNᵢ| > 50 ms) / (N−1)",
        "sd1" => "SD1 = RMSSD / √2",
        "coherence" => "coherence = peak-band RR power / total RR power",
        "coherence_confidence" => "confidence = 0.55·sample readiness + 0.45·window coverage",
        "heartmath_coherence" => {
            "HeartMath ratio = [peak-band power / (total power − peak-band power)]²"
        }
        "coherence_peak_frequency" => "fpeak = argmax P(f), 0.04 ≤ f ≤ 0.26 Hz",
        "coherence_peak_power" => "Ppeak = ∫ P(f)df from fpeak−0.015 to fpeak+0.015 Hz",
        "coherence_total_power" => "Ptotal = ∫ P(f)df from 0.0033 to 0.4 Hz",
        "acc_breathing_magnitude" => {
            "b(t) = smoothed projection of selected ACC axes onto the calibrated principal axis"
        }
        "breathing_volume" => "volume(t) = clamp[(b(t)−q₀.₀₅)/(q₀.₉₅−q₀.₀₅), 0, 1]",
        "breathing_phase" => "phase(t) = threshold[(volume(t)−volume(t−Δt))/Δt]; {−1,0,+1}",
        "breathing_calibration" => "progress(t) = collected calibration samples / required samples",
        "breathing_axis_range" => "axisRange = q₀.₉₅(b) − q₀.₀₅(b)",
        "breathing_signal_confidence" => {
            "confidence = rangeQuality · motionQuality · (0.4 + 0.6·coverage·periodicity)"
        }
        "breathing_signal_ready" => "ready = calibrated ∧ fresh ∧ motionQuality ≥ 0.35",
        "breathing_rate" => "rate = 60 / mean(same-polarity extremum intervals)",
        "breathing_dynamics_confidence" => {
            "confidence = clamp(max(Ninterval,Namplitude) / 200, 0, 1)"
        }
        "breath_interval_mean" => "μI = (1/N) Σ intervalᵢ",
        "breath_interval_sd" => "sI = √[Σ(intervalᵢ−μI)²/(N−1)]",
        "breath_interval_cv" => "CVI = sI / |μI|",
        "breath_interval_acw50" => "ACW50 = first lag k where autocorr(interval,k) < 0.5",
        "breath_interval_psd_slope" => {
            "slope = OLS slope of log power on log frequency in the low-frequency interval spectrum"
        }
        "breath_interval_lzc" => {
            "LZC = normalized Lempel–Ziv phrase count of mean-binarized intervals"
        }
        "breath_interval_sampen" => "SampEn = −ln(A/B), m=2, r=0.2·SD, delay=1",
        "breath_interval_mse" => {
            "MSE = trapezoidal AUC of SampEn across coarse-graining scales 1…5"
        }
        "breath_amplitude_mean" => "μA = (1/N) Σ |peakᵢ − troughᵢ|",
        "breath_amplitude_sd" => "sA = √[Σ(amplitudeᵢ−μA)²/(N−1)]",
        "breath_amplitude_cv" => "CVA = sA / |μA|",
        "breath_amplitude_acw50" => "ACW50 = first lag k where autocorr(amplitude,k) < 0.5",
        "breath_amplitude_psd_slope" => {
            "slope = OLS slope of log power on log frequency in the low-frequency amplitude spectrum"
        }
        "breath_amplitude_lzc" => {
            "LZC = normalized Lempel–Ziv phrase count of mean-binarized amplitudes"
        }
        "breath_amplitude_sampen" => "SampEn = −ln(A/B), m=2, r=0.2·SD, delay=1",
        "breath_amplitude_mse" => {
            "MSE = trapezoidal AUC of SampEn across coarse-graining scales 1…5"
        }
        "excitement_score" => "score = 1 − [Φ(zRR) + Φ(zRMSSD₁₀)] / 2",
        "excitometer" => "activation = logistic[0.65·z(HR) − 0.35·z(lnRMSSD)]",
        _ => "See the implementation and evidence catalog.",
    }
}

fn formula_template_for(id: &str) -> Option<&'static str> {
    match id {
        "raw_ecg" => Some("ecg"),
        "acc_magnitude" => Some("sqrt(x*x + y*y + z*z) / 1000"),
        "ecg_mean" => Some("moving_mean(ecg, 5)"),
        "ecg_rms" => Some("moving_rms(ecg, 5)"),
        "ecg_peak_to_peak" => Some("moving_max(ecg, 5) - moving_min(ecg, 5)"),
        "ecg_sd" => Some("moving_std(ecg, 5)"),
        "heart_rate" => Some("hr"),
        "rr_interval" => Some("rr"),
        "mean_nn" => Some("rr_mean(rr, 300)"),
        "mean_heart_rate" => Some("rr_mean_hr(rr, 300)"),
        "rmssd" => Some("rr_rmssd(rr, 300)"),
        "ln_rmssd" => Some("rr_ln_rmssd(rr, 300)"),
        "sdnn" => Some("rr_sdnn(rr, 300)"),
        "pnn50" => Some("rr_pnn50(rr, 300)"),
        "sd1" => Some("rr_sd1(rr, 300)"),
        "acc_breathing_magnitude" => {
            Some("breathing_magnitude(x, y, z, true, false, true, 0.75, false, false)")
        }
        "breathing_volume" => {
            Some("breathing_magnitude(x, y, z, true, false, true, 0.75, true, false)")
        }
        "breathing_phase" => Some("breathing_phase(x, y, z, true, false, true, 0.75, 0.60, false)"),
        "excitement_score" => Some("excitement(rr, 300)"),
        "excitometer" => {
            Some("sigmoid(0.65*zscore_n(60000/rr, 20) - 0.35*zscore_n(rr_ln_rmssd(rr, 300), 20))")
        }
        _ => None,
    }
}

fn formula_source_for(id: &str) -> &'static str {
    match id {
        "raw_acc"
        | "acc_magnitude"
        | "acc_breathing_magnitude"
        | "breathing_volume"
        | "breathing_phase"
        | "breathing_calibration"
        | "breathing_axis_range"
        | "breathing_signal_confidence"
        | "breathing_signal_ready"
        | "breathing_rate"
        | "breathing_dynamics_confidence"
        | "breath_interval_mean"
        | "breath_interval_sd"
        | "breath_interval_cv"
        | "breath_interval_acw50"
        | "breath_interval_psd_slope"
        | "breath_interval_lzc"
        | "breath_interval_sampen"
        | "breath_interval_mse"
        | "breath_amplitude_mean"
        | "breath_amplitude_sd"
        | "breath_amplitude_cv"
        | "breath_amplitude_acw50"
        | "breath_amplitude_psd_slope"
        | "breath_amplitude_lzc"
        | "breath_amplitude_sampen"
        | "breath_amplitude_mse" => "accelerometer",
        "heart_rate" => "heartRate",
        "rr_interval"
        | "mean_nn"
        | "mean_heart_rate"
        | "rmssd"
        | "ln_rmssd"
        | "sdnn"
        | "pnn50"
        | "sd1"
        | "coherence"
        | "coherence_confidence"
        | "heartmath_coherence"
        | "coherence_peak_frequency"
        | "coherence_peak_power"
        | "coherence_total_power"
        | "excitement_score"
        | "excitometer" => "rrInterval",
        _ => "ecg",
    }
}

pub fn metric_formula_definition(id: &str) -> MetricFormulaDefinition {
    MetricFormulaDefinition {
        formula: formula_for(id),
        formula_template: formula_template_for(id),
        formula_source: formula_source_for(id),
    }
}

pub const METRIC_CATALOG: &[MetricDefinition] = &[
    metric!(
        "raw_ecg",
        "rawECG",
        "Raw ECG",
        "Unfiltered H10 voltage samples · 130 Hz",
        "µV",
        "Raw signals",
        "The H10 exposes a single-lead ECG-like voltage waveform. It is useful for waveform inspection and independent processing, but this research stream is not a diagnostic 12-lead ECG.",
        "device signal",
        ECG_QUALITY,
        ECG_QUALITY_URL,
        "raw voltage waveform electrocardiogram",
        true,
        false,
        1,
        130.0,
        "ECG"
    ),
    metric!(
        "raw_acc",
        "rawACC",
        "Raw accelerometer",
        "X, Y and Z · 200 Hz",
        "mg",
        "Raw signals",
        "Three-axis chest acceleration contains body motion, posture and small breathing-related movements. Motion strongly confounds respiratory inference, so downstream users should retain the raw axes for quality control.",
        "device signal",
        BREATH_ACC,
        BREATH_ACC_URL,
        "raw movement acceleration x y z",
        true,
        false,
        3,
        200.0,
        "Accelerometer"
    ),
    metric!(
        "raw_force",
        "rawForce",
        "Raw Go Direct force",
        "Verified GDX-RB Force (N) · 10 Hz preferred, metadata fallback",
        "N",
        "Raw signals",
        "The Go Direct force channel is forwarded as measured by the Vernier device. Notification batches are preserved and intra-batch timestamps use the configured sample period.",
        "protocol interoperability",
        "Vernier Go Direct examples",
        "https://github.com/VernierST/godirect-examples",
        "vernier go direct respiration belt force newton raw",
        true,
        false,
        1,
        0.0,
        "RespirationForce"
    ),
    metric!(
        "acc_magnitude",
        "accMagnitude",
        "3D acceleration magnitude",
        "Euclidean magnitude of X, Y and Z",
        "g",
        "Raw signals",
        "Acceleration magnitude is √(x²+y²+z²), expressed in g. It removes orientation sign but combines gravity, body movement and breathing motion, so it is a signal feature rather than a specific physiological measure.",
        "signal feature",
        BREATH_ACC,
        BREATH_ACC_URL,
        "movement motion magnitude activity",
        false,
        true,
        1,
        200.0,
        "AccelerometerMetric"
    ),
    metric!(
        "ecg_mean",
        "ecgMean",
        "ECG window mean",
        "Five-second rolling mean",
        "µV",
        "ECG features",
        "The rolling mean estimates the local DC offset of the single-lead waveform. Changes can reflect baseline wander or electrode/contact effects and should not be interpreted as cardiac morphology.",
        "signal-quality aid",
        ECG_QUALITY,
        ECG_QUALITY_URL,
        "ecg baseline offset mean quality",
        false,
        true,
        1,
        2.0,
        "ECGMetric"
    ),
    metric!(
        "ecg_rms",
        "ecgRms",
        "ECG RMS amplitude",
        "Five-second root-mean-square amplitude",
        "µV",
        "ECG features",
        "RMS summarizes overall waveform energy within the rolling window. It is useful for monitoring amplitude and gross signal changes, but it is not a validated diagnosis or a direct measure of cardiac effort.",
        "signal-quality aid",
        ECG_QUALITY,
        ECG_QUALITY_URL,
        "ecg energy amplitude rms quality",
        false,
        true,
        1,
        2.0,
        "ECGMetric"
    ),
    metric!(
        "ecg_peak_to_peak",
        "ecgPeakToPeak",
        "ECG peak-to-peak",
        "Five-second maximum minus minimum",
        "µV",
        "ECG features",
        "Peak-to-peak amplitude is the largest voltage excursion in the rolling window. It helps reveal clipping, motion spikes and changing contact, but it is highly sensitive to artifacts.",
        "signal-quality aid",
        ECG_QUALITY,
        ECG_QUALITY_URL,
        "ecg range clipping artifact quality",
        false,
        true,
        1,
        2.0,
        "ECGMetric"
    ),
    metric!(
        "ecg_sd",
        "ecgSd",
        "ECG standard deviation",
        "Five-second sample standard deviation",
        "µV",
        "ECG features",
        "Standard deviation describes dispersion around the local ECG mean. Like RMS, it can support signal-quality inspection but does not isolate a particular ECG wave or clinical condition.",
        "signal-quality aid",
        ECG_QUALITY,
        ECG_QUALITY_URL,
        "ecg variability standard deviation quality",
        false,
        true,
        1,
        2.0,
        "ECGMetric"
    ),
    metric!(
        "heart_rate",
        "heartRate",
        "Heart rate",
        "H10 device-derived beat rate",
        "bpm",
        "Heart rate",
        "Heart rate is the number of cardiac cycles per minute reported by the sensor. It responds to activity, posture, temperature, emotion and many other influences, so context is essential.",
        "established measure",
        HRV_REVIEW,
        HRV_URL,
        "pulse bpm cardiac beats",
        false,
        true,
        1,
        0.0,
        "HeartRate"
    ),
    metric!(
        "rr_interval",
        "rrInterval",
        "RR interval",
        "Accepted beat-to-beat interval",
        "ms",
        "Heart rate",
        "An RR interval is the elapsed time between consecutive detected heartbeats. Clean normal-to-normal intervals are the input to HRV calculations; ectopic beats and detection artifacts can distort every derived metric.",
        "established measure",
        HRV_METHODS,
        HRV_METHODS_URL,
        "ibi nn beat interval heartbeat",
        false,
        true,
        1,
        0.0,
        "RR"
    ),
    metric!(
        "mean_nn",
        "meanNN",
        "Mean NN interval",
        "Five-minute accepted RR mean",
        "ms",
        "Heart rate",
        "Mean NN is the average accepted normal-to-normal interval in the analysis window. It is inversely related to mean heart rate, while preserving interval units used by HRV methods.",
        "established measure",
        HRV_REVIEW,
        HRV_URL,
        "mean rr ibi nn average",
        false,
        true,
        1,
        0.0,
        "HRV"
    ),
    metric!(
        "mean_heart_rate",
        "meanHeartRate",
        "Mean heart rate",
        "60,000 divided by mean NN",
        "bpm",
        "Heart rate",
        "Mean heart rate is calculated from the mean accepted NN interval over the current window. It smooths beat-to-beat changes and should not be confused with the sensor's latest instantaneous value.",
        "established measure",
        HRV_REVIEW,
        HRV_URL,
        "mean average bpm rolling",
        false,
        true,
        1,
        0.0,
        "HRV"
    ),
    metric!(
        "rmssd",
        "rmssd",
        "RMSSD",
        "Five-minute successive-difference HRV",
        "ms",
        "HRV & relaxation",
        "RMSSD is the root mean square of successive NN-interval differences and emphasizes short-term beat-to-beat variability. It is commonly used as a vagally mediated HRV index, but respiration, posture, artifacts and individual context still matter.",
        "well-established HRV",
        HRV_REVIEW,
        HRV_URL,
        "hrv vagal parasympathetic recovery relaxation",
        false,
        true,
        1,
        0.0,
        "HRV"
    ),
    metric!(
        "ln_rmssd",
        "lnRMSSD",
        "lnRMSSD",
        "Natural logarithm of RMSSD",
        "ln(ms)",
        "HRV & relaxation",
        "lnRMSSD is the natural-log transform of RMSSD, used because raw RMSSD is often right-skewed. It improves comparability in statistical work but retains RMSSD's physiological and measurement limitations.",
        "well-established transform",
        HRV_METHODS,
        HRV_METHODS_URL,
        "hrv log vagal recovery statistics",
        false,
        true,
        1,
        0.0,
        "HRV"
    ),
    metric!(
        "sdnn",
        "sdnn",
        "SDNN",
        "Five-minute NN sample standard deviation",
        "ms",
        "HRV & relaxation",
        "SDNN is the standard deviation of accepted NN intervals and summarizes total variability present in the chosen window. Its meaning depends strongly on recording length, so values from unlike window durations should not be compared directly.",
        "well-established HRV",
        HRV_REVIEW,
        HRV_URL,
        "hrv total variability autonomic",
        false,
        true,
        1,
        0.0,
        "HRV"
    ),
    metric!(
        "pnn50",
        "pNN50",
        "pNN50",
        "Successive NN differences over 50 ms",
        "%",
        "HRV & relaxation",
        "pNN50 is the percentage of adjacent accepted NN pairs that differ by more than 50 ms. It correlates with RMSSD and vagally mediated variability, although RMSSD is usually preferred and pNN50 depends on age and sampling context.",
        "established HRV",
        HRV_REVIEW,
        HRV_URL,
        "hrv vagal successive differences percentage",
        false,
        true,
        1,
        0.0,
        "HRV"
    ),
    metric!(
        "sd1",
        "sd1",
        "Poincaré SD1",
        "RMSSD divided by √2",
        "ms",
        "HRV & relaxation",
        "SD1 is the short-axis dispersion of the Poincaré plot and describes short-term beat-to-beat variability. For the standard calculation it is mathematically RMSSD/√2, so selecting both does not provide independent information.",
        "well-established HRV",
        HRV_REVIEW,
        HRV_URL,
        "hrv poincare short term vagal",
        false,
        true,
        1,
        0.0,
        "HRV"
    ),
    metric!(
        "coherence",
        "coherence",
        "Normalized coherence",
        "Peak-band power divided by total power",
        "0–1",
        "Coherence",
        "This app-specific score is the fraction of RR spectral power concentrated around the dominant 0.04–0.26 Hz peak. Slow breathing near cardiorespiratory resonance can increase narrow-band oscillation, but the score is not a direct measure of emotion, health or relaxation.",
        "algorithmic spectral index",
        RESONANCE_REVIEW,
        RESONANCE_URL,
        "coherence resonance breathing hrv spectral",
        false,
        true,
        1,
        0.0,
        "Coherence"
    ),
    metric!(
        "coherence_confidence",
        "coherenceConfidence",
        "Coherence confidence",
        "Window coverage and sample readiness",
        "0–1",
        "Coherence",
        "Confidence reports whether the coherence window has enough accepted RR data and near-complete time coverage. It describes computational readiness, not confidence that a psychological interpretation is true.",
        "quality indicator",
        LEGACY_FORMULAS,
        LEGACY_FORMULAS_URL,
        "coherence quality readiness coverage",
        false,
        true,
        1,
        0.0,
        "Coherence"
    ),
    metric!(
        "heartmath_coherence",
        "heartMathCoherence",
        "HeartMath-style coherence ratio",
        "(peak power ÷ remaining power)²",
        "ratio",
        "Coherence",
        "This reproduces the legacy HeartMath-style formula: the squared ratio of power in a 0.03 Hz peak window to remaining 0.0033–0.4 Hz power. It quantifies spectral concentration and is exposed for compatibility; it is not a validated clinical relaxation score.",
        "legacy/proprietary-inspired",
        RESONANCE_REVIEW,
        RESONANCE_URL,
        "heartmath coherence ratio peak power legacy",
        false,
        true,
        1,
        0.0,
        "Coherence"
    ),
    metric!(
        "coherence_peak_frequency",
        "coherencePeakFrequency",
        "Coherence peak frequency",
        "Dominant RR spectral peak",
        "Hz",
        "Coherence",
        "Peak frequency is the strongest RR-variability oscillation between 0.04 and 0.26 Hz. A value near 0.1 Hz can occur during slow paced breathing and resonance, but frequency alone does not establish why the oscillation occurred.",
        "spectral feature",
        RESONANCE_REVIEW,
        RESONANCE_URL,
        "coherence resonance frequency breathing",
        false,
        true,
        1,
        0.0,
        "Coherence"
    ),
    metric!(
        "coherence_peak_power",
        "coherencePeakPower",
        "Coherence peak-band power",
        "Integrated power ±0.015 Hz around peak",
        "ms²",
        "Coherence",
        "Peak-band power integrates the RR spectrum in a 0.03 Hz window centered on the dominant coherence peak. Its absolute scale depends on preprocessing and windowing, so comparisons require identical settings.",
        "spectral feature",
        RESONANCE_REVIEW,
        RESONANCE_URL,
        "coherence power spectrum peak band",
        false,
        true,
        1,
        0.0,
        "Coherence"
    ),
    metric!(
        "coherence_total_power",
        "coherenceTotalPower",
        "Coherence total power",
        "Integrated RR power from 0.0033–0.4 Hz",
        "ms²",
        "Coherence",
        "Total power integrates the RR spectrum across the legacy 0.0033–0.4 Hz analysis band. It supplies the denominator for the normalized score and is not by itself a specific sympathetic or parasympathetic measure.",
        "spectral feature",
        LFHF_REVIEW,
        LFHF_URL,
        "coherence total power spectrum hrv",
        false,
        true,
        1,
        0.0,
        "Coherence"
    ),
    metric!(
        "acc_breathing_magnitude",
        "accBreathingMagnitude",
        "ACC breathing magnitude estimate",
        "Smoothed principal-axis chest-motion projection",
        "g",
        "Breathing",
        "This continuous curve is a smoothed projection of selected accelerometer axes, not lung volume or airflow. It can preserve timing for exploratory breath-rate analysis, but this H10-specific estimate is unvalidated and is strongly confounded by movement, posture, strap placement and axis choice.",
        "unvalidated experimental estimate",
        BREATH_ACC,
        BREATH_ACC_URL,
        "acc breath respiration magnitude waveform projection experimental",
        false,
        true,
        1,
        20.0,
        "Breathing"
    ),
    metric!(
        "breathing_volume",
        "breathingVolume",
        "ACC breathing waveform",
        "Calibrated chest-motion projection",
        "0–1",
        "Breathing",
        "The waveform projects smoothed chest acceleration onto a calibrated principal movement axis and rescales it between observed quantile bounds. Chest accelerometers can capture breathing motion, but this H10-specific algorithm has not been validated as lung volume and is vulnerable to body movement.",
        "experimental estimate",
        BREATH_ACC,
        BREATH_ACC_URL,
        "breath respiration waveform volume chest",
        false,
        true,
        1,
        20.0,
        "Breathing"
    ),
    metric!(
        "breathing_signal_confidence",
        "breathingSignalConfidence",
        "ACC breathing signal confidence",
        "Range, motion, coverage, and periodicity quality index",
        "0–1",
        "Breathing",
        "Confidence summarizes whether the calibrated H10 chest-motion projection is strong, recent, relatively periodic, and not dominated by broadband movement. It is an app-specific signal-quality index rather than a probability of physiological correctness, and low values should cause downstream analyses to reject or flag the waveform.",
        "experimental quality indicator",
        BREATH_ACC,
        BREATH_ACC_URL,
        "breath respiration waveform signal confidence quality motion periodicity",
        false,
        false,
        1,
        20.0,
        "Breathing"
    ),
    metric!(
        "breathing_signal_ready",
        "breathingSignalReady",
        "ACC breathing signal ready",
        "Calibration, freshness, and motion gate",
        "0/1",
        "Breathing",
        "Ready becomes one only after principal-axis calibration while samples are fresh and the short-term movement score remains acceptable. It is a processing-readiness flag, not evidence that the waveform equals airflow or lung volume.",
        "experimental readiness indicator",
        BREATH_ACC,
        BREATH_ACC_URL,
        "breath respiration waveform ready calibration freshness motion gate",
        false,
        false,
        1,
        20.0,
        "Breathing"
    ),
    metric!(
        "breathing_phase",
        "breathingPhase",
        "Breath phase classifier",
        "+1 inhale · −1 exhale · 0 pause or not ready",
        "class",
        "Breathing",
        "Phase classifies the calibrated ACC waveform velocity into three public states: inhale, exhale, or pause/not ready. Its threshold is normalized per second from the accepted ACC sample count so BLE batch size does not change the classification scale. It remains an unvalidated motion classification rather than airflow; movement, posture, strap placement and axis choice can obscure or reverse the inferred phase.",
        "unvalidated experimental classification",
        BREATH_ACC,
        BREATH_ACC_URL,
        "inhale exhale pause phase respiration classifier experimental",
        false,
        false,
        1,
        20.0,
        "Breathing"
    ),
    metric!(
        "breathing_calibration",
        "breathingCalibration",
        "Breathing calibration",
        "Principal-axis calibration progress",
        "0–1",
        "Breathing",
        "Calibration progress reports how much of the initial ACC calibration interval has been observed. A completed interval does not guarantee a clean respiratory signal; sufficient chest-motion range is also required.",
        "quality indicator",
        LEGACY_FORMULAS,
        LEGACY_FORMULAS_URL,
        "breath calibration readiness quality",
        false,
        false,
        1,
        4.0,
        "Breathing"
    ),
    metric!(
        "breathing_axis_range",
        "breathingAxisRange",
        "Breathing axis range",
        "Calibrated 5th–95th percentile travel",
        "g",
        "Breathing",
        "Axis range is the robust movement span used to scale the ACC-derived waveform. A very small span suggests weak signal, while a large span may reflect breathing or unrelated movement.",
        "signal-quality aid",
        BREATH_ACC,
        BREATH_ACC_URL,
        "breath movement range quality calibration",
        false,
        true,
        1,
        4.0,
        "Breathing"
    ),
    metric!(
        "breathing_rate",
        "breathingRate",
        "Breathing rate",
        "60 divided by mean peak-to-peak interval",
        "breaths/min",
        "Breathing",
        "Breathing rate is estimated from successive like-polarity extrema in the ACC-derived waveform. It becomes available only after accepted cycles and can be unreliable during movement or irregular shallow breathing.",
        "experimental estimate",
        BREATH_ACC,
        BREATH_ACC_URL,
        "respiratory rate rpm breaths minute",
        false,
        true,
        1,
        0.0,
        "Breathing"
    ),
    metric!(
        "breathing_dynamics_confidence",
        "breathingDynamicsConfidence",
        "Breathing-dynamics confidence",
        "Accepted-breath count and freshness",
        "0–1",
        "Breathing dynamics",
        "Confidence increases as accepted respiratory cycles accumulate toward the legacy 200-breath target. It expresses data sufficiency and freshness, not certainty about a clinical or emotional state.",
        "quality indicator",
        LEGACY_FORMULAS,
        LEGACY_FORMULAS_URL,
        "breath entropy quality readiness confidence",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_interval_mean",
        "breathIntervalMean",
        "Breath interval mean",
        "Mean like-polarity extremum interval",
        "s",
        "Breathing dynamics",
        "Mean breath interval averages the time between successive peaks or successive troughs. Its reciprocal gives breathing rate, while variability around it is retained in separate dynamics metrics.",
        "experimental ACC adaptation",
        BREATH_ACC,
        BREATH_ACC_URL,
        "breath interval duration mean",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_interval_sd",
        "breathIntervalSD",
        "Breath interval SD",
        "Sample standard deviation of intervals",
        "s",
        "Breathing dynamics",
        "Interval SD measures absolute dispersion in detected respiratory-cycle duration. More variability is not inherently better or worse; artifacts, tasks and genuine adaptive variation can all increase it.",
        "experimental ACC adaptation",
        BREATH_COMPLEXITY,
        BREATH_COMPLEXITY_URL,
        "breath interval variability standard deviation",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_interval_cv",
        "breathIntervalCV",
        "Breath interval CV",
        "Interval SD divided by mean",
        "ratio",
        "Breathing dynamics",
        "The coefficient of variation divides interval SD by the absolute interval mean, making dispersion less dependent on scale. It is descriptive and should not be interpreted as a standalone marker of respiratory health.",
        "experimental ACC adaptation",
        BREATH_COMPLEXITY,
        BREATH_COMPLEXITY_URL,
        "breath interval coefficient variation normalized",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_interval_acw50",
        "breathIntervalACW50",
        "Breath interval ACW50",
        "First autocorrelation lag below 0.5",
        "breaths",
        "Breathing dynamics",
        "ACW50 is the first lag where autocorrelation drops below 0.5, summarizing how long interval patterns remain similar. It is a time-series memory descriptor, not a direct measure of relaxation or pathology.",
        "exploratory complexity",
        BREATH_COMPLEXITY,
        BREATH_COMPLEXITY_URL,
        "breath autocorrelation memory dynamics",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_interval_psd_slope",
        "breathIntervalPsdSlope",
        "Breath interval PSD slope",
        "Low-frequency log–log spectral slope",
        "slope",
        "Breathing dynamics",
        "PSD slope summarizes how interval-series power changes with frequency after detrending and standardization. It is an exploratory scaling feature whose value depends on record length and implementation choices.",
        "exploratory complexity",
        BREATH_COMPLEXITY,
        BREATH_COMPLEXITY_URL,
        "breath spectrum slope fractal dynamics",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_interval_lzc",
        "breathIntervalLZC",
        "Breath interval Lempel–Ziv",
        "Median-binarized normalized complexity",
        "0–1",
        "Breathing dynamics",
        "Lempel–Ziv complexity counts new binary patterns after thresholding intervals around their mean. Higher values indicate less compressible sequences in this encoding, not automatically healthier or more adaptive breathing.",
        "exploratory complexity",
        BREATH_COMPLEXITY,
        BREATH_COMPLEXITY_URL,
        "breath lempel ziv irregularity complexity",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_interval_sampen",
        "breathIntervalSampEn",
        "Breath interval sample entropy",
        "m=2 · r=0.2 SD · delay 1",
        "entropy",
        "Breathing dynamics",
        "Sample entropy estimates the unpredictability of interval patterns by comparing matches of length m and m+1. Values depend on sample count and parameter choices; short or artifact-contaminated series are unstable.",
        "established method, experimental input",
        BREATH_COMPLEXITY,
        BREATH_COMPLEXITY_URL,
        "breath sample entropy irregularity complexity",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_interval_mse",
        "breathIntervalMSE",
        "Breath interval multiscale entropy",
        "Entropy AUC across scales 1–5",
        "entropy AUC",
        "Breathing dynamics",
        "Multiscale entropy aggregates sample entropy after coarse-graining the interval series across several scales. It aims to capture complexity beyond one time scale, but needs substantial clean data and is not a diagnostic score here.",
        "established method, experimental input",
        BREATH_COMPLEXITY,
        BREATH_COMPLEXITY_URL,
        "breath multiscale entropy complexity",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_amplitude_mean",
        "breathAmplitudeMean",
        "Breath amplitude mean",
        "Mean alternating peak-to-trough excursion",
        "0–1",
        "Breathing dynamics",
        "Mean amplitude averages accepted peak-to-trough excursions in the normalized ACC waveform. Because the waveform is wearer- and calibration-specific, this is relative chest movement rather than tidal volume.",
        "experimental ACC adaptation",
        BREATH_ACC,
        BREATH_ACC_URL,
        "breath amplitude depth excursion",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_amplitude_sd",
        "breathAmplitudeSD",
        "Breath amplitude SD",
        "Sample standard deviation of excursions",
        "0–1",
        "Breathing dynamics",
        "Amplitude SD measures dispersion in accepted normalized chest-motion excursions. Movement artifacts and changing posture can alter it, so it should be inspected alongside the raw ACC and confidence.",
        "experimental ACC adaptation",
        BREATH_COMPLEXITY,
        BREATH_COMPLEXITY_URL,
        "breath amplitude variability standard deviation",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_amplitude_cv",
        "breathAmplitudeCV",
        "Breath amplitude CV",
        "Amplitude SD divided by mean",
        "ratio",
        "Breathing dynamics",
        "Amplitude CV expresses excursion variability relative to mean excursion. It supports within-protocol comparison but has no universal threshold for relaxation or respiratory health.",
        "experimental ACC adaptation",
        BREATH_COMPLEXITY,
        BREATH_COMPLEXITY_URL,
        "breath amplitude coefficient variation",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_amplitude_acw50",
        "breathAmplitudeACW50",
        "Breath amplitude ACW50",
        "First autocorrelation lag below 0.5",
        "breaths",
        "Breathing dynamics",
        "Amplitude ACW50 reports how many breaths the amplitude pattern remains autocorrelated above 0.5. It describes temporal persistence and does not assign that persistence a positive or negative meaning.",
        "exploratory complexity",
        BREATH_COMPLEXITY,
        BREATH_COMPLEXITY_URL,
        "breath amplitude autocorrelation memory",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_amplitude_psd_slope",
        "breathAmplitudePsdSlope",
        "Breath amplitude PSD slope",
        "Low-frequency log–log spectral slope",
        "slope",
        "Breathing dynamics",
        "This slope summarizes low-frequency scaling in the detrended amplitude sequence. It is exploratory and especially sensitive to accepted-cycle count, outliers and the selected spectral estimator.",
        "exploratory complexity",
        BREATH_COMPLEXITY,
        BREATH_COMPLEXITY_URL,
        "breath amplitude spectral slope dynamics",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_amplitude_lzc",
        "breathAmplitudeLZC",
        "Breath amplitude Lempel–Ziv",
        "Mean-binarized normalized complexity",
        "0–1",
        "Breathing dynamics",
        "Lempel–Ziv complexity measures how compressible the binarized sequence of breath amplitudes is. It is an algorithmic irregularity feature and should not be equated with emotional flexibility without a validated protocol.",
        "exploratory complexity",
        BREATH_COMPLEXITY,
        BREATH_COMPLEXITY_URL,
        "breath amplitude lempel ziv complexity",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_amplitude_sampen",
        "breathAmplitudeSampEn",
        "Breath amplitude sample entropy",
        "m=2 · r=0.2 SD · delay 1",
        "entropy",
        "Breathing dynamics",
        "Sample entropy estimates pattern unpredictability in successive normalized breath amplitudes. It requires enough accepted cycles and can become undefined when patterns are too short or too uniform.",
        "established method, experimental input",
        BREATH_COMPLEXITY,
        BREATH_COMPLEXITY_URL,
        "breath amplitude sample entropy complexity",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "breath_amplitude_mse",
        "breathAmplitudeMSE",
        "Breath amplitude multiscale entropy",
        "Entropy AUC across scales 1–5",
        "entropy AUC",
        "Breathing dynamics",
        "Multiscale entropy summarizes amplitude-sequence unpredictability across coarse-grained scales. This adaptation uses ACC-derived excursions rather than a validated respiratory belt signal, so interpretation remains exploratory.",
        "established method, experimental input",
        BREATH_COMPLEXITY,
        BREATH_COMPLEXITY_URL,
        "breath amplitude multiscale entropy complexity",
        false,
        true,
        1,
        0.0,
        "BreathingDynamics"
    ),
    metric!(
        "excitement_score",
        "excitementScore",
        "Excite-O-Meter excitement score",
        "1 − mean[Φ(zRR), Φ(zRMSSD)] · live provisional",
        "0–1",
        "Excitation (experimental)",
        "This reproduces the open-source Excite-O-Meter equation: independently standardized RR interval and rolling 10-beat RMSSD are converted through the standard-normal CDF, averaged, and subtracted from one. The source method is retrospective; this live stream uses session-to-date population statistics after a 10-pair baseline, so values are provisional and are not a validated measure of emotion or clinical arousal.",
        "legacy experimental formula",
        EXCITEOMETER_SOURCE,
        EXCITEOMETER_SOURCE_URL,
        "excite o meter excitement score arousal legacy percentile cdf rr rmssd",
        false,
        false,
        1,
        0.0,
        "Experimental"
    ),
    metric!(
        "excitometer",
        "excitometer",
        "Activation composite (experimental)",
        "Within-session HR ↑ plus lnRMSSD ↓",
        "0–1",
        "Excitation (experimental)",
        "The excitometer is an explicitly experimental within-session composite: 65% standardized heart-rate elevation plus 35% standardized lnRMSSD reduction, mapped through a logistic curve. HRV can covary with stress, but no single H10-only score uniquely identifies sympathetic arousal, excitement or emotion.",
        "unvalidated composite",
        STRESS_REVIEW,
        STRESS_URL,
        "excitation activation arousal stress experimental",
        false,
        true,
        1,
        0.0,
        "Experimental"
    ),
];

pub fn metric_definition(id: &str) -> Option<MetricDefinition> {
    METRIC_CATALOG
        .iter()
        .copied()
        .find(|metric| metric.id == id)
}
