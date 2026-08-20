use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use polar_h10_core::AccSample;
use serde::{Deserialize, Serialize};

use crate::MetricSample;

const SAMPLE_RATE_HZ: f64 = 200.0;
const PHASE_REFERENCE_BATCH_SECONDS: f32 = 0.05;

/// Saved controls shared by the experimental accelerometer breathing outputs.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreathingSettings {
    #[serde(default = "default_axes")]
    pub axes: [bool; 3],
    #[serde(default = "default_calibration_window_seconds")]
    pub calibration_window_seconds: f32,
    #[serde(default = "default_minimum_axis_range_g")]
    pub minimum_axis_range_g: f32,
    #[serde(default = "default_smoothing_window_seconds")]
    pub smoothing_window_seconds: f32,
    #[serde(default = "default_sensitivity")]
    pub sensitivity: f32,
    #[serde(default = "default_stale_timeout_seconds")]
    pub stale_timeout_seconds: f32,
    #[serde(default)]
    pub invert_direction: bool,
    #[serde(default = "enabled")]
    pub adaptive_bounds: bool,
    #[serde(default = "default_adaptive_window_seconds")]
    pub adaptive_window_seconds: f32,
    #[serde(default = "default_lower_quantile")]
    pub lower_quantile: f32,
    #[serde(default = "default_upper_quantile")]
    pub upper_quantile: f32,
}

impl Default for BreathingSettings {
    fn default() -> Self {
        Self {
            axes: default_axes(),
            calibration_window_seconds: default_calibration_window_seconds(),
            minimum_axis_range_g: default_minimum_axis_range_g(),
            smoothing_window_seconds: default_smoothing_window_seconds(),
            sensitivity: default_sensitivity(),
            stale_timeout_seconds: default_stale_timeout_seconds(),
            invert_direction: false,
            adaptive_bounds: true,
            adaptive_window_seconds: default_adaptive_window_seconds(),
            lower_quantile: default_lower_quantile(),
            upper_quantile: default_upper_quantile(),
        }
    }
}

impl BreathingSettings {
    pub fn clamped(mut self) -> Self {
        if self.axes.iter().filter(|enabled| **enabled).count() < 2 {
            self.axes = default_axes();
        }
        self.calibration_window_seconds =
            finite_or(self.calibration_window_seconds, 12.0).clamp(1.0, 60.0);
        self.minimum_axis_range_g = finite_or(self.minimum_axis_range_g, 0.01).clamp(0.001, 0.25);
        self.smoothing_window_seconds =
            finite_or(self.smoothing_window_seconds, 0.75).clamp(0.05, 5.0);
        self.sensitivity = finite_or(self.sensitivity, 0.60).clamp(0.0, 1.0);
        self.stale_timeout_seconds = finite_or(self.stale_timeout_seconds, 3.0).clamp(0.25, 30.0);
        self.adaptive_window_seconds =
            finite_or(self.adaptive_window_seconds, 20.0).clamp(5.0, 300.0);
        self.lower_quantile = finite_or(self.lower_quantile, 0.05).clamp(0.0, 0.40);
        self.upper_quantile = finite_or(self.upper_quantile, 0.95).clamp(0.60, 1.0);
        if self.upper_quantile - self.lower_quantile < 0.10 {
            self.lower_quantile = 0.05;
            self.upper_quantile = 0.95;
        }
        self
    }
}

