use polar_h10_metrics::{
    BreathingSettings, BreathingStateMode, BreathingVolumeMode, MetricDefinition,
    VERNIER_BREATHING_CONTRACT,
};

use crate::OutputConfig;

pub(crate) const POLAR_RESPIRATION_PROCESSOR: &str = "polar-stream-acc-respiration";
pub(crate) const POLAR_RESPIRATION_SETTINGS_SCHEMA: &str = "breathing-settings-v1";
pub(crate) const APPLICATION_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProcessingMetadataField {
    pub(crate) name: &'static str,
    pub(crate) value: String,
}

impl ProcessingMetadataField {
    fn new(name: &'static str, value: impl Into<String>) -> Self {
        Self {
            name,
            value: value.into(),
        }
    }
}

/// Immutable provenance for the built-in Polar ACC respiration processor.
/// Values are clamped through the same settings boundary used by processing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PolarRespirationProvenance {
    settings: BreathingSettings,
}

impl PolarRespirationProvenance {
    pub(crate) fn new(settings: BreathingSettings) -> Self {
        Self {
            settings: settings.clamped(),
        }
    }

    pub(crate) fn configured(config: &OutputConfig) -> Option<Self> {
        config
            .outputs
            .iter()
            .filter_map(|id| MetricDefinition::for_id(id))
            .any(Self::applies_to)
            .then(|| Self::new(config.breathing_settings()))
    }

    pub(crate) fn applies_to(metric: MetricDefinition) -> bool {
        matches!(metric.category, "Breathing" | "Breathing dynamics")
    }

    pub(crate) fn fields(self) -> [ProcessingMetadataField; 22] {
        let settings = self.settings;
        [
            ProcessingMetadataField::new("algorithm", POLAR_RESPIRATION_PROCESSOR),
            ProcessingMetadataField::new("settings_schema", POLAR_RESPIRATION_SETTINGS_SCHEMA),
            ProcessingMetadataField::new("application_version", APPLICATION_VERSION),
            ProcessingMetadataField::new("volume_mode", volume_mode(settings.volume_mode)),
            ProcessingMetadataField::new("state_mode", state_mode(settings.state_mode)),
            ProcessingMetadataField::new("axes", enabled_axes(settings.axes)),
            ProcessingMetadataField::new(
                "calibration_window_seconds",
                settings.calibration_window_seconds.to_string(),
            ),
            ProcessingMetadataField::new(
                "minimum_axis_range_g",
                settings.minimum_axis_range_g.to_string(),
            ),
            ProcessingMetadataField::new(
                "smoothing_window_seconds",
                settings.smoothing_window_seconds.to_string(),
            ),
            ProcessingMetadataField::new("sensitivity", settings.sensitivity.to_string()),
            ProcessingMetadataField::new(
                "stale_timeout_seconds",
                settings.stale_timeout_seconds.to_string(),
            ),
            ProcessingMetadataField::new("invert_direction", settings.invert_direction.to_string()),
            ProcessingMetadataField::new("adaptive_bounds", settings.adaptive_bounds.to_string()),
            ProcessingMetadataField::new(
                "adaptive_window_seconds",
                settings.adaptive_window_seconds.to_string(),
            ),
            ProcessingMetadataField::new("lower_quantile", settings.lower_quantile.to_string()),
            ProcessingMetadataField::new("upper_quantile", settings.upper_quantile.to_string()),
            ProcessingMetadataField::new(
                "volume_filter_tau_seconds",
                settings.volume_filter_tau_seconds.to_string(),
            ),
            ProcessingMetadataField::new(
                "phase_derivative_tau_seconds",
                settings.phase_derivative_tau_seconds.to_string(),
            ),
            ProcessingMetadataField::new(
                "phase_enter_threshold_per_second",
                settings.phase_enter_threshold_per_second.to_string(),
            ),
            ProcessingMetadataField::new(
                "phase_hold_threshold_per_second",
                settings.phase_hold_threshold_per_second.to_string(),
            ),
            ProcessingMetadataField::new(
                "phase_confirmation_seconds",
                settings.phase_confirmation_seconds.to_string(),
            ),
            ProcessingMetadataField::new(
                "phase_minimum_dwell_seconds",
                settings.phase_minimum_dwell_seconds.to_string(),
            ),
        ]
    }
}

