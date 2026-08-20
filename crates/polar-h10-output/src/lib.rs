//! Native output fan-out. Sensor traffic never takes a detour through the web UI.

mod config;
mod csv;
#[cfg(feature = "liblsl-backend")]
mod lsl;
mod osc;
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
    CustomFormulaConfig, FormulaHealth, FormulaSource, MetricOutputOptions,
    MetricProcessingOptions, MetricSpec, NormalizationMode, OutputConfig, OutputHealth,
    custom_output_stream_name, normalize_stream_base, output_stream_name,
};
use csv::CsvPublisher;
#[cfg(feature = "liblsl-backend")]
use lsl::LslPublisher;
use osc::{OSC_TARGET, OscPublisher};
use polar_h10_core::AccSample;
use polar_h10_math::{CompiledFormula, FormulaFrame, MAX_TOTAL_STATE_SAMPLES};
pub use polar_h10_math::{FormulaError, FormulaRuntimeState, FormulaValidation, validate_formula};
#[cfg(feature = "rusty-lsl-backend")]
use rusty_lsl::RustyLslPublisher as LslPublisher;
use serde::Serialize;

#[derive(Default)]
struct SensorClockMap {
    local_minus_sensor_seconds: Option<f64>,
}

impl SensorClockMap {
    fn map_newest(&mut self, sensor_timestamp_ns: u64, local_now: f64) -> f64 {
        if sensor_timestamp_ns == 0 {
            return local_now;
        }
        let sensor_seconds = sensor_timestamp_ns as f64 / 1_000_000_000.0;
        let offset = *self
            .local_minus_sensor_seconds
            .get_or_insert(local_now - sensor_seconds);
        sensor_seconds + offset
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MetricValue<'a> {
    pub id: &'a str,
    pub value: f32,
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
        let mut osc = if config.osc_enabled {
            Some(OscPublisher::connect(OSC_TARGET).await?)
        } else {
            None
        };
        if let Some(publisher) = &mut osc {
            publisher.configure(&config.stream_name, &config.outputs);
            publisher.configure_custom(&config.stream_name, &config.custom_formulas);
        }

        let csv_to_install = if config.csv_enabled {
            let needs_writer = self
                .inner
                .lock()
                .map_err(|_| "Output router lock failed")?
                .csv
                .is_none();
            if needs_writer {
                let directory = self
                    .inner
                    .lock()
                    .map_err(|_| "Output router lock failed")?
                    .csv_directory
                    .clone();
                Some(CsvPublisher::start(&directory, &config.stream_name)?)
            } else {
                None
            }
        } else {
            None
        };

        let mut inner = self.inner.lock().map_err(|_| "Output router lock failed")?;
        if !config.csv_enabled {
            inner.csv = None;
        } else if inner.csv.is_none() {
            inner.csv = csv_to_install;
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
                .ok_or("Compiled custom formula is unavailable")?;
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
        inner.rebuild_lsl();
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

    fn rebuild_lsl(&mut self) {
        self.lsl.clear();
        if !self.config.lsl_enabled {
            return;
        }
        for id in &self.config.outputs {
            if let Some(spec) = MetricSpec::for_id(id) {
                self.lsl.add_outlet(&self.config.stream_name, spec);
            }
        }
        for formula in self
            .config
            .custom_formulas
            .iter()
            .filter(|formula| formula.enabled)
        {
            self.lsl
                .add_custom_outlet(&self.config.stream_name, formula);
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
}
