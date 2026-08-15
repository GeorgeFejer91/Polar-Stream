use std::collections::HashMap;

pub use polar_h10_math::{CustomFormulaConfig, FormulaSource};
pub use polar_h10_metrics::MetricDefinition as MetricSpec;
use polar_h10_metrics::{BreathingSettings, METRIC_CATALOG};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputConfig {
    pub stream_name: String,
    pub lsl_enabled: bool,
    pub osc_enabled: bool,
    #[serde(default)]
    pub csv_enabled: bool,
    #[serde(default)]
    pub audio_enabled: bool,
    pub outputs: Vec<String>,
    #[serde(default)]
    pub metric_options: HashMap<String, MetricOutputOptions>,
    #[serde(default)]
    pub custom_formulas: Vec<CustomFormulaConfig>,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            stream_name: "Polar-H10".into(),
            lsl_enabled: false,
            osc_enabled: false,
            csv_enabled: false,
            audio_enabled: false,
            outputs: vec!["raw_ecg".into(), "raw_acc".into()],
            metric_options: HashMap::new(),
            custom_formulas: Vec::new(),
        }
    }
}

impl OutputConfig {
    /// Tolerant one-time migration for preferences produced by an older app.
    /// Size bounds still apply, but retired metric IDs may be discarded.
    pub fn migrated(self) -> Result<Self, String> {
        self.validate_collection_bounds()?;
        self.normalized()
    }

    /// Validates an untrusted renderer submission before normalizing it. The
    /// separate migration path below remains tolerant of retired metric IDs in
    /// preferences written by an older application version.
    pub fn validated(self) -> Result<Self, String> {
        self.validate_collection_bounds()?;
        if let Some(unknown) = self
            .outputs
            .iter()
            .find(|id| MetricSpec::for_id(id).is_none())
        {
            return Err(format!("Unknown output module: {unknown}"));
        }
        if let Some(orphaned) = self
            .metric_options
            .keys()
            .find(|id| !self.outputs.contains(id))
        {
            return Err(format!(
                "Output options were provided for an unselected module: {orphaned}"
            ));
        }
        self.normalized()
    }

