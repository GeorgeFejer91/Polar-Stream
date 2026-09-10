use std::collections::HashMap;

pub use polar_h10_math::{CustomFormulaConfig, FormulaSource};
pub use polar_h10_metrics::MetricDefinition as MetricSpec;
use polar_h10_metrics::{BreathingSettings, METRIC_CATALOG};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePaletteColors {
    /// Canonical source identity color.
    pub primary: String,
    /// Compatibility alias for the same source identity color.
    pub secondary: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePalette {
    pub id: String,
    pub light: SourcePaletteColors,
    pub dark: SourcePaletteColors,
}

const SOURCE_PALETTE_VALUES: [(&str, &str, &str, &str, &str); 8] = [
    ("ocean", "#1368AA", "#1368AA", "#67B7F7", "#67B7F7"),
    ("sunset", "#B43C4C", "#B43C4C", "#FF9292", "#FF9292"),
    ("meadow", "#18794E", "#18794E", "#62D394", "#62D394"),
    ("solar", "#806500", "#806500", "#E7C65C", "#E7C65C"),
    ("orchid", "#8246A3", "#8246A3", "#D3A0ED", "#D3A0ED"),
    ("lagoon", "#007A78", "#007A78", "#51D4CF", "#51D4CF"),
    ("ember", "#B94D00", "#B94D00", "#FF9A5C", "#FF9A5C"),
    ("iris", "#4F5EAD", "#4F5EAD", "#A8B1FF", "#A8B1FF"),
];

pub fn source_palette_catalog() -> Vec<SourcePalette> {
    SOURCE_PALETTE_VALUES
        .iter()
        .map(
            |(id, light_primary, light_secondary, dark_primary, dark_secondary)| SourcePalette {
                id: (*id).into(),
                light: SourcePaletteColors {
                    primary: (*light_primary).into(),
                    secondary: (*light_secondary).into(),
                },
                dark: SourcePaletteColors {
                    primary: (*dark_primary).into(),
                    secondary: (*dark_secondary).into(),
                },
            },
        )
        .collect()
}

pub fn source_palette(id: &str) -> Option<SourcePalette> {
    source_palette_catalog()
        .into_iter()
        .find(|palette| palette.id == id)
}

impl SourcePalette {
    fn validated(self) -> Result<Self, String> {
        source_palette(&self.id)
            .filter(|canonical| canonical == &self)
            .ok_or_else(|| format!("Unknown or modified source palette: {}", self.id))
    }

    pub(crate) fn metadata_fields(&self) -> [(&'static str, &str); 5] {
        [
            ("id", self.id.as_str()),
            ("light_primary", self.light.primary.as_str()),
            ("light_secondary", self.light.secondary.as_str()),
            ("dark_primary", self.dark.primary.as_str()),
            ("dark_secondary", self.dark.secondary.as_str()),
        ]
    }
}

const RELEASE_BREATHING_OUTPUT_IDS: [&str; 3] = [
    "breathing_volume",
    "breathing_signal_confidence",
    "breathing_signal_ready",
];

// Outside the complete release set, keep the historical lookup order so a
// restored compatibility-only configuration retains its saved processor.
const BREATHING_OUTPUT_IDS: [&str; 7] = [
    "breathing_phase",
    "acc_breathing_magnitude",
    "breathing_volume",
    "breathing_calibration",
    "breathing_axis_range",
    "breathing_signal_confidence",
    "breathing_signal_ready",
];

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
    #[serde(default)]
    pub source_palette: Option<SourcePalette>,
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
            source_palette: None,
            outputs: vec!["raw_ecg".into(), "raw_acc".into()],
            metric_options: HashMap::new(),
            custom_formulas: Vec::new(),
        }
    }
}

impl OutputConfig {
    fn has_complete_release_breathing_set(&self) -> bool {
        RELEASE_BREATHING_OUTPUT_IDS
            .iter()
            .all(|id| self.outputs.iter().any(|selected| selected == id))
    }