/// Immutable provenance for the fixed Vernier force-to-breathing processor.
/// The values come from the same contract used by the processor itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VernierBreathingProvenance;

impl VernierBreathingProvenance {
    pub(crate) fn fields(self) -> [ProcessingMetadataField; 16] {
        let contract = VERNIER_BREATHING_CONTRACT;
        [
            ProcessingMetadataField::new("algorithm", contract.algorithm),
            ProcessingMetadataField::new("settings_schema", contract.settings_schema),
            ProcessingMetadataField::new("application_version", APPLICATION_VERSION),
            ProcessingMetadataField::new("input_signal", "GDX-RB Force"),
            ProcessingMetadataField::new("input_unit", "N"),
            ProcessingMetadataField::new("window_seconds", contract.window_seconds.to_string()),
            ProcessingMetadataField::new(
                "robust_bounds_minimum_samples",
                contract.robust_bounds_minimum_samples.to_string(),
            ),
            ProcessingMetadataField::new(
                "bounds_update_samples",
                contract.bounds_update_samples.to_string(),
            ),
            ProcessingMetadataField::new(
                "maximum_history_samples",
                contract.maximum_history_samples.to_string(),
            ),
            ProcessingMetadataField::new("lower_quantile", contract.lower_quantile.to_string()),
            ProcessingMetadataField::new("upper_quantile", contract.upper_quantile.to_string()),
            ProcessingMetadataField::new("warmup_bounds", contract.warmup_bounds),
            ProcessingMetadataField::new("nonfinite_policy", contract.nonfinite_policy),
            ProcessingMetadataField::new(
                "degenerate_range_value",
                contract.degenerate_range_value.to_string(),
            ),
            ProcessingMetadataField::new("inhale_direction", contract.inhale_direction),
            ProcessingMetadataField::new("output_range", contract.output_range),
        ]
    }
}

fn volume_mode(mode: BreathingVolumeMode) -> &'static str {
    match mode {
        BreathingVolumeMode::LegacyV0 => "legacy-v0",
        BreathingVolumeMode::TimedPcaV1 => "timed-pca-v1",
    }
}

fn state_mode(mode: BreathingStateMode) -> &'static str {
    match mode {
        BreathingStateMode::LegacyV0 => "legacy-v0",
        BreathingStateMode::HysteresisV1 => "hysteresis-v1",
    }
}