const fn enabled() -> bool {
    true
}
const fn default_axes() -> [bool; 3] {
    [true, false, true]
}
const fn default_calibration_window_seconds() -> f32 {
    12.0
}
const fn default_minimum_axis_range_g() -> f32 {
    0.01
}
const fn default_smoothing_window_seconds() -> f32 {
    0.75
}
const fn default_sensitivity() -> f32 {
    0.60
}
const fn default_stale_timeout_seconds() -> f32 {
    3.0
}
const fn default_adaptive_window_seconds() -> f32 {
    20.0
}
const fn default_lower_quantile() -> f32 {
    0.05
}
const fn default_upper_quantile() -> f32 {
    0.95
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreathingPhase {
    Inhaling,
    Exhaling,
    Pausing,
    BadSignal,
}

impl BreathingPhase {
    /// Stable public transport values: +1 inhale, -1 exhale, and 0 pause/not ready.
    pub fn numeric(self) -> f32 {
        match self {
            Self::Inhaling => 1.0,
            Self::Exhaling => -1.0,
            Self::Pausing | Self::BadSignal => 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BreathingSnapshot {
    pub calibrated: bool,
    pub ready: bool,
    pub calibration_progress_01: f32,
    pub confidence_01: f32,
    pub volume_01: f32,
    pub magnitude_g: f32,
    pub phase: BreathingPhase,
    pub axis_range_g: f32,
    pub time_seconds: f64,
}

impl BreathingSnapshot {
    pub(crate) fn samples(self) -> Vec<MetricSample> {
        let mut values = vec![
            MetricSample {
                id: "breathing_calibration",
                value: self.calibration_progress_01,
            },
            MetricSample {
                id: "breathing_phase",
                value: self.phase.numeric(),
            },
            MetricSample {
                id: "breathing_signal_confidence",
                value: self.confidence_01,
            },
            MetricSample {
                id: "breathing_signal_ready",
                value: if self.ready { 1.0 } else { 0.0 },
            },
        ];
        if self.calibrated {
            values.extend([
                MetricSample {
                    id: "acc_breathing_magnitude",
                    value: self.magnitude_g,
                },
                MetricSample {
                    id: "breathing_volume",
                    value: self.volume_01,
                },
                MetricSample {
                    id: "breathing_axis_range",
                    value: self.axis_range_g,
                },
            ]);
        }
        values
    }
}

pub struct BreathingProcessor {
    settings: BreathingSettings,
    filtered: [f32; 3],
    has_filtered: bool,
    calibration: VecDeque<[f32; 3]>,
    last_calibration_attempt_seconds: f64,
    center: [f32; 3],
    baseline: [f32; 3],
    axis: [f32; 3],
    bound_min: f32,
    bound_max: f32,
    calibration_span: f32,
    calibrated: bool,
    projection_ema: f32,
    has_projection: bool,
    adaptive_projections: VecDeque<(f64, f32)>,
    last_adaptive_update_seconds: f64,
    last_emitted_volume: f32,
    has_emitted_volume: bool,
    elapsed_seconds: f64,
    last_push_at: Option<Instant>,
    motion_filtered: [f32; 3],
    has_motion_filtered: bool,
    motion_delta_ema_g: f32,
}

impl Default for BreathingProcessor {
    fn default() -> Self {
        Self::new(BreathingSettings::default())
    }
}

impl BreathingProcessor {
    pub fn new(settings: BreathingSettings) -> Self {
        Self {
            settings: settings.clamped(),
            filtered: [0.0; 3],
            has_filtered: false,
            calibration: VecDeque::new(),
            last_calibration_attempt_seconds: f64::NEG_INFINITY,
            center: [0.0; 3],
            baseline: [0.0; 3],
            axis: [0.0, 1.0, 0.0],
            bound_min: -0.02,
            bound_max: 0.02,
            calibration_span: 0.04,
            calibrated: false,
            projection_ema: 0.0,
            has_projection: false,
            adaptive_projections: VecDeque::new(),
            last_adaptive_update_seconds: 0.0,
            last_emitted_volume: 0.5,
            has_emitted_volume: false,
            elapsed_seconds: 0.0,
            last_push_at: None,
            motion_filtered: [0.0; 3],
            has_motion_filtered: false,
            motion_delta_ema_g: 0.0,
        }
    }

    pub fn apply_settings(&mut self, settings: BreathingSettings) {
        let settings = settings.clamped();
        if self.settings != settings {
            *self = Self::new(settings);
        }
    }

    fn calibration_target(&self) -> usize {
        (self.settings.calibration_window_seconds as f64 * SAMPLE_RATE_HZ).round() as usize
    }

    fn smoothing_alpha(&self) -> f32 {
        let sample_count = self.settings.smoothing_window_seconds * SAMPLE_RATE_HZ as f32;
        (2.0 / (sample_count + 1.0)).clamp(0.001, 1.0)
    }

    fn phase_velocity_threshold_per_second(&self) -> f32 {
        (0.0005 + (1.0 - self.settings.sensitivity).powi(2) * 0.015_625)
            / PHASE_REFERENCE_BATCH_SECONDS
    }

    pub fn push(&mut self, samples: &[AccSample]) -> Option<BreathingSnapshot> {
        if samples.is_empty() {
            return None;
        }

        let now = Instant::now();
        let stale = self.last_push_at.is_some_and(|last| {
            now.duration_since(last) > Duration::from_secs_f32(self.settings.stale_timeout_seconds)
        });
        self.last_push_at = Some(now);
        let mut latest_volume = self.last_emitted_volume;

        for sample in samples {
            let raw = [
                f32::from(sample.x_mg) / 1_000.0,
                f32::from(sample.y_mg) / 1_000.0,
                f32::from(sample.z_mg) / 1_000.0,
            ];
            if !self.has_motion_filtered {
                self.motion_filtered = raw;
                self.has_motion_filtered = true;
            } else {
                let previous = self.motion_filtered;
                let smoothing_alpha = self.smoothing_alpha();
                for (filtered, input) in self.motion_filtered.iter_mut().zip(raw) {
                    *filtered += (input - *filtered) * smoothing_alpha;
                }
                let delta = subtract(self.motion_filtered, previous);
                let magnitude = dot(delta, delta).sqrt();
                self.motion_delta_ema_g += (magnitude - self.motion_delta_ema_g) * 0.01;
            }
            let mut current = raw;
            for (index, enabled) in self.settings.axes.into_iter().enumerate() {
                if !enabled {
                    current[index] = 0.0;
                }
            }
            if !self.has_filtered {
                self.filtered = current;
                self.has_filtered = true;
            } else {
                let smoothing_alpha = self.smoothing_alpha();
                for (filtered, input) in self.filtered.iter_mut().zip(current) {
                    *filtered += (input - *filtered) * smoothing_alpha;
                }
            }
            self.elapsed_seconds += 1.0 / SAMPLE_RATE_HZ;

            if !self.calibrated {
                self.calibration.push_back(self.filtered);
                let target = self.calibration_target();
                while self.calibration.len() > target {
                    self.calibration.pop_front();
                }
                if self.calibration.len() == target
                    && self.elapsed_seconds - self.last_calibration_attempt_seconds >= 0.5
                {
                    self.last_calibration_attempt_seconds = self.elapsed_seconds;
                    self.try_calibrate();
                }
            }
            if self.calibrated {
                let baseline_alpha = 1.0 / (SAMPLE_RATE_HZ as f32 * 10.0);
                for (baseline, filtered) in self.baseline.iter_mut().zip(self.filtered) {
                    *baseline += (filtered - *baseline) * baseline_alpha;
                }
                let projection = dot(subtract(self.filtered, self.baseline), self.axis);
                if !self.has_projection {
                    self.projection_ema = projection;
                    self.has_projection = true;
                } else {
                    self.projection_ema = projection;
                }
                self.update_adaptive_bounds();
                latest_volume = inverse_lerp(self.bound_min, self.bound_max, self.projection_ema);
            }
        }

        let motion_score = self.motion_score();
        let ready = self.calibrated && !stale && motion_score >= 0.35;
        let confidence_01 = if ready {
            self.signal_confidence(motion_score)
        } else {
            0.0
        };
        let phase = if !ready {
            BreathingPhase::BadSignal
        } else if !self.has_emitted_volume {
            self.has_emitted_volume = true;
            BreathingPhase::Pausing
        } else {
            let delta = latest_volume - self.last_emitted_volume;
            classify_phase_velocity(
                delta,
                samples.len() as f32 / SAMPLE_RATE_HZ as f32,
                self.phase_velocity_threshold_per_second(),
            )
        };
        self.last_emitted_volume = latest_volume;
        Some(BreathingSnapshot {
            calibrated: self.calibrated,
            ready,
            calibration_progress_01: (self.calibration.len() as f32
                / self.calibration_target() as f32)
                .clamp(0.0, 1.0),
            confidence_01,
            volume_01: latest_volume,
            magnitude_g: self.projection_ema,
            phase,
            axis_range_g: self.bound_max - self.bound_min,
            time_seconds: self.elapsed_seconds,
        })
    }

    fn try_calibrate(&mut self) {
        let count = self.calibration.len() as f32;
        let mut center = [0.0; 3];
        for sample in &self.calibration {
            for index in 0..3 {
                center[index] += sample[index];
            }
        }
        for value in &mut center {
            *value /= count;
        }

        let mut covariance = [[0.0_f32; 3]; 3];
        for sample in &self.calibration {
            let delta = subtract(*sample, center);
            for row in 0..3 {
                for column in 0..3 {
                    covariance[row][column] += delta[row] * delta[column] / count;
                }
            }
        }
        let dominant_dimension = (0..3)
            .max_by(|left, right| covariance[*left][*left].total_cmp(&covariance[*right][*right]))
            .unwrap_or(1);
        let mut axis = [0.0; 3];
        axis[dominant_dimension] = 1.0;
        for _ in 0..8 {
            let next = [
                dot(covariance[0], axis),
                dot(covariance[1], axis),
                dot(covariance[2], axis),
            ];
            let magnitude = dot(next, next).sqrt();
            if magnitude < 1e-8 {
                return;
            }
            axis = next.map(|value| value / magnitude);
        }
        if self.settings.invert_direction {
            axis = axis.map(|value| -value);
        }
        let mut projections = self
            .calibration
            .iter()
            .map(|sample| dot(subtract(*sample, center), axis))
            .collect::<Vec<_>>();
        projections.sort_by(f32::total_cmp);
        let mut low = quantile(&projections, self.settings.lower_quantile);
        let mut high = quantile(&projections, self.settings.upper_quantile);
        let raw_range = high - low;
        if raw_range < self.settings.minimum_axis_range_g {
            return;
        }
        let ease = raw_range * 0.03;
        low += ease;
        high -= ease;
        if high <= low {
            return;
        }
        self.center = center;
        self.baseline = center;
        self.axis = axis;
        self.bound_min = low;
        self.bound_max = high;
        self.calibration_span = high - low;
        self.calibrated = true;
        self.has_projection = false;
        self.adaptive_projections.clear();
    }

    fn update_adaptive_bounds(&mut self) {
        if self
            .adaptive_projections
            .back()
            .is_some_and(|(time, _)| self.elapsed_seconds - *time < 0.05)
        {
            return;
        }
        self.adaptive_projections
            .push_back((self.elapsed_seconds, self.projection_ema));
        let cutoff = self.elapsed_seconds - f64::from(self.settings.adaptive_window_seconds);
        while self
            .adaptive_projections
            .front()
            .is_some_and(|(time, _)| *time < cutoff)
        {
            self.adaptive_projections.pop_front();
        }
        if !self.settings.adaptive_bounds {
            return;
        }
        if self.elapsed_seconds - self.last_adaptive_update_seconds < 0.5
            || self.adaptive_projections.len() < 80
        {
            return;
        }
        self.last_adaptive_update_seconds = self.elapsed_seconds;
        let mut values = self
            .adaptive_projections
            .iter()
            .map(|(_, value)| *value)
            .collect::<Vec<_>>();
        values.sort_by(f32::total_cmp);
        let low = quantile(&values, self.settings.lower_quantile);
        let high = quantile(&values, self.settings.upper_quantile);
        let span = high - low;
        if span < self.settings.minimum_axis_range_g
            || span < self.calibration_span * 0.50
            || span > self.calibration_span * 2.0
        {
            return;
        }
        self.bound_min += (low - self.bound_min) * 0.20;
        self.bound_max += (high - self.bound_max) * 0.20;
    }

    fn motion_score(&self) -> f32 {
        let threshold = (self.settings.minimum_axis_range_g * 0.10).max(0.001);
        let ratio = self.motion_delta_ema_g / threshold;
        (1.0 / (1.0 + ratio * ratio)).clamp(0.0, 1.0)
    }

    fn signal_confidence(&self, motion_score: f32) -> f32 {
        let range_score = ((self.bound_max - self.bound_min)
            / (self.settings.minimum_axis_range_g * 2.0))
            .clamp(0.0, 1.0);
        let coverage = (self.adaptive_projections.len() as f32 / (SAMPLE_RATE_HZ as f32 * 0.8))
            .clamp(0.0, 1.0);
        let periodicity = periodicity_score(&self.adaptive_projections);
        (range_score * motion_score * (0.40 + 0.60 * coverage * periodicity)).clamp(0.0, 1.0)
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}
fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}
fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}
fn inverse_lerp(low: f32, high: f32, value: f32) -> f32 {
    if (high - low).abs() < 1e-8 {
        0.5
    } else {
        ((value - low) / (high - low)).clamp(0.0, 1.0)
    }
}

fn classify_phase_velocity(
    normalized_delta: f32,
    batch_duration_seconds: f32,
    threshold_per_second: f32,
) -> BreathingPhase {
    let minimum_duration = 1.0 / SAMPLE_RATE_HZ as f32;
    let velocity = normalized_delta / batch_duration_seconds.max(minimum_duration);
    if velocity > threshold_per_second {
        BreathingPhase::Inhaling
    } else if velocity < -threshold_per_second {
        BreathingPhase::Exhaling
    } else {
        BreathingPhase::Pausing
    }
}
fn quantile(sorted: &[f32], quantile: f32) -> f32 {
    let position = (sorted.len() - 1) as f32 * quantile;
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    sorted[low] + (sorted[high] - sorted[low]) * (position - low as f32)
}

fn periodicity_score(values: &VecDeque<(f64, f32)>) -> f32 {
    if values.len() < 80 {
        return 0.0;
    }
    let samples = values.iter().map(|(_, value)| *value).collect::<Vec<_>>();
    let mean = samples.iter().sum::<f32>() / samples.len() as f32;
    let centered = samples
        .iter()
        .map(|value| *value - mean)
        .collect::<Vec<_>>();
    let minimum_lag = 29;
    let maximum_lag = 250.min(centered.len().saturating_sub(40));
    if maximum_lag < minimum_lag {
        return 0.0;
    }
    (minimum_lag..=maximum_lag)
        .map(|lag| {
            let mut covariance = 0.0;
            let mut left_energy = 0.0;
            let mut right_energy = 0.0;
            for index in lag..centered.len() {
                let left = centered[index - lag];
                let right = centered[index];
                covariance += left * right;
                left_energy += left * left;
                right_energy += right * right;
            }
            let denominator = (left_energy * right_energy).sqrt();
            if denominator > 1e-12 {
                (covariance / denominator).clamp(0.0, 1.0)
            } else {
                0.0
            }
        })
        .fold(0.0, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean_motion(index: usize) -> AccSample {
        let z = 1_000 + (25.0 * (index as f32 / 200.0 * std::f32::consts::TAU * 0.2).sin()) as i16;
        AccSample {
            x_mg: 0,
            y_mg: 0,
            z_mg: z,
        }
    }

    #[test]
    fn publishes_bad_signal_during_calibration() {
        let mut processor = BreathingProcessor::default();
        let snapshot = processor.push(&[clean_motion(0)]).unwrap();
        assert_eq!(snapshot.phase, BreathingPhase::BadSignal);
        assert!(
            snapshot
                .samples()
                .iter()
                .any(|sample| sample.id == "breathing_phase" && sample.value == 0.0)
        );
    }

    #[test]
    fn calibrates_a_clean_chest_motion_axis_and_classifies_phase() {
        let mut processor = BreathingProcessor::default();
        let mut snapshot = None;
        for index in 0..2_500 {
            snapshot = processor.push(&[clean_motion(index)]);
        }
        let snapshot = snapshot.unwrap();
        assert!(snapshot.calibrated);
        assert!(snapshot.axis_range_g > 0.01);
        assert!((0.0..=1.0).contains(&snapshot.volume_01));
        assert!(snapshot.ready);
        assert!(snapshot.confidence_01 > 0.25);
        assert_ne!(snapshot.phase, BreathingPhase::BadSignal);
    }

    #[test]
    fn broadband_motion_drops_readiness_and_confidence() {
        let mut processor = BreathingProcessor::default();
        for index in 0..2_500 {
            processor.push(&[clean_motion(index)]);
        }
        let mut snapshot = None;
        for index in 0..400 {
            let impulse = if index % 2 == 0 { 500 } else { -500 };
            snapshot = processor.push(&[AccSample {
                x_mg: impulse,
                y_mg: -impulse,
                z_mg: 1_000 + impulse,
            }]);
        }
        let snapshot = snapshot.unwrap();
        assert!(!snapshot.ready);
        assert_eq!(snapshot.confidence_01, 0.0);
        assert_eq!(snapshot.phase, BreathingPhase::BadSignal);
    }

    #[test]
    fn ordinary_three_axis_sensor_noise_does_not_block_readiness() {
        let mut processor = BreathingProcessor::default();
        let mut snapshot = None;
        for index in 0..2_500 {
            let mut sample = clean_motion(index);
            let noise = if index % 2 == 0 { 3 } else { -3 };
            sample.x_mg = noise;
            sample.y_mg = -noise;
            sample.z_mg += noise;
            snapshot = processor.push(&[sample]);
        }
        let snapshot = snapshot.unwrap();
        assert!(snapshot.ready);
        assert!(snapshot.confidence_01 > 0.20);
    }

    #[test]
    fn custom_calibration_window_is_applied() {
        let mut processor = BreathingProcessor::new(BreathingSettings {
            calibration_window_seconds: 1.0,
            adaptive_bounds: false,
            ..BreathingSettings::default()
        });
        for index in 0..200 {
            processor.push(&[clean_motion(index)]);
        }
        assert!(processor.calibrated);
    }

    #[test]
    fn transport_values_are_a_three_state_public_classifier() {
        assert_eq!(BreathingPhase::Inhaling.numeric(), 1.0);
        assert_eq!(BreathingPhase::Exhaling.numeric(), -1.0);
        assert_eq!(BreathingPhase::Pausing.numeric(), 0.0);
        assert_eq!(BreathingPhase::BadSignal.numeric(), 0.0);
    }

    #[test]
    fn phase_velocity_is_independent_of_notification_batch_duration() {
        let processor = BreathingProcessor::default();
        let threshold = processor.phase_velocity_threshold_per_second();
        assert!((threshold - 0.06).abs() < 1e-6);

        assert_eq!(
            classify_phase_velocity(0.004, 0.05, threshold),
            BreathingPhase::Inhaling
        );
        assert_eq!(
            classify_phase_velocity(0.012, 0.15, threshold),
            BreathingPhase::Inhaling
        );
        assert_eq!(
            classify_phase_velocity(-0.004, 0.05, threshold),
            BreathingPhase::Exhaling
        );
        assert_eq!(
            classify_phase_velocity(-0.012, 0.15, threshold),
            BreathingPhase::Exhaling
        );
        assert_eq!(
            classify_phase_velocity(0.002, 0.05, threshold),
            BreathingPhase::Pausing
        );
        assert_eq!(
            classify_phase_velocity(0.006, 0.15, threshold),
            BreathingPhase::Pausing
        );
    }

    #[test]
    fn inversion_flips_the_classified_direction() {
        let settings = BreathingSettings {
            calibration_window_seconds: 5.0,
            adaptive_bounds: false,
            sensitivity: 1.0,
            ..BreathingSettings::default()
        };
        let mut normal = BreathingProcessor::new(settings);
        let mut inverted = BreathingProcessor::new(BreathingSettings {
            invert_direction: true,
            ..settings
        });
        for index in 0..1_000 {
            normal.push(&[clean_motion(index)]);
            inverted.push(&[clean_motion(index)]);
        }
        let follow_up = (1_000..1_030).map(clean_motion).collect::<Vec<_>>();
        let normal_phase = normal.push(&follow_up).unwrap().phase;
        let inverted_phase = inverted.push(&follow_up).unwrap().phase;
        assert!(
            matches!(
                (normal_phase, inverted_phase),
                (BreathingPhase::Inhaling, BreathingPhase::Exhaling)
                    | (BreathingPhase::Exhaling, BreathingPhase::Inhaling)
            ),
            "normal={normal_phase:?}, inverted={inverted_phase:?}"
        );
    }

    #[test]
    fn stale_tracking_uses_the_bad_signal_class() {
        let mut processor = BreathingProcessor::new(BreathingSettings {
            calibration_window_seconds: 1.0,
            adaptive_bounds: false,
            ..BreathingSettings::default()
        });
        for index in 0..200 {
            processor.push(&[clean_motion(index)]);
        }
        processor.last_push_at = Some(Instant::now() - Duration::from_secs(4));
        let snapshot = processor.push(&[clean_motion(201)]).unwrap();
        assert_eq!(snapshot.phase, BreathingPhase::BadSignal);
        assert_eq!(snapshot.phase.numeric(), 0.0);
    }

    #[test]
    fn fewer_than_two_axes_falls_back_to_recommended_x_and_z() {
        let settings = BreathingSettings {
            axes: [false, true, false],
            ..BreathingSettings::default()
        }
        .clamped();
        assert_eq!(settings.axes, [true, false, true]);
    }

    #[test]
    fn selected_axes_and_smoothing_controls_are_clamped() {
        let settings = BreathingSettings {
            axes: [true, true, false],
            smoothing_window_seconds: 50.0,
            sensitivity: -2.0,
            ..BreathingSettings::default()
        }
        .clamped();
        assert_eq!(settings.axes, [true, true, false]);
        assert_eq!(settings.smoothing_window_seconds, 5.0);
        assert_eq!(settings.sensitivity, 0.0);
    }
}
