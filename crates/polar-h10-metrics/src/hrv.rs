use std::collections::VecDeque;

use crate::MetricSample;

const WINDOW_MS: f64 = 300_000.0;
const MINIMUM_SAMPLES: usize = 90;

#[derive(Clone, Copy, Debug)]
pub struct HrvSnapshot {
    pub rmssd_ms: f32,
    pub ln_rmssd: f32,
    pub sdnn_ms: f32,
    pub pnn50_percent: f32,
    pub sd1_ms: f32,
    pub mean_nn_ms: f32,
    pub mean_heart_rate_bpm: f32,
}

impl HrvSnapshot {
    pub(crate) fn samples(self) -> Vec<MetricSample> {
        vec![
            sample("rmssd", self.rmssd_ms),
            sample("ln_rmssd", self.ln_rmssd),
            sample("sdnn", self.sdnn_ms),
            sample("pnn50", self.pnn50_percent),
            sample("sd1", self.sd1_ms),
            sample("mean_nn", self.mean_nn_ms),
            sample("mean_heart_rate", self.mean_heart_rate_bpm),
        ]
    }
}

#[derive(Default)]
pub(crate) struct HrvProcessor {
    samples: VecDeque<(f64, f32)>,
    elapsed_ms: f64,
}

impl HrvProcessor {
    pub(crate) fn push(&mut self, rr_ms: f32) -> Option<HrvSnapshot> {
        self.samples.push_back((self.elapsed_ms, rr_ms));
        self.elapsed_ms += f64::from(rr_ms);
        while self.samples.len() > 1
            && self
                .samples
                .get(1)
                .is_some_and(|sample| sample.0 < self.elapsed_ms - WINDOW_MS)
        {
            self.samples.pop_front();
        }

        let coverage = self.samples.back()?.0 - self.samples.front()?.0;
        if self.samples.len() < MINIMUM_SAMPLES || coverage < WINDOW_MS * 0.99 {
            return None;
        }
        compute(self.samples.iter().map(|sample| sample.1))
    }
}

fn compute(values: impl Iterator<Item = f32>) -> Option<HrvSnapshot> {
    let values = values.collect::<Vec<_>>();
    if values.len() < 2 {
        return None;
    }
    let mean = values.iter().map(|value| f64::from(*value)).sum::<f64>() / values.len() as f64;
    if mean <= 0.0 {
        return None;
    }
    let variance = values
        .iter()
        .map(|value| (f64::from(*value) - mean).powi(2))
        .sum::<f64>()
        / (values.len() - 1) as f64;
    let differences = values
        .windows(2)
        .map(|pair| f64::from(pair[1] - pair[0]))
        .collect::<Vec<_>>();
    let rmssd = (differences.iter().map(|value| value.powi(2)).sum::<f64>()
        / differences.len() as f64)
        .sqrt();
    let pnn50 = differences
        .iter()
        .filter(|value| value.abs() > 50.0)
        .count() as f64
        * 100.0
        / differences.len() as f64;

    Some(HrvSnapshot {
        rmssd_ms: rmssd as f32,
        ln_rmssd: if rmssd > 0.0 { rmssd.ln() as f32 } else { 0.0 },
        sdnn_ms: variance.sqrt() as f32,
        pnn50_percent: pnn50 as f32,
        sd1_ms: (rmssd / 2.0_f64.sqrt()) as f32,
        mean_nn_ms: mean as f32,
        mean_heart_rate_bpm: (60_000.0 / mean).clamp(0.0, 260.0) as f32,
    })
}

fn sample(id: &'static str, value: f32) -> MetricSample {
    MetricSample { id, value }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_reference_time_domain_formulas_after_full_window() {
        let mut processor = HrvProcessor::default();
        let mut result = None;
        for index in 0..380 {
            result = processor.push(if index % 2 == 0 { 790.0 } else { 810.0 });
        }
        let result = result.expect("five-minute window should solve");
        assert!((result.mean_nn_ms - 800.0).abs() < 0.2);
        assert!((result.rmssd_ms - 20.0).abs() < 0.01);
        assert!((result.sd1_ms - 14.142).abs() < 0.02);
        assert_eq!(result.pnn50_percent, 0.0);
    }
}
