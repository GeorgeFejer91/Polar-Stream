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

pub use breathing::{BreathingPhase, BreathingSettings, BreathingSnapshot};
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

/// Compact, copyable processing plan derived from the outputs a user selected.
/// Raw device signals do not activate any derived processor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetricSelection {
    bits: u64,
    ecg_features: bool,
    acc_magnitude: bool,
    breathing: bool,
    breathing_dynamics: bool,
    heart_rate: bool,
    rr_interval: bool,
    hrv: bool,
    coherence: bool,
    excitement_score: bool,
    excitometer: bool,
}

impl MetricSelection {
    pub fn from_ids<'a>(ids: impl IntoIterator<Item = &'a str>) -> Self {
        let mut selection = Self::none();
        for id in ids {
            if let Some(index) = METRIC_CATALOG.iter().position(|metric| metric.id == id) {
                selection.bits |= 1_u64 << index;
            }
            match id {
                "ecg_mean" | "ecg_rms" | "ecg_peak_to_peak" | "ecg_sd" => {
                    selection.ecg_features = true;
                }
                "acc_magnitude" => selection.acc_magnitude = true,
                "breathing_volume"
                | "breathing_phase"
                | "breathing_calibration"
                | "breathing_axis_range" => selection.breathing = true,
                "breathing_rate" | "breathing_dynamics_confidence" => {
                    selection.breathing = true;
                    selection.breathing_dynamics = true;
                }
                "heart_rate" => selection.heart_rate = true,
                "rr_interval" => selection.rr_interval = true,
                "mean_nn" | "mean_heart_rate" | "rmssd" | "ln_rmssd" | "sdnn" | "pnn50" | "sd1" => {
                    selection.hrv = true
                }
                "coherence"
                | "coherence_confidence"
                | "heartmath_coherence"
                | "coherence_peak_frequency"
                | "coherence_peak_power"
                | "coherence_total_power" => selection.coherence = true,
                "excitement_score" => selection.excitement_score = true,
                "excitometer" => {
                    selection.hrv = true;
                    selection.excitometer = true;
                }
                _ if id.starts_with("breath_interval_") || id.starts_with("breath_amplitude_") => {
                    selection.breathing = true;
                    selection.breathing_dynamics = true;
                }
                _ => {}
            }
        }
        selection
    }

    pub const fn none() -> Self {
        Self {
            bits: 0,
            ecg_features: false,
            acc_magnitude: false,
            breathing: false,
            breathing_dynamics: false,
            heart_rate: false,
            rr_interval: false,
            hrv: false,
            coherence: false,
            excitement_score: false,
            excitometer: false,
        }
    }

    fn all() -> Self {
        Self::from_ids(METRIC_CATALOG.iter().map(|metric| metric.id))
    }

    fn includes(self, id: &str) -> bool {
        METRIC_CATALOG
            .iter()
            .position(|metric| metric.id == id)
            .is_some_and(|index| self.bits & (1_u64 << index) != 0)
    }

    fn retain_selected(self, values: &mut Vec<MetricSample>) {
        values.retain(|sample| {
            self.includes(sample.id)
                // The phase visualizer uses the calibrated volume even when only
                // the classifier itself is exposed as an output.
                || (self.breathing
                    && matches!(
                        sample.id,
                        "breathing_volume"
                            | "breathing_phase"
                            | "breathing_calibration"
                            | "breathing_axis_range"
                    ))
        });
    }
}

impl Default for MetricSelection {
    fn default() -> Self {
        Self::all()
    }
}

/// Owns all stateful processors for one connected sensor.
pub struct MetricEngine {
    selection: MetricSelection,
    hrv: HrvProcessor,
    coherence: CoherenceProcessor,
    breathing: BreathingProcessor,
    breathing_dynamics: BreathingDynamicsProcessor,
    ecg: EcgProcessor,
    excitation: ExcitationProcessor,
    excitement_score: ExcitementScoreProcessor,
}

impl Default for MetricEngine {
    fn default() -> Self {
        Self::with_selection(MetricSelection::default())
    }
}

impl MetricEngine {
    pub fn with_selection(selection: MetricSelection) -> Self {
        Self {
            selection,
            hrv: HrvProcessor::default(),
            coherence: CoherenceProcessor::default(),
            breathing: BreathingProcessor::default(),
            breathing_dynamics: BreathingDynamicsProcessor::default(),
            ecg: EcgProcessor::default(),
            excitation: ExcitationProcessor::default(),
            excitement_score: ExcitementScoreProcessor::default(),
        }
    }