fn enabled_axes(axes: [bool; 3]) -> String {
    ["x", "y", "z"]
        .into_iter()
        .zip(axes)
        .filter_map(|(axis, enabled)| enabled.then_some(axis))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MetricOutputOptions, MetricProcessingOptions};
    use polar_h10_metrics::METRIC_CATALOG;
    use std::collections::HashMap;

    fn field<'a>(fields: &'a [ProcessingMetadataField], name: &str) -> &'a str {
        fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.as_str())
            .unwrap()
    }

    #[test]
    fn versioned_modes_and_complete_settings_have_stable_metadata_keys() {
        let timed = PolarRespirationProvenance::new(BreathingSettings::default()).fields();
        assert_eq!(field(&timed, "algorithm"), POLAR_RESPIRATION_PROCESSOR);
        assert_eq!(
            field(&timed, "settings_schema"),
            POLAR_RESPIRATION_SETTINGS_SCHEMA
        );
        assert_eq!(field(&timed, "application_version"), APPLICATION_VERSION);
        assert_eq!(field(&timed, "volume_mode"), "timed-pca-v1");
        assert_eq!(field(&timed, "state_mode"), "hysteresis-v1");
        assert_eq!(field(&timed, "axes"), "x,z");
        assert_eq!(
            timed.iter().map(|field| field.name).collect::<Vec<_>>(),
            [
                "algorithm",
                "settings_schema",
                "application_version",
                "volume_mode",
                "state_mode",
                "axes",
                "calibration_window_seconds",
                "minimum_axis_range_g",
                "smoothing_window_seconds",
                "sensitivity",
                "stale_timeout_seconds",
                "invert_direction",
                "adaptive_bounds",
                "adaptive_window_seconds",
                "lower_quantile",
                "upper_quantile",
                "volume_filter_tau_seconds",
                "phase_derivative_tau_seconds",
                "phase_enter_threshold_per_second",
                "phase_hold_threshold_per_second",
                "phase_confirmation_seconds",
                "phase_minimum_dwell_seconds",
            ]
        );

        let legacy = PolarRespirationProvenance::new(BreathingSettings {
            volume_mode: BreathingVolumeMode::LegacyV0,
            state_mode: BreathingStateMode::LegacyV0,
            ..BreathingSettings::default()
        })
        .fields();
        assert_eq!(field(&legacy, "volume_mode"), "legacy-v0");
        assert_eq!(field(&legacy, "state_mode"), "legacy-v0");
    }

    #[test]
    fn provenance_is_present_only_for_selected_builtin_polar_respiration() {
        let raw = OutputConfig {
            outputs: vec!["raw_acc".into()],
            ..OutputConfig::default()
        };
        assert!(PolarRespirationProvenance::configured(&raw).is_none());

        let mut metric_options = HashMap::new();
        metric_options.insert(
            "breathing_volume".into(),
            MetricOutputOptions {
                processing: MetricProcessingOptions {
                    breathing: Some(BreathingSettings {
                        volume_mode: BreathingVolumeMode::LegacyV0,
                        state_mode: BreathingStateMode::LegacyV0,
                        ..BreathingSettings::default()
                    }),
                },
                ..MetricOutputOptions::default()
            },
        );
        let configured = OutputConfig {
            outputs: vec!["breathing_volume".into()],
            metric_options,
            ..OutputConfig::default()
        };
        let fields = PolarRespirationProvenance::configured(&configured)
            .unwrap()
            .fields();
        assert_eq!(field(&fields, "volume_mode"), "legacy-v0");

        for metric in METRIC_CATALOG {
            assert_eq!(
                PolarRespirationProvenance::applies_to(*metric),
                matches!(metric.category, "Breathing" | "Breathing dynamics"),
                "{}",
                metric.id
            );
        }
    }

    #[test]
    fn vernier_breathing_provenance_is_versioned_and_complete() {
        let fields = VernierBreathingProvenance.fields();
        assert_eq!(
            field(&fields, "algorithm"),
            VERNIER_BREATHING_CONTRACT.algorithm
        );
        assert_eq!(
            field(&fields, "settings_schema"),
            "vernier-breathing-settings-v1"
        );
        assert_eq!(field(&fields, "application_version"), APPLICATION_VERSION);
        assert_eq!(field(&fields, "input_signal"), "GDX-RB Force");
        assert_eq!(field(&fields, "input_unit"), "N");
        assert_eq!(field(&fields, "window_seconds"), "30");
        assert_eq!(field(&fields, "lower_quantile"), "0.05");
        assert_eq!(field(&fields, "upper_quantile"), "0.95");
        assert_eq!(field(&fields, "inhale_direction"), "increasing-force");
        assert_eq!(field(&fields, "nonfinite_policy"), "hold-last-output");
        assert_eq!(field(&fields, "output_range"), "0,1");
        assert_eq!(
            fields.iter().map(|field| field.name).collect::<Vec<_>>(),
            [
                "algorithm",
                "settings_schema",
                "application_version",
                "input_signal",
                "input_unit",
                "window_seconds",
                "robust_bounds_minimum_samples",
                "bounds_update_samples",
                "maximum_history_samples",
                "lower_quantile",
                "upper_quantile",
                "warmup_bounds",
                "nonfinite_policy",
                "degenerate_range_value",
                "inhale_direction",
                "output_range",
            ]
        );
    }
}
