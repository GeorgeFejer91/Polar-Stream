use std::collections::VecDeque;

use polar_h10_core::AccSample;

use crate::TimedAccBatch;

const SHORT_NS: u64 = 200_000_000;
const LONG_NS: u64 = 200_000_000_000;
const RATE_NS: u64 = 60_000_000_000;
const QUIET_NS: u64 = 1_000_000_000;
const REFRACTORY_NS: u64 = 250_000_000;
const GAP_NS: u64 = 1_000_000_000;
const SHORT_LIMIT: usize = 40;
const LONG_LIMIT: usize = 40_000;

#[derive(Clone, Copy)]
pub(crate) struct PhanBreathingSnapshot {
    pub breath_detected: bool,
    pub breaths_per_minute: f32,
}

#[derive(Default)]
struct AxisWindow {
    values: VecDeque<(u64, [f32; 3])>,
    sums: [f64; 3],
}

impl AxisWindow {
    fn push(&mut self, time_ns: u64, value: [f32; 3], duration_ns: u64, limit: usize) {
        self.values.push_back((time_ns, value));
        for (sum, axis_value) in self.sums.iter_mut().zip(value) {
            *sum += f64::from(axis_value);
        }
        while self.values.len() > limit
            || self
                .values
                .front()
                .is_some_and(|(old, _)| time_ns.saturating_sub(*old) > duration_ns)
        {
            let (_, old) = self.values.pop_front().expect("nonempty window");
            for (sum, axis_value) in self.sums.iter_mut().zip(old) {
                *sum -= f64::from(axis_value);
            }
        }
    }

    fn mean(&self, axis: usize) -> f64 {
        self.sums[axis] / self.values.len() as f64
    }
}

/// Time-scaled adaptation of Phan's single-accelerometer threshold detector.
/// Only the two final outputs leave this processor; raw ACC remains unchanged.
pub(crate) struct PhanBreathingProcessor {
    short: AxisWindow,
    long: AxisWindow,
    threshold: f64,
    above: bool,
    last_above_ns: Option<u64>,
    last_breath_ns: Option<u64>,
    start_ns: Option<u64>,
    last_sample_ns: Option<u64>,
    breaths: VecDeque<u64>,
}

impl Default for PhanBreathingProcessor {
    fn default() -> Self {
        Self {
            short: AxisWindow::default(),
            long: AxisWindow::default(),
            threshold: 0.05,
            above: false,
            last_above_ns: None,
            last_breath_ns: None,
            start_ns: None,
            last_sample_ns: None,
            breaths: VecDeque::new(),
        }
    }
}

