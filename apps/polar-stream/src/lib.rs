//! Thin application coordinator. Protocol decoding, Bluetooth input, and
//! network output are independent crates below `crates/`.

mod error;
mod lab_recorder;
mod preferences;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use polar_h10_core::AccSample;
use polar_h10_input::{InputEvent as PolarInputEvent, InputSessionPool as PolarInputSessionPool};
use polar_h10_metrics::{
    BreathingSettings, METRIC_CATALOG, MetricCitation, MetricDefinition, MetricEngine,
    MetricSample, MetricSelection, TimedAccBatch, VernierBreathingProcessor, metric_citations,
    metric_formula_definition,
};
use polar_h10_output::{
    CustomFormulaConfig, FormulaError, FormulaPublishBatch, FormulaValidation, SourcePalette,
    source_palette, source_palette_catalog, validate_formula,
};
use polar_h10_output::{MetricValue, OutputConfig, OutputHealth, OutputRouter};
use polar_stream_time::{ClockMapping, SourceClockMapper, monotonic_now_ns};
use serde::{Deserialize, Serialize};
use tauri::{Manager, State, ipc::Channel, path::BaseDirectory};
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;
use vernier_gdx_core::{
    SampleEncoding as GdxSampleEncoding, SensorInfo as GdxSensorInfo,
    SensorSamples as GdxSensorSamples,
};
use vernier_gdx_input::{
    InputEvent as GdxInputEvent, InputSessionPool as GdxInputSessionPool, SessionConfig,
};

use error::{CommandError, CommandResult};
use lab_recorder::{
    LabRecorderCapability, LabRecorderInstallation, LabRecorderLaunch, unavailable_error,
};
use preferences::{PreferencesSnapshot, PreferencesStore, SavedDevice};

#[cfg(feature = "rusty-lsl-backend")]
const LSL_POLL_INTERVAL: Duration = Duration::from_millis(5);
#[cfg(feature = "rusty-lsl-backend")]
const LSL_POLL_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg(feature = "rusty-lsl-backend")]
fn run_lsl_poll_loop<F>(stop: &AtomicBool, interval: Duration, mut poll: F)
where
    F: FnMut(),
{
    while !stop.load(Ordering::Acquire) {
        poll();
        std::thread::sleep(interval);
    }
}

#[cfg(feature = "rusty-lsl-backend")]
async fn stop_and_join_lsl_poll(
    stop: &AtomicBool,
    task: tokio::task::JoinHandle<()>,
    timeout: Duration,
) -> Result<(), &'static str> {
    stop.store(true, Ordering::Release);
    match tokio::time::timeout(timeout, task).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err("Rusty LSL blocking poll task failed"),
        Err(_) => Err("Rusty LSL blocking poll task did not stop within its deadline"),
    }
}

struct AppState {
    polar_input: Arc<PolarInputSessionPool>,
    gdx_input: Arc<GdxInputSessionPool>,
    output_config: RwLock<OutputConfig>,
    source_outputs: Arc<tokio::sync::Mutex<HashMap<String, Arc<OutputRouter>>>>,
    active_sources: Arc<tokio::sync::Mutex<HashMap<String, SourceDescriptor>>>,
    display_endpoints: Arc<tokio::sync::Mutex<HashMap<String, Arc<DisplayEndpoint>>>>,
    source_tasks: Arc<tokio::sync::Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>>,
    input_configuration: tokio::sync::Mutex<()>,
    bundled_lsl: Option<PathBuf>,
    lab_recorder: Option<LabRecorderInstallation>,
    recording_directory: PathBuf,
    preferences: Arc<PreferencesStore>,
    output_configuration: tokio::sync::Mutex<()>,
    processing_settings: tokio::sync::watch::Sender<ProcessingSettings>,
    shutdown_started: AtomicBool,
}

impl AppState {
    fn new(
        bundled_lsl: Option<PathBuf>,
        lab_recorder: Option<LabRecorderInstallation>,
        preferences_path: PathBuf,
        recording_directory: PathBuf,
    ) -> Self {
        let preferences = Arc::new(PreferencesStore::load(preferences_path));
        let initial_config = preferences.snapshot().output_config;
        let (processing_settings, _) =
            tokio::sync::watch::channel(ProcessingSettings::from_config(&initial_config));
        Self {
            polar_input: Arc::new(PolarInputSessionPool::with_max_sessions(MAX_INPUT_SOURCES)),
            gdx_input: Arc::new(GdxInputSessionPool::new(MAX_INPUT_SOURCES)),
            output_config: RwLock::new(initial_config),
            source_outputs: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            active_sources: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            display_endpoints: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            source_tasks: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            input_configuration: tokio::sync::Mutex::new(()),
            bundled_lsl,
            lab_recorder,
            recording_directory,
            preferences,
            output_configuration: tokio::sync::Mutex::new(()),
            processing_settings,
            shutdown_started: AtomicBool::new(false),
        }
    }
}

