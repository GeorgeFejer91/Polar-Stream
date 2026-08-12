use std::collections::HashMap;

use polar_h10_metrics::BreathingSettings;
pub use polar_h10_metrics::MetricDefinition as MetricSpec;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputConfig {
    pub stream_name: String,
    pub lsl_enabled: bool,
    pub osc_enabled: bool,
    pub outputs: Vec<String>,
    #[serde(default)]
    pub metric_options: HashMap<String, MetricOutputOptions>,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            stream_name: "Polar-H10".into(),
            lsl_enabled: false,
            osc_enabled: false,
            outputs: vec!["raw_ecg".into(), "raw_acc".into()],
            metric_options: HashMap::new(),
        }
    }
}

impl OutputConfig {
    pub fn normalized(mut self) -> Result<Self, String> {
        self.stream_name = normalize_stream_base(&self.stream_name)?;
        self.outputs.sort();
        self.outputs.dedup();
        self.outputs.retain(|id| MetricSpec::for_id(id).is_some());
        self.metric_options
            .retain(|id, _| self.outputs.contains(id));
        for (id, options) in &mut self.metric_options {
            options.window_seconds = options.window_seconds.clamp(5, 3_600);
            options.display_window_seconds = options.display_window_seconds.clamp(1, 600);
            if !MetricSpec::for_id(id).is_some_and(|metric| metric.normalizable) {
                options.normalization = NormalizationMode::None;
            }
            if id == "breathing_phase" {
                options.processing.breathing_phase = Some(
                    options
                        .processing
                        .breathing_phase
                        .unwrap_or_default()
                        .clamped(),
                );
            } else {
                options.processing.breathing_phase = None;
            }
        }
        Ok(self)
    }

    pub(crate) fn includes(&self, id: &str) -> bool {
        self.outputs.iter().any(|candidate| candidate == id)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NormalizationMode {
    #[default]
    None,
    SlidingWindow,
    Session,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricOutputOptions {
    #[serde(default)]
    pub normalization: NormalizationMode,
    #[serde(default = "default_window_seconds")]
    pub window_seconds: u32,
    #[serde(default = "default_display_window_seconds")]
    pub display_window_seconds: u32,
    #[serde(default)]
    pub processing: MetricProcessingOptions,
}

impl Default for MetricOutputOptions {
    fn default() -> Self {
        Self {
            normalization: NormalizationMode::None,
            window_seconds: default_window_seconds(),
            display_window_seconds: default_display_window_seconds(),
            processing: MetricProcessingOptions::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricProcessingOptions {
    #[serde(default)]
    pub breathing_phase: Option<BreathingSettings>,
}

const fn default_window_seconds() -> u32 {
    60
}

const fn default_display_window_seconds() -> u32 {
    5
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputHealth {
    pub stream_name: String,
    pub lsl: String,
    pub osc: String,
}

/// Produces a protocol-safe base shared by LSL stream names and OSC paths.
pub fn normalize_stream_base(value: &str) -> Result<String, String> {
    let mut normalized = String::with_capacity(value.len());
    let mut separator_pending = false;

    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() {
            if separator_pending && !normalized.is_empty() && !normalized.ends_with(['_', '-']) {
                normalized.push('_');
            }
            normalized.push(character);
            separator_pending = false;
        } else if character == '-' {
            normalized.push(character);
            separator_pending = false;
        } else {
            separator_pending = true;
        }
    }

    let normalized = normalized.trim_matches(['_', '-']).to_string();
    if normalized.is_empty() {
        return Err("Stream name must contain at least one letter or number.".into());
    }
    if normalized.chars().count() > 64 {
        return Err("Stream name must be 64 characters or fewer.".into());
    }
    Ok(normalized)
}

/// Returns the exact discoverable LSL name and OSC path component for a metric.
pub fn output_stream_name(base_name: &str, metric_id: &str) -> Option<String> {
    MetricSpec::for_id(metric_id).map(|spec| format!("{base_name}_{}", spec.suffix()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_the_two_raw_streams() {
        assert_eq!(OutputConfig::default().outputs, ["raw_ecg", "raw_acc"]);
    }

    #[test]
    fn normalization_removes_unknowns_and_duplicates() {
        let config = OutputConfig {
            stream_name: "  Lab A ".into(),
            outputs: vec!["rmssd".into(), "unknown".into(), "rmssd".into()],
            ..OutputConfig::default()
        }
        .normalized()
        .unwrap();
        assert_eq!(config.stream_name, "Lab_A");
        assert_eq!(config.outputs, ["rmssd"]);
    }

    #[test]
    fn clamps_normalization_windows_and_drops_orphaned_options() {
        let mut metric_options = HashMap::new();
        metric_options.insert(
            "rmssd".into(),
            MetricOutputOptions {
                normalization: NormalizationMode::SlidingWindow,
                window_seconds: 2,
                ..MetricOutputOptions::default()
            },
        );
        metric_options.insert("not_selected".into(), MetricOutputOptions::default());
        let config = OutputConfig {
            outputs: vec!["rmssd".into()],
            metric_options,
            ..OutputConfig::default()
        }
        .normalized()
        .unwrap();
        assert_eq!(config.metric_options["rmssd"].window_seconds, 5);
        assert_eq!(config.metric_options["rmssd"].display_window_seconds, 5);
        assert!(!config.metric_options.contains_key("not_selected"));
    }

    #[test]
    fn clamps_and_scopes_breathing_classifier_settings() {
        let mut metric_options = HashMap::new();
        metric_options.insert(
            "breathing_phase".into(),
            MetricOutputOptions {
                display_window_seconds: 900,
                processing: MetricProcessingOptions {
                    breathing_phase: Some(BreathingSettings {
                        calibration_window_seconds: 0.1,
                        phase_delta_threshold: 9.0,
                        ..BreathingSettings::default()
                    }),
                },
                ..MetricOutputOptions::default()
            },
        );
        let config = OutputConfig {
            outputs: vec!["breathing_phase".into()],
            metric_options,
            ..OutputConfig::default()
        }
        .normalized()
        .unwrap();
        let options = config.metric_options["breathing_phase"];
        let classifier = options.processing.breathing_phase.unwrap();
        assert_eq!(options.display_window_seconds, 600);
        assert_eq!(classifier.calibration_window_seconds, 1.0);
        assert_eq!(classifier.phase_delta_threshold, 0.25);
    }

    #[test]
    fn replaces_characters_that_cannot_cross_native_protocols() {
        let config = OutputConfig {
            stream_name: "bad\0name".into(),
            ..OutputConfig::default()
        }
        .normalized()
        .unwrap();
        assert_eq!(config.stream_name, "bad_name");
    }

    #[test]
    fn generates_the_same_exact_discoverable_names_for_every_protocol() {
        let base = normalize_stream_base(" participant 07 ").unwrap();
        assert_eq!(base, "participant_07");
        assert_eq!(
            output_stream_name(&base, "raw_ecg").as_deref(),
            Some("participant_07_rawECG")
        );
        assert_eq!(
            output_stream_name(&base, "raw_acc").as_deref(),
            Some("participant_07_rawACC")
        );
        assert_eq!(
            output_stream_name(&base, "heart_rate").as_deref(),
            Some("participant_07_heartRate")
        );
        assert_eq!(
            output_stream_name(&base, "excitement_score").as_deref(),
            Some("participant_07_excitementScore")
        );
    }
}