impl PhanBreathingProcessor {
    pub(crate) fn push(
        &mut self,
        samples: &[AccSample],
        timing: Option<TimedAccBatch>,
    ) -> Option<PhanBreathingSnapshot> {
        if samples.is_empty() {
            return None;
        }
        if timing.is_some_and(|batch| batch.clock_reset || batch.gap_before) {
            *self = Self::default();
        }
        let mut detected = false;
        let period_ns = timing.map_or(5_000_000, |batch| batch.sample_period_ns.max(1));
        let newest_ns = timing.map_or_else(
            || {
                self.last_sample_ns
                    .unwrap_or(0)
                    .saturating_add(period_ns.saturating_mul(samples.len() as u64))
            },
            |batch| batch.newest_sensor_timestamp_ns,
        );
        let first_ns = newest_ns
            .saturating_sub(period_ns.saturating_mul(samples.len().saturating_sub(1) as u64));
        let mut accepted = false;
        for (index, sample) in samples.iter().enumerate() {
            let time_ns = first_ns.saturating_add(period_ns.saturating_mul(index as u64));
            if self.last_sample_ns.is_some_and(|last| time_ns <= last) {
                continue;
            }
            if self
                .last_sample_ns
                .is_some_and(|last| time_ns.saturating_sub(last) > GAP_NS)
            {
                *self = Self::default();
            }
            let axes = [sample.x_mg, sample.y_mg, sample.z_mg].map(|mg| f32::from(mg) / 1_000.0);
            let start_ns = *self.start_ns.get_or_insert(time_ns);
            self.short.push(time_ns, axes, SHORT_NS, SHORT_LIMIT);
            self.long.push(time_ns, axes, LONG_NS, LONG_LIMIT);
            let difference = (0..3)
                .map(|axis| (self.short.mean(axis) - self.long.mean(axis)).abs())
                .sum::<f64>();
            if !(0.0..1.0).contains(&self.threshold) {
                self.threshold = 0.05;
            }
            // The source changes the threshold by 0.000005 per 60 Hz frame.
            // Scale that slope by source time, not by the H10's 200 Hz rate.
            let dt = self.last_sample_ns.map_or(0.0, |last| {
                time_ns.saturating_sub(last).min(20_000_000) as f64 / 1e9
            });
            self.threshold += if self.above {
                0.0003 * dt
            } else {
                -0.0003 * dt
            };
            let above = difference > self.threshold;
            if above {
                let quiet = self
                    .last_above_ns
                    .is_none_or(|last| time_ns.saturating_sub(last) >= QUIET_NS);
                let refractory = self
                    .last_breath_ns
                    .is_none_or(|last| time_ns.saturating_sub(last) >= REFRACTORY_NS);
                if !self.above
                    && quiet
                    && refractory
                    && time_ns.saturating_sub(start_ns) >= REFRACTORY_NS
                {
                    self.breaths.push_back(time_ns);
                    self.last_breath_ns = Some(time_ns);
                    detected = true;
                }
                self.last_above_ns = Some(time_ns);
            }
            self.above = above;
            self.last_sample_ns = Some(time_ns);
            accepted = true;
            while self
                .breaths
                .front()
                .is_some_and(|old| time_ns.saturating_sub(*old) > RATE_NS)
            {
                self.breaths.pop_front();
            }
        }
        accepted.then_some(PhanBreathingSnapshot {
            breath_detected: detected,
            breaths_per_minute: self.breaths.len() as f32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_event_per_separated_motion_pulse_and_rolling_rate() {
        let mut detector = PhanBreathingProcessor::default();
        let mut events = 0;
        let mut rate = 0.0;
        for index in 0..2_000_u64 {
            let moving = index % 400 >= 100 && index % 400 < 180;
            let sample = AccSample {
                x_mg: if moving { 180 } else { 0 },
                y_mg: 0,
                z_mg: 1_000,
            };
            let snapshot = detector
                .push(
                    &[sample],
                    Some(TimedAccBatch {
                        newest_sensor_timestamp_ns: 1_000_000_000 + index * 5_000_000,
                        sample_period_ns: 5_000_000,
                        clock_revision: 0,
                        clock_reset: false,
                        gap_before: false,
                    }),
                )
                .unwrap();
            events += usize::from(snapshot.breath_detected);
            rate = snapshot.breaths_per_minute;
        }
        assert_eq!(events, 5);
        assert_eq!(rate, 5.0);
        for index in 2_000..14_400_u64 {
            rate = detector
                .push(
                    &[AccSample {
                        x_mg: 0,
                        y_mg: 0,
                        z_mg: 1_000,
                    }],
                    Some(TimedAccBatch {
                        newest_sensor_timestamp_ns: 1_000_000_000 + index * 5_000_000,
                        sample_period_ns: 5_000_000,
                        clock_revision: 0,
                        clock_reset: false,
                        gap_before: false,
                    }),
                )
                .unwrap()
                .breaths_per_minute;
        }
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn gap_resets_rate_and_baseline() {
        let mut detector = PhanBreathingProcessor::default();
        let rest = AccSample {
            x_mg: 0,
            y_mg: 0,
            z_mg: 1_000,
        };
        let motion = AccSample { x_mg: 200, ..rest };
        for index in 0..60_u64 {
            detector.push(
                &[if index > 20 { motion } else { rest }],
                Some(TimedAccBatch {
                    newest_sensor_timestamp_ns: 1_000_000_000 + index * 5_000_000,
                    sample_period_ns: 5_000_000,
                    clock_revision: 0,
                    clock_reset: false,
                    gap_before: false,
                }),
            );
        }
        let reset = detector
            .push(
                &[rest],
                Some(TimedAccBatch {
                    newest_sensor_timestamp_ns: 5_000_000_000,
                    sample_period_ns: 5_000_000,
                    clock_revision: 1,
                    clock_reset: true,
                    gap_before: true,
                }),
            )
            .unwrap();
        assert_eq!(reset.breaths_per_minute, 0.0);
        assert!(!reset.breath_detected);
    }
}