const MAX_INPUT_SOURCES: usize = 8;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum InputKind {
    PolarH10,
    VernierGoDirect,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceDescriptor {
    id: String,
    slot: String,
    label: String,
    color: String,
    palette: SourcePalette,
    input_kind: InputKind,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourcePaletteUpdate {
    source: SourceDescriptor,
    health: OutputHealth,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceSummary {
    id: String,
    name: String,
    rssi: Option<i16>,
    input_kind: InputKind,
    model_code: String,
    detail: String,
    respiration_belt_candidate: bool,
}

struct DisplayEndpoint {
    state: std::sync::Mutex<DisplayEndpointState>,
}

#[derive(Default)]
struct DisplayEndpointState {
    channel: Option<Channel<AppEvent>>,
    connection_snapshot: Option<AppEvent>,
}

impl DisplayEndpoint {
    fn new(channel: Channel<AppEvent>) -> Self {
        Self {
            state: std::sync::Mutex::new(DisplayEndpointState {
                channel: Some(channel),
                connection_snapshot: None,
            }),
        }
    }

    fn attach(&self, channel: Channel<AppEvent>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.channel = Some(channel);
        if let (Some(channel), Some(snapshot)) = (&state.channel, &state.connection_snapshot)
            && channel.send(snapshot.clone()).is_err()
        {
            state.channel = None;
        }
    }

    fn send(&self, event: AppEvent) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if matches!(event, AppEvent::Connection { .. }) {
            state.connection_snapshot = Some(event.clone());
        }
        if state
            .channel
            .as_ref()
            .is_some_and(|channel| channel.send(event).is_err())
        {
            state.channel = None;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ProcessingSettings {
    breathing: BreathingSettings,
    metrics: MetricSelection,
}

impl ProcessingSettings {
    fn from_config(config: &OutputConfig) -> Self {
        let breathing = [
            "breathing_volume",
            "breathing_signal_confidence",
            "breathing_signal_ready",
            "breathing_phase",
            "acc_breathing_magnitude",
            "breathing_calibration",
            "breathing_axis_range",
        ]
        .iter()
        .find_map(|id| {
            config
                .metric_options
                .get(*id)
                .and_then(|options| options.processing.breathing)
        })
        .unwrap_or_default()
        .clamped();
        Self {
            breathing,
            metrics: MetricSelection::from_ids(config.outputs.iter().map(String::as_str)),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimingEnvelope {
    source_timestamp_ns: Option<String>,
    host_receive_timestamp_ns: String,
    mapped_host_timestamp_ns: String,
    sample_period_ns: Option<String>,
    mapping_revision: u64,
    clock_quality: &'static str,
    clock_uncertainty_ns: String,
    gap_before: bool,
}

impl TimingEnvelope {
    fn mapped_source(
        mapper: &mut SourceClockMapper,
        source_timestamp_ns: u64,
        host_receive_timestamp_ns: u64,
        sample_period_ns: u64,
    ) -> Self {
        let mapping = mapper.observe_and_map(source_timestamp_ns, host_receive_timestamp_ns);
        Self::from_mapping(
            Some(source_timestamp_ns),
            host_receive_timestamp_ns,
            sample_period_ns,
            mapping,
        )
    }

    fn arrival(
        host_receive_timestamp_ns: u64,
        sample_period_ns: Option<u64>,
        gap_before: bool,
    ) -> Self {
        Self {
            source_timestamp_ns: None,
            host_receive_timestamp_ns: host_receive_timestamp_ns.to_string(),
            mapped_host_timestamp_ns: host_receive_timestamp_ns.to_string(),
            sample_period_ns: sample_period_ns.map(|value| value.to_string()),
            mapping_revision: 0,
            clock_quality: "arrival",
            clock_uncertainty_ns: "0".into(),
            gap_before,
        }
    }

    fn from_mapping(
        source_timestamp_ns: Option<u64>,
        host_receive_timestamp_ns: u64,
        sample_period_ns: u64,
        mapping: ClockMapping,
    ) -> Self {
        Self {
            source_timestamp_ns: source_timestamp_ns.map(|value| value.to_string()),
            host_receive_timestamp_ns: host_receive_timestamp_ns.to_string(),
            mapped_host_timestamp_ns: mapping.mapped_time_ns.to_string(),
            sample_period_ns: Some(sample_period_ns.to_string()),
            mapping_revision: mapping.revision,
            clock_quality: mapping.quality.as_str(),
            clock_uncertainty_ns: mapping.uncertainty_ns.to_string(),
            gap_before: mapping.reset,
        }
    }
}

/// A renderer-only source-time point. This deliberately remains separate from
/// the canonical metric publication path: the WebView may interpolate or
/// smooth it for presentation, but it cannot feed a classifier or output
/// transport.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BreathingPresentationPoint {
    source_timestamp_ns: String,
    volume_01: f32,
}

fn timed_acc_batch(sensor_timestamp_ns: u64, mapping: ClockMapping) -> TimedAccBatch {
    TimedAccBatch {
        newest_sensor_timestamp_ns: sensor_timestamp_ns,
        sample_period_ns: ACC_SAMPLE_PERIOD_NS,
        clock_revision: mapping.revision,
        clock_reset: mapping.reset,
        // The input adapter currently exposes PMD source-clock discontinuities
        // through the mapper reset. Preserve that evidence rather than
        // inferring a gap from BLE notification cadence.
        gap_before: mapping.reset,
    }
}

fn breathing_presentation_points(
    points: Vec<polar_h10_metrics::BreathingWaveformPoint>,
) -> Vec<BreathingPresentationPoint> {
    points
        .into_iter()
        .map(|point| BreathingPresentationPoint {
            source_timestamp_ns: point.source_timestamp_ns.to_string(),
            volume_01: point.volume_01,
        })
        .collect()
}

const ECG_SAMPLE_PERIOD_NS: u64 = 1_000_000_000 / 130;
const ACC_SAMPLE_PERIOD_NS: u64 = 1_000_000_000 / 200;

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum AppEvent {
    Status {
        source: SourceDescriptor,
        phase: String,
        message: String,
    },
    Connection {
        source: SourceDescriptor,
        connected: bool,
        streaming: bool,
        device_name: String,
        battery_percent: Option<u8>,
        device_model: Option<String>,
        firmware_version: Option<String>,
        sensor_number: Option<u8>,
        sensor_name: Option<String>,
        sensor_unit: Option<String>,
        sample_period_us: Option<u32>,
        message: String,
    },
    Ecg {
        source: SourceDescriptor,
        sensor_timestamp_ns: u64,
        timing: TimingEnvelope,
        microvolts: Vec<i32>,
        formulas: FormulaPublishBatch,
    },
    Accelerometer {
        source: SourceDescriptor,
        sensor_timestamp_ns: u64,
        timing: TimingEnvelope,
        samples: Vec<AccSample>,
        formulas: FormulaPublishBatch,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        breathing_presentation_points: Vec<BreathingPresentationPoint>,
    },
    Metrics {
        source: SourceDescriptor,
        timing: TimingEnvelope,
        values: Vec<MetricSample>,
        formulas: FormulaPublishBatch,
    },
    Force {
        source: SourceDescriptor,
        timing: TimingEnvelope,
        sensor_number: u8,
        host_receive_timestamp_ns: u64,
        sample_period_us: u32,
        sequence: u64,
        values: Vec<f32>,
        breathing_values: Vec<f32>,
        dropped_before: u64,
        decode_latency_ns: u64,
    },
    StreamHealth {
        source: SourceDescriptor,
        notifications: u64,
        samples: u64,
        sample_rows: u64,
        malformed_frames: u64,
        dropped_batches: u64,
        device_drop_reports: u64,
        queue_high_water: usize,
        decode_latency_p50_ns: u64,
        decode_latency_p95_ns: u64,
        decode_latency_p99_ns: u64,
        max_decode_latency_ns: u64,
    },
    Error {
        source: SourceDescriptor,
        code: &'static str,
        message: String,
    },
}

enum DeviceEventReceiver {
    Polar(tokio::sync::mpsc::Receiver<PolarInputEvent>),
    Vernier(tokio::sync::mpsc::Receiver<GdxInputEvent>),
}

enum UnifiedInputEvent {
    Status {
        phase: &'static str,
        message: String,
    },
    Connected {
        device_name: String,
        battery_percent: Option<u8>,
        device_model: Option<String>,
        firmware_version: Option<String>,
        sensor_number: Option<u8>,
        sensor_name: Option<String>,
        sensor_unit: Option<String>,
        sample_period_us: Option<u32>,
        vernier_sensors: Option<Vec<GdxSensorInfo>>,
    },
    Ecg {
        sensor_timestamp_ns: u64,
        host_receive_timestamp_ns: u64,
        microvolts: Vec<i32>,
    },
    Accelerometer {
        sensor_timestamp_ns: u64,
        host_receive_timestamp_ns: u64,
        samples: Vec<AccSample>,
    },
    HeartRate {
        host_receive_timestamp_ns: u64,
        beats_per_minute: u16,
        rr_intervals_ms: Vec<f32>,
    },
    VernierSamples {
        encoding: GdxSampleEncoding,
        sensors: Vec<GdxSensorSamples>,
        host_receive_timestamp_ns: u64,
        sample_period_us: u32,
        sequence: u64,
        dropped_before: u64,
        device_drop_reports_before: u64,
        decode_latency_ns: u64,
    },
    StreamHealth {
        notifications: u64,
        samples: u64,
        sample_rows: u64,
        malformed_frames: u64,
        dropped_batches: u64,
        device_drop_reports: u64,
        queue_high_water: usize,
        decode_latency_p50_ns: u64,
        decode_latency_p95_ns: u64,
        decode_latency_p99_ns: u64,
        max_decode_latency_ns: u64,
    },
    Error(String),
    Disconnected {
        device_name: String,
        battery_percent: Option<u8>,
    },
}

impl DeviceEventReceiver {
    async fn recv(&mut self) -> Option<UnifiedInputEvent> {
        match self {
            Self::Polar(receiver) => receiver.recv().await.map(|event| match event {
                PolarInputEvent::Status { phase, message } => {
                    UnifiedInputEvent::Status { phase, message }
                }
                PolarInputEvent::Connected {
                    device_name,
                    battery_percent,
                } => UnifiedInputEvent::Connected {
                    device_name,
                    battery_percent,
                    device_model: Some("Polar H10".into()),
                    firmware_version: None,
                    sensor_number: None,
                    sensor_name: None,
                    sensor_unit: None,
                    sample_period_us: None,
                    vernier_sensors: None,
                },
                PolarInputEvent::Ecg {
                    sensor_timestamp_ns,
                    host_receive_timestamp_ns,
                    microvolts,
                } => UnifiedInputEvent::Ecg {
                    sensor_timestamp_ns,
                    host_receive_timestamp_ns,
                    microvolts,
                },
                PolarInputEvent::Accelerometer {
                    sensor_timestamp_ns,
                    host_receive_timestamp_ns,
                    samples,
                } => UnifiedInputEvent::Accelerometer {
                    sensor_timestamp_ns,
                    host_receive_timestamp_ns,
                    samples,
                },
                PolarInputEvent::HeartRate {
                    host_receive_timestamp_ns,
                    beats_per_minute,
                    rr_intervals_ms,
                } => UnifiedInputEvent::HeartRate {
                    host_receive_timestamp_ns,
                    beats_per_minute,
                    rr_intervals_ms,
                },
                PolarInputEvent::Error(message) => UnifiedInputEvent::Error(message),
                PolarInputEvent::Disconnected {
                    device_name,
                    battery_percent,
                } => UnifiedInputEvent::Disconnected {
                    device_name,
                    battery_percent,
                },
            }),
            Self::Vernier(receiver) => receiver.recv().await.map(|event| match event {
                GdxInputEvent::Status { phase, message } => {
                    UnifiedInputEvent::Status { phase, message }
                }
                GdxInputEvent::Connected {
                    device_name,
                    model_code,
                    sensor_number,
                    sensor_name,
                    sensor_unit,
                    sample_period_us,
                    main_firmware_version,
                    battery_percent,
                    sensors,
                } => UnifiedInputEvent::Connected {
                    device_name,
                    battery_percent: Some(battery_percent),
                    device_model: Some(model_code),
                    firmware_version: Some(main_firmware_version),
                    sensor_number: Some(sensor_number),
                    sensor_name: Some(sensor_name),
                    sensor_unit: Some(sensor_unit),
                    sample_period_us: Some(sample_period_us),
                    vernier_sensors: Some(sensors),
                },
                GdxInputEvent::Samples {
                    encoding,
                    sensors,
                    host_receive_timestamp_ns,
                    sample_period_us,
                    sequence,
                    dropped_before,
                    device_drop_reports_before,
                    decode_latency_ns,
                } => UnifiedInputEvent::VernierSamples {
                    encoding,
                    sensors,
                    host_receive_timestamp_ns,
                    sample_period_us,
                    sequence,
                    dropped_before,
                    device_drop_reports_before,
                    decode_latency_ns,
                },
                GdxInputEvent::StreamHealth {
                    notifications,
                    samples,
                    sample_rows,
                    malformed_frames,
                    dropped_batches,
                    device_drop_reports,
                    queue_high_water,
                    decode_latency_p50_ns,
                    decode_latency_p95_ns,
                    decode_latency_p99_ns,
                    max_decode_latency_ns,
                } => UnifiedInputEvent::StreamHealth {
                    notifications,
                    samples,
                    sample_rows,
                    malformed_frames,
                    dropped_batches,
                    device_drop_reports,
                    queue_high_water,
                    decode_latency_p50_ns,
                    decode_latency_p95_ns,
                    decode_latency_p99_ns,
                    max_decode_latency_ns,
                },
                GdxInputEvent::Error(message) => UnifiedInputEvent::Error(message),
                GdxInputEvent::Disconnected { device_name } => UnifiedInputEvent::Disconnected {
                    device_name,
                    battery_percent: None,
                },
            }),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    config: OutputConfig,
    preferences: PreferencesSnapshot,
    has_saved_preferences: bool,
    platform: &'static str,
    metric_catalog: Vec<MetricDescriptor>,
    source_palettes: Vec<SourcePalette>,
    lab_recorder: LabRecorderCapability,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MetricDescriptor {
    #[serde(flatten)]
    metric: MetricDefinition,
    formula: &'static str,
    formula_template: Option<&'static str>,
    formula_source: &'static str,
    sources: Vec<MetricCitation>,
}

#[tauri::command]
fn get_bootstrap(state: State<'_, AppState>) -> Bootstrap {
    let preferences = state.preferences.snapshot();
    Bootstrap {
        config: preferences.output_config.clone(),
        preferences,
        has_saved_preferences: state.preferences.has_saved_preferences(),
        platform: std::env::consts::OS,
        metric_catalog: METRIC_CATALOG
            .iter()
            .copied()
            .map(|metric| {
                let formula = metric_formula_definition(metric.id);
                MetricDescriptor {
                    metric,
                    formula: formula.formula,
                    formula_template: formula.formula_template,
                    formula_source: formula.formula_source,
                    sources: metric_citations(metric),
                }
            })
            .collect(),
        source_palettes: source_palette_catalog(),
        lab_recorder: LabRecorderInstallation::capability(state.lab_recorder.as_ref()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyPreferences {
    output_config: Option<OutputConfig>,
    last_device: Option<SavedDevice>,
    #[serde(default)]
    device_palettes: HashMap<String, String>,
}

#[tauri::command]
async fn migrate_legacy_preferences(
    state: State<'_, AppState>,
    legacy: LegacyPreferences,
) -> CommandResult<PreferencesSnapshot> {
    if state.preferences.has_saved_preferences() {
        return Ok(state.preferences.snapshot());
    }
    let output_config = legacy
        .output_config
        .map(OutputConfig::migrated)
        .transpose()
        .map_err(|message| CommandError::new("LEGACY_PREFERENCES_INVALID", message, false))?;
    let last_device = legacy.last_device.map(validate_saved_device).transpose()?;
    let device_palettes = legacy
        .device_palettes
        .into_iter()
        .filter(|(device_id, palette_id)| {
            !device_id.is_empty() && device_id.len() <= 512 && source_palette(palette_id).is_some()
        })
        .collect();
    state
        .preferences
        .migrate_legacy(output_config, last_device, device_palettes)
        .await
        .map_err(|message| CommandError::new("PREFERENCES_WRITE_FAILED", message, true))
}

fn validate_saved_device(device: SavedDevice) -> CommandResult<SavedDevice> {
    if !device.is_valid() {
        return Err(CommandError::new(
            "LEGACY_PREFERENCES_INVALID",
            "The legacy preferred-sensor record is invalid.",
            false,
        ));
    }
    Ok(device)
}

#[tauri::command]
async fn save_device_palette(
    state: State<'_, AppState>,
    device_id: String,
    palette_id: String,
) -> CommandResult<()> {
    if device_id.is_empty() || device_id.len() > 512 || source_palette(&palette_id).is_none() {
        return Err(CommandError::new(
            "SOURCE_PALETTE_PREFERENCE_INVALID",
            "The device identity or source palette is invalid.",
            false,
        ));
    }
    state
        .preferences
        .save_device_palette(device_id, palette_id)
        .await
        .map_err(|message| CommandError::new("PREFERENCES_WRITE_FAILED", message, true))
}

#[tauri::command]
async fn scan_devices(
    state: State<'_, AppState>,
    preferred_device_id: Option<String>,
) -> CommandResult<Vec<DeviceSummary>> {
    let _configuration = state.input_configuration.lock().await;
    let active_kinds = state
        .active_sources
        .lock()
        .await
        .values()
        .map(|source| source.input_kind)
        .collect::<Vec<_>>();
    let preferred_kind = preferred_device_id
        .as_deref()
        .and_then(|device_id| parse_device_id(device_id).ok())
        .map(|(kind, _)| kind);
    let (scan_polar_enabled, scan_vernier_enabled) = input_scan_plan(&active_kinds, preferred_kind);
    let scan_polar = async || {
        state.polar_input.scan().await.map(|found| {
            found
                .into_iter()
                .map(|device| DeviceSummary {
                    id: format!("polar:{}", device.id),
                    name: device.name,
                    rssi: device.rssi,
                    input_kind: InputKind::PolarH10,
                    model_code: "H10".into(),
                    detail: "Polar H10 · ECG + accelerometer".into(),
                    respiration_belt_candidate: false,
                })
                .collect::<Vec<_>>()
        })
    };
    let scan_vernier = async || {
        state.gdx_input.scan().await.map(|found| {
            found
                .into_iter()
                .map(|device| DeviceSummary {
                    id: format!("vernier:{}", device.id),
                    name: device.name,
                    rssi: device.rssi,
                    input_kind: InputKind::VernierGoDirect,
                    model_code: device.model_code.into(),
                    detail: if device.respiration_belt_candidate {
                        format!("{} · respiration belt · Force (N)", device.model_code)
                    } else {
                        format!(
                            "{} · model and channels verified on connection",
                            device.model_name
                        )
                    },
                    respiration_belt_candidate: device.respiration_belt_candidate,
                })
                .collect::<Vec<_>>()
        })
    };
    let polar_scan = async {
        if scan_polar_enabled {
            scan_polar().await
        } else {
            Ok(Vec::new())
        }
    };
    let vernier_scan = async {
        if scan_vernier_enabled {
            scan_vernier().await
        } else {
            Ok(Vec::new())
        }
    };
    let (polar, vernier) = tokio::join!(polar_scan, vernier_scan);
    let mut devices = Vec::new();
    if let Ok(found) = &polar {
        devices.extend(found.iter().cloned());
    }
    if let Ok(found) = &vernier {
        devices.extend(found.iter().cloned());
    }
    if devices.is_empty() {
        let message = format!(
            "Polar scan: {}; Go Direct scan: {}",
            polar.err().unwrap_or_else(|| "no matching sensors".into()),
            vernier
                .err()
                .unwrap_or_else(|| "no matching sensors".into())
        );
        return Err(CommandError::new("BLUETOOTH_SCAN_FAILED", message, true));
    }
    devices.sort_by(|left, right| {
        right
            .rssi
            .cmp(&left.rssi)
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(devices)
}

fn input_scan_plan(active_kinds: &[InputKind], preferred_kind: Option<InputKind>) -> (bool, bool) {
    let _ = active_kinds;
    match preferred_kind {
        Some(InputKind::PolarH10) => (true, false),
        Some(InputKind::VernierGoDirect) => (false, true),
        None => (true, true),
    }
}

#[tauri::command]
async fn attach_active_sources(
    state: State<'_, AppState>,
    events: Channel<AppEvent>,
) -> CommandResult<Vec<SourceDescriptor>> {
    let sources = state.active_sources.lock().await.clone();
    let endpoints = state.display_endpoints.lock().await;
    for source_id in sources.keys() {
        if let Some(endpoint) = endpoints.get(source_id) {
            endpoint.attach(events.clone());
        }
    }
    Ok(sources.into_values().collect())
}

#[tauri::command]
async fn connect_device(
    state: State<'_, AppState>,
    device_id: String,
    palette_id: Option<String>,
    events: Channel<AppEvent>,
) -> CommandResult<SourceDescriptor> {
    let _configuration = state.input_configuration.lock().await;
    let (input_kind, raw_device_id) = parse_device_id(&device_id)?;
    let source = allocate_source(&state.active_sources, input_kind, palette_id.as_deref()).await?;
    let mut input_events = match input_kind {
        InputKind::PolarH10 => DeviceEventReceiver::Polar(
            state
                .polar_input
                .connect(&source.slot, raw_device_id)
                .await
                .map_err(|message| CommandError::new("POLAR_CONNECTION_FAILED", message, true))?,
        ),
        InputKind::VernierGoDirect => DeviceEventReceiver::Vernier(
            state
                .gdx_input
                .connect(&source.slot, raw_device_id, SessionConfig::default())
                .await
                .map_err(|message| CommandError::new("VERNIER_CONNECTION_FAILED", message, true))?,
        ),
    };
    let output = Arc::new(OutputRouter::with_bundled_lsl_and_recordings(
        state.bundled_lsl.clone(),
        state.recording_directory.clone(),
    ));
    // Serialize the read/configure/publish transition with renderer-driven
    // reconfiguration. A newly connected source must never start with a stale
    // snapshot after a newer global configuration has already committed.
    let output_configuration = state.output_configuration.lock().await;
    let config = match current_output_config(&state)
        .and_then(|config| source_output_config(config, &source))
    {
        Ok(config) => config,
        Err(error) => {
            match input_kind {
                InputKind::PolarH10 => {
                    let _ = state.polar_input.disconnect(&source.slot).await;
                }
                InputKind::VernierGoDirect => {
                    let _ = state.gdx_input.disconnect(&source.slot).await;
                }
            }
            return Err(error);
        }
    };
    if let Err(message) = output.configure(config).await {
        match input_kind {
            InputKind::PolarH10 => {
                let _ = state.polar_input.disconnect(&source.slot).await;
            }
            InputKind::VernierGoDirect => {
                let _ = state.gdx_input.disconnect(&source.slot).await;
            }
        }
        return Err(CommandError::new(
            "OUTPUT_CONFIGURATION_FAILED",
            message,
            true,
        ));
    }
    output.reset_measurement();
    state
        .source_outputs
        .lock()
        .await
        .insert(source.id.clone(), output.clone());
    state
        .active_sources
        .lock()
        .await
        .insert(source.id.clone(), source.clone());
    let display_endpoint = Arc::new(DisplayEndpoint::new(events));
    state
        .display_endpoints
        .lock()
        .await
        .insert(source.id.clone(), display_endpoint.clone());
    drop(output_configuration);
    let preferences = state.preferences.clone();
    let active_sources = state.active_sources.clone();
    let source_outputs = state.source_outputs.clone();
    let display_endpoints = state.display_endpoints.clone();
    let source_tasks = state.source_tasks.clone();
    let mut processing_settings = state.processing_settings.subscribe();
    let task_source = source.clone();
    let source_task_id = source.id.clone();
    let coordinator = tauri::async_runtime::spawn(async move {
        // Keep WebView serialization completely off the sensor/output path. A
        // slow or hidden renderer may lose display frames, but never raw LSL or
        // OSC data. Control events retain ordered delivery.
        let (ui_tx, mut ui_rx) = tokio::sync::mpsc::channel::<AppEvent>(8);
        let ui_task = tauri::async_runtime::spawn(async move {
            while let Some(event) = ui_rx.recv().await {
                display_endpoint.send(event);
            }
        });

        #[cfg(feature = "rusty-lsl-backend")]
        let lsl_poll_stop = Arc::new(AtomicBool::new(false));
        #[cfg(feature = "rusty-lsl-backend")]
        let lsl_poll_task = {
            let output = output.clone();
            let ui_tx = ui_tx.clone();
            let stop = lsl_poll_stop.clone();
            let poll_source = task_source.clone();
            tokio::task::spawn_blocking(move || {
                run_lsl_poll_loop(stop.as_ref(), LSL_POLL_INTERVAL, || {
                    if let Some(message) = output.poll_lsl() {
                        let _ = ui_tx.try_send(AppEvent::Error {
                            source: poll_source.clone(),
                            code: "LSL_TRANSPORT_WARNING",
                            message,
                        });
                    }
                });
            })
        };

        let settings = *processing_settings.borrow_and_update();
        let mut metrics_engine = MetricEngine::with_selection(settings.metrics);
        metrics_engine.apply_breathing_settings(settings.breathing);
        let mut vernier_breathing = VernierBreathingProcessor::default();
        let mut source_clock = SourceClockMapper::default();
        while let Some(event) = input_events.recv().await {
            if processing_settings.has_changed().unwrap_or(false) {
                let settings = *processing_settings.borrow_and_update();
                metrics_engine.apply_selection(settings.metrics);
                metrics_engine.apply_breathing_settings(settings.breathing);
            }
            let continue_streaming = match event {
                UnifiedInputEvent::Status { phase, message } => {
                    forward_control_event(
                        &ui_tx,
                        AppEvent::Status {
                            source: task_source.clone(),
                            phase: phase.into(),
                            message,
                        },
                    )
                    .await
                }
                UnifiedInputEvent::Connected {
                    device_name,
                    battery_percent,
                    device_model,
                    firmware_version,
                    sensor_number,
                    sensor_name,
                    sensor_unit,
                    sample_period_us,
                    vernier_sensors,
                } => {
                    let vernier_channel_count = vernier_sensors.as_ref().map(Vec::len);
                    if let (Some(model), Some(period), Some(sensors)) = (
                        device_model.as_deref(),
                        sample_period_us,
                        vernier_sensors.as_deref(),
                    ) {
                        vernier_breathing = VernierBreathingProcessor::default();
                        if let Err(message) =
                            output.configure_vernier_streams(model, period, sensors)
                        {
                            let _ = forward_display_event(
                                &ui_tx,
                                AppEvent::Error {
                                    source: task_source.clone(),
                                    code: "LSL_TRANSPORT_WARNING",
                                    message,
                                },
                            );
                        }
                    }
                    let save_preferences = preferences.clone();
                    let save_events = ui_tx.clone();
                    let save_source = task_source.clone();
                    let saved_device = SavedDevice {
                        id: device_id.clone(),
                        name: device_name.clone(),
                    };
                    tauri::async_runtime::spawn(async move {
                        if let Err(message) = save_preferences.save_last_device(saved_device).await
                        {
                            let _ = save_events
                                .send(AppEvent::Error {
                                    source: save_source,
                                    code: "PREFERENCES_WRITE_FAILED",
                                    message: format!(
                                        "Connected, but the preferred sensor could not be saved: {message}"
                                    ),
                                })
                                .await;
                        }
                    });
                    forward_control_event(
                        &ui_tx,
                        AppEvent::Connection {
                            source: task_source.clone(),
                            connected: true,
                            streaming: true,
                            device_name,
                            battery_percent,
                            device_model,
                            firmware_version,
                            sensor_number,
                            sensor_name: sensor_name.clone(),
                            sensor_unit: sensor_unit.clone(),
                            sample_period_us,
                            message: match task_source.input_kind {
                                InputKind::PolarH10 => {
                                    "Raw ECG and accelerometer are streaming".into()
                                }
                                InputKind::VernierGoDirect => format!(
                                    "Verified {} ({}) plus all {} device channels are streaming at a {:.1} Hz base period",
                                    sensor_name.as_deref().unwrap_or("Force"),
                                    sensor_unit.as_deref().unwrap_or("N"),
                                    vernier_channel_count.unwrap_or(1),
                                    sample_period_us
                                        .filter(|period| *period > 0)
                                        .map(|period| 1_000_000.0 / period as f64)
                                        .unwrap_or(10.0),
                                ),
                            },
                        },
                    )
                    .await
                }
                UnifiedInputEvent::Ecg {
                    sensor_timestamp_ns,
                    host_receive_timestamp_ns,
                    microvolts,
                } => {
                    let timing = TimingEnvelope::mapped_source(
                        &mut source_clock,
                        sensor_timestamp_ns,
                        host_receive_timestamp_ns,
                        ECG_SAMPLE_PERIOD_NS,
                    );
                    let output_open = forward_output_warning(
                        &ui_tx,
                        &task_source,
                        output.publish_ecg(sensor_timestamp_ns, &microvolts),
                    );
                    let formulas = output.process_ecg_formulas(sensor_timestamp_ns, &microvolts);
                    let derived = metrics_engine.process_ecg(&microvolts);
                    if !output_open
                        || !forward_display_event(
                            &ui_tx,
                            AppEvent::Ecg {
                                source: task_source.clone(),
                                sensor_timestamp_ns,
                                timing: timing.clone(),
                                microvolts,
                                formulas,
                            },
                        )
                    {
                        false
                    } else {
                        publish_metrics(
                            &output,
                            &ui_tx,
                            &task_source,
                            sensor_timestamp_ns,
                            timing,
                            derived,
                            FormulaPublishBatch::default(),
                        )
                    }
                }
                UnifiedInputEvent::Accelerometer {
                    sensor_timestamp_ns,
                    host_receive_timestamp_ns,
                    samples,
                } => {
                    let mapping = source_clock
                        .observe_and_map(sensor_timestamp_ns, host_receive_timestamp_ns);
                    let timing = TimingEnvelope::from_mapping(
                        Some(sensor_timestamp_ns),
                        host_receive_timestamp_ns,
                        ACC_SAMPLE_PERIOD_NS,
                        mapping,
                    );
                    let output_open = forward_output_warning(
                        &ui_tx,
                        &task_source,
                        output.publish_accelerometer(sensor_timestamp_ns, &samples),
                    );
                    let formulas =
                        output.process_accelerometer_formulas(sensor_timestamp_ns, &samples);
                    let derived = metrics_engine.process_accelerometer_timed(
                        &samples,
                        timed_acc_batch(sensor_timestamp_ns, mapping),
                    );
                    let breathing_presentation_points = breathing_presentation_points(
                        metrics_engine.take_breathing_presentation_points(),
                    );
                    if !output_open
                        || !forward_display_event(
                            &ui_tx,
                            AppEvent::Accelerometer {
                                source: task_source.clone(),
                                sensor_timestamp_ns,
                                timing: timing.clone(),
                                samples,
                                formulas,
                                breathing_presentation_points,
                            },
                        )
                    {
                        false
                    } else {
                        publish_metrics(
                            &output,
                            &ui_tx,
                            &task_source,
                            sensor_timestamp_ns,
                            timing,
                            derived,
                            FormulaPublishBatch::default(),
                        )
                    }
                }
                UnifiedInputEvent::HeartRate {
                    host_receive_timestamp_ns,
                    beats_per_minute,
                    rr_intervals_ms,
                } => {
                    let timing = TimingEnvelope::arrival(host_receive_timestamp_ns, None, false);
                    let output_open = forward_output_warning(
                        &ui_tx,
                        &task_source,
                        output.publish_heart_rate(beats_per_minute, &rr_intervals_ms),
                    );
                    let formulas =
                        output.process_heart_rate_formulas(beats_per_minute, &rr_intervals_ms);
                    output_open
                        && publish_metrics(
                            &output,
                            &ui_tx,
                            &task_source,
                            0,
                            timing,
                            metrics_engine.process_heart_rate(beats_per_minute, &rr_intervals_ms),
                            formulas,
                        )
                }
                UnifiedInputEvent::VernierSamples {
                    encoding,
                    sensors,
                    host_receive_timestamp_ns,
                    sample_period_us,
                    sequence,
                    dropped_before,
                    device_drop_reports_before,
                    decode_latency_ns,
                } => {
                    let timing = TimingEnvelope::arrival(
                        host_receive_timestamp_ns,
                        Some(u64::from(sample_period_us) * 1_000),
                        dropped_before > 0 || device_drop_reports_before > 0,
                    );
                    output.publish_vernier_raw(
                        host_receive_timestamp_ns,
                        sample_period_us,
                        sequence,
                        dropped_before,
                        device_drop_reports_before,
                        decode_latency_ns,
                        encoding,
                        &sensors,
                    );
                    if let Some(force) = sensors.iter().find(|samples| samples.sensor_number == 1) {
                        let values = force
                            .values
                            .iter()
                            .map(|value| *value as f32)
                            .collect::<Vec<_>>();
                        let output_open = forward_output_warning(
                            &ui_tx,
                            &task_source,
                            output.publish_force(
                                host_receive_timestamp_ns,
                                &values,
                                sample_period_us,
                            ),
                        );
                        let waveform = vernier_breathing.push(&force.values, sample_period_us);
                        output.publish_vernier_breathing(
                            host_receive_timestamp_ns,
                            &waveform,
                            sample_period_us,
                        );
                        output_open
                            && forward_display_event(
                                &ui_tx,
                                AppEvent::Force {
                                    source: task_source.clone(),
                                    timing,
                                    sensor_number: force.sensor_number,
                                    host_receive_timestamp_ns,
                                    sample_period_us,
                                    sequence,
                                    values,
                                    breathing_values: waveform,
                                    dropped_before,
                                    decode_latency_ns,
                                },
                            )
                    } else {
                        true
                    }
                }
                UnifiedInputEvent::StreamHealth {
                    notifications,
                    samples,
                    sample_rows,
                    malformed_frames,
                    dropped_batches,
                    device_drop_reports,
                    queue_high_water,
                    decode_latency_p50_ns,
                    decode_latency_p95_ns,
                    decode_latency_p99_ns,
                    max_decode_latency_ns,
                } => forward_display_event(
                    &ui_tx,
                    AppEvent::StreamHealth {
                        source: task_source.clone(),
                        notifications,
                        samples,
                        sample_rows,
                        malformed_frames,
                        dropped_batches,
                        device_drop_reports,
                        queue_high_water,
                        decode_latency_p50_ns,
                        decode_latency_p95_ns,
                        decode_latency_p99_ns,
                        max_decode_latency_ns,
                    },
                ),
                UnifiedInputEvent::Error(message) => forward_display_event(
                    &ui_tx,
                    AppEvent::Error {
                        source: task_source.clone(),
                        code: "SENSOR_DATA_WARNING",
                        message,
                    },
                ),
                UnifiedInputEvent::Disconnected {
                    device_name,
                    battery_percent,
                } => {
                    let phase_sent = publish_metrics(
                        &output,
                        &ui_tx,
                        &task_source,
                        0,
                        TimingEnvelope::arrival(monotonic_now_ns(), None, true),
                        vec![
                            MetricSample {
                                id: "breathing_phase",
                                value: 0.0,
                            },
                            MetricSample {
                                id: "breathing_signal_confidence",
                                value: 0.0,
                            },
                            MetricSample {
                                id: "breathing_signal_ready",
                                value: 0.0,
                            },
                        ],
                        FormulaPublishBatch::default(),
                    );
                    phase_sent
                        && forward_control_event(
                            &ui_tx,
                            AppEvent::Connection {
                                source: task_source.clone(),
                                connected: false,
                                streaming: false,
                                device_name,
                                battery_percent,
                                device_model: None,
                                firmware_version: None,
                                sensor_number: None,
                                sensor_name: None,
                                sensor_unit: None,
                                sample_period_us: None,
                                message: "Disconnected".into(),
                            },
                        )
                        .await
                }
            };
            if !continue_streaming {
                break;
            }
        }
        #[cfg(feature = "rusty-lsl-backend")]
        {
            if let Err(message) =
                stop_and_join_lsl_poll(lsl_poll_stop.as_ref(), lsl_poll_task, LSL_POLL_JOIN_TIMEOUT)
                    .await
            {
                let _ = ui_tx.try_send(AppEvent::Error {
                    source: task_source.clone(),
                    code: "LSL_TRANSPORT_WARNING",
                    message: message.into(),
                });
            }
        }
        drop(ui_tx);
        let _ = ui_task.await;
        active_sources.lock().await.remove(&task_source.id);
        source_outputs.lock().await.remove(&task_source.id);
        display_endpoints.lock().await.remove(&task_source.id);
        source_tasks.lock().await.remove(&task_source.id);
    });
    state
        .source_tasks
        .lock()
        .await
        .insert(source_task_id, coordinator);
    Ok(source)
}

fn publish_metrics(
    output: &OutputRouter,
    ui_tx: &tokio::sync::mpsc::Sender<AppEvent>,
    source: &SourceDescriptor,
    sensor_timestamp_ns: u64,
    timing: TimingEnvelope,
    values: Vec<MetricSample>,
    formulas: FormulaPublishBatch,
) -> bool {
    if values.is_empty()
        && formulas.series.is_empty()
        && formulas.faults.is_empty()
        && formulas.warnings.is_empty()
    {
        return true;
    }
    let routed = values
        .iter()
        .map(|metric| MetricValue {
            id: metric.id,
            value: metric.value,
        })
        .collect::<Vec<_>>();
    let output_open = forward_output_warning(
        ui_tx,
        source,
        output.publish_metrics_at(sensor_timestamp_ns, &routed),
    );
    output_open
        && forward_display_event(
            ui_tx,
            AppEvent::Metrics {
                source: source.clone(),
                timing,
                values,
                formulas,
            },
        )
}

fn forward_output_warning(
    ui_tx: &tokio::sync::mpsc::Sender<AppEvent>,
    source: &SourceDescriptor,
    message: Option<String>,
) -> bool {
    message.is_none_or(|message| {
        forward_display_event(
            ui_tx,
            AppEvent::Error {
                source: source.clone(),
                code: "CSV_RECORDING_STOPPED",
                message,
            },
        )
    })
}

fn forward_display_event(ui_tx: &tokio::sync::mpsc::Sender<AppEvent>, event: AppEvent) -> bool {
    match ui_tx.try_send(event) {
        Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => true,
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => true,
    }
}

async fn forward_control_event(
    ui_tx: &tokio::sync::mpsc::Sender<AppEvent>,
    event: AppEvent,
) -> bool {
    let _ = ui_tx.send(event).await;
    true
}

#[tauri::command]
async fn disconnect_device(state: State<'_, AppState>, source_id: String) -> CommandResult<()> {
    let _configuration = state.input_configuration.lock().await;
    let source = state
        .active_sources
        .lock()
        .await
        .get(&source_id)
        .cloned()
        .ok_or_else(|| CommandError::new("UNKNOWN_SOURCE", "That source is not active.", false))?;
    let result = match source.input_kind {
        InputKind::PolarH10 => state.polar_input.disconnect(&source.slot).await,
        InputKind::VernierGoDirect => state.gdx_input.disconnect(&source.slot).await,
    };
    result.map_err(|message| CommandError::new("SENSOR_DISCONNECT_FAILED", message, true))?;
    if let Some(mut task) = state.source_tasks.lock().await.remove(&source_id)
        && tokio::time::timeout(Duration::from_secs(3), &mut task)
            .await
            .is_err()
    {
        task.abort();
    }
    state.active_sources.lock().await.remove(&source_id);
    if let Some(output) = state.source_outputs.lock().await.remove(&source_id) {
        output.reset_measurement();
    }
    state.display_endpoints.lock().await.remove(&source_id);
    Ok(())
}

#[tauri::command]
async fn update_source_palette(
    state: State<'_, AppState>,
    source_id: String,
    palette_id: String,
) -> CommandResult<SourcePaletteUpdate> {
    let _input_configuration = state.input_configuration.lock().await;
    let _output_configuration = state.output_configuration.lock().await;
    let palette = source_palette(&palette_id).ok_or_else(|| {
        CommandError::new(
            "UNKNOWN_SOURCE_PALETTE",
            format!("Unknown source palette: {palette_id}"),
            false,
        )
    })?;
    let sources = state.active_sources.lock().await.clone();
    if sources
        .iter()
        .any(|(id, source)| id != &source_id && source.palette.id == palette.id)
    {
        return Err(CommandError::new(
            "SOURCE_PALETTE_IN_USE",
            "That color pair is already assigned to another connected source.",
            true,
        ));
    }
    let mut source = sources.get(&source_id).cloned().ok_or_else(|| {
        CommandError::new(
            "SOURCE_NOT_FOUND",
            "The source disconnected before its palette could be changed.",
            true,
        )
    })?;
    let output = state
        .source_outputs
        .lock()
        .await
        .get(&source_id)
        .cloned()
        .ok_or_else(|| {
            CommandError::new(
                "SOURCE_OUTPUT_NOT_FOUND",
                "The source output router is unavailable.",
                true,
            )
        })?;
    if source.palette == palette {
        return Ok(SourcePaletteUpdate {
            source,
            health: output.health(),
        });
    }

    let previous = output.config();
    let gated = palette_output_boundary(previous.clone());
    output.configure(gated).await.map_err(|message| {
        CommandError::new(
            "SOURCE_PALETTE_GATE_FAILED",
            format!("Could not stop the affected outputs before changing color: {message}"),
            true,
        )
    })?;

    source.color = palette.light.primary.clone();
    source.palette = palette;
    state
        .active_sources
        .lock()
        .await
        .insert(source_id, source.clone());
    let next = source_output_config(current_output_config(&state)?, &source)?;
    let health = match output.configure(next.clone()).await {
        Ok(health) => health,
        Err(message) => {
            let degraded = palette_output_boundary(next);
            let _ = output.configure(degraded).await;
            return Err(CommandError::new(
                "SOURCE_PALETTE_RESTART_FAILED",
                format!(
                    "The palette changed, but LSL/CSV could not be restarted. Acquisition remains active and the affected destinations are off: {message}"
                ),
                true,
            ));
        }
    };
    Ok(SourcePaletteUpdate { source, health })
}

fn palette_output_boundary(mut config: OutputConfig) -> OutputConfig {
    config.lsl_enabled = false;
    config.csv_enabled = false;
    config
}

fn parse_device_id(device_id: &str) -> CommandResult<(InputKind, &str)> {
    if let Some(id) = device_id.strip_prefix("polar:") {
        Ok((InputKind::PolarH10, id))
    } else if let Some(id) = device_id.strip_prefix("vernier:") {
        Ok((InputKind::VernierGoDirect, id))
    } else {
        Err(CommandError::new(
            "UNKNOWN_SENSOR_KIND",
            "The selected device does not carry a supported sensor kind.",
            false,
        ))
    }
}

async fn allocate_source(
    active: &tokio::sync::Mutex<HashMap<String, SourceDescriptor>>,
    input_kind: InputKind,
    requested_palette_id: Option<&str>,
) -> CommandResult<SourceDescriptor> {
    let active = active.lock().await;
    let palettes = source_palette_catalog();
    let used_palettes = active
        .values()
        .map(|source| source.palette.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let requested = match requested_palette_id {
        Some(id) => Some(source_palette(id).ok_or_else(|| {
            CommandError::new(
                "INVALID_SOURCE_PALETTE",
                format!("Unknown source palette `{id}`."),
                true,
            )
        })?),
        None => None,
    }
    .filter(|palette| !used_palettes.contains(palette.id.as_str()));
    let palette = requested.or_else(|| {
        palettes
            .into_iter()
            .find(|palette| !used_palettes.contains(palette.id.as_str()))
    });
    let Some(palette) = palette else {
        return Err(CommandError::new(
            "SOURCE_PALETTE_CAPACITY_REACHED",
            "Every source palette is already assigned to an active source.",
            true,
        ));
    };
    for index in 0..MAX_INPUT_SOURCES {
        let slot = format!("source-{}", index + 1);
        if !active.values().any(|source| source.slot == slot) {
            return Ok(SourceDescriptor {
                id: format!("source-instance-{}", Uuid::new_v4().simple()),
                slot,
                label: format!("Source {}", index + 1),
                color: palette.light.primary.clone(),
                palette,
                input_kind,
            });
        }
    }
    Err(CommandError::new(
        "INPUT_CAPACITY_REACHED",
        format!("At most {MAX_INPUT_SOURCES} simultaneous sources are supported."),
        true,
    ))
}

fn source_output_config(
    mut config: OutputConfig,
    source: &SourceDescriptor,
) -> CommandResult<OutputConfig> {
    config.stream_name = format!("{}_{}", config.stream_name, source.slot);
    config.source_palette = Some(source.palette.clone());
    match source.input_kind {
        InputKind::PolarH10 => {
            config.outputs.retain(|id| id != "raw_force");
        }
        InputKind::VernierGoDirect => {
            config.outputs.retain(|id| id == "raw_force");
            config.custom_formulas.clear();
        }
    }
    config
        .metric_options
        .retain(|id, _| config.outputs.contains(id));
    config
        .validated()
        .map_err(|message| CommandError::new("OUTPUT_CONFIGURATION_FAILED", message, true))
}

fn current_output_config(state: &AppState) -> CommandResult<OutputConfig> {
    state
        .output_config
        .read()
        .map(|config| config.clone())
        .map_err(|_| {
            CommandError::new(
                "OUTPUT_CONFIGURATION_FAILED",
                "The output configuration lock is unavailable.",
                true,
            )
        })
}

#[tauri::command]
fn validate_custom_formula(
    formula: CustomFormulaConfig,
) -> Result<FormulaValidation, FormulaError> {
    validate_formula(formula)
}

#[tauri::command]
async fn update_output_config(
    state: State<'_, AppState>,
    config: OutputConfig,
) -> CommandResult<OutputHealth> {
    // Renderer updates can overlap when a name debounce and a destination
    // toggle fire close together. Serialize the rare reconfiguration lifecycle
    // so an older async OSC setup cannot overwrite a newer selection.
    let _configuration = state.output_configuration.lock().await;
    let applied = OutputRouter::validate_config(config)
        .map_err(|message| CommandError::new("OUTPUT_CONFIGURATION_FAILED", message, false))?;
    let sources = state.active_sources.lock().await.clone();
    let outputs = state.source_outputs.lock().await.clone();
    let mut configured: Vec<(Arc<OutputRouter>, OutputConfig)> = Vec::new();
    for (source_id, output) in &outputs {
        let Some(source) = sources.get(source_id) else {
            continue;
        };
        let previous = output.config();
        let next = match source_output_config(applied.clone(), source) {
            Ok(next) => next,
            Err(error) => {
                for (configured_output, previous_config) in configured.into_iter().rev() {
                    let _ = configured_output.configure(previous_config).await;
                }
                return Err(error);
            }
        };
        if let Err(message) = output.configure(next).await {
            let mut rollback_failures = 0;
            for (configured_output, previous_config) in configured.into_iter().rev() {
                if configured_output.configure(previous_config).await.is_err() {
                    rollback_failures += 1;
                }
            }
            let rollback = if rollback_failures == 0 {
                "All previously updated sources were restored.".to_string()
            } else {
                format!(
                    "{rollback_failures} source rollback(s) also failed; disconnect affected outputs before retrying."
                )
            };
            return Err(CommandError::new(
                "OUTPUT_CONFIGURATION_FAILED",
                format!("{message} {rollback}"),
                true,
            ));
        }
        configured.push((output.clone(), previous));
    }
    state
        .output_config
        .write()
        .map_err(|_| {
            CommandError::new(
                "OUTPUT_CONFIGURATION_FAILED",
                "The output configuration lock is unavailable.",
                true,
            )
        })?
        .clone_from(&applied);
    let source_count = sources.len();
    let route_status = |enabled: bool| {
        if !enabled {
            "Off".into()
        } else if source_count == 0 {
            "Waiting for a connected source".into()
        } else {
            format!("Active for {source_count} source(s)")
        }
    };
    let health = OutputHealth {
        stream_name: applied.stream_name.clone(),
        lsl: route_status(applied.lsl_enabled),
        osc: route_status(applied.osc_enabled),
        csv: route_status(applied.csv_enabled),
        audio: route_status(applied.audio_enabled),
        formulas: outputs
            .values()
            .next()
            .map(|output| output.formula_health())
            .unwrap_or_default(),
    };
    state
        .processing_settings
        .send_replace(ProcessingSettings::from_config(&applied));
    state
        .preferences
        .save_output_config(applied)
        .await
        .map_err(|message| {
            CommandError::new(
                "PREFERENCES_WRITE_FAILED",
                format!("Outputs are active for this session, but could not be saved: {message}"),
                true,
            )
        })?;
    Ok(health)
}

#[tauri::command(async)]
fn open_metric_citation(
    app: tauri::AppHandle,
    metric_id: String,
    source_url: String,
) -> CommandResult<()> {
    let metric = MetricDefinition::for_id(&metric_id).ok_or_else(|| {
        CommandError::new(
            "UNKNOWN_METRIC",
            "That metric is not in the catalog.",
            false,
        )
    })?;
    let allowed_url = reviewed_metric_source(metric, &source_url).ok_or_else(|| {
        CommandError::new(
            "UNKNOWN_METRIC_SOURCE",
            "That source is not in the selected metric's reviewed source list.",
            false,
        )
    })?;
    if !allowed_url.starts_with("https://") {
        return Err(CommandError::new(
            "UNSAFE_CITATION_URL",
            "The citation URL was not opened because it is not HTTPS.",
            false,
        ));
    }
    app.opener()
        .open_url(allowed_url, None::<&str>)
        .map_err(|message| {
            CommandError::new(
                "CITATION_OPEN_FAILED",
                format!("Could not open the citation in the system browser: {message}"),
                true,
            )
        })
}

fn reviewed_metric_source(metric: MetricDefinition, requested_url: &str) -> Option<&'static str> {
    metric_citations(metric)
        .into_iter()
        .find(|citation| citation.url == requested_url)
        .map(|citation| citation.url)
}

#[tauri::command]
fn open_lab_recorder(state: State<'_, AppState>) -> CommandResult<LabRecorderLaunch> {
    state
        .lab_recorder
        .as_ref()
        .ok_or_else(unavailable_error)?
        .open()
}

async fn graceful_shutdown(app: tauri::AppHandle) {
    let state = app.state::<AppState>();
    let _configuration = state.input_configuration.lock().await;
    let sources = state.active_sources.lock().await.clone();
    for source in sources.values() {
        match source.input_kind {
            InputKind::PolarH10 => {
                let _ = state.polar_input.disconnect(&source.slot).await;
            }
            InputKind::VernierGoDirect => {
                let _ = state.gdx_input.disconnect(&source.slot).await;
            }
        }
    }

    let tasks = std::mem::take(&mut *state.source_tasks.lock().await);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    for (_, mut task) in tasks {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() || tokio::time::timeout(remaining, &mut task).await.is_err() {
            task.abort();
        }
    }
    for output in state.source_outputs.lock().await.values() {
        output.reset_measurement();
    }
    state.source_outputs.lock().await.clear();
    state.active_sources.lock().await.clear();
    state.display_endpoints.lock().await.clear();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                // Keep the runtime taskbar/pinned-window icon explicit. The
                // native bundle still uses the full ICO/ICNS/PNG icon sets.
                window.set_icon(tauri::include_image!("icons/64x64.png"))?;
            }
            let bundled_lsl = app
                .path()
                .resolve(lsl_resource_path(), BaseDirectory::Resource)
                .ok()
                .filter(|path| path.is_file());
            let lab_recorder = app
                .path()
                .resource_dir()
                .ok()
                .and_then(|path| LabRecorderInstallation::locate(&path));
            let preferences_path = app.path().app_config_dir()?.join("preferences.json");
            let recording_directory = app
                .path()
                .download_dir()
                .map(|path| path.join("Polar Stream"))
                .unwrap_or_else(|_| {
                    app.path()
                        .app_data_dir()
                        .unwrap_or_else(|_| {
                            preferences_path
                                .parent()
                                .unwrap_or_else(|| std::path::Path::new("."))
                                .to_path_buf()
                        })
                        .join("recordings")
                });
            app.manage(AppState::new(
                bundled_lsl,
                lab_recorder,
                preferences_path,
                recording_directory,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_bootstrap,
            migrate_legacy_preferences,
            save_device_palette,
            scan_devices,
            connect_device,
            attach_active_sources,
            disconnect_device,
            update_source_palette,
            update_output_config,
            validate_custom_formula,
            open_metric_citation,
            open_lab_recorder,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Polar Stream");
    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
            let state = app.state::<AppState>();
            if !state.shutdown_started.swap(true, Ordering::AcqRel) {
                api.prevent_exit();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    graceful_shutdown(app.clone()).await;
                    app.exit(code.unwrap_or(0));
                });
            }
        }
    });
}

#[cfg(target_os = "windows")]
fn lsl_resource_path() -> &'static str {
    "lsl.dll"
}

#[cfg(target_os = "linux")]
fn lsl_resource_path() -> &'static str {
    "liblsl.so"
}

#[cfg(target_os = "macos")]
fn lsl_resource_path() -> &'static str {
    "liblsl.dylib"
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn lsl_resource_path() -> &'static str {
    "liblsl"
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "rusty-lsl-backend")]
    use std::sync::atomic::AtomicUsize;

    fn test_source(
        id: &str,
        slot: &str,
        palette_id: &str,
        input_kind: InputKind,
    ) -> SourceDescriptor {
        let palette = source_palette(palette_id).expect("test palette must exist");
        SourceDescriptor {
            id: id.into(),
            slot: slot.into(),
            label: slot.replace('-', " "),
            color: palette.light.primary.clone(),
            palette,
            input_kind,
        }
    }

    #[test]
    fn citation_opener_accepts_only_reviewed_sources_for_the_selected_metric() {
        let metric = MetricDefinition::for_id("rmssd").expect("RMSSD must remain in the catalog");
        let reviewed = metric_citations(metric);

        assert_eq!(
            reviewed_metric_source(metric, reviewed[0].url),
            Some(reviewed[0].url)
        );
        assert_eq!(reviewed_metric_source(metric, "https://example.com"), None);

        let other_metric = MetricDefinition::for_id("raw_force")
            .expect("raw Go Direct force must remain in the catalog");
        assert_eq!(
            reviewed_metric_source(metric, metric_citations(other_metric)[0].url),
            None
        );
    }

    #[test]
    fn source_outputs_are_filtered_to_the_connected_sensor_kind() {
        let config = OutputConfig {
            outputs: vec!["raw_ecg".into(), "raw_acc".into(), "raw_force".into()],
            ..OutputConfig::default()
        };
        let polar = test_source("source-1", "source-1", "ocean", InputKind::PolarH10);
        let vernier = test_source("source-2", "source-2", "sunset", InputKind::VernierGoDirect);

        let polar_config = source_output_config(config.clone(), &polar).unwrap();
        assert_eq!(polar_config.outputs.len(), 2);
        assert!(polar_config.outputs.iter().any(|id| id == "raw_ecg"));
        assert!(polar_config.outputs.iter().any(|id| id == "raw_acc"));
        assert_eq!(polar_config.stream_name, "Polar-H10_source-1");
        assert_eq!(polar_config.source_palette.as_ref().unwrap().id, "ocean");

        let vernier_config = source_output_config(config, &vernier).unwrap();
        assert_eq!(vernier_config.outputs, ["raw_force"]);
        assert_eq!(vernier_config.stream_name, "Polar-H10_source-2");
        assert_eq!(vernier_config.source_palette.as_ref().unwrap().id, "sunset");
    }

    #[tokio::test]
    async fn source_palette_allocation_is_deterministic_and_resolves_remembered_conflicts() {
        let active = tokio::sync::Mutex::new(HashMap::new());
        let first = allocate_source(&active, InputKind::PolarH10, Some("ocean"))
            .await
            .expect("first source");
        assert_eq!(first.slot, "source-1");
        assert_eq!(first.palette.id, "ocean");
        active.lock().await.insert(first.id.clone(), first);

        let conflict = allocate_source(&active, InputKind::VernierGoDirect, Some("ocean"))
            .await
            .expect("conflicting remembered palette falls back");
        assert_eq!(conflict.slot, "source-2");
        assert_eq!(conflict.palette.id, "sunset");
    }

    #[tokio::test]
    async fn source_palette_allocation_rejects_unknown_ids_and_capacity_overflow() {
        let active = tokio::sync::Mutex::new(HashMap::new());
        assert!(
            allocate_source(&active, InputKind::PolarH10, Some("not-a-palette"))
                .await
                .is_err()
        );
        for (index, palette) in source_palette_catalog().into_iter().enumerate() {
            let source = test_source(
                &format!("active-{index}"),
                &format!("source-{}", index + 1),
                &palette.id,
                InputKind::PolarH10,
            );
            active.lock().await.insert(source.id.clone(), source);
        }
        assert!(
            allocate_source(&active, InputKind::PolarH10, None)
                .await
                .is_err()
        );
    }

    #[test]
    fn palette_output_boundary_stops_only_metadata_bearing_destinations() {
        let config = OutputConfig {
            lsl_enabled: true,
            csv_enabled: true,
            osc_enabled: true,
            audio_enabled: true,
            ..OutputConfig::default()
        };
        let gated = palette_output_boundary(config);
        assert!(!gated.lsl_enabled);
        assert!(!gated.csv_enabled);
        assert!(gated.osc_enabled);
        assert!(gated.audio_enabled);
    }

    #[test]
    fn discovery_can_refresh_any_family_while_sources_remain_active() {
        assert_eq!(input_scan_plan(&[], None), (true, true));
        assert_eq!(input_scan_plan(&[InputKind::PolarH10], None), (true, true));
        assert_eq!(
            input_scan_plan(&[InputKind::VernierGoDirect], None),
            (true, true)
        );
        assert_eq!(
            input_scan_plan(&[InputKind::PolarH10, InputKind::VernierGoDirect], None),
            (true, true)
        );
        assert_eq!(
            input_scan_plan(&[InputKind::PolarH10], Some(InputKind::VernierGoDirect)),
            (false, true)
        );
        assert_eq!(
            input_scan_plan(&[InputKind::PolarH10], Some(InputKind::PolarH10)),
            (true, false)
        );
    }

    #[test]
    fn timed_acc_batch_preserves_pmd_and_clock_evidence() {
        let batch = timed_acc_batch(
            42_000_000,
            ClockMapping {
                mapped_time_ns: 8_000_000,
                revision: 7,
                quality: polar_stream_time::ClockQuality::Tracking,
                uncertainty_ns: 2_000_000,
                reset: true,
            },
        );

        assert_eq!(batch.newest_sensor_timestamp_ns, 42_000_000);
        assert_eq!(batch.sample_period_ns, ACC_SAMPLE_PERIOD_NS);
        assert_eq!(batch.clock_revision, 7);
        assert!(batch.clock_reset);
        assert!(batch.gap_before);
    }

    #[test]
    fn empty_presentation_points_do_not_change_accelerometer_ipc_shape() {
        let event = AppEvent::Accelerometer {
            source: test_source("source-1", "source-1", "ocean", InputKind::PolarH10),
            sensor_timestamp_ns: 42_000_000,
            timing: TimingEnvelope::arrival(45_000_000, Some(ACC_SAMPLE_PERIOD_NS), false),
            samples: Vec::new(),
            formulas: FormulaPublishBatch::default(),
            breathing_presentation_points: Vec::new(),
        };

        let value = serde_json::to_value(event).expect("accelerometer event must serialize");
        assert!(value.get("breathingPresentationPoints").is_none());
    }

    #[test]
    fn presentation_points_keep_source_time_as_decimal_strings() {
        let points =
            breathing_presentation_points(vec![polar_h10_metrics::BreathingWaveformPoint {
                source_timestamp_ns: 9_876_543_210_123,
                volume_01: 0.75,
            }]);

        let value = serde_json::to_value(&points).expect("presentation points must serialize");
        assert_eq!(value[0]["sourceTimestampNs"], "9876543210123");
        assert_eq!(value[0]["volume01"], 0.75);
    }

    #[cfg(feature = "rusty-lsl-backend")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn rusty_lsl_polling_uses_the_blocking_pool_and_stops_within_a_bound() {
        let stop = Arc::new(AtomicBool::new(false));
        let polls = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let mut started_tx = Some(started_tx);
        let task = {
            let stop = stop.clone();
            let polls = polls.clone();
            tokio::task::spawn_blocking(move || {
                run_lsl_poll_loop(stop.as_ref(), Duration::from_millis(1), || {
                    polls.fetch_add(1, Ordering::Relaxed);
                    if let Some(sender) = started_tx.take() {
                        let _ = sender.send(());
                    }
                    std::thread::sleep(Duration::from_millis(40));
                });
            })
        };

        tokio::time::timeout(Duration::from_millis(200), started_rx)
            .await
            .expect("blocking poll loop must start")
            .expect("blocking poll loop must signal startup");
        tokio::time::timeout(
            Duration::from_millis(100),
            tokio::time::sleep(Duration::from_millis(10)),
        )
        .await
        .expect("Tokio timers must remain responsive while Rusty LSL polls");

        stop_and_join_lsl_poll(stop.as_ref(), task, Duration::from_millis(200))
            .await
            .expect("stop signal must terminate the blocking poll loop within its bound");
        assert!(polls.load(Ordering::Relaxed) >= 1);
    }
}
