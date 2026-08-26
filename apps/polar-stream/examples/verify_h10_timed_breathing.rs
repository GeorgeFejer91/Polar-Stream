use std::{collections::BTreeSet, sync::Arc, time::Duration};

use polar_h10_input::{DeviceSummary, InputEvent, InputManager};
use polar_h10_metrics::{BreathingSettings, MetricEngine, MetricSelection, TimedAccBatch};
use serde_json::json;

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(90);
const REQUIRED_SOURCE_SECONDS: f64 = 30.0;
const ACC_SAMPLE_PERIOD_NS: u64 = 5_000_000;
// H10 ACC is configured nominally at 200 Hz. The device clock is independent
// of the host and a physical capture has shown approximately 4.94 ms spacing,
// so this verifies a bounded nominal cadence instead of an exact 5 ms integer
// boundary across every PMD notification.
const MIN_ACCEPTED_ACC_INTERVAL_NS: u64 = 4_500_000;
const MAX_ACCEPTED_ACC_INTERVAL_NS: u64 = 5_500_000;

#[derive(Default)]
struct Observation {
    connected: bool,
    acc_frames: u64,
    acc_samples: u64,
    first_acc_timestamp_ns: Option<u64>,
    last_acc_timestamp_ns: Option<u64>,
    batch_min: usize,
    batch_max: usize,
    each_axis_nonzero: [bool; 3],
    calibrated: bool,
    ready_frames: u64,
    confidence_max: f32,
    volume_min: Option<f32>,
    volume_max: Option<f32>,
    phases: BTreeSet<i8>,
    presentation_points: u64,
    presentation_order_errors: u64,
    presentation_spacing_errors: u64,
    presentation_spacing_delta_min_ns: Option<u64>,
    presentation_spacing_delta_max_ns: Option<u64>,
    presentation_spacing_max_abs_error_ns: u64,
    presentation_interval_count: u64,
    presentation_interval_sum_ns: u128,
    last_presentation_timestamp_ns: Option<u64>,
}

impl Observation {
    fn observe_batch(&mut self, timestamp_ns: u64, samples: &[polar_h10_core::AccSample]) {
        self.acc_frames = self.acc_frames.saturating_add(1);
        self.acc_samples = self
            .acc_samples
            .saturating_add(u64::try_from(samples.len()).unwrap_or(u64::MAX));
        self.first_acc_timestamp_ns.get_or_insert(timestamp_ns);
        self.last_acc_timestamp_ns = Some(timestamp_ns);
        self.batch_min = if self.batch_min == 0 {
            samples.len()
        } else {
            self.batch_min.min(samples.len())
        };
        self.batch_max = self.batch_max.max(samples.len());
        for sample in samples {
            self.each_axis_nonzero[0] |= sample.x_mg != 0;
            self.each_axis_nonzero[1] |= sample.y_mg != 0;
            self.each_axis_nonzero[2] |= sample.z_mg != 0;
        }
    }

    fn observe_metrics(&mut self, values: &[polar_h10_metrics::MetricSample]) {
        let mut ready = false;
        let mut phase = None;
        for value in values {
            match value.id {
                "breathing_calibration" => self.calibrated |= value.value >= 1.0,
                "breathing_signal_ready" => ready = value.value >= 1.0,
                "breathing_signal_confidence" => {
                    self.confidence_max = self.confidence_max.max(value.value)
                }
                "breathing_volume" => {
                    self.volume_min = Some(
                        self.volume_min
                            .map_or(value.value, |old| old.min(value.value)),
                    );
                    self.volume_max = Some(
                        self.volume_max
                            .map_or(value.value, |old| old.max(value.value)),
                    );
                }
                "breathing_phase" => phase = Some(value.value.round() as i8),
                _ => {}
            }
        }
        if ready {
            self.ready_frames = self.ready_frames.saturating_add(1);
            if let Some(phase) = phase {
                self.phases.insert(phase);
            }
        }
    }

