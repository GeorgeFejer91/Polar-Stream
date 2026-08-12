use std::collections::VecDeque;

use crate::MetricSample;

const WINDOW_SAMPLES: usize = 130 * 5;
const EMIT_EVERY: usize = 65;

#[derive(Clone, Copy, Debug)]
pub struct EcgSnapshot {
    pub mean_uv: f32,
    pub rms_uv: f32,
    pub peak_to_peak_uv: f32,
    pub standard_deviation_uv: f32,
}

impl EcgSnapshot {
    pub(crate) fn samples(self) -> Vec<MetricSample> {
        vec![
            sample("ecg_mean", self.mean_uv),
            sample("ecg_rms", self.rms_uv),
            sample("ecg_peak_to_peak", self.peak_to_peak_uv),
            sample("ecg_sd", self.standard_deviation_uv),
        ]
    }
}

#[derive(Default)]
pub(crate) struct EcgProcessor {
    samples: VecDeque<f32>,
    since_emit: usize,
}

impl EcgProcessor {
    pub(crate) fn push(&mut self, values: &[i32]) -> Option<EcgSnapshot> {
        self.samples
            .extend(values.iter().map(|value| *value as f32));
        while self.samples.len() > WINDOW_SAMPLES {
            self.samples.pop_front();
        }
        self.since_emit += values.len();
        if self.samples.len() < 130 || self.since_emit < EMIT_EVERY {
            return None;
        }
        self.since_emit = 0;
        let count = self.samples.len() as f64;
        let sum = self
            .samples
            .iter()
            .map(|value| f64::from(*value))
            .sum::<f64>();
        let mean = sum / count;
        let squared = self
            .samples
            .iter()
            .map(|value| f64::from(*value).powi(2))
            .sum::<f64>();
        let variance = self
            .samples
            .iter()
            .map(|value| (f64::from(*value) - mean).powi(2))
            .sum::<f64>()
            / (count - 1.0).max(1.0);
        let min = self.samples.iter().copied().fold(f32::INFINITY, f32::min);
        let max = self
            .samples
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        Some(EcgSnapshot {
            mean_uv: mean as f32,
            rms_uv: (squared / count).sqrt() as f32,
            peak_to_peak_uv: max - min,
            standard_deviation_uv: variance.sqrt() as f32,
        })
    }
}

fn sample(id: &'static str, value: f32) -> MetricSample {
    MetricSample { id, value }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_windowed_signal_features() {
        let mut processor = EcgProcessor::default();
        let values = (0..130)
            .map(|index| if index % 2 == 0 { -10 } else { 10 })
            .collect::<Vec<_>>();
        let snapshot = processor.push(&values).unwrap();
        assert_eq!(snapshot.mean_uv, 0.0);
        assert_eq!(snapshot.rms_uv, 10.0);
        assert_eq!(snapshot.peak_to_peak_uv, 20.0);
    }
}