    fn preferred_breathing_settings(&self) -> Option<BreathingSettings> {
        let ids = if self.has_complete_release_breathing_set() {
            // A complete release set is one atomic contract. Do not let a
            // coexisting restored compatibility output silently select its
            // legacy processor instead.
            RELEASE_BREATHING_OUTPUT_IDS.as_slice()
        } else {
            BREATHING_OUTPUT_IDS.as_slice()
        };
        ids.iter().find_map(|id| {
            self.metric_options
                .get(*id)
                .and_then(|options| options.processing.breathing)
        })
    }

    fn preferred_breathing_presentation(&self) -> Option<BreathingPresentationSettings> {
        let ids = if self.has_complete_release_breathing_set() {
            RELEASE_BREATHING_OUTPUT_IDS.as_slice()
        } else {
            BREATHING_OUTPUT_IDS.as_slice()
        };
        ids.iter().find_map(|id| {
            self.metric_options
                .get(*id)
                .and_then(|options| options.presentation.breathing)
        })
    }

    /// Returns the one clamped processing configuration shared by every
    /// built-in Polar ACC respiration output.
    pub fn breathing_settings(&self) -> BreathingSettings {
        self.preferred_breathing_settings()
            .unwrap_or_default()
            .clamped()
    }

    /// Tolerant one-time migration for preferences produced by an older app.
    /// Size bounds still apply, but retired metric IDs may be discarded.
    pub fn migrated(mut self) -> Result<Self, String> {
        self.validate_collection_bounds()?;
        self.source_palette = self
            .source_palette
            .map(|palette| {
                source_palette(&palette.id)
                    .ok_or_else(|| format!("Unknown source palette: {}", palette.id))
            })
            .transpose()?;
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
        self.source_palette = self
            .source_palette
            .map(SourcePalette::validated)
            .transpose()?;
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
            if BREATHING_OUTPUT_IDS.contains(&id.as_str()) {
                options.processing.breathing =
                    Some(options.processing.breathing.unwrap_or_default().clamped());
                options.presentation.breathing =
                    Some(options.presentation.breathing.unwrap_or_default().clamped());
            } else {
                options.processing.breathing = None;
                options.presentation.breathing = None;
            }
        }
        let shared_breathing = self.preferred_breathing_settings();
        if let Some(shared_breathing) = shared_breathing {
            for id in BREATHING_OUTPUT_IDS {
                if let Some(options) = self.metric_options.get_mut(id) {
                    options.processing.breathing = Some(shared_breathing);
                }
            }
        }
        let shared_breathing_presentation = self.preferred_breathing_presentation();
        if let Some(shared_breathing_presentation) = shared_breathing_presentation {
            for id in BREATHING_OUTPUT_IDS {
                if let Some(options) = self.metric_options.get_mut(id) {
                    options.presentation.breathing = Some(shared_breathing_presentation);
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
    #[serde(default)]
    pub presentation: MetricPresentationOptions,
}

impl Default for MetricOutputOptions {
    fn default() -> Self {
        Self {
            normalization: NormalizationMode::None,
            window_seconds: default_window_seconds(),
            display_window_seconds: default_display_window_seconds(),
            processing: MetricProcessingOptions::default(),
            presentation: MetricPresentationOptions::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricProcessingOptions {
    #[serde(default, alias = "breathingPhase")]
    pub breathing: Option<BreathingSettings>,
}

/// Non-authoritative display controls. These values never change native
/// metric calculation or LSL/OSC/CSV publication.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricPresentationOptions {
    #[serde(default)]
    pub breathing: Option<BreathingPresentationSettings>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BreathingPresentationMode {
    #[default]
    FreshSmooth,
    TimestampFaithful,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BreathingPresentationSettings {
    pub mode: BreathingPresentationMode,
    pub smoothing_tau_seconds: f32,
    pub delay_seconds: f32,
}

impl Default for BreathingPresentationSettings {
    fn default() -> Self {
        Self {
            mode: BreathingPresentationMode::FreshSmooth,
            smoothing_tau_seconds: 0.12,
            delay_seconds: 0.18,
        }
    }
}

impl BreathingPresentationSettings {
    fn clamped(mut self) -> Self {
        self.smoothing_tau_seconds = finite_or(self.smoothing_tau_seconds, 0.12).clamp(0.01, 2.0);
        self.delay_seconds = finite_or(self.delay_seconds, 0.18).clamp(0.0, 1.0);
        self
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
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
    use polar_h10_metrics::{MetricSelectionTier, metric_selection_tier};

    fn relative_luminance(color: &str) -> f64 {
        let channel = |offset| {
            let value = u8::from_str_radix(&color[offset..offset + 2], 16).unwrap() as f64 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5)
    }

    fn contrast_ratio(foreground: &str, background: &str) -> f64 {
        let foreground = relative_luminance(foreground);
        let background = relative_luminance(background);
        (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05)
    }

    fn hue_degrees(color: &str) -> f64 {
        let channel =
            |offset| u8::from_str_radix(&color[offset..offset + 2], 16).unwrap() as f64 / 255.0;
        let red = channel(1);
        let green = channel(3);
        let blue = channel(5);
        let maximum = red.max(green).max(blue);
        let minimum = red.min(green).min(blue);
        let delta = maximum - minimum;
        if delta == 0.0 {
            return 0.0;
        }
        let hue = if maximum == red {
            60.0 * ((green - blue) / delta).rem_euclid(6.0)
        } else if maximum == green {
            60.0 * ((blue - red) / delta + 2.0)
        } else {
            60.0 * ((red - green) / delta + 4.0)
        };
        hue.rem_euclid(360.0)
    }

    #[test]
    fn source_palette_catalog_is_unique_and_canonical() {
        let palettes = source_palette_catalog();
        assert_eq!(palettes.len(), 8);
        let ids = palettes
            .iter()
            .map(|palette| palette.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), palettes.len());
        let light_primaries = palettes
            .iter()
            .map(|palette| palette.light.primary.as_str())
            .collect::<std::collections::HashSet<_>>();
        let light_secondaries = palettes
            .iter()
            .map(|palette| palette.light.secondary.as_str())
            .collect::<std::collections::HashSet<_>>();
        let dark_primaries = palettes
            .iter()
            .map(|palette| palette.dark.primary.as_str())
            .collect::<std::collections::HashSet<_>>();
        let dark_secondaries = palettes
            .iter()
            .map(|palette| palette.dark.secondary.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(light_primaries.len(), palettes.len());
        assert_eq!(light_secondaries.len(), palettes.len());
        assert_eq!(dark_primaries.len(), palettes.len());
        assert_eq!(dark_secondaries.len(), palettes.len());
        for palette in palettes {
            for color in [
                palette.light.primary,
                palette.light.secondary,
                palette.dark.primary,
                palette.dark.secondary,
            ] {
                assert_eq!(color.len(), 7);
                assert!(color.starts_with('#'));
                assert!(color[1..].bytes().all(|byte| byte.is_ascii_hexdigit()));
            }
        }
    }

    #[test]
    fn source_palettes_are_single_color_identities_with_distinct_hues() {
        let palettes = source_palette_catalog();
        for palette in source_palette_catalog() {
            assert_eq!(palette.light.primary, palette.light.secondary);
            assert_eq!(palette.dark.primary, palette.dark.secondary);
        }
        for (index, left) in palettes.iter().enumerate() {
            for right in palettes.iter().skip(index + 1) {
                let left_hue = hue_degrees(&left.light.primary);
                let right_hue = hue_degrees(&right.light.primary);
                let hue_distance = (left_hue - right_hue).abs();
                let circular_distance = hue_distance.min(360.0 - hue_distance);
                assert!(
                    circular_distance >= 19.0,
                    "{} and {} are not visually separated enough ({circular_distance} degrees)",
                    left.id,
                    right.id,
                );
            }
        }
    }

    #[test]
    fn source_palettes_remain_visible_on_their_theme_foundations() {
        for palette in source_palette_catalog() {
            assert!(contrast_ratio(&palette.light.primary, "#FFFFFF") >= 4.5);
            assert!(contrast_ratio(&palette.light.secondary, "#FFFFFF") >= 4.5);
            assert!(contrast_ratio(&palette.dark.primary, "#090B0A") >= 7.0);
            assert!(contrast_ratio(&palette.dark.secondary, "#090B0A") >= 7.0);
        }
    }

    #[test]
    fn source_palette_metadata_contract_is_complete_and_stable() {
        let palette = source_palette("ocean").unwrap();
        assert_eq!(
            palette.metadata_fields(),
            [
                ("id", "ocean"),
                ("light_primary", "#1368AA"),
                ("light_secondary", "#1368AA"),
                ("dark_primary", "#67B7F7"),
                ("dark_secondary", "#67B7F7"),
            ]
        );
    }

    #[test]
    fn breathing_display_names_change_without_changing_public_ids_or_suffixes() {
        let normalized = MetricSpec::for_id("breathing_volume").unwrap();
        assert_eq!(normalized.id, "breathing_volume");
        assert_eq!(normalized.suffix(), "breathingVolume");
        assert_eq!(normalized.label, "ACC breathing magnitude (0–1)");
        let projection = MetricSpec::for_id("acc_breathing_magnitude").unwrap();
        assert_eq!(projection.id, "acc_breathing_magnitude");
        assert_eq!(projection.suffix(), "accBreathingMagnitude");
        assert_eq!(projection.label, "ACC breathing projection (g)");
    }

    #[test]
    fn output_config_rejects_modified_palette_values() {
        let mut palette = source_palette("ocean").unwrap();
        palette.dark.primary = "#000000".into();
        let config = OutputConfig {
            source_palette: Some(palette),
            ..OutputConfig::default()
        };
        assert!(
            config
                .validated()
                .unwrap_err()
                .contains("modified source palette")
        );
    }

    #[test]
    fn defaults_to_polar_raw_streams_without_the_compatibility_force_outlet() {
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
    fn compatibility_metrics_survive_normalization_and_migration_with_exact_suffixes() {
        const EXPECTED: [(&str, &str); 22] = [
            ("acc_breathing_magnitude", "accBreathingMagnitude"),
            ("breathing_phase", "breathingPhase"),
            ("breathing_calibration", "breathingCalibration"),
            ("breathing_axis_range", "breathingAxisRange"),
            ("breathing_rate", "breathingRate"),
            (
                "breathing_dynamics_confidence",
                "breathingDynamicsConfidence",
            ),
            ("breath_interval_mean", "breathIntervalMean"),
            ("breath_interval_sd", "breathIntervalSD"),
            ("breath_interval_cv", "breathIntervalCV"),
            ("breath_interval_acw50", "breathIntervalACW50"),
            ("breath_interval_psd_slope", "breathIntervalPsdSlope"),
            ("breath_interval_lzc", "breathIntervalLZC"),
            ("breath_interval_sampen", "breathIntervalSampEn"),
            ("breath_interval_mse", "breathIntervalMSE"),
            ("breath_amplitude_mean", "breathAmplitudeMean"),
            ("breath_amplitude_sd", "breathAmplitudeSD"),
            ("breath_amplitude_cv", "breathAmplitudeCV"),
            ("breath_amplitude_acw50", "breathAmplitudeACW50"),
            ("breath_amplitude_psd_slope", "breathAmplitudePsdSlope"),
            ("breath_amplitude_lzc", "breathAmplitudeLZC"),
            ("breath_amplitude_sampen", "breathAmplitudeSampEn"),
            ("breath_amplitude_mse", "breathAmplitudeMSE"),
        ];

        let catalog_compatibility = METRIC_CATALOG
            .iter()
            .filter(|metric| metric_selection_tier(metric.id) == MetricSelectionTier::Compatibility)
            .map(|metric| (metric.id, metric.stream_suffix))
            .collect::<Vec<_>>();
        assert_eq!(catalog_compatibility, EXPECTED);

        let config = OutputConfig {
            outputs: EXPECTED.iter().map(|(id, _)| (*id).into()).collect(),
            ..OutputConfig::default()
        };
        let mut expected_ids = EXPECTED.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        expected_ids.sort_unstable();
        for preserved in [
            config.clone().normalized().unwrap(),
            config.migrated().unwrap(),
        ] {
            assert_eq!(
                preserved
                    .outputs
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                expected_ids
            );
            for (id, suffix) in EXPECTED {
                assert_eq!(MetricSpec::for_id(id).unwrap().suffix(), suffix);
                let expected_stream_name = format!("compatibility_{suffix}");
                assert_eq!(
                    output_stream_name("compatibility", id).as_deref(),
                    Some(expected_stream_name.as_str())
                );
            }
        }
    }

    #[test]
    fn notification_emitted_breathing_scalars_use_irregular_transport_rate() {
        for id in BREATHING_OUTPUT_IDS {
            let metric = MetricSpec::for_id(id).unwrap();
            assert_eq!(
                metric.rate_hz, 0.0,
                "{id} must not claim a regular LSL rate"
            );
        }
    }

    #[test]
    fn legacy_migration_canonicalizes_embedded_palette_by_stable_id() {
        let legacy_palette = SourcePalette {
            id: "ocean".into(),
            light: SourcePaletteColors {
                primary: "#176B9E".into(),
                secondary: "#2AA8B8".into(),
            },
            dark: SourcePaletteColors {
                primary: "#7CCBFF".into(),
                secondary: "#72E4EA".into(),
            },
        };
        let config = OutputConfig {
            source_palette: Some(legacy_palette),
            ..OutputConfig::default()
        }
        .migrated()
        .unwrap();

        assert_eq!(config.source_palette, source_palette("ocean"));
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
    fn submitted_and_legacy_configs_cannot_normalize_canonical_breathing_volume() {
        for normalization in [NormalizationMode::SlidingWindow, NormalizationMode::Session] {
            let mut metric_options = HashMap::new();
            metric_options.insert(
                "breathing_volume".into(),
                MetricOutputOptions {
                    normalization,
                    ..MetricOutputOptions::default()
                },
            );
            let config = OutputConfig {
                outputs: vec!["breathing_volume".into()],
                metric_options,
                ..OutputConfig::default()
            };

            for normalized in [config.clone().validated(), config.migrated()] {
                let normalized = normalized.unwrap();
                assert_eq!(
                    normalized.metric_options["breathing_volume"].normalization,
                    NormalizationMode::None
                );
            }
        }
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
    fn compatibility_breathing_outputs_keep_historical_shared_precedence() {
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
        let projection = config.metric_options["acc_breathing_magnitude"]
            .processing
            .breathing
            .unwrap();
        assert_eq!(phase, projection);
        assert_eq!(config.breathing_settings(), phase);
        assert_eq!(phase.axes, [true, true, false]);
        assert_eq!(phase.sensitivity, 0.25);
    }

    #[test]
    fn complete_current_release_set_cannot_be_overridden_by_selected_legacy_phase() {
        let legacy = BreathingSettings {
            volume_mode: polar_h10_metrics::BreathingVolumeMode::LegacyV0,
            state_mode: polar_h10_metrics::BreathingStateMode::LegacyV0,
            axes: [true, true, false],
            stale_timeout_seconds: 3.0,
            ..BreathingSettings::default()
        };
        let current = BreathingSettings {
            axes: [true, false, true],
            stale_timeout_seconds: 0.75,
            ..BreathingSettings::default()
        };
        let mut metric_options = HashMap::new();
        metric_options.insert(
            "breathing_phase".into(),
            MetricOutputOptions {
                processing: MetricProcessingOptions {
                    breathing: Some(legacy),
                },
                ..MetricOutputOptions::default()
            },
        );
        for id in RELEASE_BREATHING_OUTPUT_IDS {
            metric_options.insert(
                id.into(),
                MetricOutputOptions {
                    processing: MetricProcessingOptions {
                        breathing: Some(current),
                    },
                    ..MetricOutputOptions::default()
                },
            );
        }

        let config = OutputConfig {
            outputs: vec![
                "breathing_phase".into(),
                "breathing_volume".into(),
                "breathing_signal_confidence".into(),
                "breathing_signal_ready".into(),
            ],
            metric_options,
            ..OutputConfig::default()
        }
        .validated()
        .unwrap();

        assert!(config.outputs.iter().any(|id| id == "breathing_phase"));
        assert_eq!(config.breathing_settings(), current.clamped());
        for id in BREATHING_OUTPUT_IDS {
            if let Some(options) = config.metric_options.get(id) {
                assert_eq!(
                    options.processing.breathing,
                    Some(current.clamped()),
                    "{id}"
                );
            }
        }
        assert_eq!(
            config.breathing_settings().volume_mode,
            polar_h10_metrics::BreathingVolumeMode::TimedPcaV1
        );
        assert_eq!(
            config.breathing_settings().state_mode,
            polar_h10_metrics::BreathingStateMode::HysteresisV1
        );
    }

    #[test]
    fn breathing_outputs_share_bounded_non_authoritative_presentation() {
        let mut metric_options = HashMap::new();
        metric_options.insert(
            "breathing_phase".into(),
            MetricOutputOptions {
                presentation: MetricPresentationOptions {
                    breathing: Some(BreathingPresentationSettings {
                        mode: BreathingPresentationMode::TimestampFaithful,
                        smoothing_tau_seconds: f32::NAN,
                        delay_seconds: 4.0,
                    }),
                },
                ..MetricOutputOptions::default()
            },
        );
        metric_options.insert("breathing_volume".into(), MetricOutputOptions::default());
        let config = OutputConfig {
            outputs: vec!["breathing_phase".into(), "breathing_volume".into()],
            metric_options,
            ..OutputConfig::default()
        }
        .normalized()
        .unwrap();

        let phase = config.metric_options["breathing_phase"]
            .presentation
            .breathing
            .unwrap();
        let volume = config.metric_options["breathing_volume"]
            .presentation
            .breathing
            .unwrap();
        assert_eq!(phase, volume);
        assert_eq!(phase.mode, BreathingPresentationMode::TimestampFaithful);
        assert_eq!(phase.smoothing_tau_seconds, 0.12);
        assert_eq!(phase.delay_seconds, 1.0);
    }

    #[test]
    fn non_breathing_outputs_cannot_retain_breathing_presentation_settings() {
        let mut metric_options = HashMap::new();
        metric_options.insert(
            "rmssd".into(),
            MetricOutputOptions {
                presentation: MetricPresentationOptions {
                    breathing: Some(BreathingPresentationSettings::default()),
                },
                ..MetricOutputOptions::default()
            },
        );
        let config = OutputConfig {
            outputs: vec!["rmssd".into()],
            metric_options,
            ..OutputConfig::default()
        }
        .normalized()
        .unwrap();

        assert!(
            config.metric_options["rmssd"]
                .presentation
                .breathing
                .is_none()
        );
    }

    #[test]
    fn dynamics_only_configs_use_default_upstream_breathing_settings() {
        let mut metric_options = HashMap::new();
        metric_options.insert(
            "breath_interval_mean".into(),
            MetricOutputOptions {
                processing: MetricProcessingOptions {
                    breathing: Some(BreathingSettings {
                        volume_mode: polar_h10_metrics::BreathingVolumeMode::LegacyV0,
                        state_mode: polar_h10_metrics::BreathingStateMode::LegacyV0,
                        invert_direction: true,
                        ..BreathingSettings::default()
                    }),
                },
                ..MetricOutputOptions::default()
            },
        );
        let config = OutputConfig {
            outputs: vec!["breath_interval_mean".into()],
            metric_options,
            ..OutputConfig::default()
        }
        .validated()
        .unwrap();

        assert!(
            config.metric_options["breath_interval_mean"]
                .processing
                .breathing
                .is_none()
        );
        assert_eq!(config.breathing_settings(), BreathingSettings::default());
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