    fn observe_presentation(&mut self, points: Vec<polar_h10_metrics::BreathingWaveformPoint>) {
        for point in points {
            if let Some(previous) = self.last_presentation_timestamp_ns {
                if point.source_timestamp_ns <= previous {
                    self.presentation_order_errors =
                        self.presentation_order_errors.saturating_add(1);
                } else {
                    let delta_ns = point.source_timestamp_ns - previous;
                    self.presentation_spacing_delta_min_ns = Some(
                        self.presentation_spacing_delta_min_ns
                            .map_or(delta_ns, |old| old.min(delta_ns)),
                    );
                    self.presentation_spacing_delta_max_ns = Some(
                        self.presentation_spacing_delta_max_ns
                            .map_or(delta_ns, |old| old.max(delta_ns)),
                    );
                    let error_ns = delta_ns.abs_diff(ACC_SAMPLE_PERIOD_NS);
                    self.presentation_spacing_max_abs_error_ns =
                        self.presentation_spacing_max_abs_error_ns.max(error_ns);
                    self.presentation_interval_count =
                        self.presentation_interval_count.saturating_add(1);
                    self.presentation_interval_sum_ns = self
                        .presentation_interval_sum_ns
                        .saturating_add(u128::from(delta_ns));
                    if !(MIN_ACCEPTED_ACC_INTERVAL_NS..=MAX_ACCEPTED_ACC_INTERVAL_NS)
                        .contains(&delta_ns)
                    {
                        self.presentation_spacing_errors =
                            self.presentation_spacing_errors.saturating_add(1);
                    }
                }
            }
            self.last_presentation_timestamp_ns = Some(point.source_timestamp_ns);
            self.presentation_points = self.presentation_points.saturating_add(1);
        }
    }

    fn source_seconds(&self) -> f64 {
        self.first_acc_timestamp_ns
            .zip(self.last_acc_timestamp_ns)
            .map(|(first, last)| last.saturating_sub(first) as f64 / 1e9)
            .unwrap_or(0.0)
    }

    fn volume_span(&self) -> f32 {
        self.volume_min
            .zip(self.volume_max)
            .map(|(minimum, maximum)| maximum - minimum)
            .unwrap_or(0.0)
    }

    fn presentation_effective_rate_hz(&self) -> Option<f64> {
        (self.presentation_interval_count > 0 && self.presentation_interval_sum_ns > 0).then(|| {
            self.presentation_interval_count as f64 * 1_000_000_000.0
                / self.presentation_interval_sum_ns as f64
        })
    }

    fn presentation_timing_evidence(&self) -> serde_json::Value {
        json!({
            "nominal_sample_period_ns": ACC_SAMPLE_PERIOD_NS,
            "accepted_interval_min_ns": MIN_ACCEPTED_ACC_INTERVAL_NS,
            "accepted_interval_max_ns": MAX_ACCEPTED_ACC_INTERVAL_NS,
            "interval_count": self.presentation_interval_count,
            "minimum_interval_ns": self.presentation_spacing_delta_min_ns,
            "maximum_interval_ns": self.presentation_spacing_delta_max_ns,
            "maximum_absolute_nominal_error_ns": self.presentation_spacing_max_abs_error_ns,
            "effective_rate_hz": self.presentation_effective_rate_hz(),
            "order_errors": self.presentation_order_errors,
            "spacing_errors": self.presentation_spacing_errors,
        })
    }

    fn partial_evidence(
        &self,
        diagnostics: polar_h10_metrics::BreathingDiagnostics,
    ) -> serde_json::Value {
        json!({
            "schema": "polar.stream.h10_timed_breathing_physical.v1",
            "partial": true,
            "acc": {
                "frames": self.acc_frames,
                "samples": self.acc_samples,
                "source_seconds": self.source_seconds(),
                "minimum_samples_per_frame": self.batch_min,
                "maximum_samples_per_frame": self.batch_max,
                "each_axis_nonzero": self.each_axis_nonzero.into_iter().all(|value| value),
            },
            "breathing": {
                "calibrated": self.calibrated,
                "ready_frames": self.ready_frames,
                "confidence_max": self.confidence_max,
                "volume_span": self.volume_span(),
                "phases_observed": self.phases,
                "presentation_points": self.presentation_points,
                "presentation_timing": self.presentation_timing_evidence(),
                "accepted_samples": diagnostics.accepted_samples,
                "late_samples_dropped": diagnostics.late_samples_dropped,
                "gap_count": diagnostics.gap_count,
                "clock_reset_count": diagnostics.clock_reset_count,
                "latest_effective_sample_period_ns": diagnostics.latest_effective_sample_period_ns,
                "latest_anchor_residual_ns": diagnostics.latest_anchor_residual_ns,
                "maximum_absolute_anchor_residual_ns": diagnostics.maximum_absolute_anchor_residual_ns,
                "interpolated_batch_count": diagnostics.interpolated_batch_count,
            },
            "output_transport_initialized": false,
            "physiological_acceptance_established": false,
        })
    }

    fn complete(&self) -> bool {
        self.connected
            && self.source_seconds() >= REQUIRED_SOURCE_SECONDS
            && self.calibrated
            && self.ready_frames >= 2
            && self.volume_span() >= 0.10
            && self.phases.contains(&-1)
            && self.phases.contains(&0)
            && self.phases.contains(&1)
            && self.presentation_points >= 1_000
    }
}

