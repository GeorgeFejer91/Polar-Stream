use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use polar_h10_core::AccSample;
use serde::{Deserialize, Serialize};

use crate::MetricSample;

const SAMPLE_RATE_HZ: f64 = 200.0;

/// Saved, per-output controls for the accelerometer breathing classifier.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreathingSettings {
    #[serde(default = "default_calibration_window_seconds")]
    pub calibration_window_seconds: f32,
    #[serde(default = "default_minimum_axis_range_g")]
    pub minimum_axis_range_g: f32,
    #[serde(default = "default_sample_ema_alpha")]
    pub sample_ema_alpha: f32,
    #[serde(default = "default_projection_ema_alpha")]
    pub projection_ema_alpha: f32,
    #[serde(default = "default_phase_delta_threshold")]
    pub phase_delta_threshold: f32,
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
            calibration_window_seconds: default_calibration_window_seconds(),
            minimum_axis_range_g: default_minimum_axis_range_g(),
            sample_ema_alpha: default_sample_ema_alpha(),
            projection_ema_alpha: default_projection_ema_alpha(),
            phase_delta_threshold: default_phase_delta_threshold(),
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
        self.calibration_window_seconds =
            finite_or(self.calibration_window_seconds, 12.0).clamp(1.0, 60.0);
        self.minimum_axis_range_g = finite_or(self.minimum_axis_range_g, 0.01).clamp(0.001, 0.25);
        self.sample_ema_alpha = finite_or(self.sample_ema_alpha, 0.10).clamp(0.01, 1.0);
        self.projection_ema_alpha = finite_or(self.projection_ema_alpha, 0.10).clamp(0.01, 1.0);
        self.phase_delta_threshold =
            finite_or(self.phase_delta_threshold, 0.003).clamp(0.0001, 0.25);
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
const fn default_calibration_window_seconds() -> f32 {
    12.0
}
const fn default_minimum_axis_range_g() -> f32 {
    0.01
}
const fn default_sample_ema_alpha() -> f32 {
    0.10
}
const fn default_projection_ema_alpha() -> f32 {
    0.10
}
const fn default_phase_delta_threshold() -> f32 {
    0.003
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
    /// Stable transport values: +1 inhale, -1 exhale, 0 pause, -2 bad signal.
    pub fn numeric(self) -> f32 {
        match self {
            Self::Inhaling => 1.0,
            Self::Exhaling => -1.0,
            Self::Pausing => 0.0,
            Self::BadSignal => -2.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BreathingSnapshot {
    pub calibrated: bool,
    pub calibration_progress_01: f32,
    pub volume_01: f32,
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
        ];
        if self.calibrated {
            values.extend([
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

pub(crate) struct BreathingProcessor {
    settings: BreathingSettings,
    filtered: [f32; 3],
    has_filtered: bool,
    calibration: VecDeque<[f32; 3]>,
    last_calibration_attempt_seconds: f64,
    center: [f32; 3],
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
}

impl Default for BreathingProcessor {
    fn default() -> Self {
        Self::new(BreathingSettings::default())
    }
}

impl BreathingProcessor {
    pub(crate) fn new(settings: BreathingSettings) -> Self {
        Self {
            settings: settings.clamped(),
            filtered: [0.0; 3],
            has_filtered: false,
            calibration: VecDeque::new(),
            last_calibration_attempt_seconds: f64::NEG_INFINITY,
            center: [0.0; 3],
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
        }
    }

    pub(crate) fn apply_settings(&mut self, settings: BreathingSettings) {
        let settings = settings.clamped();
        if self.settings != settings {
            *self = Self::new(settings);
        }
    }

    fn calibration_target(&self) -> usize {
        (self.settings.calibration_window_seconds as f64 * SAMPLE_RATE_HZ).round() as usize
    }

    pub(crate) fn push(&mut self, samples: &[AccSample]) -> Option<BreathingSnapshot> {
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
            let current = [
                f32::from(sample.x_mg) / 1_000.0,
                f32::from(sample.y_mg) / 1_000.0,
                f32::from(sample.z_mg) / 1_000.0,
            ];
            if !self.has_filtered {
                self.filtered = current;
                self.has_filtered = true;
            } else {
                for (filtered, input) in self.filtered.iter_mut().zip(current) {
                    *filtered += (input - *filtered) * self.settings.sample_ema_alpha;
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
                let projection = dot(subtract(self.filtered, self.center), self.axis);
                if !self.has_projection {
                    self.projection_ema = projection;
                    self.has_projection = true;
                } else {
                    self.projection_ema +=
                        (projection - self.projection_ema) * self.settings.projection_ema_alpha;
                }
                self.update_adaptive_bounds();
                latest_volume = inverse_lerp(self.bound_min, self.bound_max, self.projection_ema);
            }
        }

        let phase = if !self.calibrated || stale {
            BreathingPhase::BadSignal
        } else if !self.has_emitted_volume {
            self.has_emitted_volume = true;
            BreathingPhase::Pausing
        } else {
            let delta = latest_volume - self.last_emitted_volume;
            if delta > self.settings.phase_delta_threshold {
                BreathingPhase::Inhaling
            } else if delta < -self.settings.phase_delta_threshold {
                BreathingPhase::Exhaling
            } else {
                BreathingPhase::Pausing
            }
        };
        self.last_emitted_volume = latest_volume;
        Some(BreathingSnapshot {
            calibrated: self.calibrated,
            calibration_progress_01: (self.calibration.len() as f32
                / self.calibration_target() as f32)
                .clamp(0.0, 1.0),
            volume_01: latest_volume,
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
        self.axis = axis;
        self.bound_min = low;
        self.bound_max = high;
        self.calibration_span = high - low;
        self.calibrated = true;
        self.has_projection = false;
        self.adaptive_projections.clear();
    }

    fn update_adaptive_bounds(&mut self) {
        if !self.settings.adaptive_bounds {
            return;
        }
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
fn quantile(sorted: &[f32], quantile: f32) -> f32 {
    let position = (sorted.len() - 1) as f32 * quantile;
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    sorted[low] + (sorted[high] - sorted[low]) * (position - low as f32)
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
                .any(|sample| sample.id == "breathing_phase" && sample.value == -2.0)
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
        assert_ne!(snapshot.phase, BreathingPhase::BadSignal);
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
    fn transport_values_include_a_distinct_bad_signal_state() {
        assert_eq!(BreathingPhase::Inhaling.numeric(), 1.0);
        assert_eq!(BreathingPhase::Exhaling.numeric(), -1.0);
        assert_eq!(BreathingPhase::Pausing.numeric(), 0.0);
        assert_eq!(BreathingPhase::BadSignal.numeric(), -2.0);
    }

    #[test]
    fn inversion_flips_the_classified_direction() {
        let settings = BreathingSettings {
            calibration_window_seconds: 5.0,
            adaptive_bounds: false,
            phase_delta_threshold: 0.0001,
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
    }
}