    /// Updates the processing plan without disturbing state for dependency
    /// groups that remain active. Newly activated groups start a clean window.
    pub fn apply_selection(&mut self, selection: MetricSelection) {
        if self.selection.ecg_features != selection.ecg_features {
            self.ecg = EcgProcessor::default();
        }
        if self.selection.breathing != selection.breathing {
            self.breathing = BreathingProcessor::default();
        }
        if self.selection.breathing_dynamics != selection.breathing_dynamics {
            self.breathing_dynamics = BreathingDynamicsProcessor::default();
        }
        if self.selection.hrv != selection.hrv {
            self.hrv = HrvProcessor::default();
            self.excitation = ExcitationProcessor::default();
        }
        if self.selection.coherence != selection.coherence {
            self.coherence = CoherenceProcessor::default();
        }
        if self.selection.excitement_score != selection.excitement_score {
            self.excitement_score = ExcitementScoreProcessor::default();
        }
        self.selection = selection;
    }

    /// Applies saved classifier controls. Tuning changes intentionally restart
    /// calibration, matching the original tracker and avoiding mixed settings.
    pub fn apply_breathing_settings(&mut self, settings: BreathingSettings) {
        self.breathing.apply_settings(settings);
        self.breathing_dynamics = BreathingDynamicsProcessor::default();
    }

    pub fn process_heart_rate(&mut self, bpm: u16, rr_intervals_ms: &[f32]) -> Vec<MetricSample> {
        let mut output = Vec::new();
        if self.selection.heart_rate {
            output.push(MetricSample {
                id: "heart_rate",
                value: f32::from(bpm),
            });
        }

        for &rr in rr_intervals_ms {
            if !is_valid_rr(rr) {
                continue;
            }
            if self.selection.rr_interval {
                output.push(MetricSample {
                    id: "rr_interval",
                    value: rr,
                });
            }

            if self.selection.excitement_score
                && let Some(score) = self.excitement_score.update(rr)
            {
                output.push(MetricSample {
                    id: "excitement_score",
                    value: score,
                });
            }

            if self.selection.hrv
                && let Some(hrv) = self.hrv.push(rr)
            {
                output.extend(hrv.samples());
                if self.selection.excitometer
                    && let Some(excitation) = self.excitation.update(f32::from(bpm), hrv.ln_rmssd)
                {
                    output.push(MetricSample {
                        id: "excitometer",
                        value: excitation,
                    });
                }
            }
            if self.selection.coherence
                && let Some(coherence) = self.coherence.push(rr)
            {
                output.extend(coherence.samples());
            }
        }
        self.selection.retain_selected(&mut output);
        output
    }

    pub fn process_ecg(&mut self, microvolts: &[i32]) -> Vec<MetricSample> {
        if !self.selection.ecg_features {
            return Vec::new();
        }
        self.ecg
            .push(microvolts)
            .map(|snapshot| {
                let mut values = snapshot.samples();
                self.selection.retain_selected(&mut values);
                values
            })
            .unwrap_or_default()
    }

    pub fn process_accelerometer(&mut self, samples: &[AccSample]) -> Vec<MetricSample> {
        let mut output = if self.selection.acc_magnitude {
            samples
                .iter()
                .map(|sample| MetricSample {
                    id: "acc_magnitude",
                    value: sample.magnitude_g(),
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if self.selection.breathing
            && let Some(breathing) = self.breathing.push(samples)
        {
            output.extend(breathing.samples());
            if self.selection.breathing_dynamics
                && let Some(dynamics) = self.breathing_dynamics.push(breathing)
            {
                output.extend(dynamics.samples());
            }
        }
        self.selection.retain_selected(&mut output);
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

    #[test]
    fn raw_only_selection_skips_every_derived_processor() {
        let mut engine =
            MetricEngine::with_selection(MetricSelection::from_ids(["raw_ecg", "raw_acc"]));
        assert!(engine.process_ecg(&[1, 2, 3]).is_empty());
        assert!(
            engine
                .process_accelerometer(&[AccSample {
                    x_mg: 1_000,
                    y_mg: 0,
                    z_mg: 0,
                }])
                .is_empty()
        );
        assert!(engine.process_heart_rate(72, &[833.0]).is_empty());
    }

    #[test]
    fn selection_emits_only_requested_metric_group_results() {
        let mut engine = MetricEngine::with_selection(MetricSelection::from_ids(["acc_magnitude"]));
        let values = engine.process_accelerometer(&[AccSample {
            x_mg: 1_000,
            y_mg: 0,
            z_mg: 0,
        }]);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].id, "acc_magnitude");
    }
}