fn select_h10(devices: &[DeviceSummary]) -> Result<DeviceSummary, String> {
    let matches = devices
        .iter()
        .filter(|device| device.name.to_ascii_lowercase().contains("polar h10"))
        .cloned()
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [selected] => Ok(selected.clone()),
        [] => Err("timed-breathing verifier found no exact H10 candidate".into()),
        _ => Err(format!(
            "timed-breathing verifier requires one exact H10; observed {}",
            matches.len()
        )),
    }
}

async fn capture(manager: &Arc<InputManager>) -> Result<serde_json::Value, String> {
    let selected = select_h10(&manager.scan().await?)?;
    println!(
        "POLAR_H10_TIMED_BREATHING_SELECTED {}",
        json!({"exact_candidate_count": 1})
    );
    let mut events = manager.connect(&selected.id).await?;
    let selection = MetricSelection::from_ids([
        "acc_breathing_magnitude",
        "breathing_volume",
        "breathing_phase",
        "breathing_calibration",
        "breathing_axis_range",
        "breathing_signal_confidence",
        "breathing_signal_ready",
    ]);
    let mut engine = MetricEngine::with_selection(selection);
    engine.apply_breathing_settings(BreathingSettings::default());
    let mut observation = Observation::default();
    let deadline = tokio::time::Instant::now() + CAPTURE_TIMEOUT;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            println!(
                "POLAR_H10_TIMED_BREATHING_PARTIAL {}",
                observation.partial_evidence(engine.breathing_diagnostics())
            );
            return Err("timed-breathing verifier capture deadline elapsed".into());
        }
        let event = match tokio::time::timeout(remaining, events.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => return Err("timed-breathing verifier event stream closed".into()),
            Err(_) => {
                println!(
                    "POLAR_H10_TIMED_BREATHING_PARTIAL {}",
                    observation.partial_evidence(engine.breathing_diagnostics())
                );
                return Err("timed-breathing verifier capture timed out".into());
            }
        };
        match event {
            InputEvent::Status { phase, message } => println!(
                "POLAR_H10_TIMED_BREATHING_STATUS {}",
                json!({"phase": phase, "message": message})
            ),
            InputEvent::Connected { .. } => observation.connected = true,
            InputEvent::Accelerometer {
                sensor_timestamp_ns,
                samples,
                ..
            } => {
                observation.observe_batch(sensor_timestamp_ns, &samples);
                let values = engine.process_accelerometer_timed(
                    &samples,
                    TimedAccBatch {
                        newest_sensor_timestamp_ns: sensor_timestamp_ns,
                        sample_period_ns: ACC_SAMPLE_PERIOD_NS,
                        clock_revision: 0,
                        clock_reset: false,
                        gap_before: false,
                    },
                );
                observation.observe_metrics(&values);
                observation.observe_presentation(engine.take_breathing_presentation_points());
            }
            InputEvent::Ecg { .. } | InputEvent::HeartRate { .. } => {}
            InputEvent::Error(error) => {
                println!(
                    "POLAR_H10_TIMED_BREATHING_PARTIAL {}",
                    observation.partial_evidence(engine.breathing_diagnostics())
                );
                return Err(format!(
                    "timed-breathing verifier received an input error: {error}"
                ));
            }
            InputEvent::Disconnected { .. } => {
                println!(
                    "POLAR_H10_TIMED_BREATHING_PARTIAL {}",
                    observation.partial_evidence(engine.breathing_diagnostics())
                );
                return Err("timed-breathing verifier disconnected before qualification".into());
            }
        }
        if observation.complete() {
            let diagnostics = engine.breathing_diagnostics();
            if diagnostics.accepted_samples != observation.acc_samples {
                println!(
                    "POLAR_H10_TIMED_BREATHING_PARTIAL {}",
                    observation.partial_evidence(diagnostics)
                );
                return Err("timed estimator did not accept every observed ACC sample".into());
            }
            if diagnostics.late_samples_dropped != 0
                || diagnostics.gap_count != 0
                || observation.presentation_order_errors != 0
                || observation.presentation_spacing_errors != 0
            {
                println!(
                    "POLAR_H10_TIMED_BREATHING_PARTIAL {}",
                    observation.partial_evidence(diagnostics)
                );
                return Err(format!(
                    "timed estimator reported source-order, source-gap, or cadence damage: {}",
                    observation.presentation_timing_evidence()
                ));
            }
            return Ok(json!({
                "schema": "polar.stream.h10_timed_breathing_physical.v1",
                "settings": {
                    "volume_mode": "timed-pca-v1",
                    "state_mode": "hysteresis-v1",
                    "adaptive_bounds": false,
                    "sample_period_ns": ACC_SAMPLE_PERIOD_NS,
                },
                "acc": {
                    "frames": observation.acc_frames,
                    "samples": observation.acc_samples,
                    "source_seconds": observation.source_seconds(),
                    "mean_samples_per_frame": observation.acc_samples as f64 / observation.acc_frames as f64,
                    "minimum_samples_per_frame": observation.batch_min,
                    "maximum_samples_per_frame": observation.batch_max,
                    "each_axis_nonzero": observation.each_axis_nonzero.into_iter().all(|value| value),
                },
                "breathing": {
                    "calibrated": observation.calibrated,
                    "ready_frames": observation.ready_frames,
                    "confidence_max": observation.confidence_max,
                    "volume_span": observation.volume_span(),
                    "phases_observed": observation.phases,
                    "presentation_points": observation.presentation_points,
                    "presentation_timing": observation.presentation_timing_evidence(),
                    "accepted_samples": diagnostics.accepted_samples,
                    "late_samples_dropped": diagnostics.late_samples_dropped,
                    "gap_count": diagnostics.gap_count,
                    "latest_effective_sample_period_ns": diagnostics.latest_effective_sample_period_ns,
                    "latest_anchor_residual_ns": diagnostics.latest_anchor_residual_ns,
                    "maximum_absolute_anchor_residual_ns": diagnostics.maximum_absolute_anchor_residual_ns,
                    "interpolated_batch_count": diagnostics.interpolated_batch_count,
                    "pca_dominance_01": diagnostics.pca_dominance_01,
                    "calibration_span_g": diagnostics.calibration_span_g,
                },
                "output_transport_initialized": false,
                "physiological_acceptance_established": false,
                "result": "pass",
            }));
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let manager = Arc::new(InputManager::new());
    let result = capture(&manager).await;
    let cleanup = manager.disconnect().await;
    match (result, cleanup) {
        (Ok(evidence), Ok(())) => {
            println!("POLAR_H10_TIMED_BREATHING_COMPLETE {evidence}");
            println!("POLAR_H10_TIMED_BREATHING_STOPPED");
            Ok(())
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(format!("timed-breathing cleanup failed: {error}")),
        (Err(error), Err(cleanup)) => Err(format!(
            "{error}; timed-breathing cleanup also failed: {cleanup}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_requires_both_directions_and_source_timed_points() {
        let mut observation = Observation {
            connected: true,
            first_acc_timestamp_ns: Some(1),
            last_acc_timestamp_ns: Some(30_000_000_001),
            calibrated: true,
            ready_frames: 2,
            volume_min: Some(0.2),
            volume_max: Some(0.8),
            presentation_points: 1_000,
            ..Observation::default()
        };
        observation.phases.extend([-1, 0]);
        assert!(!observation.complete());
        observation.phases.insert(1);
        assert!(observation.complete());
    }

    #[test]
    fn presentation_timing_accepts_observed_nominal_200_hz_clock_drift() {
        let mut observation = Observation::default();
        observation.observe_presentation(vec![
            polar_h10_metrics::BreathingWaveformPoint {
                source_timestamp_ns: 1_000_000_000,
                volume_01: 0.5,
            },
            polar_h10_metrics::BreathingWaveformPoint {
                source_timestamp_ns: 1_004_940_000,
                volume_01: 0.5,
            },
            polar_h10_metrics::BreathingWaveformPoint {
                source_timestamp_ns: 1_009_880_000,
                volume_01: 0.5,
            },
        ]);

        assert_eq!(observation.presentation_order_errors, 0);
        assert_eq!(observation.presentation_spacing_errors, 0);
        assert_eq!(
            observation.presentation_spacing_delta_min_ns,
            Some(4_940_000)
        );
        assert_eq!(
            observation.presentation_spacing_delta_max_ns,
            Some(4_940_000)
        );
        assert!((observation.presentation_effective_rate_hz().unwrap() - 202.43).abs() < 0.01);
    }

    #[test]
    fn presentation_timing_rejects_out_of_band_and_non_monotonic_points() {
        let mut observation = Observation::default();
        observation.observe_presentation(vec![
            polar_h10_metrics::BreathingWaveformPoint {
                source_timestamp_ns: 1_000_000_000,
                volume_01: 0.5,
            },
            polar_h10_metrics::BreathingWaveformPoint {
                source_timestamp_ns: 1_004_400_000,
                volume_01: 0.5,
            },
            polar_h10_metrics::BreathingWaveformPoint {
                source_timestamp_ns: 1_004_400_000,
                volume_01: 0.5,
            },
            polar_h10_metrics::BreathingWaveformPoint {
                source_timestamp_ns: 1_010_600_000,
                volume_01: 0.5,
            },
        ]);

        assert_eq!(observation.presentation_spacing_errors, 2);
        assert_eq!(observation.presentation_order_errors, 1);
        assert_eq!(observation.presentation_interval_count, 2);
        assert_eq!(
            observation.presentation_spacing_delta_min_ns,
            Some(4_400_000)
        );
        assert_eq!(
            observation.presentation_spacing_delta_max_ns,
            Some(6_200_000)
        );
    }
}
