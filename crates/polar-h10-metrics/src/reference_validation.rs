use std::{error::Error, fmt};

use serde::Serialize;

const MAX_INPUT_SAMPLES: usize = 200_000;
const MIN_RESPIRATORY_FREQUENCY_HZ: f64 = 0.05;
const MAX_RESPIRATORY_FREQUENCY_HZ: f64 = 0.70;
const FREQUENCY_STEP_HZ: f64 = 0.005;

#[derive(Clone, Copy, Debug)]
pub struct TimedRespirationSample {
    pub host_time_seconds: f64,
    pub waveform_01: f32,
    pub signed_projection_g: f32,
    pub ready: bool,
    pub confidence_01: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct TimedReferenceSample {
    pub host_time_seconds: f64,
    pub force_n: f32,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RespirationReferenceSettings {
    pub resample_rate_hz: f64,
    pub maximum_lag_seconds: f64,
    pub baseline_time_constant_seconds: f64,
    pub maximum_interpolation_gap_seconds: f64,
    pub minimum_overlap_seconds: f64,
    pub minimum_usable_seconds: f64,
    pub minimum_ready_fraction: f64,
    pub minimum_reference_robust_span_n: f64,
    pub window_seconds: f64,
    pub window_step_seconds: f64,
}

impl Default for RespirationReferenceSettings {
    fn default() -> Self {
        Self {
            resample_rate_hz: 10.0,
            maximum_lag_seconds: 3.0,
            baseline_time_constant_seconds: 10.0,
            maximum_interpolation_gap_seconds: 0.5,
            minimum_overlap_seconds: 120.0,
            minimum_usable_seconds: 60.0,
            minimum_ready_fraction: 0.75,
            minimum_reference_robust_span_n: 0.01,
            window_seconds: 30.0,
            window_step_seconds: 15.0,
        }
    }
}

impl RespirationReferenceSettings {
    fn validate(self) -> Result<Self, AgreementError> {
        let values = [
            self.resample_rate_hz,
            self.maximum_lag_seconds,
            self.baseline_time_constant_seconds,
            self.maximum_interpolation_gap_seconds,
            self.minimum_overlap_seconds,
            self.minimum_usable_seconds,
            self.minimum_ready_fraction,
            self.minimum_reference_robust_span_n,
            self.window_seconds,
            self.window_step_seconds,
        ];
        if values.iter().any(|value| !value.is_finite())
            || !(1.0..=100.0).contains(&self.resample_rate_hz)
            || !(0.0..=10.0).contains(&self.maximum_lag_seconds)
            || !(1.0..=120.0).contains(&self.baseline_time_constant_seconds)
            || !(0.05..=5.0).contains(&self.maximum_interpolation_gap_seconds)
            || !(10.0..=3_600.0).contains(&self.minimum_overlap_seconds)
            || !(10.0..=self.minimum_overlap_seconds).contains(&self.minimum_usable_seconds)
            || !(0.0..=1.0).contains(&self.minimum_ready_fraction)
            || !(0.0..=100.0).contains(&self.minimum_reference_robust_span_n)
            || !(10.0..=300.0).contains(&self.window_seconds)
            || !(1.0..=self.window_seconds).contains(&self.window_step_seconds)
        {
            return Err(AgreementError::InvalidSettings);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlignmentPolarity {
    Same,
    Inverted,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowAgreement {
    pub window_count: usize,
    pub median_polarity_adjusted_correlation: Option<f64>,
    pub p10_polarity_adjusted_correlation: Option<f64>,
    pub minimum_polarity_adjusted_correlation: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalAgreement {
    pub zero_lag_correlation: f64,
    pub best_lag_seconds: f64,
    pub best_lag_convention: &'static str,
    pub best_signed_correlation: f64,
    pub polarity: AlignmentPolarity,
    pub polarity_adjusted_correlation: f64,
    pub normalized_rmse_after_polarity: f64,
    pub paired_samples: usize,
    pub h10_dominant_rate_bpm: Option<f64>,
    pub reference_dominant_rate_bpm: Option<f64>,
    pub dominant_rate_absolute_error_bpm: Option<f64>,
    pub windows: WindowAgreement,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RespirationReferenceReport {
    pub schema: &'static str,
    pub analysis_scope: &'static str,
    pub settings: RespirationReferenceSettings,
    pub synchronized_overlap_seconds: f64,
    pub resampled_grid_points: usize,
    pub usable_grid_points: usize,
    pub usable_seconds: f64,
    pub ready_fraction: f64,
    pub confidence_mean: f64,
    pub confidence_p10: f64,
    pub reference_force_robust_span_n: f64,
    pub h10_signed_projection_robust_span_g: f64,
    pub h10_normalized_waveform_robust_span: f64,
    pub recording_quality_passed: bool,
    pub recording_quality_failures: Vec<String>,
    pub signed_projection: SignalAgreement,
    pub normalized_waveform: SignalAgreement,
    pub recommended_invert_direction_for_this_mounting: bool,
    pub physiological_acceptance_established: bool,
    pub interpretation: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgreementError {
    InvalidSettings,
    TooFewRespirationSamples,
    TooFewReferenceSamples,
    TooManySamples,
    InvalidRespirationSample,
    InvalidReferenceSample,
    NonMonotonicRespirationTime,
    NonMonotonicReferenceTime,
    InsufficientOverlap,
    InsufficientPairedSamples,
}

impl fmt::Display for AgreementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidSettings => "respiration reference settings are outside their bounds",
            Self::TooFewRespirationSamples => {
                "fewer than two H10 respiration samples were supplied"
            }
            Self::TooFewReferenceSamples => "fewer than two reference force samples were supplied",
            Self::TooManySamples => "respiration reference input exceeded its bounded sample count",
            Self::InvalidRespirationSample => {
                "an H10 respiration sample was non-finite or out of range"
            }
            Self::InvalidReferenceSample => "a reference force sample was non-finite",
            Self::NonMonotonicRespirationTime => {
                "H10 respiration host times were not strictly increasing"
            }
            Self::NonMonotonicReferenceTime => {
                "reference force host times were not strictly increasing"
            }
            Self::InsufficientOverlap => {
                "H10 and reference signals did not have enough synchronized overlap"
            }
            Self::InsufficientPairedSamples => {
                "too little ready, gap-free paired signal remained for comparison"
            }
        };
        formatter.write_str(message)
    }
}

impl Error for AgreementError {}

#[derive(Clone, Copy)]
struct GridSample {
    waveform: f64,
    signed: f64,
    ready: bool,
    confidence: f64,
    force: f64,
}

#[derive(Clone, Copy)]
struct FilteredGridSample {
    waveform: f64,
    signed: f64,
    ready: bool,
    confidence: f64,
    force: f64,
}

#[derive(Default)]
struct CausalBaseline {
    value: f64,
    initialized: bool,
}

impl CausalBaseline {
    fn high_pass(&mut self, value: f64, alpha: f64) -> f64 {
        if !self.initialized {
            self.value = value;
            self.initialized = true;
            return 0.0;
        }
        self.value += (value - self.value) * alpha;
        value - self.value
    }
}

pub fn analyze_respiration_reference(
    respiration: &[TimedRespirationSample],
    reference: &[TimedReferenceSample],
    settings: RespirationReferenceSettings,
) -> Result<RespirationReferenceReport, AgreementError> {
    let settings = settings.validate()?;
    validate_respiration(respiration)?;
    validate_reference(reference)?;

    let overlap_start = respiration[0]
        .host_time_seconds
        .max(reference[0].host_time_seconds);
    let overlap_end = respiration[respiration.len() - 1]
        .host_time_seconds
        .min(reference[reference.len() - 1].host_time_seconds);
    let overlap_seconds = overlap_end - overlap_start;
    if overlap_seconds < settings.minimum_overlap_seconds {
        return Err(AgreementError::InsufficientOverlap);
    }

    let step = 1.0 / settings.resample_rate_hz;
    let grid_count = (overlap_seconds / step).floor() as usize + 1;
    if grid_count > MAX_INPUT_SAMPLES {
        return Err(AgreementError::TooManySamples);
    }
    let mut respiration_cursor = 0;
    let mut reference_cursor = 0;
    let mut grid = Vec::with_capacity(grid_count);
    for index in 0..grid_count {
        let time = overlap_start + index as f64 * step;
        let h10 = interpolate_respiration(
            respiration,
            &mut respiration_cursor,
            time,
            settings.maximum_interpolation_gap_seconds,
        );
        let belt = interpolate_reference(
            reference,
            &mut reference_cursor,
            time,
            settings.maximum_interpolation_gap_seconds,
        );
        grid.push(h10.zip(belt).map(|(h10, force)| GridSample {
            waveform: h10.0,
            signed: h10.1,
            ready: h10.2,
            confidence: h10.3,
            force,
        }));
    }

    let alpha = 1.0 - (-step / settings.baseline_time_constant_seconds).exp();
    let mut waveform_baseline = CausalBaseline::default();
    let mut signed_baseline = CausalBaseline::default();
    let mut force_baseline = CausalBaseline::default();
    let filtered = grid
        .iter()
        .map(|sample| {
            sample.map(|sample| FilteredGridSample {
                waveform: waveform_baseline.high_pass(sample.waveform, alpha),
                signed: signed_baseline.high_pass(sample.signed, alpha),
                ready: sample.ready,
                confidence: sample.confidence,
                force: force_baseline.high_pass(sample.force, alpha),
            })
        })
        .collect::<Vec<_>>();

    let available = filtered.iter().flatten().count();
    let usable = filtered
        .iter()
        .flatten()
        .filter(|sample| sample.ready)
        .count();
    if available == 0
        || usable < (settings.minimum_usable_seconds * settings.resample_rate_hz) as usize
    {
        return Err(AgreementError::InsufficientPairedSamples);
    }
    let ready_fraction = usable as f64 / available as f64;
    let usable_seconds = usable as f64 / settings.resample_rate_hz;

    let confidence = filtered
        .iter()
        .flatten()
        .filter(|sample| sample.ready)
        .map(|sample| sample.confidence)
        .collect::<Vec<_>>();
    let reference_values = grid
        .iter()
        .flatten()
        .map(|sample| sample.force)
        .collect::<Vec<_>>();
    let signed_values = grid
        .iter()
        .flatten()
        .filter(|sample| sample.ready)
        .map(|sample| sample.signed)
        .collect::<Vec<_>>();
    let waveform_values = grid
        .iter()
        .flatten()
        .filter(|sample| sample.ready)
        .map(|sample| sample.waveform)
        .collect::<Vec<_>>();

    let force_span = robust_span(&reference_values);
    let signed_span = robust_span(&signed_values);
    let waveform_span = robust_span(&waveform_values);
    let mut quality_failures = Vec::new();
    if ready_fraction < settings.minimum_ready_fraction {
        quality_failures.push(format!(
            "ready fraction {ready_fraction:.3} was below {:.3}",
            settings.minimum_ready_fraction
        ));
    }
    if force_span < settings.minimum_reference_robust_span_n {
        quality_failures.push(format!(
            "reference force robust span {force_span:.6} N was below {:.6} N",
            settings.minimum_reference_robust_span_n
        ));
    }
    if usable_seconds < settings.minimum_usable_seconds {
        quality_failures.push(format!(
            "usable synchronized signal {usable_seconds:.3} s was below {:.3} s",
            settings.minimum_usable_seconds
        ));
    }

    let signed_projection = signal_agreement(&filtered, false, settings)?;
    let normalized_waveform = signal_agreement(&filtered, true, settings)?;
    let confidence_mean = confidence.iter().sum::<f64>() / confidence.len() as f64;

    Ok(RespirationReferenceReport {
        schema: "polar.stream.respiration_reference_analysis.v1",
        analysis_scope: "descriptive synchronized host-time comparison",
        settings,
        synchronized_overlap_seconds: overlap_seconds,
        resampled_grid_points: grid_count,
        usable_grid_points: usable,
        usable_seconds,
        ready_fraction,
        confidence_mean,
        confidence_p10: quantile(&confidence, 0.10),
        reference_force_robust_span_n: force_span,
        h10_signed_projection_robust_span_g: signed_span,
        h10_normalized_waveform_robust_span: waveform_span,
        recording_quality_passed: quality_failures.is_empty(),
        recording_quality_failures: quality_failures,
        recommended_invert_direction_for_this_mounting: matches!(
            signed_projection.polarity,
            AlignmentPolarity::Inverted
        ),
        signed_projection,
        normalized_waveform,
        physiological_acceptance_established: false,
        interpretation: "Agreement describes this recording only. It does not establish lung volume, airflow, clinical validity, or a production acceptance threshold.",
    })
}

fn validate_respiration(samples: &[TimedRespirationSample]) -> Result<(), AgreementError> {
    if samples.len() < 2 {
        return Err(AgreementError::TooFewRespirationSamples);
    }
    if samples.len() > MAX_INPUT_SAMPLES {
        return Err(AgreementError::TooManySamples);
    }
    let mut previous = f64::NEG_INFINITY;
    for sample in samples {
        if !sample.host_time_seconds.is_finite()
            || !sample.waveform_01.is_finite()
            || !(0.0..=1.0).contains(&sample.waveform_01)
            || !sample.signed_projection_g.is_finite()
            || !sample.confidence_01.is_finite()
            || !(0.0..=1.0).contains(&sample.confidence_01)
        {
            return Err(AgreementError::InvalidRespirationSample);
        }
        if sample.host_time_seconds <= previous {
            return Err(AgreementError::NonMonotonicRespirationTime);
        }
        previous = sample.host_time_seconds;
    }
    Ok(())
}

fn validate_reference(samples: &[TimedReferenceSample]) -> Result<(), AgreementError> {
    if samples.len() < 2 {
        return Err(AgreementError::TooFewReferenceSamples);
    }
    if samples.len() > MAX_INPUT_SAMPLES {
        return Err(AgreementError::TooManySamples);
    }
    let mut previous = f64::NEG_INFINITY;
    for sample in samples {
        if !sample.host_time_seconds.is_finite() || !sample.force_n.is_finite() {
            return Err(AgreementError::InvalidReferenceSample);
        }
        if sample.host_time_seconds <= previous {
            return Err(AgreementError::NonMonotonicReferenceTime);
        }
        previous = sample.host_time_seconds;
    }
    Ok(())
}

fn interpolate_respiration(
    samples: &[TimedRespirationSample],
    cursor: &mut usize,
    time: f64,
    maximum_gap: f64,
) -> Option<(f64, f64, bool, f64)> {
    while *cursor + 1 < samples.len() && samples[*cursor + 1].host_time_seconds < time {
        *cursor += 1;
    }
    let left = samples.get(*cursor)?;
    let right = samples.get((*cursor + 1).min(samples.len() - 1))?;
    if time < left.host_time_seconds || time > right.host_time_seconds {
        return None;
    }
    let gap = right.host_time_seconds - left.host_time_seconds;
    if gap > maximum_gap {
        return None;
    }
    let fraction = if gap <= f64::EPSILON {
        0.0
    } else {
        (time - left.host_time_seconds) / gap
    };
    Some((
        interpolate(
            f64::from(left.waveform_01),
            f64::from(right.waveform_01),
            fraction,
        ),
        interpolate(
            f64::from(left.signed_projection_g),
            f64::from(right.signed_projection_g),
            fraction,
        ),
        left.ready && right.ready,
        f64::from(left.confidence_01.min(right.confidence_01)),
    ))
}

fn interpolate_reference(
    samples: &[TimedReferenceSample],
    cursor: &mut usize,
    time: f64,
    maximum_gap: f64,
) -> Option<f64> {
    while *cursor + 1 < samples.len() && samples[*cursor + 1].host_time_seconds < time {
        *cursor += 1;
    }
    let left = samples.get(*cursor)?;
    let right = samples.get((*cursor + 1).min(samples.len() - 1))?;
    if time < left.host_time_seconds || time > right.host_time_seconds {
        return None;
    }
    let gap = right.host_time_seconds - left.host_time_seconds;
    if gap > maximum_gap {
        return None;
    }
    let fraction = if gap <= f64::EPSILON {
        0.0
    } else {
        (time - left.host_time_seconds) / gap
    };
    Some(interpolate(
        f64::from(left.force_n),
        f64::from(right.force_n),
        fraction,
    ))
}

fn interpolate(left: f64, right: f64, fraction: f64) -> f64 {
    left + (right - left) * fraction.clamp(0.0, 1.0)
}

fn signal_agreement(
    grid: &[Option<FilteredGridSample>],
    waveform: bool,
    settings: RespirationReferenceSettings,
) -> Result<SignalAgreement, AgreementError> {
    let maximum_lag = (settings.maximum_lag_seconds * settings.resample_rate_hz).round() as isize;
    let minimum_pairs =
        (settings.minimum_usable_seconds * settings.resample_rate_hz).round() as usize;
    let zero_lag = correlation_for_lag(grid, waveform, 0, minimum_pairs)
        .ok_or(AgreementError::InsufficientPairedSamples)?;
    // PCA sign is arbitrary, but a periodic signal also has strong
    // anti-correlation near half a breath cycle. Anchor orientation at zero
    // lag, where transport/filter latency is expected to be much shorter than
    // a half-cycle, then search lag only within that fixed orientation. This
    // prevents the optimizer from "explaining" latency by flipping polarity at
    // a remote half-cycle peak.
    let orientation = if zero_lag.0 < 0.0 { -1.0 } else { 1.0 };
    let mut best_lag = 0_isize;
    let mut best_correlation = zero_lag.0;
    let mut best_pairs = zero_lag.1;
    for lag in -maximum_lag..=maximum_lag {
        let Some((correlation, pairs)) = correlation_for_lag(grid, waveform, lag, minimum_pairs)
        else {
            continue;
        };
        let ordering = (correlation * orientation).total_cmp(&(best_correlation * orientation));
        if ordering.is_gt()
            || (ordering.is_eq() && lag.abs() < best_lag.abs())
            || (ordering.is_eq() && lag.abs() == best_lag.abs() && lag < best_lag)
        {
            best_lag = lag;
            best_correlation = correlation;
            best_pairs = pairs;
        }
    }
    let polarity = if orientation < 0.0 {
        AlignmentPolarity::Inverted
    } else {
        AlignmentPolarity::Same
    };
    let pairs = collect_pairs(grid, waveform, best_lag);
    let normalized_rmse = normalized_rmse(&pairs, orientation);
    let (h10_rate, reference_rate) =
        dominant_rates_for_longest_segment(grid, waveform, best_lag, settings.resample_rate_hz);
    let windows = window_agreement(grid, waveform, best_lag, orientation, settings);

    Ok(SignalAgreement {
        zero_lag_correlation: zero_lag.0,
        best_lag_seconds: best_lag as f64 / settings.resample_rate_hz,
        best_lag_convention: "positive means the H10 signal follows the earlier GDX-RB force signal",
        best_signed_correlation: best_correlation,
        polarity,
        polarity_adjusted_correlation: best_correlation * orientation,
        normalized_rmse_after_polarity: normalized_rmse,
        paired_samples: best_pairs,
        h10_dominant_rate_bpm: h10_rate,
        reference_dominant_rate_bpm: reference_rate,
        dominant_rate_absolute_error_bpm: h10_rate
            .zip(reference_rate)
            .map(|(left, right)| (left - right).abs()),
        windows,
    })
}

fn correlation_for_lag(
    grid: &[Option<FilteredGridSample>],
    waveform: bool,
    lag: isize,
    minimum_pairs: usize,
) -> Option<(f64, usize)> {
    let pairs = collect_pairs(grid, waveform, lag);
    if pairs.len() < minimum_pairs {
        return None;
    }
    pearson(&pairs).map(|correlation| (correlation, pairs.len()))
}

fn collect_pairs(
    grid: &[Option<FilteredGridSample>],
    waveform: bool,
    lag: isize,
) -> Vec<(f64, f64)> {
    let mut pairs = Vec::with_capacity(grid.len());
    for (h10_index, h10) in grid.iter().enumerate() {
        let Some(reference_index) = h10_index.checked_add_signed(-lag) else {
            continue;
        };
        let Some(h10) = h10.filter(|sample| sample.ready) else {
            continue;
        };
        let Some(reference) = grid.get(reference_index).and_then(Option::as_ref) else {
            continue;
        };
        pairs.push((
            if waveform { h10.waveform } else { h10.signed },
            reference.force,
        ));
    }
    pairs
}

fn pearson(pairs: &[(f64, f64)]) -> Option<f64> {
    if pairs.len() < 3 {
        return None;
    }
    let count = pairs.len() as f64;
    let left_mean = pairs.iter().map(|pair| pair.0).sum::<f64>() / count;
    let right_mean = pairs.iter().map(|pair| pair.1).sum::<f64>() / count;
    let mut covariance = 0.0;
    let mut left_power = 0.0;
    let mut right_power = 0.0;
    for &(left, right) in pairs {
        let left = left - left_mean;
        let right = right - right_mean;
        covariance += left * right;
        left_power += left * left;
        right_power += right * right;
    }
    let denominator = (left_power * right_power).sqrt();
    (denominator > f64::EPSILON).then(|| (covariance / denominator).clamp(-1.0, 1.0))
}

fn normalized_rmse(pairs: &[(f64, f64)], orientation: f64) -> f64 {
    let count = pairs.len() as f64;
    let left_mean = pairs.iter().map(|pair| pair.0).sum::<f64>() / count;
    let right_mean = pairs.iter().map(|pair| pair.1).sum::<f64>() / count;
    let left_sd = (pairs
        .iter()
        .map(|pair| (pair.0 - left_mean).powi(2))
        .sum::<f64>()
        / count)
        .sqrt();
    let right_sd = (pairs
        .iter()
        .map(|pair| (pair.1 - right_mean).powi(2))
        .sum::<f64>()
        / count)
        .sqrt();
    if left_sd <= f64::EPSILON || right_sd <= f64::EPSILON {
        return f64::INFINITY;
    }
    (pairs
        .iter()
        .map(|pair| {
            let left = (pair.0 - left_mean) / left_sd;
            let right = (pair.1 - right_mean) / right_sd * orientation;
            (left - right).powi(2)
        })
        .sum::<f64>()
        / count)
        .sqrt()
}

fn dominant_rates_for_longest_segment(
    grid: &[Option<FilteredGridSample>],
    waveform: bool,
    lag: isize,
    rate_hz: f64,
) -> (Option<f64>, Option<f64>) {
    let mut best_h10 = Vec::new();
    let mut best_reference = Vec::new();
    let mut h10 = Vec::new();
    let mut reference = Vec::new();
    for h10_index in 0..grid.len() {
        let paired = h10_index
            .checked_add_signed(-lag)
            .and_then(|reference_index| {
                let h10 = grid
                    .get(h10_index)?
                    .as_ref()
                    .filter(|sample| sample.ready)?;
                let reference = grid.get(reference_index)?.as_ref()?;
                Some((
                    if waveform { h10.waveform } else { h10.signed },
                    reference.force,
                ))
            });
        if let Some((left, right)) = paired {
            h10.push(left);
            reference.push(right);
        } else if h10.len() > best_h10.len() {
            best_h10 = std::mem::take(&mut h10);
            best_reference = std::mem::take(&mut reference);
        } else {
            h10.clear();
            reference.clear();
        }
    }
    if h10.len() > best_h10.len() {
        best_h10 = h10;
        best_reference = reference;
    }
    (
        dominant_rate_bpm(&best_h10, rate_hz),
        dominant_rate_bpm(&best_reference, rate_hz),
    )
}

fn dominant_rate_bpm(values: &[f64], rate_hz: f64) -> Option<f64> {
    if values.len() < (20.0 * rate_hz) as usize {
        return None;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let mut best_frequency = None;
    let mut best_power = 0.0;
    let mut frequency = MIN_RESPIRATORY_FREQUENCY_HZ;
    while frequency <= MAX_RESPIRATORY_FREQUENCY_HZ + f64::EPSILON {
        let mut real = 0.0;
        let mut imaginary = 0.0;
        for (index, value) in values.iter().enumerate() {
            let window = if values.len() <= 1 {
                1.0
            } else {
                0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / (values.len() - 1) as f64).cos()
            };
            let phase = std::f64::consts::TAU * frequency * index as f64 / rate_hz;
            let centered = (value - mean) * window;
            real += centered * phase.cos();
            imaginary -= centered * phase.sin();
        }
        let power = real * real + imaginary * imaginary;
        if power > best_power {
            best_power = power;
            best_frequency = Some(frequency);
        }
        frequency += FREQUENCY_STEP_HZ;
    }
    (best_power > f64::EPSILON).then(|| best_frequency.unwrap_or_default() * 60.0)
}

fn window_agreement(
    grid: &[Option<FilteredGridSample>],
    waveform: bool,
    lag: isize,
    orientation: f64,
    settings: RespirationReferenceSettings,
) -> WindowAgreement {
    let window = (settings.window_seconds * settings.resample_rate_hz).round() as usize;
    let step = (settings.window_step_seconds * settings.resample_rate_hz).round() as usize;
    let required = (window as f64 * settings.minimum_ready_fraction).ceil() as usize;
    let mut correlations = Vec::new();
    let mut start = 0;
    while start + window <= grid.len() {
        let mut pairs = Vec::with_capacity(window);
        for h10_index in start..start + window {
            let Some(reference_index) = h10_index.checked_add_signed(-lag) else {
                continue;
            };
            let Some(h10) = grid
                .get(h10_index)
                .and_then(Option::as_ref)
                .filter(|sample| sample.ready)
            else {
                continue;
            };
            let Some(reference) = grid.get(reference_index).and_then(Option::as_ref) else {
                continue;
            };
            pairs.push((
                if waveform { h10.waveform } else { h10.signed },
                reference.force,
            ));
        }
        if pairs.len() >= required
            && let Some(correlation) = pearson(&pairs)
        {
            correlations.push(correlation * orientation);
        }
        start = start.saturating_add(step.max(1));
    }
    correlations.sort_by(f64::total_cmp);
    WindowAgreement {
        window_count: correlations.len(),
        median_polarity_adjusted_correlation: (!correlations.is_empty())
            .then(|| quantile_sorted(&correlations, 0.50)),
        p10_polarity_adjusted_correlation: (!correlations.is_empty())
            .then(|| quantile_sorted(&correlations, 0.10)),
        minimum_polarity_adjusted_correlation: correlations.first().copied(),
    }
}

fn robust_span(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    quantile(values, 0.95) - quantile(values, 0.05)
}

fn quantile(values: &[f64], fraction: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    quantile_sorted(&sorted, fraction)
}

fn quantile_sorted(values: &[f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let position = (values.len() - 1) as f64 * fraction.clamp(0.0, 1.0);
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    interpolate(values[lower], values[upper], position - lower as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use polar_h10_core::AccSample;

    fn synthetic(
        delay_seconds: f64,
        inverted: bool,
    ) -> (Vec<TimedRespirationSample>, Vec<TimedReferenceSample>) {
        let mut h10 = Vec::new();
        let mut reference = Vec::new();
        for index in 0..=3_600 {
            let time = index as f64 / 20.0;
            let respiratory = (std::f64::consts::TAU * 0.2 * (time - delay_seconds)).sin();
            let oriented = respiratory * if inverted { -1.0 } else { 1.0 };
            h10.push(TimedRespirationSample {
                host_time_seconds: time,
                waveform_01: (0.5 + 0.4 * oriented) as f32,
                signed_projection_g: (0.02 * oriented) as f32,
                ready: time >= 12.0,
                confidence_01: if time >= 12.0 { 0.9 } else { 0.0 },
            });
        }
        for index in 0..=1_800 {
            let time = index as f64 / 10.0;
            let respiratory = (std::f64::consts::TAU * 0.2 * time).sin();
            reference.push(TimedReferenceSample {
                host_time_seconds: time,
                force_n: (10.0 + 2.0 * respiratory) as f32,
            });
        }
        (h10, reference)
    }

    #[test]
    fn recovers_lag_polarity_rate_and_window_agreement() {
        let (h10, reference) = synthetic(0.7, false);
        let report = analyze_respiration_reference(
            &h10,
            &reference,
            RespirationReferenceSettings::default(),
        )
        .unwrap();
        assert!(report.recording_quality_passed);
        assert!((report.signed_projection.best_lag_seconds - 0.7).abs() <= 0.1);
        assert!(matches!(
            report.signed_projection.polarity,
            AlignmentPolarity::Same
        ));
        assert!(report.signed_projection.polarity_adjusted_correlation > 0.99);
        assert!(report.normalized_waveform.polarity_adjusted_correlation > 0.99);
        assert!((report.signed_projection.h10_dominant_rate_bpm.unwrap() - 12.0).abs() <= 0.31);
        assert!(report.signed_projection.windows.window_count >= 5);
        assert!(!report.physiological_acceptance_established);
    }

    #[test]
    fn reports_mounting_inversion_without_silently_changing_samples() {
        let (h10, reference) = synthetic(0.5, true);
        let report = analyze_respiration_reference(
            &h10,
            &reference,
            RespirationReferenceSettings::default(),
        )
        .unwrap();
        assert!(matches!(
            report.signed_projection.polarity,
            AlignmentPolarity::Inverted
        ));
        assert!(report.recommended_invert_direction_for_this_mounting);
        assert!(report.signed_projection.best_signed_correlation < -0.99);
        assert!(report.signed_projection.polarity_adjusted_correlation > 0.99);
    }

    #[test]
    fn rejects_nonmonotonic_and_short_inputs() {
        let (mut h10, reference) = synthetic(0.0, false);
        h10[100].host_time_seconds = h10[99].host_time_seconds;
        assert!(matches!(
            analyze_respiration_reference(
                &h10,
                &reference,
                RespirationReferenceSettings::default(),
            ),
            Err(AgreementError::NonMonotonicRespirationTime)
        ));

        let settings = RespirationReferenceSettings {
            minimum_overlap_seconds: 120.0,
            ..RespirationReferenceSettings::default()
        };
        assert!(matches!(
            analyze_respiration_reference(&h10[..100], &reference[..50], settings),
            Err(AgreementError::InsufficientOverlap)
        ));
    }

    #[test]
    fn quality_gate_is_separate_from_descriptive_correlation() {
        let (mut h10, mut reference) = synthetic(0.0, false);
        for sample in &mut h10 {
            sample.ready = sample.host_time_seconds >= 100.0;
            sample.confidence_01 = if sample.ready { 0.8 } else { 0.0 };
        }
        for sample in &mut reference {
            sample.force_n = 10.0 + (sample.force_n - 10.0) * 0.001;
        }
        let settings = RespirationReferenceSettings {
            minimum_usable_seconds: 40.0,
            ..RespirationReferenceSettings::default()
        };
        let report = analyze_respiration_reference(&h10, &reference, settings).unwrap();
        assert!(!report.recording_quality_passed);
        assert!(!report.recording_quality_failures.is_empty());
        assert!(report.signed_projection.polarity_adjusted_correlation > 0.99);
    }

    #[test]
    fn current_breathing_processor_is_comparable_without_a_second_implementation() {
        let mut processor = crate::BreathingProcessor::default();
        let mut h10 = Vec::new();
        for batch in 0..3_000 {
            let samples = (0..10)
                .map(|within| {
                    let time = (batch * 10 + within) as f64 / 200.0;
                    let respiratory = (std::f64::consts::TAU * 0.2 * time).sin();
                    AccSample {
                        x_mg: (450.0 + 25.0 * respiratory) as i16,
                        y_mg: 30,
                        z_mg: (820.0 + 15.0 * respiratory) as i16,
                    }
                })
                .collect::<Vec<_>>();
            let Some(snapshot) = processor.push(&samples) else {
                continue;
            };
            if snapshot.calibrated {
                h10.push(TimedRespirationSample {
                    host_time_seconds: (batch * 10 + 9) as f64 / 200.0,
                    waveform_01: snapshot.volume_01,
                    signed_projection_g: snapshot.magnitude_g,
                    ready: snapshot.ready,
                    confidence_01: snapshot.confidence_01,
                });
            }
        }
        let reference = (0..1_500)
            .map(|index| {
                let time = index as f64 / 10.0;
                TimedReferenceSample {
                    host_time_seconds: time,
                    force_n: (8.0 + 1.5 * (std::f64::consts::TAU * 0.2 * time).sin()) as f32,
                }
            })
            .collect::<Vec<_>>();
        let report = analyze_respiration_reference(
            &h10,
            &reference,
            RespirationReferenceSettings::default(),
        )
        .unwrap();
        assert!(report.recording_quality_passed);
        assert!(report.signed_projection.polarity_adjusted_correlation > 0.90);
        assert!(report.normalized_waveform.polarity_adjusted_correlation > 0.85);
        assert!(
            report
                .signed_projection
                .dominant_rate_absolute_error_bpm
                .unwrap()
                < 0.4
        );
    }
}
