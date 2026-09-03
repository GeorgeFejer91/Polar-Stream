//! Native output fan-out. Sensor traffic never takes a detour through the web UI.

mod config;
mod csv;
#[cfg(feature = "liblsl-backend")]
mod lsl;
mod osc;
mod provenance;
#[cfg(feature = "rusty-lsl-backend")]
mod rusty_lsl;

#[cfg(all(feature = "liblsl-backend", feature = "rusty-lsl-backend"))]
compile_error!("select exactly one LSL backend feature");
#[cfg(not(any(feature = "liblsl-backend", feature = "rusty-lsl-backend")))]
compile_error!("select either the liblsl-backend or rusty-lsl-backend feature");

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

pub use config::{
    BreathingPresentationMode, BreathingPresentationSettings, CustomFormulaConfig, FormulaHealth,
    FormulaSource, MetricOutputOptions, MetricPresentationOptions, MetricProcessingOptions,
    MetricSpec, NormalizationMode, OutputConfig, OutputHealth, SourcePalette, SourcePaletteColors,
    custom_output_stream_name, normalize_stream_base, output_stream_name, source_palette,
    source_palette_catalog,
};
use csv::CsvPublisher;
#[cfg(feature = "liblsl-backend")]
use lsl::LslPublisher;
use osc::{OSC_TARGET, OscPublisher};
use polar_h10_core::AccSample;
use polar_h10_math::{CompiledFormula, FormulaFrame, MAX_TOTAL_STATE_SAMPLES};
pub use polar_h10_math::{FormulaError, FormulaRuntimeState, FormulaValidation, validate_formula};
use polar_stream_time::SourceClockMapper;
use provenance::PolarRespirationProvenance;
#[cfg(feature = "rusty-lsl-backend")]
use rusty_lsl::RustyLslPublisher as LslPublisher;
use serde::Serialize;
use vernier_gdx_core::{SampleEncoding, SensorInfo, SensorSamples};

#[cfg(feature = "liblsl-backend")]
const VERNIER_RAW_OUTLET_KEY: &str = "__vernier_raw";
const VERNIER_BREATHING_OUTLET_KEY: &str = "__vernier_breathing";
pub const VERNIER_RAW_STREAM_SUFFIX: &str = "rawVernier";
pub const VERNIER_BREATHING_STREAM_SUFFIX: &str = "vernierBreathing";
pub const VERNIER_BREATHING_RECORDING_ID: &str = "vernier_breathing";
pub const VERNIER_RAW_DIAGNOSTIC_CHANNELS: usize = 7;

#[derive(Default)]
struct SensorClockMap {
    mapper: SourceClockMapper,
}

