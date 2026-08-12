use std::collections::VecDeque;

use polar_h10_core::AccSample;

use crate::MetricSample;

const CALIBRATION_SAMPLES: usize = 200 * 12;
const MINIMUM_AXIS_RANGE_G: f32 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreathingPhase {
    Inhaling,
    Exhaling,
    Pausing,
    Unavailable,
}

impl BreathingPhase {
    pub fn numeric(self) -> f32 {
        match self {
            Self::Inhaling => 1.0,
            Self::Exhaling => -1.0,
            Self::Pausing => 0.0,
            Self::Unavailable => f32::NAN,
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
        let mut values = vec![MetricSample {
            id: "breathing_calibration",
            value: self.calibration_progress_01,
        }];
        if self.calibrated {
            values.extend([
                MetricSample {
                    id: "breathing_volume",
                    value: self.volume_01,
                },
                MetricSample {
                    id: "breathing_phase",
                    value: self.phase.numeric(),
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
    filtered: [f32; 3],
    has_filtered: bool,
    calibration: VecDeque<[f32; 3]>,
    center: [f32; 3],
    axis: [f32; 3],
    bound_min: f32,
    bound_max: f32,
    calibrated: bool,
    projection_ema: f32,
    has_projection: bool,
    last_emitted_volume: f32,
    elapsed_seconds: f64,
}

impl Default for BreathingProcessor {
    fn default() -> Self {
        Self {
            filtered: [0.0; 3],
            has_filtered: false,
            calibration: VecDeque::with_capacity(CALIBRATION_SAMPLES),
            center: [0.0; 3],
            axis: [0.0, 1.0, 0.0],
            bound_min: -0.02,
            bound_max: 0.02,
            calibrated: false,
            projection_ema: 0.0,
            has_projection: false,
            last_emitted_volume: 0.5,
            elapsed_seconds: 0.0,
        }
    }
}

impl BreathingProcessor {
    pub(crate) fn push(&mut self, samples: &[AccSample]) -> Option<BreathingSnapshot> {
        if samples.is_empty() {
            return None;
        }
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
                    *filtered += (input - *filtered) * 0.10;
                }
            }
            self.elapsed_seconds += 1.0 / 200.0;

            if !self.calibrated {
                self.calibration.push_back(self.filtered);
                if self.calibration.len() > CALIBRATION_SAMPLES {
                    self.calibration.pop_front();
                }
                if self.calibration.len() == CALIBRATION_SAMPLES {
                    self.try_calibrate();
                }
            }
            if self.calibrated {
                let centered = subtract(self.filtered, self.center);
                let projection = dot(centered, self.axis);
                if !self.has_projection {
                    self.projection_ema = projection;
                    self.has_projection = true;
                } else {
                    self.projection_ema += (projection - self.projection_ema) * 0.10;
                }
                latest_volume = inverse_lerp(self.bound_min, self.bound_max, self.projection_ema);
            }
        }

        let phase = if !self.calibrated {
            BreathingPhase::Unavailable
        } else {
            let delta = latest_volume - self.last_emitted_volume;
            if delta > 0.003 {
                BreathingPhase::Inhaling
            } else if delta < -0.003 {
                BreathingPhase::Exhaling
            } else {
                BreathingPhase::Pausing
            }
        };
        self.last_emitted_volume = latest_volume;
        Some(BreathingSnapshot {
            calibrated: self.calibrated,
            calibration_progress_01: (self.calibration.len() as f32 / CALIBRATION_SAMPLES as f32)
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
        if dot(axis, self.axis) < 0.0 {
            axis = axis.map(|value| -value);
        }
        let mut projections = self
            .calibration
            .iter()
            .map(|sample| dot(subtract(*sample, center), axis))
            .collect::<Vec<_>>();
        projections.sort_by(f32::total_cmp);
        let mut low = quantile(&projections, 0.05);
        let mut high = quantile(&projections, 0.95);
        let raw_range = high - low;
        if raw_range < MINIMUM_AXIS_RANGE_G {
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
        self.calibrated = true;
        self.has_projection = false;
    }
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

    #[test]
    fn calibrates_a_clean_chest_motion_axis_and_classifies_phase() {
        let mut processor = BreathingProcessor::default();
        let mut snapshot = None;
        for index in 0..2_500 {
            let z =
                1_000 + (25.0 * (index as f32 / 200.0 * std::f32::consts::TAU * 0.2).sin()) as i16;
            snapshot = processor.push(&[AccSample {
                x_mg: 0,
                y_mg: 0,
                z_mg: z,
            }]);
        }
        let snapshot = snapshot.unwrap();
        assert!(snapshot.calibrated);
        assert!(snapshot.axis_range_g > 0.01);
        assert!((0.0..=1.0).contains(&snapshot.volume_01));
    }
}
