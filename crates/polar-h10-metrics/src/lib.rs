//! Modular, platform-neutral processors for derived Polar H10 metrics.
//!
//! The processors accept decoded sensor samples and never depend on Bluetooth,
//! Tauri, LSL, OSC, or the HTML UI. This keeps computation testable and makes
//! the crate reusable in headless applications.

mod breathing;
mod breathing_dynamics;
mod catalog;
mod coherence;
mod ecg;
mod excitation;
mod hrv;

use polar_h10_core::AccSample;
use serde::Serialize;

pub use breathing::{BreathingPhase, BreathingSnapshot};
pub use breathing_dynamics::{BreathingDynamicsSnapshot, FeatureSet};
pub use catalog::{METRIC_CATALOG, MetricDefinition, metric_definition};
pub use coherence::CoherenceSnapshot;
pub use ecg::EcgSnapshot;
pub use hrv::HrvSnapshot;

use breathing::BreathingProcessor;
use breathing_dynamics::BreathingDynamicsProcessor;
use coherence::CoherenceProcessor;
use ecg::EcgProcessor;
use excitation::{ExcitationProcessor, ExcitementScoreProcessor};
use hrv::HrvProcessor;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricSample {
    pub id: &'static str,
    pub value: f32,
}

/// Owns all stateful processors for one connected sensor.
#[derive(Default)]
pub struct MetricEngine {
    hrv: HrvProcessor,
    coherence: CoherenceProcessor,
    breathing: BreathingProcessor,
    breathing_dynamics: BreathingDynamicsProcessor,
    ecg: EcgProcessor,
    excitation: ExcitationProcessor,
    excitement_score: ExcitementScoreProcessor,
}

impl MetricEngine {
    pub fn process_heart_rate(&mut self, bpm: u16, rr_intervals_ms: &[f32]) -> Vec<MetricSample> {
        let mut output = vec![MetricSample {
            id: "heart_rate",
            value: f32::from(bpm),
        }];

        for &rr in rr_intervals_ms {
            if !is_valid_rr(rr) {
                continue;
            }
            output.push(MetricSample {
                id: "rr_interval",
                value: rr,
            });

            if let Some(score) = self.excitement_score.update(rr) {
                output.push(MetricSample {
                    id: "excitement_score",
                    value: score,
                });
            }

            if let Some(hrv) = self.hrv.push(rr) {
                output.extend(hrv.samples());
                if let Some(excitation) = self.excitation.update(f32::from(bpm), hrv.ln_rmssd) {
                    output.push(MetricSample {
                        id: "excitometer",
                        value: excitation,
                    });
                }
            }
            if let Some(coherence) = self.coherence.push(rr) {
                output.extend(coherence.samples());
            }
        }
        output
    }

    pub fn process_ecg(&mut self, microvolts: &[i32]) -> Vec<MetricSample> {
        self.ecg
            .push(microvolts)
            .map(EcgSnapshot::samples)
            .unwrap_or_default()
    }

    pub fn process_accelerometer(&mut self, samples: &[AccSample]) -> Vec<MetricSample> {
        let mut output = samples
            .iter()
            .map(|sample| MetricSample {
                id: "acc_magnitude",
                value: sample.magnitude_g(),
            })
            .collect::<Vec<_>>();
        if let Some(breathing) = self.breathing.push(samples) {
            output.extend(breathing.samples());
            if let Some(dynamics) = self.breathing_dynamics.push(breathing) {
                output.extend(dynamics.samples());
            }
        }
        output
    }
}

fn is_valid_rr(value: f32) -> bool {
    (250.0..=2_500.0).contains(&value) && value.is_finite()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_are_unique_and_resolvable() {
        let mut ids = METRIC_CATALOG
            .iter()
            .map(|metric| metric.id)
            .collect::<Vec<_>>();
        let original = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), original);
        assert!(
            METRIC_CATALOG
                .iter()
                .all(|metric| metric_definition(metric.id).is_some())
        );
        for metric in METRIC_CATALOG {
            assert!(
                !metric.detail.trim().is_empty(),
                "{} lacks a definition",
                metric.id
            );
            assert!(
                metric.explainer.matches('.').count() >= 2,
                "{} needs a two-sentence scientific interpretation",
                metric.id
            );
            assert!(
                metric.citation_url.starts_with("https://"),
                "{} lacks a secure citation URL",
                metric.id
            );
            assert!(
                !metric.citation_label.trim().is_empty(),
                "{} lacks a citation",
                metric.id
            );
        }
    }

    #[test]
    fn engine_always_emits_device_heart_rate() {
        let mut engine = MetricEngine::default();
        let values = engine.process_heart_rate(72, &[833.0]);
        assert!(
            values
                .iter()
                .any(|value| value.id == "heart_rate" && value.value == 72.0)
        );
        assert!(values.iter().any(|value| value.id == "rr_interval"));
    }

    #[test]
    fn engine_emits_acceleration_magnitude_for_every_native_sample() {
        let mut engine = MetricEngine::default();
        let values = engine.process_accelerometer(&[
            AccSample {
                x_mg: 1_000,
                y_mg: 0,
                z_mg: 0,
            },
            AccSample {
                x_mg: 0,
                y_mg: 600,
                z_mg: 800,
            },
        ]);
        let magnitudes = values
            .iter()
            .filter(|value| value.id == "acc_magnitude")
            .map(|value| value.value)
            .collect::<Vec<_>>();
        assert_eq!(magnitudes, vec![1.0, 1.0]);
    }
}
