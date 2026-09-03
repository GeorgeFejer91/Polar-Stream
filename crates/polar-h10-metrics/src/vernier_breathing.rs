use std::collections::VecDeque;

/// Stable, machine-readable contract for the fixed Vernier force-to-breathing
/// transform. Output transports use this same value for provenance metadata so
/// recorded settings cannot drift from the implementation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VernierBreathingContract {
    pub algorithm: &'static str,
    pub settings_schema: &'static str,
    pub window_seconds: f64,
    pub robust_bounds_minimum_samples: usize,
    pub bounds_update_samples: usize,
    pub maximum_history_samples: usize,
    pub lower_quantile: f64,
    pub upper_quantile: f64,
    pub warmup_bounds: &'static str,
    pub nonfinite_policy: &'static str,
    pub degenerate_range_value: f32,
    pub inhale_direction: &'static str,
    pub output_range: &'static str,
}

pub const VERNIER_BREATHING_CONTRACT: VernierBreathingContract = VernierBreathingContract {
    algorithm: "polar-stream-vernier-force-respiration",
    settings_schema: "vernier-breathing-settings-v1",
    window_seconds: 30.0,
    robust_bounds_minimum_samples: 20,
    bounds_update_samples: 5,
    maximum_history_samples: 30_000,
    lower_quantile: 0.05,
    upper_quantile: 0.95,
    warmup_bounds: "observed-min-max",
    nonfinite_policy: "hold-last-output",
    degenerate_range_value: 0.5,
    inhale_direction: "increasing-force",
    output_range: "0,1",
};

/// Causal, bounded normalization for the GDX-RB Force channel.
///
/// Force increases during inhalation according to Vernier's device contract.
/// This processor leaves that polarity intact and maps the recent force range
/// to 0-1. It never enters the raw recording path. Non-finite force samples
/// hold the last derived value (0.5 before the first finite range) so the
/// derived outlet remains bounded while the raw outlet retains the exact input.
pub struct VernierBreathingProcessor {
    history: VecDeque<(f64, f64)>,
    elapsed_seconds: f64,
    lower_force_n: f64,
    upper_force_n: f64,
    samples_since_bounds: usize,
    last_value_01: f32,
}

impl Default for VernierBreathingProcessor {
    fn default() -> Self {
        Self {
            history: VecDeque::new(),
            elapsed_seconds: 0.0,
            lower_force_n: 0.0,
            upper_force_n: 0.0,
            samples_since_bounds: 0,
            last_value_01: 0.5,
        }
    }
}

impl VernierBreathingProcessor {
    pub fn push(&mut self, force_values_n: &[f64], sample_period_us: u32) -> Vec<f32> {
        let step_seconds = f64::from(sample_period_us) / 1_000_000.0;
        let mut normalized = Vec::with_capacity(force_values_n.len());
        for force_n in force_values_n.iter().copied() {
            if force_n.is_finite() {
                self.history.push_back((self.elapsed_seconds, force_n));
                self.samples_since_bounds = self.samples_since_bounds.saturating_add(1);
                self.prune_history();
                if self.history.len() <= VERNIER_BREATHING_CONTRACT.bounds_update_samples
                    || self.samples_since_bounds >= VERNIER_BREATHING_CONTRACT.bounds_update_samples
                {
                    self.update_bounds();
                }
                self.last_value_01 =
                    normalize_force(force_n, self.lower_force_n, self.upper_force_n);
            }
            normalized.push(self.last_value_01);
            self.elapsed_seconds += step_seconds;
        }
        normalized
    }

    fn prune_history(&mut self) {
        let cutoff = self.elapsed_seconds - VERNIER_BREATHING_CONTRACT.window_seconds;
        while self.history.front().is_some_and(|(time, _)| *time < cutoff)
            || self.history.len() > VERNIER_BREATHING_CONTRACT.maximum_history_samples
        {
            self.history.pop_front();
        }
    }

    fn update_bounds(&mut self) {
        let mut values = self
            .history
            .iter()
            .map(|(_, value)| *value)
            .collect::<Vec<_>>();
        values.sort_by(f64::total_cmp);
        if let (Some(first), Some(last)) = (values.first(), values.last()) {
            if values.len() >= VERNIER_BREATHING_CONTRACT.robust_bounds_minimum_samples {
                self.lower_force_n = quantile(&values, VERNIER_BREATHING_CONTRACT.lower_quantile);
                self.upper_force_n = quantile(&values, VERNIER_BREATHING_CONTRACT.upper_quantile);
            } else {
                self.lower_force_n = *first;
                self.upper_force_n = *last;
            }
        }
        self.samples_since_bounds = 0;
    }
}

fn normalize_force(value: f64, lower: f64, upper: f64) -> f32 {
    if !value.is_finite() || !lower.is_finite() || !upper.is_finite() || upper - lower < 1e-9 {
        return VERNIER_BREATHING_CONTRACT.degenerate_range_value;
    }
    ((value - lower) / (upper - lower)).clamp(0.0, 1.0) as f32
}

fn quantile(sorted: &[f64], quantile: f64) -> f64 {
    let position = (sorted.len() - 1) as f64 * quantile;
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    sorted[low] + (sorted[high] - sorted[low]) * (position - low as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_rise_and_fall_produce_a_bounded_waveform_with_matching_polarity() {
        let mut processor = VernierBreathingProcessor::default();
        let rising = processor.push(&[10.0, 11.0, 12.0, 13.0, 14.0], 100_000);
        let falling = processor.push(&[13.0, 12.0, 11.0, 10.0], 100_000);

        assert!(rising.iter().all(|value| (0.0..=1.0).contains(value)));
        assert!(falling.iter().all(|value| (0.0..=1.0).contains(value)));
        assert!(rising.last() > rising.first());
        assert!(falling.last() < falling.first());
    }

    #[test]
    fn nonfinite_force_holds_the_last_bounded_derived_value() {
        let mut processor = VernierBreathingProcessor::default();
        let finite = processor.push(&[5.0, 6.0], 100_000);
        let held = processor.push(&[f64::NAN, f64::INFINITY], 100_000);
        assert_eq!(held, vec![*finite.last().unwrap(), *finite.last().unwrap()]);
        assert!(held.iter().all(|value| (0.0..=1.0).contains(value)));
    }

    #[test]
    fn reset_starts_a_new_session_at_midscale() {
        let mut processor = VernierBreathingProcessor::default();
        processor.push(&[1.0, 2.0, 3.0], 100_000);
        processor = VernierBreathingProcessor::default();
        assert_eq!(processor.push(&[100.0], 100_000), [0.5]);
    }
}
