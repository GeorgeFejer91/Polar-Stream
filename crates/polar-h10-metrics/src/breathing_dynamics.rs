use std::f64::consts::PI;

use crate::{BreathingPhase, BreathingSnapshot, MetricSample};

const RETAINED_BREATHS: usize = 256;
const MINIMUM_BASIC: usize = 8;
const MINIMUM_ENTROPY: usize = 24;

#[derive(Clone, Copy, Debug, Default)]
pub struct FeatureSet {
    pub mean: f32,
    pub standard_deviation: f32,
    pub coefficient_of_variation: f32,
    pub autocorrelation_window_50: f32,
    pub psd_slope: f32,
    pub lempel_ziv_complexity: f32,
    pub sample_entropy: f32,
    pub multiscale_entropy: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct BreathingDynamicsSnapshot {
    pub interval: Option<FeatureSet>,
    pub amplitude: Option<FeatureSet>,
    pub confidence_01: f32,
}

impl BreathingDynamicsSnapshot {
    pub(crate) fn samples(self) -> Vec<MetricSample> {
        let mut values = vec![sample("breathing_dynamics_confidence", self.confidence_01)];
        if let Some(interval) = self.interval {
            values.extend(feature_samples("interval", interval));
            if interval.mean > 0.0 {
                values.push(sample("breathing_rate", 60.0 / interval.mean));
            }
        }
        if let Some(amplitude) = self.amplitude {
            values.extend(feature_samples("amplitude", amplitude));
        }
        values
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExtremumKind {
    Peak,
    Trough,
}

#[derive(Clone, Copy)]
struct Extremum {
    kind: ExtremumKind,
    time: f64,
    value: f32,
}

#[derive(Default)]
pub(crate) struct BreathingDynamicsProcessor {
    last: Option<(f64, f32)>,
    direction: i8,
    trend_extreme: Option<(f64, f32)>,
    accepted: Option<Extremum>,
    last_peak: Option<Extremum>,
    last_trough: Option<Extremum>,
    intervals: Vec<f32>,
    amplitudes: Vec<f32>,
}

impl BreathingDynamicsProcessor {
    pub(crate) fn push(&mut self, input: BreathingSnapshot) -> Option<BreathingDynamicsSnapshot> {
        if !input.calibrated || input.phase == BreathingPhase::BadSignal {
            return None;
        }
        let Some((last_time, last_volume)) = self.last else {
            self.last = Some((input.time_seconds, input.volume_01));
            return None;
        };
        self.last = Some((input.time_seconds, input.volume_01));
        let delta = input.volume_01 - last_volume;
        if delta.abs() < 0.0025 {
            return None;
        }
        let direction = if delta > 0.0 { 1 } else { -1 };
        if self.direction == 0 {
            self.direction = direction;
            self.trend_extreme = Some((last_time, last_volume));
            return None;
        }
        if direction == self.direction {
            let replace = self.trend_extreme.is_none_or(|(_, value)| {
                if direction > 0 {
                    input.volume_01 >= value
                } else {
                    input.volume_01 <= value
                }
            });
            if replace {
                self.trend_extreme = Some((input.time_seconds, input.volume_01));
            }
            return None;
        }

        let (time, value) = self.trend_extreme?;
        let candidate = Extremum {
            kind: if self.direction > 0 {
                ExtremumKind::Peak
            } else {
                ExtremumKind::Trough
            },
            time,
            value,
        };
        self.direction = direction;
        self.trend_extreme = Some((input.time_seconds, input.volume_01));
        if !self.accept(candidate) {
            return None;
        }
        Some(BreathingDynamicsSnapshot {
            interval: compute_features(&self.intervals),
            amplitude: compute_features(&self.amplitudes),
            confidence_01: (self.intervals.len().max(self.amplitudes.len()) as f32 / 200.0)
                .clamp(0.0, 1.0),
        })
    }

    fn accept(&mut self, candidate: Extremum) -> bool {
        let Some(previous) = self.accepted else {
            self.remember(candidate);
            return false;
        };
        if previous.kind == candidate.kind {
            let better = if candidate.kind == ExtremumKind::Peak {
                candidate.value > previous.value
            } else {
                candidate.value < previous.value
            };
            if better {
                self.remember(candidate);
            }
            return false;
        }
        if candidate.time - previous.time < 0.35 || (candidate.value - previous.value).abs() < 0.08
        {
            return false;
        }
        let same_kind = if candidate.kind == ExtremumKind::Peak {
            self.last_peak
        } else {
            self.last_trough
        };
        append_rolling(
            &mut self.amplitudes,
            (candidate.value - previous.value).abs(),
        );
        if let Some(previous_same) = same_kind {
            append_rolling(
                &mut self.intervals,
                (candidate.time - previous_same.time).max(0.0) as f32,
            );
        }
        self.remember(candidate);
        true
    }

    fn remember(&mut self, extremum: Extremum) {
        self.accepted = Some(extremum);
        if extremum.kind == ExtremumKind::Peak {
            self.last_peak = Some(extremum);
        } else {
            self.last_trough = Some(extremum);
        }
    }
}

fn append_rolling(values: &mut Vec<f32>, value: f32) {
    values.push(value);
    if values.len() > RETAINED_BREATHS {
        values.remove(0);
    }
}

fn compute_features(values: &[f32]) -> Option<FeatureSet> {
    if values.len() < MINIMUM_BASIC {
        return None;
    }
    let mean = mean(values);
    let standard_deviation = standard_deviation(values, mean);
    let mut output = FeatureSet {
        mean,
        standard_deviation,
        coefficient_of_variation: if mean.abs() > 1e-6 {
            standard_deviation / mean.abs()
        } else {
            0.0
        },
        autocorrelation_window_50: autocorrelation_window_50(values, mean),
        psd_slope: psd_slope(values),
        ..FeatureSet::default()
    };
    if values.len() >= MINIMUM_ENTROPY {
        output.lempel_ziv_complexity = lempel_ziv(values, mean);
        output.sample_entropy = sample_entropy(values, 2, 1, 0.2, None).unwrap_or(0.0);
        output.multiscale_entropy = multiscale_entropy_auc(values, 3, 1, 0.2, 5);
    }
    Some(output)
}

fn mean(values: &[f32]) -> f32 {
    values.iter().map(|value| f64::from(*value)).sum::<f64>() as f32 / values.len() as f32
}

fn standard_deviation(values: &[f32], mean: f32) -> f32 {
    if values.len() < 2 {
        return 0.0;
    }
    (values
        .iter()
        .map(|value| f64::from(*value - mean).powi(2))
        .sum::<f64>()
        / (values.len() - 1) as f64)
        .sqrt() as f32
}

fn autocorrelation_window_50(values: &[f32], mean: f32) -> f32 {
    let denominator = values
        .iter()
        .map(|value| f64::from(*value - mean).powi(2))
        .sum::<f64>();
    if denominator <= 0.0 {
        return 0.0;
    }
    for lag in 1..values.len() {
        let numerator = (0..values.len() - lag)
            .map(|index| f64::from(values[index] - mean) * f64::from(values[index + lag] - mean))
            .sum::<f64>();
        if numerator / denominator < 0.5 {
            return lag as f32;
        }
    }
    (values.len() - 1) as f32
}

fn psd_slope(values: &[f32]) -> f32 {
    if values.len() < 8 {
        return 0.0;
    }
    let detrended = detrend(values);
    let sd =
        (detrended.iter().map(|value| value * value).sum::<f64>() / values.len() as f64).sqrt();
    if sd < 1e-9 {
        return 0.0;
    }
    let normalized = detrended.iter().map(|value| value / sd).collect::<Vec<_>>();
    let cutoff = values.len() / 8;
    let mut x = Vec::new();
    let mut y = Vec::new();
    for bin in 1..=cutoff.max(1) {
        let (mut real, mut imaginary) = (0.0, 0.0);
        for (index, value) in normalized.iter().enumerate() {
            let angle = -2.0 * PI * bin as f64 * index as f64 / values.len() as f64;
            real += value * angle.cos();
            imaginary += value * angle.sin();
        }
        let power = real * real + imaginary * imaginary;
        if power > 0.0 {
            x.push((bin as f64).log10());
            y.push(power.log10());
        }
    }
    linear_slope(&x, &y) as f32
}

fn detrend(values: &[f32]) -> Vec<f64> {
    let n = values.len() as f64;
    let mean_x = (n - 1.0) / 2.0;
    let mean_y = values.iter().map(|value| f64::from(*value)).sum::<f64>() / n;
    let numerator = values
        .iter()
        .enumerate()
        .map(|(index, value)| (index as f64 - mean_x) * (f64::from(*value) - mean_y))
        .sum::<f64>();
    let denominator = (0..values.len())
        .map(|index| (index as f64 - mean_x).powi(2))
        .sum::<f64>();
    let slope = if denominator > 0.0 {
        numerator / denominator
    } else {
        0.0
    };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| f64::from(*value) - (mean_y + slope * (index as f64 - mean_x)))
        .collect()
}

fn linear_slope(x: &[f64], y: &[f64]) -> f64 {
    if x.len() < 3 || x.len() != y.len() {
        return 0.0;
    }
    let mean_x = x.iter().sum::<f64>() / x.len() as f64;
    let mean_y = y.iter().sum::<f64>() / y.len() as f64;
    let numerator = x
        .iter()
        .zip(y)
        .map(|(x, y)| (*x - mean_x) * (*y - mean_y))
        .sum::<f64>();
    let denominator = x.iter().map(|x| (*x - mean_x).powi(2)).sum::<f64>();
    if denominator > 0.0 {
        numerator / denominator
    } else {
        0.0
    }
}

fn lempel_ziv(values: &[f32], threshold: f32) -> f32 {
    let binary = values
        .iter()
        .map(|value| *value >= threshold)
        .collect::<Vec<_>>();
    let mut complexity = 1_usize;
    let mut index = 0;
    let mut length = 1;
    while index + length <= binary.len() {
        let found = (0..index).any(|start| {
            start + length <= index
                && binary[start..start + length] == binary[index..index + length]
        });
        if found && index + length < binary.len() {
            length += 1;
            continue;
        }
        if index + length < binary.len() {
            complexity += 1;
        }
        index += length;
        length = 1;
    }
    let normalization = binary.len() as f32 / (binary.len() as f32).log2().max(1.0);
    (complexity as f32 / normalization).clamp(0.0, 1.0)
}

fn sample_entropy(
    values: &[f32],
    dimension: usize,
    delay: usize,
    tolerance_factor: f32,
    fixed_tolerance: Option<f32>,
) -> Option<f32> {
    if values.len() < (dimension + 1) * delay + 1 {
        return None;
    }
    let sd = standard_deviation(values, mean(values));
    let tolerance = fixed_tolerance.unwrap_or(sd * tolerance_factor);
    if tolerance <= 0.0 {
        return None;
    }
    let b = match_count(values, dimension, delay, tolerance);
    let a = match_count(values, dimension + 1, delay, tolerance);
    if a == 0 || b == 0 {
        return None;
    }
    Some(-(a as f32 / b as f32).ln())
}

fn match_count(values: &[f32], dimension: usize, delay: usize, tolerance: f32) -> usize {
    let limit = values.len() - (dimension - 1) * delay;
    let mut count = 0;
    for left in 0..limit {
        for right in left + 1..limit {
            if (0..dimension).all(|offset| {
                (values[left + offset * delay] - values[right + offset * delay]).abs() <= tolerance
            }) {
                count += 1;
            }
        }
    }
    count
}

fn multiscale_entropy_auc(
    values: &[f32],
    dimension: usize,
    delay: usize,
    tolerance_factor: f32,
    max_scale: usize,
) -> f32 {
    let tolerance = standard_deviation(values, mean(values)) * tolerance_factor;
    let entropies = (1..=max_scale)
        .filter_map(|scale| {
            sample_entropy(
                &coarse_grain(values, scale),
                dimension,
                delay,
                tolerance_factor,
                Some(tolerance),
            )
            .map(|value| (scale, value))
        })
        .collect::<Vec<_>>();
    entropies
        .windows(2)
        .map(|pair| (pair[1].0 - pair[0].0) as f32 * (pair[0].1 + pair[1].1) * 0.5)
        .sum()
}

fn coarse_grain(values: &[f32], scale: usize) -> Vec<f32> {
    values.chunks_exact(scale).map(mean).collect()
}

fn feature_samples(kind: &str, features: FeatureSet) -> Vec<MetricSample> {
    let ids = if kind == "interval" {
        [
            "breath_interval_mean",
            "breath_interval_sd",
            "breath_interval_cv",
            "breath_interval_acw50",
            "breath_interval_psd_slope",
            "breath_interval_lzc",
            "breath_interval_sampen",
            "breath_interval_mse",
        ]
    } else {
        [
            "breath_amplitude_mean",
            "breath_amplitude_sd",
            "breath_amplitude_cv",
            "breath_amplitude_acw50",
            "breath_amplitude_psd_slope",
            "breath_amplitude_lzc",
            "breath_amplitude_sampen",
            "breath_amplitude_mse",
        ]
    };
    let values = [
        features.mean,
        features.standard_deviation,
        features.coefficient_of_variation,
        features.autocorrelation_window_50,
        features.psd_slope,
        features.lempel_ziv_complexity,
        features.sample_entropy,
        features.multiscale_entropy,
    ];
    ids.into_iter()
        .zip(values)
        .map(|(id, value)| sample(id, value))
        .collect()
}

fn sample(id: &'static str, value: f32) -> MetricSample {
    MetricSample { id, value }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_series_has_expected_basic_statistics() {
        let values = vec![4.0; 24];
        let features = compute_features(&values).unwrap();
        assert_eq!(features.mean, 4.0);
        assert_eq!(features.standard_deviation, 0.0);
        assert_eq!(features.coefficient_of_variation, 0.0);
    }

    #[test]
    fn accepted_cycles_produce_interval_and_amplitude_metrics() {
        let mut processor = BreathingDynamicsProcessor::default();
        let mut result = None;
        for index in 0..500 {
            let time = index as f64 * 0.1;
            let volume = ((time * PI * 0.5).sin() * 0.45 + 0.5) as f32;
            result = processor
                .push(BreathingSnapshot {
                    calibrated: true,
                    calibration_progress_01: 1.0,
                    volume_01: volume,
                    magnitude_g: volume - 0.5,
                    phase: crate::BreathingPhase::Pausing,
                    axis_range_g: 0.02,
                    time_seconds: time,
                })
                .or(result);
        }
        let result = result.unwrap();
        assert!(result.interval.is_some());
        assert!(result.amplitude.is_some());
    }
}
