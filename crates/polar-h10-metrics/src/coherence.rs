use std::{collections::VecDeque, f64::consts::PI};

use crate::MetricSample;

const FFT_LENGTH: usize = 128;
const WINDOW_MS: f64 = 64_000.0;
const MINIMUM_SAMPLES: usize = 20;
const STABILIZATION_SAMPLES: usize = 5;

#[derive(Clone, Copy, Debug)]
pub struct CoherenceSnapshot {
    pub normalized_01: f32,
    pub confidence_01: f32,
    pub peak_frequency_hz: f32,
    pub peak_band_power: f32,
    pub total_power: f32,
    pub heartmath_ratio: f32,
}

impl CoherenceSnapshot {
    pub(crate) fn samples(self) -> Vec<MetricSample> {
        vec![
            sample("coherence", self.normalized_01),
            sample("coherence_confidence", self.confidence_01),
            sample("heartmath_coherence", self.heartmath_ratio),
            sample("coherence_peak_frequency", self.peak_frequency_hz),
            sample("coherence_peak_power", self.peak_band_power),
            sample("coherence_total_power", self.total_power),
        ]
    }
}

pub(crate) struct CoherenceProcessor {
    samples: VecDeque<(f64, f32)>,
    elapsed_ms: f64,
    consecutive_valid: usize,
    hann: [f64; FFT_LENGTH],
}

impl Default for CoherenceProcessor {
    fn default() -> Self {
        let mut hann = [0.0; FFT_LENGTH];
        for (index, value) in hann.iter_mut().enumerate() {
            *value = 0.5 * (1.0 - (2.0 * PI * index as f64 / (FFT_LENGTH - 1) as f64).cos());
        }
        Self {
            samples: VecDeque::new(),
            elapsed_ms: 0.0,
            consecutive_valid: 0,
            hann,
        }
    }
}

impl CoherenceProcessor {
    pub(crate) fn push(&mut self, rr_ms: f32) -> Option<CoherenceSnapshot> {
        self.consecutive_valid += 1;
        if self.consecutive_valid < STABILIZATION_SAMPLES {
            return None;
        }
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
        if self.samples.len() < MINIMUM_SAMPLES {
            return None;
        }
        let span = self.samples.back()?.0 - self.samples.front()?.0;
        if span < WINDOW_MS * 0.99 {
            return None;
        }
        self.solve(span)
    }

    fn solve(&self, span_ms: f64) -> Option<CoherenceSnapshot> {
        let start = self.elapsed_ms - WINDOW_MS;
        let step_ms = WINDOW_MS / FFT_LENGTH as f64;
        let mut resampled = [0.0; FFT_LENGTH];
        let points = self.samples.iter().copied().collect::<Vec<_>>();
        let mut cursor = 0;
        for (index, value) in resampled.iter_mut().enumerate() {
            let time = start + (index + 1) as f64 * step_ms;
            while cursor + 1 < points.len() && points[cursor + 1].0 < time {
                cursor += 1;
            }
            let left = points[cursor];
            let right = points.get(cursor + 1).copied().unwrap_or(left);
            let t = if right.0 > left.0 {
                ((time - left.0) / (right.0 - left.0)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            *value = f64::from(left.1) + (f64::from(right.1) - f64::from(left.1)) * t;
        }

        let mean = resampled.iter().sum::<f64>() / FFT_LENGTH as f64;
        for (index, value) in resampled.iter_mut().enumerate() {
            *value = (*value - mean) * self.hann[index];
        }
        let sample_rate_hz = 1_000.0 / step_ms;
        let spectrum = power_spectrum(&resampled, sample_rate_hz);
        let peak = spectrum
            .iter()
            .filter(|(frequency, _)| (0.04..=0.26).contains(frequency))
            .max_by(|left, right| left.1.total_cmp(&right.1))?;
        let peak_frequency = peak.0;
        let peak_power = integrate(&spectrum, peak_frequency - 0.015, peak_frequency + 0.015);
        let total_power = integrate(&spectrum, 0.0033, 0.4);
        let remaining = total_power - peak_power;
        if total_power <= 0.0 || remaining <= 1e-9 {
            return None;
        }
        let coverage = (span_ms / WINDOW_MS).clamp(0.0, 1.0) as f32;
        let sample_confidence =
            ((self.samples.len() - MINIMUM_SAMPLES) as f32 / 32.0).clamp(0.0, 1.0);
        Some(CoherenceSnapshot {
            normalized_01: (peak_power / total_power).clamp(0.0, 1.0) as f32,
            confidence_01: sample_confidence * 0.55 + coverage * 0.45,
            peak_frequency_hz: peak_frequency as f32,
            peak_band_power: peak_power.min(f32::MAX as f64) as f32,
            total_power: total_power.min(f32::MAX as f64) as f32,
            heartmath_ratio: (peak_power / remaining).powi(2).min(f32::MAX as f64) as f32,
        })
    }
}

fn power_spectrum(values: &[f64; FFT_LENGTH], sample_rate_hz: f64) -> Vec<(f64, f64)> {
    (0..=(FFT_LENGTH / 2))
        .map(|bin| {
            let (mut real, mut imaginary) = (0.0, 0.0);
            for (index, value) in values.iter().enumerate() {
                let angle = -2.0 * PI * bin as f64 * index as f64 / FFT_LENGTH as f64;
                real += value * angle.cos();
                imaginary += value * angle.sin();
            }
            (
                bin as f64 * sample_rate_hz / FFT_LENGTH as f64,
                real * real + imaginary * imaginary,
            )
        })
        .collect()
}

fn integrate(spectrum: &[(f64, f64)], low: f64, high: f64) -> f64 {
    spectrum
        .windows(2)
        .filter_map(|pair| {
            let left = pair[0];
            let right = pair[1];
            let segment_low = left.0.max(low);
            let segment_high = right.0.min(high);
            if segment_high <= segment_low || right.0 <= left.0 {
                return None;
            }
            let interpolate = |frequency: f64| {
                left.1 + (right.1 - left.1) * (frequency - left.0) / (right.0 - left.0)
            };
            Some(
                (interpolate(segment_low) + interpolate(segment_high))
                    * 0.5
                    * (segment_high - segment_low),
            )
        })
        .sum()
}

fn sample(id: &'static str, value: f32) -> MetricSample {
    MetricSample { id, value }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_slow_rr_oscillation_near_point_one_hz() {
        let mut processor = CoherenceProcessor::default();
        let mut elapsed = 0.0_f64;
        let mut snapshot = None;
        while elapsed < 70.0 {
            let rr = 800.0 + 80.0 * (2.0 * PI * 0.1 * elapsed).sin();
            snapshot = processor.push(rr as f32).or(snapshot);
            elapsed += rr / 1_000.0;
        }
        let snapshot = snapshot.expect("full coherence window should solve");
        assert!((snapshot.peak_frequency_hz - 0.1).abs() < 0.025);
        assert!(snapshot.normalized_01 > 0.3);
    }
}
