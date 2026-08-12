use std::collections::VecDeque;

const EXCITEOMETER_RMSSD_BEATS: usize = 10;
const EXCITEOMETER_BASELINE_PAIRS: u64 = 10;

/// Polar Stream's newer, explicitly experimental activation composite.
#[derive(Default)]
pub(crate) struct ExcitationProcessor {
    heart_rate: RunningStats,
    ln_rmssd: RunningStats,
}

/// Causal adaptation of the original open-source Excite-O-Meter score.
///
/// The Unity implementation calculates the score retrospectively with the
/// completed session's population mean and standard deviation. A live LSL/OSC
/// stream cannot revise values already sent, so this processor applies the
/// same equation to session-to-date statistics. Values are therefore
/// provisional until the session ends.
#[derive(Default)]
pub(crate) struct ExcitementScoreProcessor {
    recent_rr: VecDeque<f32>,
    rr: RunningStats,
    rmssd: RunningStats,
}

impl ExcitementScoreProcessor {
    pub(crate) fn update(&mut self, rr_ms: f32) -> Option<f32> {
        if !rr_ms.is_finite() {
            return None;
        }
        self.recent_rr.push_back(rr_ms);
        if self.recent_rr.len() > EXCITEOMETER_RMSSD_BEATS {
            self.recent_rr.pop_front();
        }
        if self.recent_rr.len() < EXCITEOMETER_RMSSD_BEATS {
            return None;
        }

        let rmssd = rolling_rmssd(&self.recent_rr)?;
        self.rr.push(rr_ms);
        self.rmssd.push(rmssd);
        if self.rr.count < EXCITEOMETER_BASELINE_PAIRS
            || self.rmssd.count < EXCITEOMETER_BASELINE_PAIRS
        {
            return None;
        }

        Some(exciteometer_score(
            self.rr.population_z_score(rr_ms),
            self.rmssd.population_z_score(rmssd),
        ))
    }
}

fn rolling_rmssd(values: &VecDeque<f32>) -> Option<f32> {
    if values.len() < 2 {
        return None;
    }
    let mean_square = values
        .iter()
        .zip(values.iter().skip(1))
        .map(|(left, right)| f64::from(right - left).powi(2))
        .sum::<f64>()
        / (values.len() - 1) as f64;
    Some(mean_square.sqrt() as f32)
}

fn exciteometer_score(z_rr: f32, z_rmssd: f32) -> f32 {
    (1.0 - (normal_cdf(z_rr) + normal_cdf(z_rmssd)) * 0.5).clamp(0.0, 1.0)
}

/// Standard-normal cumulative distribution using Abramowitz and Stegun
/// formula 7.1.26, matching the legacy C# implementation.
fn normal_cdf(value: f32) -> f32 {
    let x = f64::from(value);
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let scaled = x.abs() / 2.0_f64.sqrt();
    let t = 1.0 / (1.0 + 0.327_591_1 * scaled);
    let erf = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-scaled * scaled).exp());
    (0.5 * (1.0 + sign * erf)) as f32
}

impl ExcitationProcessor {
    pub(crate) fn update(&mut self, heart_rate: f32, ln_rmssd: f32) -> Option<f32> {
        self.heart_rate.push(heart_rate);
        self.ln_rmssd.push(ln_rmssd);
        if self.heart_rate.count < 20 || self.ln_rmssd.count < 20 {
            return None;
        }
        let activation =
            self.heart_rate.z_score(heart_rate) * 0.65 - self.ln_rmssd.z_score(ln_rmssd) * 0.35;
        Some((1.0 / (1.0 + (-activation).exp())).clamp(0.0, 1.0))
    }
}

#[derive(Default)]
struct RunningStats {
    count: u64,
    mean: f64,
    m2: f64,
}

impl RunningStats {
    fn push(&mut self, value: f32) {
        if !value.is_finite() {
            return;
        }
        self.count += 1;
        let delta = f64::from(value) - self.mean;
        self.mean += delta / self.count as f64;
        self.m2 += delta * (f64::from(value) - self.mean);
    }

    fn z_score(&self, value: f32) -> f32 {
        if self.count < 2 {
            return 0.0;
        }
        let sd = (self.m2 / (self.count - 1) as f64).sqrt();
        if sd < 1e-6 {
            0.0
        } else {
            ((f64::from(value) - self.mean) / sd) as f32
        }
    }

    fn population_z_score(&self, value: f32) -> f32 {
        if self.count < 2 {
            return 0.0;
        }
        let sd = (self.m2 / self.count as f64).sqrt();
        if sd < 1e-6 {
            0.0
        } else {
            ((f64::from(value) - self.mean) / sd) as f32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_formula_maps_low_rr_and_rmssd_to_higher_score() {
        let high_activation = exciteometer_score(-1.0, -1.0);
        let neutral = exciteometer_score(0.0, 0.0);
        let low_activation = exciteometer_score(1.0, 1.0);

        assert!((neutral - 0.5).abs() < 1e-6);
        assert!((high_activation - 0.841_344_7).abs() < 1e-4);
        assert!((low_activation - 0.158_655_3).abs() < 1e-4);
    }

    #[test]
    fn live_score_waits_for_rolling_rmssd_and_session_baseline() {
        let mut processor = ExcitementScoreProcessor::default();
        for index in 0..18 {
            let rr = if index % 3 == 0 { 780.0 } else { 820.0 };
            assert!(processor.update(rr).is_none());
        }

        let score = processor
            .update(790.0)
            .expect("ten paired values form baseline");
        assert!((0.0..=1.0).contains(&score));
    }

    #[test]
    fn constant_session_is_neutral_instead_of_dividing_by_zero() {
        let mut processor = ExcitementScoreProcessor::default();
        let mut score = None;
        for _ in 0..19 {
            score = processor.update(800.0);
        }
        assert!((score.expect("baseline should solve") - 0.5).abs() < 1e-6);
    }
}
