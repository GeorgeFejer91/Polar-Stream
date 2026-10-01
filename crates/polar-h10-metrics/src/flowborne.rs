use std::collections::VecDeque;

use crate::BreathingSnapshot;

// The Unity classifier averages 24 and 180 frames at approximately 90 Hz.
const SHORT_SECONDS: f64 = 24.0 / 90.0;
const LONG_SECONDS: f64 = 180.0 / 90.0;
const GAP_SECONDS: f64 = 1.0;

// The questionnaire scene uses a 1:3 inhale/exhale threshold ratio. These
// dimensionless H10 thresholds are exploratory, not the controller's metres.
const INHALE_SPAN_FRACTION: f32 = 0.025;
const EXHALE_SPAN_FRACTION: f32 = -0.075;
const MAX_SCORE_SPANS: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FlowborneSnapshot {
    pub phase: f32,
    pub motion_score: Option<f32>,
}

/// Controller-style short/long signed-motion comparison on the H10's
/// calibrated, signed ACC projection. This is not tracked position: g and m
/// cannot share literal thresholds or validation claims.
#[derive(Default)]
pub(crate) struct FlowborneProcessor {
    values: VecDeque<(f64, f32)>,
    first_time: Option<f64>,
    last_time: Option<f64>,
}

impl FlowborneProcessor {
    pub(crate) fn push(
        &mut self,
        snapshot: BreathingSnapshot,
        discontinuity: bool,
    ) -> FlowborneSnapshot {
        let time = snapshot.time_seconds;
        if discontinuity
            || self
                .last_time
                .is_some_and(|last| time <= last || time - last > GAP_SECONDS)
            || !snapshot.ready
            || !time.is_finite()
            || !snapshot.magnitude_g.is_finite()
            || !snapshot.axis_range_g.is_finite()
            || snapshot.axis_range_g <= 0.0
        {
            *self = Self::default();
            if discontinuity
                || !snapshot.ready
                || !time.is_finite()
                || !snapshot.magnitude_g.is_finite()
                || !snapshot.axis_range_g.is_finite()
                || snapshot.axis_range_g <= 0.0
            {
                return Self::bad_signal();
            }
        }

        self.first_time.get_or_insert(time);
        self.last_time = Some(time);
        self.values.push_back((time, snapshot.magnitude_g));
        while self
            .values
            .front()
            .is_some_and(|(old, _)| time - old > LONG_SECONDS)
        {
            self.values.pop_front();
        }
        if time - self.first_time.unwrap_or(time) < LONG_SECONDS {
            return Self::bad_signal();
        }

        let mut short_sum = 0.0_f64;
        let mut short_count = 0_u32;
        let mut long_sum = 0.0_f64;
        for &(sample_time, value) in &self.values {
            long_sum += f64::from(value);
            if time - sample_time <= SHORT_SECONDS {
                short_sum += f64::from(value);
                short_count += 1;
            }
        }
        if short_count == 0 || self.values.is_empty() {
            return Self::bad_signal();
        }
        let score = ((short_sum / f64::from(short_count) - long_sum / self.values.len() as f64)
            / f64::from(snapshot.axis_range_g)) as f32;
        if !score.is_finite() || score.abs() > MAX_SCORE_SPANS {
            return Self::bad_signal();
        }
        let phase = if score > INHALE_SPAN_FRACTION {
            1.0
        } else if score < EXHALE_SPAN_FRACTION {
            -1.0
        } else {
            0.0
        };
        FlowborneSnapshot {
            phase,
            motion_score: Some(score),
        }
    }

    const fn bad_signal() -> FlowborneSnapshot {
        FlowborneSnapshot {
            phase: -2.0,
            motion_score: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BreathingPhase;

    fn sample(time_seconds: f64, magnitude_g: f32, ready: bool) -> BreathingSnapshot {
        BreathingSnapshot {
            calibrated: ready,
            ready,
            calibration_progress_01: if ready { 1.0 } else { 0.0 },
            confidence_01: if ready { 1.0 } else { 0.0 },
            volume_01: 0.5,
            magnitude_g,
            phase: BreathingPhase::Pausing,
            axis_range_g: 1.0,
            time_seconds,
        }
    }

    #[test]
    fn classifies_four_states_and_restarts_after_a_gap() {
        let mut classifier = FlowborneProcessor::default();
        assert_eq!(classifier.push(sample(0.0, 0.0, false), false).phase, -2.0);
        for index in 0..=20 {
            classifier.push(sample(index as f64 * 0.1, 0.0, true), false);
        }
        assert_eq!(classifier.push(sample(2.1, 0.0, true), false).phase, 0.0);
        assert_eq!(classifier.push(sample(2.2, 0.5, true), false).phase, 1.0);
        classifier.push(sample(2.3, -0.5, true), false);
        assert_eq!(classifier.push(sample(2.4, -0.5, true), false).phase, -1.0);
        assert_eq!(classifier.push(sample(4.0, 0.0, true), false).phase, -2.0);
        assert_eq!(classifier.push(sample(4.1, 0.0, false), false).phase, -2.0);
    }
}