    fn validate_collection_bounds(&self) -> Result<(), String> {
        if self.outputs.len() > METRIC_CATALOG.len()
            || self.metric_options.len() > METRIC_CATALOG.len()
            || self.custom_formulas.len() > polar_h10_math::MAX_FORMULAS
        {
            return Err(format!(
                "At most {} built-in outputs and {} custom formulas can be configured.",
                METRIC_CATALOG.len(),
                polar_h10_math::MAX_FORMULAS,
            ));
        }
        if self.outputs.iter().any(|id| id.len() > 64)
            || self.metric_options.keys().any(|id| id.len() > 64)
        {
            return Err("Output identifiers must be 64 bytes or fewer.".into());
        }
        Ok(())
    }

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
            if matches!(id.as_str(), "breathing_phase" | "acc_breathing_magnitude") {
                options.processing.breathing =
                    Some(options.processing.breathing.unwrap_or_default().clamped());
            } else {
                options.processing.breathing = None;
            }
        }
        let shared_breathing = ["breathing_phase", "acc_breathing_magnitude"]
            .iter()
            .find_map(|id| {
                self.metric_options
                    .get(*id)
                    .and_then(|options| options.processing.breathing)
            });
        if let Some(shared_breathing) = shared_breathing {
            for id in ["breathing_phase", "acc_breathing_magnitude"] {
                if let Some(options) = self.metric_options.get_mut(id) {
                    options.processing.breathing = Some(shared_breathing);
                }
            }
        }
        let mut formula_ids = std::collections::HashSet::new();
        let mut formula_names = std::collections::HashSet::new();
        for formula in &mut self.custom_formulas {
            if !formula_ids.insert(formula.id.clone()) {
                return Err("Custom formula IDs must be unique.".into());
            }
            if formula.enabled {
                *formula = formula
                    .clone()
                    .normalized()
                    .map_err(|error| error.to_string())?;
                let normalized_name = formula.name.to_ascii_lowercase();
                if !formula_names.insert(normalized_name) {
                    return Err("Enabled custom formula names must be unique.".into());
                }
                if METRIC_CATALOG.iter().any(|metric| {
                    metric
                        .stream_suffix
                        .eq_ignore_ascii_case(formula.name.as_str())
                }) {
                    return Err(format!(
                        "Custom formula name '{}' conflicts with a built-in output.",
                        formula.name
                    ));
                }
            } else if formula.expression.len() > polar_h10_math::MAX_EXPRESSION_BYTES
                || formula.name.len() > 256
                || formula.unit.len() > 128
            {
                return Err("Disabled formula draft exceeds storage limits.".into());
            }
        }
        Ok(self)
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
    #[serde(default, alias = "breathingPhase")]
    pub breathing: Option<BreathingSettings>,
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
    pub csv: String,
    pub audio: String,
    pub formulas: Vec<FormulaHealth>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormulaHealth {
    pub formula_id: String,
    pub state: polar_h10_math::FormulaRuntimeState,
    pub message: Option<String>,
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

/// Returns the canonical discoverable name for a validated custom formula.
pub fn custom_output_stream_name(base_name: &str, formula: &CustomFormulaConfig) -> String {
    format!("{base_name}_{}", formula.name)
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
    fn renderer_validation_rejects_unknown_outputs() {
        let config = OutputConfig {
            outputs: vec!["raw_ecg".into(), "not_a_metric".into()],
            ..OutputConfig::default()
        };
        assert_eq!(
            config.validated().unwrap_err(),
            "Unknown output module: not_a_metric"
        );
    }

    #[test]
    fn renderer_validation_rejects_orphaned_options() {
        let mut config = OutputConfig::default();
        config
            .metric_options
            .insert("rmssd".into(), MetricOutputOptions::default());
        assert_eq!(
            config.validated().unwrap_err(),
            "Output options were provided for an unselected module: rmssd"
        );
    }

    #[test]
    fn legacy_migration_drops_retired_outputs_within_bounds() {
        let config = OutputConfig {
            outputs: vec!["raw_ecg".into(), "retired_metric".into()],
            ..OutputConfig::default()
        }
        .migrated()
        .unwrap();
        assert_eq!(config.outputs, ["raw_ecg"]);
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
                    breathing: Some(BreathingSettings {
                        calibration_window_seconds: 0.1,
                        sensitivity: 9.0,
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
        let classifier = options.processing.breathing.unwrap();
        assert_eq!(options.display_window_seconds, 600);
        assert_eq!(classifier.calibration_window_seconds, 1.0);
        assert_eq!(classifier.sensitivity, 1.0);
    }

    #[test]
    fn breathing_outputs_share_one_processing_configuration() {
        let mut metric_options = HashMap::new();
        metric_options.insert(
            "breathing_phase".into(),
            MetricOutputOptions {
                processing: MetricProcessingOptions {
                    breathing: Some(BreathingSettings {
                        axes: [true, true, false],
                        sensitivity: 0.25,
                        ..BreathingSettings::default()
                    }),
                },
                ..MetricOutputOptions::default()
            },
        );
        metric_options.insert(
            "acc_breathing_magnitude".into(),
            MetricOutputOptions {
                processing: MetricProcessingOptions {
                    breathing: Some(BreathingSettings {
                        axes: [true, false, true],
                        sensitivity: 0.90,
                        ..BreathingSettings::default()
                    }),
                },
                ..MetricOutputOptions::default()
            },
        );
        let config = OutputConfig {
            outputs: vec!["breathing_phase".into(), "acc_breathing_magnitude".into()],
            metric_options,
            ..OutputConfig::default()
        }
        .normalized()
        .unwrap();
        let phase = config.metric_options["breathing_phase"]
            .processing
            .breathing
            .unwrap();
        let magnitude = config.metric_options["acc_breathing_magnitude"]
            .processing
            .breathing
            .unwrap();
        assert_eq!(phase, magnitude);
        assert_eq!(phase.axes, [true, true, false]);
        assert_eq!(phase.sensitivity, 0.25);
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