impl SensorClockMap {
    fn map_newest(&mut self, sensor_timestamp_ns: u64, local_now: f64) -> f64 {
        if sensor_timestamp_ns == 0 {
            return local_now;
        }
        let local_now_ns = if local_now.is_finite() && local_now > 0.0 {
            (local_now * 1_000_000_000.0)
                .round()
                .clamp(0.0, u64::MAX as f64) as u64
        } else {
            0
        };
        self.mapper
            .observe_and_map(sensor_timestamp_ns, local_now_ns)
            .mapped_time_ns as f64
            / 1_000_000_000.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MetricValue<'a> {
    pub id: &'a str,
    pub value: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VernierStreamSchema {
    model_code: String,
    sample_period_us: u32,
    channels: Vec<SensorInfo>,
}

impl VernierStreamSchema {
    pub fn new(
        model_code: impl Into<String>,
        sample_period_us: u32,
        sensors: &[SensorInfo],
    ) -> Result<Self, String> {
        if !(1_000..=60_000_000).contains(&sample_period_us) {
            return Err("Vernier stream period must be between 1 ms and 60 s.".into());
        }
        if sensors.is_empty() || sensors.len() > 32 {
            return Err("Vernier stream schema must contain 1-32 measurement channels.".into());
        }
        let mut channels = sensors.to_vec();
        channels.sort_by_key(|sensor| sensor.number);
        if channels
            .windows(2)
            .any(|pair| pair[0].number == pair[1].number)
        {
            return Err("Vernier stream sensor numbers must be unique.".into());
        }
        if channels.iter().any(|sensor| {
            sensor.number >= 32
                || !sensor.has_supported_measurement_shape()
                || !sensor.has_recording_identity()
                || sensor.description.is_empty()
                || sensor.unit.is_empty()
                || sensor.description.contains('\0')
                || sensor.unit.contains('\0')
        }) {
            return Err("Vernier stream labels and units must be nonempty valid text.".into());
        }
        if !channels.iter().any(SensorInfo::is_respiration_force) {
            return Err("Vernier stream schema requires periodic Force (N).".into());
        }
        let model_code = model_code.into();
        if model_code.is_empty() || model_code.contains('\0') {
            return Err("Vernier stream model code must be valid text.".into());
        }
        Ok(Self {
            model_code,
            sample_period_us,
            channels,
        })
    }

    pub fn model_code(&self) -> &str {
        &self.model_code
    }

    pub const fn sample_period_us(&self) -> u32 {
        self.sample_period_us
    }

    pub fn channels(&self) -> &[SensorInfo] {
        &self.channels
    }

    pub fn force_sensor_number(&self) -> Option<u8> {
        self.channels
            .iter()
            .find(|sensor| sensor.is_respiration_force())
            .map(|sensor| sensor.number)
    }

    pub fn raw_channel_count(&self) -> usize {
        self.channels.len() + VERNIER_RAW_DIAGNOSTIC_CHANNELS
    }
}

pub fn vernier_raw_stream_name(base_name: &str) -> String {
    format!("{base_name}_{VERNIER_RAW_STREAM_SUFFIX}")
}

pub fn vernier_breathing_stream_name(base_name: &str) -> String {
    format!("{base_name}_{VERNIER_BREATHING_STREAM_SUFFIX}")
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(feature = "liblsl-backend", test))]
fn encode_vernier_raw_rows(
    target: &mut Vec<f64>,
    schema: &VernierStreamSchema,
    host_receive_timestamp_ns: u64,
    sample_period_us: u32,
    sequence: u64,
    dropped_before: u64,
    device_drop_reports_before: u64,
    decode_latency_ns: u64,
    encoding: SampleEncoding,
    sensors: &[SensorSamples],
) -> usize {
    target.clear();
    if sensors.is_empty()
        || sensors.iter().any(|samples| {
            !schema
                .channels
                .iter()
                .any(|channel| channel.number == samples.sensor_number)
        })
        || sensors.iter().enumerate().any(|(index, samples)| {
            sensors[..index]
                .iter()
                .any(|prior| prior.sensor_number == samples.sensor_number)
        })
    {
        return 0;
    }
    let row_count = sensors
        .iter()
        .map(|samples| samples.values.len())
        .max()
        .unwrap_or(0);
    if row_count == 0 {
        return 0;
    }
    target.reserve(row_count.saturating_mul(schema.raw_channel_count()));
    let encoding_code = match encoding {
        SampleEncoding::Float32 => 0.0,
        SampleEncoding::Integer32 => 1.0,
    };
    for row in 0..row_count {
        for channel in &schema.channels {
            let value = sensors
                .iter()
                .find(|samples| samples.sensor_number == channel.number)
                .and_then(|samples| samples.values.get(row))
                .copied()
                .unwrap_or(f64::NAN);
            target.push(value);
        }
        target.extend([
            sequence.saturating_add(row as u64) as f64,
            if row == 0 { dropped_before as f64 } else { 0.0 },
            if row == 0 {
                device_drop_reports_before as f64
            } else {
                0.0
            },
            f64::from(sample_period_us),
            decode_latency_ns as f64,
            host_receive_timestamp_ns as f64,
            encoding_code,
        ]);
    }
    row_count
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormulaSeries {
    pub formula_id: String,
    pub values: Vec<f32>,
    pub state: FormulaRuntimeState,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormulaPublishBatch {
    pub series: Vec<FormulaSeries>,
    pub faults: Vec<FormulaError>,
    pub warnings: Vec<String>,
}

impl FormulaPublishBatch {
    fn append(&mut self, mut other: Self) {
        self.series.append(&mut other.series);
        self.faults.append(&mut other.faults);
        self.warnings.append(&mut other.warnings);
    }
}

pub struct OutputRouter {
    inner: Mutex<RouterInner>,
}

#[cfg(feature = "rusty-lsl-backend")]
pub struct RustyLslTwoSessionOutput {
    inner: Mutex<rusty_lsl::RustyLslPublisher>,
    slots: [RustyLslSessionSlot; 2],
}

#[cfg(feature = "rusty-lsl-backend")]
struct RustyLslSessionSlot {
    label: String,
    stream_base: String,
    ecg_key: String,
    acc_key: String,
}

#[cfg(feature = "rusty-lsl-backend")]
impl RustyLslTwoSessionOutput {
    /// Creates exactly two independent ECG/ACC outlet pairs behind one Rusty
    /// LSL discovery registry. The slot labels are local routing keys and are
    /// not derived from a Bluetooth identity.
    pub fn new(sessions: [(&str, &str); 2]) -> Result<Self, String> {
        let [(first_slot, first_base), (second_slot, second_base)] = sessions;
        validate_two_session_label(first_slot)?;
        validate_two_session_label(second_slot)?;
        if first_slot == second_slot {
            return Err("Rusty LSL two-session slot labels must be distinct.".into());
        }
        let first_base = normalize_stream_base(first_base)?;
        let second_base = normalize_stream_base(second_base)?;
        if first_base == second_base {
            return Err("Rusty LSL two-session stream bases must be distinct.".into());
        }

        let slots = [
            RustyLslSessionSlot::new(first_slot, first_base),
            RustyLslSessionSlot::new(second_slot, second_base),
        ];
        let ecg = MetricSpec::for_id("raw_ecg").expect("raw ECG metric must exist");
        let acc = MetricSpec::for_id("raw_acc").expect("raw ACC metric must exist");
        let mut publisher = rusty_lsl::RustyLslPublisher::new(None);
        for slot in &slots {
            publisher.try_add_outlet_with_key(&slot.stream_base, ecg, slot.ecg_key.clone())?;
            publisher.try_add_outlet_with_key(&slot.stream_base, acc, slot.acc_key.clone())?;
        }
        Ok(Self {
            inner: Mutex::new(publisher),
            slots,
        })
    }

    pub fn poll_lsl(&self) -> Option<String> {
        let Ok(mut publisher) = self.inner.lock() else {
            return Some("Rusty LSL two-session output lock failed".into());
        };
        publisher.poll()
    }

    pub fn health(&self) -> String {
        self.inner
            .lock()
            .map(|publisher| publisher.status().to_string())
            .unwrap_or_else(|_| "Rusty LSL two-session output lock failed".into())
    }

    pub fn connected_consumers(&self, slot: &str) -> Option<usize> {
        let route = self.slots.iter().find(|route| route.label == slot)?;
        self.inner
            .lock()
            .ok()?
            .connected_consumers_for(&[route.ecg_key.as_str(), route.acc_key.as_str()])
    }

    pub fn publish_ecg(
        &self,
        slot: &str,
        sensor_timestamp_ns: u64,
        samples: &[i32],
    ) -> Result<(), String> {
        let route = self
            .slots
            .iter()
            .find(|route| route.label == slot)
            .ok_or_else(|| "Unknown Rusty LSL two-session slot.".to_string())?;
        let mut publisher = self
            .inner
            .lock()
            .map_err(|_| "Rusty LSL two-session output lock failed".to_string())?;
        publisher.push_scalar_series_at_key(
            &route.ecg_key,
            samples.iter().map(|value| *value as f32),
            sensor_timestamp_ns,
        );
        Ok(())
    }

    pub fn publish_accelerometer(
        &self,
        slot: &str,
        sensor_timestamp_ns: u64,
        samples: &[AccSample],
    ) -> Result<(), String> {
        let route = self
            .slots
            .iter()
            .find(|route| route.label == slot)
            .ok_or_else(|| "Unknown Rusty LSL two-session slot.".to_string())?;
        let mut publisher = self
            .inner
            .lock()
            .map_err(|_| "Rusty LSL two-session output lock failed".to_string())?;
        publisher.push_accelerometer_at_key(&route.acc_key, samples, sensor_timestamp_ns);
        Ok(())
    }
}

#[cfg(feature = "rusty-lsl-backend")]
impl RustyLslSessionSlot {
    fn new(label: &str, stream_base: String) -> Self {
        Self {
            label: label.to_string(),
            ecg_key: format!("{stream_base}/{label}/raw_ecg"),
            acc_key: format!("{stream_base}/{label}/raw_acc"),
            stream_base,
        }
    }
}

#[cfg(feature = "rusty-lsl-backend")]
fn validate_two_session_label(label: &str) -> Result<(), String> {
    if label.is_empty()
        || label.len() > 32
        || !label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(
            "A Rusty LSL two-session slot must be 1-32 ASCII letters, digits, hyphens, or underscores."
                .into(),
        );
    }
    Ok(())
}

struct RouterInner {
    config: OutputConfig,
    osc: Option<OscPublisher>,
    lsl: LslPublisher,
    csv: Option<CsvPublisher>,
    csv_directory: PathBuf,
    normalizers: HashMap<String, Normalizer>,
    selected: HashSet<String>,
    formulas: HashMap<String, FormulaRuntime>,
    vernier_schema: Option<VernierStreamSchema>,
    #[cfg(test)]
    fail_lsl_build_after_outlets: Option<usize>,
}

enum StagedLsl {
    #[cfg(feature = "rusty-lsl-backend")]
    Keep,
    Disable,
    #[cfg(feature = "liblsl-backend")]
    Replace(Box<LslPublisher>),
}

struct FormulaRuntime {
    config: CustomFormulaConfig,
    compiled: CompiledFormula,
    state: FormulaRuntimeState,
    message: Option<String>,
}

impl Default for OutputRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputRouter {
    pub fn new() -> Self {
        Self::with_bundled_lsl(None)
    }

    pub fn with_bundled_lsl(library_path: Option<PathBuf>) -> Self {
        Self::with_bundled_lsl_and_recordings(
            library_path,
            std::env::temp_dir().join("Polar Stream recordings"),
        )
    }

    pub fn with_bundled_lsl_and_recordings(
        library_path: Option<PathBuf>,
        csv_directory: PathBuf,
    ) -> Self {
        Self {
            inner: Mutex::new(RouterInner {
                config: OutputConfig::default(),
                osc: None,
                lsl: LslPublisher::new(library_path),
                csv: None,
                csv_directory,
                normalizers: HashMap::new(),
                selected: OutputConfig::default().outputs.into_iter().collect(),
                formulas: HashMap::new(),
                vernier_schema: None,
                #[cfg(test)]
                fail_lsl_build_after_outlets: None,
            }),
        }
    }

    /// Validates renderer-owned configuration and formula state budgets without
    /// constructing any transport or recording endpoint.
    pub fn validate_config(config: OutputConfig) -> Result<OutputConfig, String> {
        let (config, _) = Self::validated_with_formulas(config)?;
        Ok(config)
    }

    fn validated_with_formulas(
        config: OutputConfig,
    ) -> Result<(OutputConfig, HashMap<String, CompiledFormula>), String> {
        let config = config.validated()?;
        let mut compiled = HashMap::new();
        let mut total_state_samples = 0usize;
        for formula in config
            .custom_formulas
            .iter()
            .filter(|formula| formula.enabled)
        {
            let runtime =
                CompiledFormula::compile(formula.clone()).map_err(|error| error.to_string())?;
            total_state_samples = total_state_samples
                .checked_add(runtime.state_samples())
                .ok_or("Custom formula state budget overflow")?;
            if total_state_samples > MAX_TOTAL_STATE_SAMPLES {
                return Err("Custom formulas exceed the aggregate DSP state budget.".into());
            }
            compiled.insert(formula.id.clone(), runtime);
        }
        Ok((config, compiled))
    }

    pub async fn configure(&self, config: OutputConfig) -> Result<OutputHealth, String> {
        let (config, mut compiled) = Self::validated_with_formulas(config)?;
        let respiration_provenance = PolarRespirationProvenance::configured(&config);
        let mut osc = if config.osc_enabled {
            Some(OscPublisher::connect(OSC_TARGET).await?)
        } else {
            None
        };
        if let Some(publisher) = &mut osc {
            publisher.configure(&config.stream_name, &config.outputs);
            publisher.configure_custom(&config.stream_name, &config.custom_formulas);
            publisher.configure_vernier_breathing(&config.stream_name);
        }

        let mut inner = self.inner.lock().map_err(|_| "Output router lock failed")?;
        #[cfg(feature = "rusty-lsl-backend")]
        if config.lsl_enabled && inner.vernier_schema.is_some() {
            return Err(
                "Aggregate Vernier LSL requires the packaged liblsl backend; the optional Rusty backend does not support this dynamic Double64 schema."
                    .into(),
            );
        }
        #[cfg(feature = "rusty-lsl-backend")]
        inner.ensure_lsl_reconfiguration_supported(&config, inner.vernier_schema.as_ref())?;
        #[cfg(feature = "liblsl-backend")]
        let staged_lsl = inner.build_lsl(&config, inner.vernier_schema.as_ref())?;
        let csv_to_install = if config.csv_enabled
            && inner.csv.as_ref().is_none_or(|csv| {
                !csv.records_header_configuration(
                    &config.stream_name,
                    config.source_palette.as_ref(),
                    respiration_provenance.as_ref(),
                )
            }) {
            let csv = CsvPublisher::start(
                &inner.csv_directory,
                &config.stream_name,
                config.source_palette.as_ref(),
                respiration_provenance.as_ref(),
            )?;
            if inner.vernier_schema.is_some() {
                csv.publish_vernier_breathing_provenance()?;
            }
            Some(csv)
        } else {
            None
        };
        // Rusty LSL owns one process-wide discovery registry. Stage other
        // fallible transports first, then populate that existing publisher on
        // first enable; active contract changes were rejected above.
        #[cfg(feature = "rusty-lsl-backend")]
        let staged_lsl = {
            let vernier_schema = inner.vernier_schema.clone();
            inner.build_lsl(&config, vernier_schema.as_ref())?
        };

        // Every operation below is an in-memory, infallible commit. Until this
        // point the live transports, configuration, and processor state remain
        // untouched if any candidate endpoint cannot be created.
        if !config.csv_enabled {
            inner.csv = None;
        } else if let Some(csv) = csv_to_install {
            inner.csv = Some(csv);
        }
        inner.reconcile_normalizers(&config);
        let mut previous = std::mem::take(&mut inner.formulas);
        let mut formulas = HashMap::new();
        for formula in config
            .custom_formulas
            .iter()
            .filter(|formula| formula.enabled)
        {
            let candidate = compiled
                .remove(&formula.id)
                .expect("validated enabled formulas are compiled before transport staging");
            let runtime = if let Some(mut existing) =
                previous.remove(&formula.id).filter(|existing| {
                    existing.config.source == formula.source
                        && existing.config.expression == formula.expression
                }) {
                existing.config = formula.clone();
                existing
            } else {
                FormulaRuntime {
                    config: formula.clone(),
                    compiled: candidate,
                    state: FormulaRuntimeState::Ready,
                    message: None,
                }
            };
            formulas.insert(formula.id.clone(), runtime);
        }
        inner.formulas = formulas;
        inner.selected = config.outputs.iter().cloned().collect();
        inner.config = config;
        inner.osc = osc;
        inner.install_lsl(staged_lsl);
        Ok(inner.health())
    }

    pub fn config(&self) -> OutputConfig {
        self.inner
            .lock()
            .map(|inner| inner.config.clone())
            .unwrap_or_default()
    }

    /// Returns the current fail-soft transport and recording state.
    pub fn health(&self) -> OutputHealth {
        self.inner
            .lock()
            .map(|inner| inner.health())
            .unwrap_or_else(|_| OutputHealth {
                stream_name: self.config().stream_name,
                lsl: "Output router lock failed".into(),
                osc: "Output router lock failed".into(),
                csv: "Output router lock failed".into(),
                audio: "Output router lock failed".into(),
                formulas: Vec::new(),
            })
    }

    /// Starts fresh whole-run and sliding normalization state for a new sensor session.
    pub fn reset_measurement(&self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.normalizers.clear();
        let config = inner.config.clone();
        inner.reconcile_normalizers(&config);
        for runtime in inner.formulas.values_mut() {
            if runtime.compiled.reset().is_ok() {
                runtime.state = FormulaRuntimeState::Ready;
                runtime.message = None;
            }
        }
    }

    /// Installs the metadata-verified per-device Go Direct schema and creates
    /// its aggregate raw plus derived breathing outlets before sample delivery.
    pub fn configure_vernier_streams(
        &self,
        model_code: &str,
        sample_period_us: u32,
        sensors: &[SensorInfo],
    ) -> Result<VernierStreamSchema, String> {
        let schema = VernierStreamSchema::new(model_code, sample_period_us, sensors)?;
        let mut inner = self.inner.lock().map_err(|_| "Output router lock failed")?;
        #[cfg(feature = "rusty-lsl-backend")]
        if inner.config.lsl_enabled {
            return Err(
                "Aggregate Vernier LSL requires the packaged liblsl backend; the optional Rusty backend does not support this dynamic Double64 schema."
                    .into(),
            );
        }
        if inner.vernier_schema.as_ref() != Some(&schema) {
            let config = inner.config.clone();
            let staged_lsl = inner
                .build_lsl(&config, Some(&schema))
                .map_err(|message| {
                    format!(
                        "The Vernier raw and derived LSL outlets could not be installed atomically: {message}"
                    )
                })?;
            if let Some(csv) = inner.csv.as_ref()
                && let Err(message) = csv.publish_vernier_breathing_provenance()
            {
                inner.csv = None;
                return Err(format!(
                    "The Vernier CSV processing metadata could not be recorded: {message}"
                ));
            }
            inner.vernier_schema = Some(schema.clone());
            inner.install_lsl(staged_lsl);
        }
        Ok(schema)
    }

    pub fn formula_health(&self) -> Vec<FormulaHealth> {
        self.inner
            .lock()
            .map(|inner| inner.formula_health())
            .unwrap_or_default()
    }

    pub fn process_ecg_formulas(
        &self,
        sensor_timestamp_ns: u64,
        samples: &[i32],
    ) -> FormulaPublishBatch {
        let Ok(mut inner) = self.inner.lock() else {
            return FormulaPublishBatch::default();
        };
        let frames = samples.iter().copied().map(FormulaFrame::ecg).collect();
        inner.process_custom(FormulaSource::Ecg, frames, sensor_timestamp_ns)
    }

    pub fn process_accelerometer_formulas(
        &self,
        sensor_timestamp_ns: u64,
        samples: &[AccSample],
    ) -> FormulaPublishBatch {
        let Ok(mut inner) = self.inner.lock() else {
            return FormulaPublishBatch::default();
        };
        let frames = samples
            .iter()
            .copied()
            .map(FormulaFrame::accelerometer)
            .collect();
        inner.process_custom(FormulaSource::Accelerometer, frames, sensor_timestamp_ns)
    }

    pub fn process_heart_rate_formulas(
        &self,
        beats_per_minute: u16,
        rr_intervals_ms: &[f32],
    ) -> FormulaPublishBatch {
        let Ok(mut inner) = self.inner.lock() else {
            return FormulaPublishBatch::default();
        };
        let mut batch = inner.process_custom(
            FormulaSource::HeartRate,
            vec![FormulaFrame::heart_rate(beats_per_minute)],
            0,
        );
        batch.append(
            inner.process_custom(
                FormulaSource::RrInterval,
                rr_intervals_ms
                    .iter()
                    .copied()
                    .map(FormulaFrame::rr_interval)
                    .collect(),
                0,
            ),
        );
        batch
    }

    /// Advances caller-owned Rusty LSL discovery, timedata, and consumer work.
    ///
    /// The default liblsl backend owns its own service lifecycle and therefore
    /// does not expose this operation. The experimental Rusty backend is
    /// deliberately polled by the native application coordinator instead of
    /// hiding a worker in the transport crate.
    #[cfg(feature = "rusty-lsl-backend")]
    pub fn poll_lsl(&self) -> Option<String> {
        let Ok(mut inner) = self.inner.lock() else {
            return Some("Rusty LSL output lock failed".into());
        };
        inner.lsl.poll()
    }

    pub fn publish_ecg(&self, sensor_timestamp_ns: u64, samples: &[i32]) -> Option<String> {
        let Ok(mut inner) = self.inner.lock() else {
            return None;
        };
        if inner.selected.contains("raw_ecg") {
            inner.lsl.push_scalar_series_at(
                "raw_ecg",
                samples.iter().map(|value| *value as f32),
                sensor_timestamp_ns,
            );
            if let Some(osc) = &mut inner.osc {
                osc.send_series(
                    "raw_ecg",
                    sensor_timestamp_ns,
                    samples.len(),
                    samples.iter().map(|value| *value as f32),
                );
            }
        }
        let error = inner
            .csv
            .as_ref()
            .and_then(|csv| csv.publish_ecg(sensor_timestamp_ns, samples).err());
        if error.is_some() {
            inner.csv = None;
        }
        error
    }

    /// Publishes one already-received Go Direct notification as a chunk. The
    /// timestamp denotes the newest sample and the nominal stream rate
    /// reconstructs earlier samples without delaying the notification.
    pub fn publish_force(
        &self,
        host_receive_timestamp_ns: u64,
        values: &[f32],
        sample_period_us: u32,
    ) -> Option<String> {
        let Ok(mut inner) = self.inner.lock() else {
            return None;
        };
        if inner.selected.contains("raw_force") {
            inner.lsl.push_scalar_series_period_at(
                "raw_force",
                values.iter().copied(),
                host_receive_timestamp_ns,
                sample_period_us,
            );
            if let Some(osc) = &mut inner.osc {
                for (index, value) in values.iter().copied().enumerate() {
                    let remaining = values.len().saturating_sub(index + 1) as u64;
                    let timestamp_ns = host_receive_timestamp_ns.saturating_sub(
                        remaining.saturating_mul(u64::from(sample_period_us)) * 1_000,
                    );
                    osc.send_series("raw_force", timestamp_ns, 1, std::iter::once(value));
                }
            }
        }
        let error = inner.csv.as_ref().and_then(|csv| {
            csv.publish_force(host_receive_timestamp_ns, values, sample_period_us)
                .err()
        });
        if error.is_some() {
            inner.csv = None;
        }
        error
    }

    /// Publishes the complete decoded Go Direct frame before conversion,
    /// normalization, CSV, OSC, or display work.
    #[allow(clippy::too_many_arguments)]
    pub fn publish_vernier_raw(
        &self,
        host_receive_timestamp_ns: u64,
        sample_period_us: u32,
        sequence: u64,
        dropped_before: u64,
        device_drop_reports_before: u64,
        decode_latency_ns: u64,
        encoding: SampleEncoding,
        sensors: &[SensorSamples],
    ) {
        #[cfg_attr(feature = "rusty-lsl-backend", allow(unused_mut))]
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if !inner.config.lsl_enabled {
            return;
        }
        if inner.vernier_schema.is_none() {
            return;
        }
        #[cfg(feature = "liblsl-backend")]
        {
            let RouterInner {
                lsl,
                vernier_schema,
                ..
            } = &mut *inner;
            let schema = vernier_schema
                .as_ref()
                .expect("Vernier schema presence was checked above");
            lsl.push_vernier_raw(
                schema,
                host_receive_timestamp_ns,
                sample_period_us,
                sequence,
                dropped_before,
                device_drop_reports_before,
                decode_latency_ns,
                encoding,
                sensors,
            );
        }
        #[cfg(feature = "rusty-lsl-backend")]
        let _ = (
            host_receive_timestamp_ns,
            sample_period_us,
            sequence,
            dropped_before,
            device_drop_reports_before,
            decode_latency_ns,
            encoding,
            sensors,
        );
    }

    /// Publishes the explicitly derived force normalization after raw output.
    pub fn publish_vernier_breathing(
        &self,
        host_receive_timestamp_ns: u64,
        values_01: &[f32],
        sample_period_us: u32,
    ) -> Option<String> {
        let Ok(mut inner) = self.inner.lock() else {
            return None;
        };
        if inner.config.lsl_enabled && inner.vernier_schema.is_some() {
            inner.lsl.push_scalar_series_period_at(
                VERNIER_BREATHING_OUTLET_KEY,
                values_01.iter().copied(),
                host_receive_timestamp_ns,
                sample_period_us,
            );
        }
        if let Some(osc) = &mut inner.osc {
            osc.send_vernier_breathing(host_receive_timestamp_ns, values_01, sample_period_us);
        }
        let error = inner.csv.as_ref().and_then(|csv| {
            csv.publish_metric_series_at(
                host_receive_timestamp_ns,
                sample_period_us,
                VERNIER_BREATHING_RECORDING_ID,
                "0–1",
                values_01,
            )
            .err()
        });
        if error.is_some() {
            inner.csv = None;
        }
        error
    }

    pub fn publish_accelerometer(
        &self,
        sensor_timestamp_ns: u64,
        samples: &[AccSample],
    ) -> Option<String> {
        let Ok(mut inner) = self.inner.lock() else {
            return None;
        };
        if inner.selected.contains("raw_acc") {
            inner
                .lsl
                .push_accelerometer_at(samples, sensor_timestamp_ns);
            if let Some(osc) = &mut inner.osc {
                osc.send_accelerometer(sensor_timestamp_ns, samples);
            }
        }
        let error = inner.csv.as_ref().and_then(|csv| {
            csv.publish_accelerometer(sensor_timestamp_ns, samples)
                .err()
        });
        if error.is_some() {
            inner.csv = None;
        }
        error
    }

    pub fn publish_heart_rate(
        &self,
        beats_per_minute: u16,
        rr_intervals_ms: &[f32],
    ) -> Option<String> {
        let Ok(mut inner) = self.inner.lock() else {
            return None;
        };
        let error = inner.csv.as_ref().and_then(|csv| {
            csv.publish_heart_rate(beats_per_minute, rr_intervals_ms)
                .err()
        });
        if error.is_some() {
            inner.csv = None;
        }
        error
    }

    pub fn publish_metrics(&self, values: &[MetricValue<'_>]) -> Option<String> {
        self.publish_metrics_at(0, values)
    }

    /// Publishes one derived snapshot at the newest source-sample timestamp.
    /// A zero timestamp deliberately retains the backend's local-clock path for
    /// sources such as standard heart-rate notifications that carry no sensor
    /// clock in the application event contract.
    pub fn publish_metrics_at(
        &self,
        sensor_timestamp_ns: u64,
        values: &[MetricValue<'_>],
    ) -> Option<String> {
        let Ok(mut inner) = self.inner.lock() else {
            return None;
        };
        let mut recorded = Vec::with_capacity(values.len());
        for metric in values {
            if !inner.selected.contains(metric.id) {
                continue;
            }
            let value = inner.transform(metric.id, metric.value);
            inner
                .lsl
                .push_scalar_series_at(metric.id, std::iter::once(value), sensor_timestamp_ns);
            if let Some(osc) = &mut inner.osc {
                osc.send_series(metric.id, sensor_timestamp_ns, 1, std::iter::once(value));
            }
            recorded.push((metric.id, value));
        }
        let error = inner
            .csv
            .as_ref()
            .and_then(|csv| csv.publish_metrics_at(sensor_timestamp_ns, &recorded).err());
        if error.is_some() {
            inner.csv = None;
        }
        error
    }
}

impl RouterInner {
    fn process_custom(
        &mut self,
        source: FormulaSource,
        frames: Vec<FormulaFrame>,
        sensor_timestamp_ns: u64,
    ) -> FormulaPublishBatch {
        if frames.is_empty() {
            return FormulaPublishBatch::default();
        }
        let ids = self
            .config
            .custom_formulas
            .iter()
            .filter(|formula| formula.enabled && formula.source == source)
            .map(|formula| formula.id.clone())
            .collect::<Vec<_>>();
        let mut batch = FormulaPublishBatch::default();
        let mut publications = Vec::new();

        for id in ids {
            let Some(runtime) = self.formulas.get_mut(&id) else {
                continue;
            };
            let mut values = Vec::with_capacity(frames.len());
            let mut final_state = runtime.state;
            for frame in frames.iter().copied() {
                let evaluation = runtime.compiled.process(frame);
                final_state = evaluation.state;
                if let Some(value) = evaluation.value {
                    values.push(value);
                }
                if let Some(fault) = evaluation.fault {
                    runtime.message = Some(fault.message.clone());
                    batch.faults.push(fault);
                }
            }
            runtime.state = final_state;
            publications.push((runtime.config.clone(), values.clone()));
            batch.series.push(FormulaSeries {
                formula_id: id,
                values,
                state: final_state,
            });
        }

        let mut custom_rows = Vec::new();
        for (config, values) in publications {
            if values.is_empty() {
                continue;
            }
            self.lsl
                .push_scalar_series(&config.id, values.iter().copied());
            if let Some(osc) = &mut self.osc {
                osc.send_series(
                    &config.id,
                    sensor_timestamp_ns,
                    values.len(),
                    values.iter().copied(),
                );
            }
            custom_rows.extend(
                values
                    .into_iter()
                    .map(|value| (config.name.clone(), value, config.unit.clone())),
            );
        }
        if let Some(error) = self
            .csv
            .as_ref()
            .and_then(|csv| csv.publish_custom_metrics(&custom_rows).err())
        {
            self.csv = None;
            batch.warnings.push(error);
        }
        batch
    }

    fn formula_health(&self) -> Vec<FormulaHealth> {
        self.config
            .custom_formulas
            .iter()
            .filter_map(|formula| self.formulas.get(&formula.id))
            .map(|runtime| FormulaHealth {
                formula_id: runtime.config.id.clone(),
                state: runtime.state,
                message: runtime.message.clone(),
            })
            .collect()
    }

    fn reconcile_normalizers(&mut self, config: &OutputConfig) {
        self.normalizers.retain(|id, _| config.outputs.contains(id));
        for id in &config.outputs {
            let options = config.metric_options.get(id).copied().unwrap_or_default();
            if options.normalization == NormalizationMode::None {
                self.normalizers.remove(id);
                continue;
            }
            self.normalizers
                .entry(id.clone())
                .and_modify(|normalizer| normalizer.reconfigure(options))
                .or_insert_with(|| Normalizer::new(options));
        }
    }

    fn transform(&mut self, id: &str, value: f32) -> f32 {
        self.normalizers
            .get_mut(id)
            .map_or(value, |normalizer| normalizer.apply(value))
    }

    #[cfg(feature = "liblsl-backend")]
    fn build_lsl(
        &self,
        config: &OutputConfig,
        vernier_schema: Option<&VernierStreamSchema>,
    ) -> Result<StagedLsl, String> {
        if !config.lsl_enabled {
            return Ok(StagedLsl::Disable);
        }
        let mut lsl = self.lsl.fresh();
        let fail_after_outlets = {
            #[cfg(test)]
            {
                self.fail_lsl_build_after_outlets
            }
            #[cfg(not(test))]
            {
                None
            }
        };
        Self::populate_lsl(&mut lsl, config, vernier_schema, fail_after_outlets)?;
        Ok(StagedLsl::Replace(Box::new(lsl)))
    }

    #[cfg(feature = "rusty-lsl-backend")]
    fn build_lsl(
        &mut self,
        config: &OutputConfig,
        vernier_schema: Option<&VernierStreamSchema>,
    ) -> Result<StagedLsl, String> {
        if !config.lsl_enabled {
            return Ok(StagedLsl::Disable);
        }
        self.ensure_lsl_reconfiguration_supported(config, vernier_schema)?;
        if self.config.lsl_enabled && self.lsl.outlet_count() > 0 {
            return Ok(StagedLsl::Keep);
        }

        // The optional backend owns one process-wide runtime admission and
        // discovery socket, so first enable must populate the existing
        // publisher rather than constructing a competing candidate.
        self.lsl.clear();
        let fail_after_outlets = {
            #[cfg(test)]
            {
                self.fail_lsl_build_after_outlets
            }
            #[cfg(not(test))]
            {
                None
            }
        };
        if let Err(message) =
            Self::populate_lsl(&mut self.lsl, config, vernier_schema, fail_after_outlets)
        {
            self.lsl.clear();
            return Err(message);
        }
        Ok(StagedLsl::Keep)
    }

    fn populate_lsl(
        lsl: &mut LslPublisher,
        config: &OutputConfig,
        vernier_schema: Option<&VernierStreamSchema>,
        fail_after_outlets: Option<usize>,
    ) -> Result<(), String> {
        let respiration_provenance = PolarRespirationProvenance::configured(config);
        for id in &config.outputs {
            if let Some(spec) = MetricSpec::for_id(id) {
                lsl.add_outlet_with_palette(
                    &config.stream_name,
                    spec,
                    config.source_palette.as_ref(),
                    respiration_provenance.as_ref(),
                );
            }
        }
        for formula in config
            .custom_formulas
            .iter()
            .filter(|formula| formula.enabled)
        {
            lsl.add_custom_outlet_with_palette(
                &config.stream_name,
                formula,
                config.source_palette.as_ref(),
            );
        }
        #[cfg(feature = "liblsl-backend")]
        if let Some(schema) = vernier_schema {
            lsl.add_vernier_outlets(&config.stream_name, schema, config.source_palette.as_ref());
        }
        let expected = config.outputs.len()
            + config
                .custom_formulas
                .iter()
                .filter(|formula| formula.enabled)
                .count()
            + usize::from(vernier_schema.is_some()) * 2;
        let actual = lsl.outlet_count();
        if fail_after_outlets.is_some_and(|outlets| actual >= outlets) {
            return Err(format!(
                "Injected LSL candidate failure after {actual} outlet(s)"
            ));
        }
        if actual != expected {
            return Err(format!(
                "Expected {expected} LSL outlet(s), opened {actual}: {}",
                lsl.status()
            ));
        }
        Ok(())
    }

    #[cfg(feature = "rusty-lsl-backend")]
    fn ensure_lsl_reconfiguration_supported(
        &self,
        config: &OutputConfig,
        vernier_schema: Option<&VernierStreamSchema>,
    ) -> Result<(), String> {
        if config.lsl_enabled
            && self.config.lsl_enabled
            && self.lsl.outlet_count() > 0
            && !self.lsl_contract_unchanged(config, vernier_schema)
        {
            return Err(
                "The optional Rusty LSL backend cannot atomically replace active outlets. Turn LSL off, apply the output change, then turn LSL on again."
                    .into(),
            );
        }
        Ok(())
    }

    #[cfg(feature = "rusty-lsl-backend")]
    fn lsl_contract_unchanged(
        &self,
        config: &OutputConfig,
        vernier_schema: Option<&VernierStreamSchema>,
    ) -> bool {
        self.config.stream_name == config.stream_name
            && self.config.outputs == config.outputs
            && self.config.source_palette == config.source_palette
            && self.config.custom_formulas == config.custom_formulas
            && PolarRespirationProvenance::configured(&self.config)
                == PolarRespirationProvenance::configured(config)
            && self.vernier_schema.as_ref() == vernier_schema
    }

    fn install_lsl(&mut self, staged: StagedLsl) {
        match staged {
            #[cfg(feature = "rusty-lsl-backend")]
            StagedLsl::Keep => {}
            StagedLsl::Disable => self.lsl.clear(),
            #[cfg(feature = "liblsl-backend")]
            StagedLsl::Replace(mut staged) => {
                staged.inherit_source_clock(&mut self.lsl);
                self.lsl = *staged;
            }
        }
    }

    fn health(&self) -> OutputHealth {
        OutputHealth {
            stream_name: self.config.stream_name.clone(),
            lsl: if !self.config.lsl_enabled {
                "Off".into()
            } else {
                self.lsl.status().into()
            },
            osc: if !self.config.osc_enabled {
                "Off".into()
            } else if self.osc.is_some() {
                format!("Sending to {OSC_TARGET}")
            } else {
                "Unavailable".into()
            },
            csv: if !self.config.csv_enabled {
                "Off".into()
            } else if let Some(csv) = &self.csv {
                if let Some(error) = csv.error() {
                    error
                } else {
                    format!(
                        "Recording {}",
                        csv.path()
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("local CSV")
                    )
                }
            } else {
                "Unavailable".into()
            },
            audio: if self.config.audio_enabled {
                "Experimental PCM data modem".into()
            } else {
                "Off".into()
            },
            formulas: self.formula_health(),
        }
    }
}

struct Normalizer {
    options: MetricOutputOptions,
    session_min: f32,
    session_max: f32,
    sequence: u64,
    window: VecDeque<WindowPoint>,
    window_min: VecDeque<(u64, f32)>,
    window_max: VecDeque<(u64, f32)>,
}

struct WindowPoint {
    sequence: u64,
    time: Instant,
}

impl Normalizer {
    fn new(options: MetricOutputOptions) -> Self {
        Self {
            options,
            session_min: f32::INFINITY,
            session_max: f32::NEG_INFINITY,
            sequence: 0,
            window: VecDeque::new(),
            window_min: VecDeque::new(),
            window_max: VecDeque::new(),
        }
    }

    fn reconfigure(&mut self, options: MetricOutputOptions) {
        if self.options != options {
            *self = Self::new(options);
        }
    }

    fn apply(&mut self, value: f32) -> f32 {
        if !value.is_finite() {
            return value;
        }
        match self.options.normalization {
            NormalizationMode::None => value,
            NormalizationMode::Session => {
                self.session_min = self.session_min.min(value);
                self.session_max = self.session_max.max(value);
                min_max(value, self.session_min, self.session_max)
            }
            NormalizationMode::SlidingWindow => {
                let now = Instant::now();
                self.sequence = self.sequence.wrapping_add(1);
                let sequence = self.sequence;
                self.window.push_back(WindowPoint {
                    sequence,
                    time: now,
                });
                while self
                    .window_min
                    .back()
                    .is_some_and(|(_, candidate)| *candidate >= value)
                {
                    self.window_min.pop_back();
                }
                self.window_min.push_back((sequence, value));
                while self
                    .window_max
                    .back()
                    .is_some_and(|(_, candidate)| *candidate <= value)
                {
                    self.window_max.pop_back();
                }
                self.window_max.push_back((sequence, value));
                let span = Duration::from_secs(u64::from(self.options.window_seconds));
                while self
                    .window
                    .front()
                    .is_some_and(|point| now.duration_since(point.time) > span)
                {
                    let Some(expired) = self.window.pop_front().map(|point| point.sequence) else {
                        break;
                    };
                    if self
                        .window_min
                        .front()
                        .is_some_and(|(candidate, _)| *candidate == expired)
                    {
                        self.window_min.pop_front();
                    }
                    if self
                        .window_max
                        .front()
                        .is_some_and(|(candidate, _)| *candidate == expired)
                    {
                        self.window_max.pop_front();
                    }
                }
                let minimum = self.window_min.front().map_or(value, |(_, value)| *value);
                let maximum = self.window_max.front().map_or(value, |(_, value)| *value);
                min_max(value, minimum, maximum)
            }
        }
    }
}

fn min_max(value: f32, minimum: f32, maximum: f32) -> f32 {
    if (maximum - minimum).abs() < f32::EPSILON {
        0.5
    } else {
        ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod normalization_tests {
    use super::*;

    fn vernier_sensor(
        number: u8,
        description: &str,
        unit: &str,
        numeric_type: vernier_gdx_core::NumericMeasurementType,
        sampling_mode: vernier_gdx_core::SamplingMode,
    ) -> SensorInfo {
        SensorInfo {
            number,
            sensor_id: u32::from(number),
            numeric_type,
            sampling_mode,
            description: description.into(),
            unit: unit.into(),
            uncertainty: 0.01,
            minimum: 0.0,
            maximum: 100.0,
            minimum_period_us: 50_000,
            maximum_period_us: 60_000_000,
            typical_period_us: 100_000,
            period_granularity_us: 1_000,
            mutual_exclusion_mask: 0,
        }
    }

    #[test]
    fn session_normalization_tracks_measurement_extrema() {
        let mut normalizer = Normalizer::new(MetricOutputOptions {
            normalization: NormalizationMode::Session,
            window_seconds: 60,
            ..MetricOutputOptions::default()
        });
        assert_eq!(normalizer.apply(10.0), 0.5);
        assert_eq!(normalizer.apply(20.0), 1.0);
        assert_eq!(normalizer.apply(15.0), 0.5);
        assert_eq!(normalizer.apply(5.0), 0.0);
    }

    #[test]
    fn sensor_clock_mapping_preserves_buffered_frame_spacing() {
        let mut clock = SensorClockMap::default();
        assert_eq!(clock.map_newest(10_000_000_000, 100.0), 100.0);
        assert_eq!(clock.map_newest(10_500_000_000, 100.01), 100.5);
        assert_eq!(clock.map_newest(11_000_000_000, 100.02), 101.0);
    }

    #[test]
    fn aggregate_vernier_rows_preserve_schema_order_sparse_updates_and_diagnostics() {
        let schema = VernierStreamSchema::new(
            "GDX-RB",
            100_000,
            &[
                vernier_sensor(
                    3,
                    "Steps",
                    "count",
                    vernier_gdx_core::NumericMeasurementType::Integer,
                    vernier_gdx_core::SamplingMode::Aperiodic,
                ),
                vernier_sensor(
                    1,
                    "Force",
                    "N",
                    vernier_gdx_core::NumericMeasurementType::Real,
                    vernier_gdx_core::SamplingMode::Periodic,
                ),
                vernier_sensor(
                    2,
                    "Respiration Rate",
                    "breaths/min",
                    vernier_gdx_core::NumericMeasurementType::Real,
                    vernier_gdx_core::SamplingMode::Aperiodic,
                ),
                vernier_sensor(
                    4,
                    "Step Rate",
                    "steps/min",
                    vernier_gdx_core::NumericMeasurementType::Real,
                    vernier_gdx_core::SamplingMode::Aperiodic,
                ),
            ],
        )
        .unwrap();
        assert_eq!(
            schema
                .channels()
                .iter()
                .map(|sensor| sensor.number)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        assert_eq!(schema.raw_channel_count(), 11);

        let mut rows = Vec::new();
        let count = encode_vernier_raw_rows(
            &mut rows,
            &schema,
            9_000_000,
            100_000,
            42,
            3,
            1,
            700,
            SampleEncoding::Float32,
            &[
                SensorSamples {
                    sensor_number: 1,
                    values: vec![10.25, 10.5],
                },
                SensorSamples {
                    sensor_number: 2,
                    values: vec![18.0, 18.5],
                },
            ],
        );
        assert_eq!(count, 2);
        assert_eq!(&rows[..2], &[10.25, 18.0]);
        assert!(rows[2].is_nan());
        assert!(rows[3].is_nan());
        assert_eq!(
            &rows[4..11],
            &[42.0, 3.0, 1.0, 100_000.0, 700.0, 9_000_000.0, 0.0]
        );
        assert_eq!(&rows[11..13], &[10.5, 18.5]);
        assert!(rows[13].is_nan());
        assert!(rows[14].is_nan());
        assert_eq!(rows[15], 43.0);
        assert_eq!(rows[16], 0.0);
        assert_eq!(rows[17], 0.0);
    }

    #[test]
    fn vernier_stream_names_are_stable_and_distinguish_raw_from_derived() {
        assert_eq!(
            vernier_raw_stream_name("participant_source-2"),
            "participant_source-2_rawVernier"
        );
        assert_eq!(
            vernier_breathing_stream_name("participant_source-2"),
            "participant_source-2_vernierBreathing"
        );
    }

    #[tokio::test]
    async fn configured_formula_is_evaluated_without_blocking_raw_publication() {
        let router = OutputRouter::new();
        let mut config = OutputConfig::default();
        config.custom_formulas.push(CustomFormulaConfig {
            id: "beedcafe-0000-4000-8000-000000000001".into(),
            name: "Half_ECG".into(),
            source: FormulaSource::Ecg,
            expression: "ecg / 2".into(),
            unit: "µV".into(),
            enabled: true,
        });
        let health = router.configure(config).await.unwrap();
        assert_eq!(health.formulas.len(), 1);
        let batch = router.process_ecg_formulas(1_000_000, &[10, -6]);
        assert!(batch.faults.is_empty());
        assert_eq!(batch.series.len(), 1);
        assert_eq!(batch.series[0].values, vec![5.0, -3.0]);
        assert_eq!(router.publish_ecg(1_000_000, &[10, -6]), None);
    }

    #[cfg(feature = "liblsl-backend")]
    #[tokio::test]
    async fn failed_lsl_candidate_leaves_config_csv_and_selected_outputs_unchanged() {
        let directory = std::env::temp_dir().join(format!(
            "polar-stream-output-transaction-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let router = OutputRouter::with_bundled_lsl_and_recordings(None, directory.clone());
        let previous = OutputConfig {
            stream_name: "Stable_Stream".into(),
            csv_enabled: true,
            outputs: vec!["raw_acc".into()],
            ..OutputConfig::default()
        };
        router.configure(previous.clone()).await.unwrap();
        let previous_csv = router
            .inner
            .lock()
            .unwrap()
            .csv
            .as_ref()
            .unwrap()
            .path()
            .to_owned();
        router.inner.lock().unwrap().fail_lsl_build_after_outlets = Some(0);

        let error = router
            .configure(OutputConfig {
                stream_name: "Rejected_Stream".into(),
                lsl_enabled: true,
                csv_enabled: true,
                osc_enabled: true,
                outputs: vec!["breathing_volume".into()],
                ..OutputConfig::default()
            })
            .await
            .unwrap_err();
        assert!(error.contains("Injected LSL candidate failure"), "{error}");
        let inner = router.inner.lock().unwrap();
        assert_eq!(inner.config.stream_name, previous.stream_name);
        assert_eq!(inner.config.outputs, previous.outputs);
        assert!(!inner.config.lsl_enabled);
        assert!(!inner.config.osc_enabled);
        assert!(inner.selected.contains("raw_acc"));
        assert!(!inner.selected.contains("breathing_volume"));
        assert_eq!(inner.csv.as_ref().unwrap().path(), previous_csv);
        drop(inner);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);

        drop(router);
        for _ in 0..40 {
            match std::fs::remove_dir_all(&directory) {
                Ok(()) => break,
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert!(!directory.exists());
    }

    #[cfg(feature = "rusty-lsl-backend")]
    #[tokio::test]
    async fn changed_rusty_lsl_contract_is_rejected_without_disturbing_live_outlets() {
        let router = OutputRouter::new();
        let previous = OutputConfig {
            stream_name: format!("transaction_{}", std::process::id()),
            lsl_enabled: true,
            outputs: vec!["raw_acc".into()],
            ..OutputConfig::default()
        };
        router.configure(previous.clone()).await.unwrap();
        {
            let mut inner = router.inner.lock().unwrap();
            assert_eq!(inner.lsl.outlet_count(), 1);
            inner.fail_lsl_build_after_outlets = Some(1);
        }

        let error = router
            .configure(OutputConfig {
                stream_name: format!("rejected_{}", std::process::id()),
                lsl_enabled: true,
                outputs: vec!["raw_acc".into(), "breathing_volume".into()],
                ..OutputConfig::default()
            })
            .await
            .unwrap_err();
        assert!(
            error.contains("cannot atomically replace active outlets"),
            "{error}"
        );
        let inner = router.inner.lock().unwrap();
        assert_eq!(inner.config.stream_name, previous.stream_name);
        assert_eq!(inner.config.outputs, previous.outputs);
        assert_eq!(inner.lsl.outlet_count(), 1);
        assert!(inner.lsl.status().contains("1 stream(s)"));
    }

    #[tokio::test]
    async fn csv_starts_a_new_file_when_respiration_provenance_changes() {
        use polar_h10_metrics::{BreathingSettings, BreathingStateMode, BreathingVolumeMode};

        let directory = std::env::temp_dir().join(format!(
            "polar-stream-provenance-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let router = OutputRouter::with_bundled_lsl_and_recordings(None, directory.clone());
        router
            .configure(OutputConfig {
                csv_enabled: true,
                outputs: vec!["raw_acc".into()],
                ..OutputConfig::default()
            })
            .await
            .unwrap();
        let raw_path = router
            .inner
            .lock()
            .unwrap()
            .csv
            .as_ref()
            .unwrap()
            .path()
            .to_owned();
        assert!(
            !std::fs::read_to_string(&raw_path)
                .unwrap()
                .contains("# polar_respiration_")
        );

        router
            .configure(OutputConfig {
                csv_enabled: true,
                outputs: vec!["breathing_volume".into()],
                ..OutputConfig::default()
            })
            .await
            .unwrap();
        let timed_path = router
            .inner
            .lock()
            .unwrap()
            .csv
            .as_ref()
            .unwrap()
            .path()
            .to_owned();
        assert_ne!(timed_path, raw_path);
        let timed_header = std::fs::read_to_string(&timed_path).unwrap();
        assert!(timed_header.contains("# polar_respiration_volume_mode,timed-pca-v1"));
        assert!(timed_header.contains("# polar_respiration_state_mode,hysteresis-v1"));

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
        router
            .configure(OutputConfig {
                csv_enabled: true,
                outputs: vec!["breathing_volume".into()],
                metric_options,
                ..OutputConfig::default()
            })
            .await
            .unwrap();
        let legacy_path = router
            .inner
            .lock()
            .unwrap()
            .csv
            .as_ref()
            .unwrap()
            .path()
            .to_owned();
        assert_ne!(legacy_path, timed_path);
        let legacy_header = std::fs::read_to_string(&legacy_path).unwrap();
        assert!(legacy_header.contains("# polar_respiration_volume_mode,legacy-v0"));
        assert!(legacy_header.contains("# polar_respiration_state_mode,legacy-v0"));

        drop(router);
        for _ in 0..40 {
            match std::fs::remove_dir_all(&directory) {
                Ok(()) => break,
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert!(!directory.exists());
    }

    #[tokio::test]
    async fn csv_starts_a_new_file_when_header_identity_changes() {
        let directory = std::env::temp_dir().join(format!(
            "polar-stream-csv-identity-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let router = OutputRouter::with_bundled_lsl_and_recordings(None, directory.clone());
        router
            .configure(OutputConfig {
                stream_name: "First_Stream".into(),
                csv_enabled: true,
                outputs: vec!["raw_acc".into()],
                ..OutputConfig::default()
            })
            .await
            .unwrap();
        let first_path = router
            .inner
            .lock()
            .unwrap()
            .csv
            .as_ref()
            .unwrap()
            .path()
            .to_owned();

        router
            .configure(OutputConfig {
                stream_name: "Second_Stream".into(),
                csv_enabled: true,
                outputs: vec!["raw_acc".into()],
                ..OutputConfig::default()
            })
            .await
            .unwrap();
        let second_path = router
            .inner
            .lock()
            .unwrap()
            .csv
            .as_ref()
            .unwrap()
            .path()
            .to_owned();
        assert_ne!(second_path, first_path);
        assert!(
            std::fs::read_to_string(&second_path)
                .unwrap()
                .contains("# stream_name,Second_Stream")
        );

        router
            .configure(OutputConfig {
                stream_name: "Second_Stream".into(),
                csv_enabled: true,
                source_palette: source_palette("ocean"),
                outputs: vec!["raw_acc".into()],
                ..OutputConfig::default()
            })
            .await
            .unwrap();
        let palette_path = router
            .inner
            .lock()
            .unwrap()
            .csv
            .as_ref()
            .unwrap()
            .path()
            .to_owned();
        assert_ne!(palette_path, second_path);
        let palette_header = std::fs::read_to_string(&palette_path).unwrap();
        assert!(palette_header.contains("# stream_name,Second_Stream"));
        assert!(palette_header.contains("# source_palette_id,ocean"));

        drop(router);
        for _ in 0..40 {
            match std::fs::remove_dir_all(&directory) {
                Ok(()) => break,
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert!(!directory.exists());
    }

    #[tokio::test]
    async fn polar_and_vernier_routers_log_distinct_native_csv_streams_with_provenance() {
        let directory = std::env::temp_dir().join(format!(
            "polar-stream-mixed-router-csv-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let polar = OutputRouter::with_bundled_lsl_and_recordings(None, directory.clone());
        let vernier = OutputRouter::with_bundled_lsl_and_recordings(None, directory.clone());

        polar
            .configure(OutputConfig {
                stream_name: "Mixed_source-1".into(),
                csv_enabled: true,
                outputs: vec!["raw_acc".into(), "breathing_volume".into()],
                ..OutputConfig::default()
            })
            .await
            .unwrap();
        vernier
            .configure(OutputConfig {
                stream_name: "Mixed_source-2".into(),
                csv_enabled: true,
                outputs: Vec::new(),
                ..OutputConfig::default()
            })
            .await
            .unwrap();
        vernier
            .configure_vernier_streams(
                "GDX-RB",
                100_000,
                &[vernier_sensor(
                    1,
                    "Force",
                    "N",
                    vernier_gdx_core::NumericMeasurementType::Real,
                    vernier_gdx_core::SamplingMode::Periodic,
                )],
            )
            .unwrap();

        assert_eq!(
            polar.publish_accelerometer(
                200_000_000,
                &[AccSample {
                    x_mg: 1,
                    y_mg: 2,
                    z_mg: 3,
                }],
            ),
            None
        );
        assert_eq!(
            polar.publish_metrics_at(
                200_000_000,
                &[MetricValue {
                    id: "breathing_volume",
                    value: 0.75,
                }],
            ),
            None
        );
        assert_eq!(
            vernier.publish_force(300_000_000, &[10.0, 11.0], 100_000),
            None
        );
        assert_eq!(
            vernier.publish_vernier_breathing(300_000_000, &[0.25, 0.75], 100_000),
            None
        );

        let polar_path = polar
            .inner
            .lock()
            .unwrap()
            .csv
            .as_ref()
            .unwrap()
            .path()
            .to_owned();
        let vernier_path = vernier
            .inner
            .lock()
            .unwrap()
            .csv
            .as_ref()
            .unwrap()
            .path()
            .to_owned();
        assert_ne!(polar_path, vernier_path);
        assert!(
            polar_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("Mixed_source-1_")
        );
        assert!(
            vernier_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("Mixed_source-2_")
        );
        drop((polar, vernier));

        let (mut polar_csv, mut vernier_csv) = (String::new(), String::new());
        for _ in 0..40 {
            polar_csv = std::fs::read_to_string(&polar_path).unwrap_or_default();
            vernier_csv = std::fs::read_to_string(&vernier_path).unwrap_or_default();
            if polar_csv.contains(",breathing_volume,")
                && vernier_csv.contains(",vernier_breathing,")
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(polar_csv.contains(",raw_acc,0,1,2,3,,mg"));
        assert!(polar_csv.contains(",200000000,breathing_volume,0,,,,0.75,0–1"));
        assert!(polar_csv.contains("# polar_respiration_settings_schema,breathing-settings-v1"));
        assert!(!polar_csv.contains("# vernier_breathing_"));
        assert!(vernier_csv.contains(",raw_force,0,,,,10,N"));
        assert!(vernier_csv.contains(",vernier_breathing,0,,,,0.25,0–1"));
        assert!(vernier_csv.contains(",vernier_breathing,1,,,,0.75,0–1"));
        assert!(
            vernier_csv
                .contains("# vernier_breathing_settings_schema,vernier-breathing-settings-v1")
        );
        assert!(vernier_csv.contains(&format!(
            "# vernier_breathing_application_version,{}",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(!vernier_csv.contains("# polar_respiration_"));

        for _ in 0..40 {
            match std::fs::remove_dir_all(&directory) {
                Ok(()) => break,
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert!(!directory.exists());
    }
}
