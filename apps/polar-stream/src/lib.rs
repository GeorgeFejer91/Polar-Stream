//! Thin application coordinator. Protocol decoding, Bluetooth input, and
//! network output are independent crates below `crates/`.

mod error;
mod preferences;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};

#[cfg(feature = "rusty-lsl-backend")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "rusty-lsl-backend")]
use std::time::Duration;

use polar_h10_core::AccSample;
use polar_h10_input::{InputEvent as PolarInputEvent, InputSessionPool as PolarInputSessionPool};
use polar_h10_metrics::{
    BreathingSettings, METRIC_CATALOG, MetricCitation, MetricDefinition, MetricEngine,
    MetricSample, MetricSelection, metric_citations, metric_formula_definition,
};
use polar_h10_output::{
    CustomFormulaConfig, FormulaError, FormulaPublishBatch, FormulaValidation, validate_formula,
};
use polar_h10_output::{MetricValue, OutputConfig, OutputHealth, OutputRouter};
use serde::{Deserialize, Serialize};
use tauri::{Manager, State, ipc::Channel, path::BaseDirectory};
use tauri_plugin_opener::OpenerExt;
use vernier_gdx_input::{
    InputEvent as GdxInputEvent, InputSessionPool as GdxInputSessionPool, SessionConfig,
};

use error::{CommandError, CommandResult};
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
    input_configuration: tokio::sync::Mutex<()>,
    bundled_lsl: Option<PathBuf>,
    recording_directory: PathBuf,
    preferences: Arc<PreferencesStore>,
    output_configuration: tokio::sync::Mutex<()>,
    processing_settings: tokio::sync::watch::Sender<ProcessingSettings>,
}

impl AppState {
    fn new(
        bundled_lsl: Option<PathBuf>,
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
            input_configuration: tokio::sync::Mutex::new(()),
            bundled_lsl,
            recording_directory,
            preferences,
            output_configuration: tokio::sync::Mutex::new(()),
            processing_settings,
        }
    }
}

const MAX_INPUT_SOURCES: usize = 8;
const SOURCE_COLORS: [&str; MAX_INPUT_SOURCES] = [
    "#00c2ff", "#ffb000", "#ff5c8a", "#7bd88f", "#b392f0", "#ff7b54", "#58d6c7", "#e5d85c",
];

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
    color: &'static str,
    input_kind: InputKind,
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
        microvolts: Vec<i32>,
        formulas: FormulaPublishBatch,
    },
    Accelerometer {
        source: SourceDescriptor,
        sensor_timestamp_ns: u64,
        samples: Vec<AccSample>,
        formulas: FormulaPublishBatch,
    },
    Metrics {
        source: SourceDescriptor,
        values: Vec<MetricSample>,
        formulas: FormulaPublishBatch,
    },
    Force {
        source: SourceDescriptor,
        sensor_number: u8,
        host_receive_timestamp_ns: u64,
        sample_period_us: u32,
        sequence: u64,
        values: Vec<f32>,
        dropped_before: u64,
        decode_latency_ns: u64,
    },
    StreamHealth {
        source: SourceDescriptor,
        notifications: u64,
        samples: u64,
        malformed_frames: u64,
        dropped_batches: u64,
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
    },
    Ecg {
        sensor_timestamp_ns: u64,
        microvolts: Vec<i32>,
    },
    Accelerometer {
        sensor_timestamp_ns: u64,
        samples: Vec<AccSample>,
    },
    HeartRate {
        beats_per_minute: u16,
        rr_intervals_ms: Vec<f32>,
    },
    Force {
        sensor_number: u8,
        host_receive_timestamp_ns: u64,
        sample_period_us: u32,
        sequence: u64,
        values: Vec<f32>,
        dropped_before: u64,
        decode_latency_ns: u64,
    },
    StreamHealth {
        notifications: u64,
        samples: u64,
        malformed_frames: u64,
        dropped_batches: u64,
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
                },
                PolarInputEvent::Ecg {
                    sensor_timestamp_ns,
                    microvolts,
                } => UnifiedInputEvent::Ecg {
                    sensor_timestamp_ns,
                    microvolts,
                },
                PolarInputEvent::Accelerometer {
                    sensor_timestamp_ns,
                    samples,
                } => UnifiedInputEvent::Accelerometer {
                    sensor_timestamp_ns,
                    samples,
                },
                PolarInputEvent::HeartRate {
                    beats_per_minute,
                    rr_intervals_ms,
                } => UnifiedInputEvent::HeartRate {
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
                } => UnifiedInputEvent::Connected {
                    device_name,
                    battery_percent: Some(battery_percent),
                    device_model: Some(model_code),
                    firmware_version: Some(main_firmware_version),
                    sensor_number: Some(sensor_number),
                    sensor_name: Some(sensor_name),
                    sensor_unit: Some(sensor_unit),
                    sample_period_us: Some(sample_period_us),
                },
                GdxInputEvent::Samples {
                    sensor_number,
                    host_receive_timestamp_ns,
                    sample_period_us,
                    sequence,
                    values,
                    dropped_before,
                    decode_latency_ns,
                } => UnifiedInputEvent::Force {
                    sensor_number,
                    host_receive_timestamp_ns,
                    sample_period_us,
                    sequence,
                    values,
                    dropped_before,
                    decode_latency_ns,
                },
                GdxInputEvent::StreamHealth {
                    notifications,
                    samples,
                    malformed_frames,
                    dropped_batches,
                    queue_high_water,
                    decode_latency_p50_ns,
                    decode_latency_p95_ns,
                    decode_latency_p99_ns,
                    max_decode_latency_ns,
                } => UnifiedInputEvent::StreamHealth {
                    notifications,
                    samples,
                    malformed_frames,
                    dropped_batches,
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
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyPreferences {
    output_config: Option<OutputConfig>,
    last_device: Option<SavedDevice>,
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
    state
        .preferences
        .migrate_legacy(output_config, last_device)
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
async fn scan_devices(
    state: State<'_, AppState>,
    preferred_device_id: Option<String>,
) -> CommandResult<Vec<DeviceSummary>> {
    let _configuration = state.input_configuration.lock().await;
    if !state.active_sources.lock().await.is_empty() {
        return Err(CommandError::new(
            "SCAN_WHILE_STREAMING",
            "Disconnect active sources before refreshing Bluetooth discovery.",
            true,
        ));
    }
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
    let preferred_kind = preferred_device_id
        .as_deref()
        .and_then(|device_id| parse_device_id(device_id).ok())
        .map(|(kind, _)| kind);
    let (polar, vernier) = match preferred_kind {
        Some(InputKind::PolarH10) => (scan_polar().await, Ok(Vec::new())),
        Some(InputKind::VernierGoDirect) => (Ok(Vec::new()), scan_vernier().await),
        None => tokio::join!(scan_polar(), scan_vernier()),
    };
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

#[tauri::command]
async fn connect_device(
    state: State<'_, AppState>,
    device_id: String,
    events: Channel<AppEvent>,
) -> CommandResult<SourceDescriptor> {
    let _configuration = state.input_configuration.lock().await;
    let (input_kind, raw_device_id) = parse_device_id(&device_id)?;
    let source = allocate_source(&state.active_sources, input_kind).await?;
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
    let config = source_output_config(current_output_config(&state)?, &source)?;
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
    let preferences = state.preferences.clone();
    let active_sources = state.active_sources.clone();
    let source_outputs = state.source_outputs.clone();
    let mut processing_settings = state.processing_settings.subscribe();
    let task_source = source.clone();
    tauri::async_runtime::spawn(async move {
        // Keep WebView serialization completely off the sensor/output path. A
        // slow or hidden renderer may lose display frames, but never raw LSL or
        // OSC data. Control events retain ordered delivery.
        let (ui_tx, mut ui_rx) = tokio::sync::mpsc::channel::<AppEvent>(8);
        let ui_task = tauri::async_runtime::spawn(async move {
            while let Some(event) = ui_rx.recv().await {
                if events.send(event).is_err() {
                    break;
                }
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
        while let Some(event) = input_events.recv().await {
            if processing_settings.has_changed().unwrap_or(false) {
                let settings = *processing_settings.borrow_and_update();
                metrics_engine.apply_selection(settings.metrics);
                metrics_engine.apply_breathing_settings(settings.breathing);
            }
            let continue_streaming = match event {
                UnifiedInputEvent::Status { phase, message } => ui_tx
                    .send(AppEvent::Status {
                        source: task_source.clone(),
                        phase: phase.into(),
                        message,
                    })
                    .await
                    .is_ok(),
                UnifiedInputEvent::Connected {
                    device_name,
                    battery_percent,
                    device_model,
                    firmware_version,
                    sensor_number,
                    sensor_name,
                    sensor_unit,
                    sample_period_us,
                } => {
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
                    ui_tx
                        .send(AppEvent::Connection {
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
                                    "Verified {} ({}) on channel {} is streaming at {:.1} Hz",
                                    sensor_name.as_deref().unwrap_or("Force"),
                                    sensor_unit.as_deref().unwrap_or("N"),
                                    sensor_number.unwrap_or(1),
                                    sample_period_us
                                        .filter(|period| *period > 0)
                                        .map(|period| 1_000_000.0 / period as f64)
                                        .unwrap_or(10.0),
                                ),
                            },
                        })
                        .await
                        .is_ok()
                }
                UnifiedInputEvent::Ecg {
                    sensor_timestamp_ns,
                    microvolts,
                } => {
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
                            derived,
                            FormulaPublishBatch::default(),
                        )
                    }
                }
                UnifiedInputEvent::Accelerometer {
                    sensor_timestamp_ns,
                    samples,
                } => {
                    let output_open = forward_output_warning(
                        &ui_tx,
                        &task_source,
                        output.publish_accelerometer(sensor_timestamp_ns, &samples),
                    );
                    let formulas =
                        output.process_accelerometer_formulas(sensor_timestamp_ns, &samples);
                    let derived = metrics_engine.process_accelerometer(&samples);
                    if !output_open
                        || !forward_display_event(
                            &ui_tx,
                            AppEvent::Accelerometer {
                                source: task_source.clone(),
                                sensor_timestamp_ns,
                                samples,
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
                            derived,
                            FormulaPublishBatch::default(),
                        )
                    }
                }
                UnifiedInputEvent::HeartRate {
                    beats_per_minute,
                    rr_intervals_ms,
                } => {
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
                            metrics_engine.process_heart_rate(beats_per_minute, &rr_intervals_ms),
                            formulas,
                        )
                }
                UnifiedInputEvent::Force {
                    sensor_number,
                    host_receive_timestamp_ns,
                    sample_period_us,
                    sequence,
                    values,
                    dropped_before,
                    decode_latency_ns,
                } => {
                    let output_open = forward_output_warning(
                        &ui_tx,
                        &task_source,
                        output.publish_force(host_receive_timestamp_ns, &values, sample_period_us),
                    );
                    output_open
                        && forward_display_event(
                            &ui_tx,
                            AppEvent::Force {
                                source: task_source.clone(),
                                sensor_number,
                                host_receive_timestamp_ns,
                                sample_period_us,
                                sequence,
                                values,
                                dropped_before,
                                decode_latency_ns,
                            },
                        )
                }
                UnifiedInputEvent::StreamHealth {
                    notifications,
                    samples,
                    malformed_frames,
                    dropped_batches,
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
                        malformed_frames,
                        dropped_batches,
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
                        && ui_tx
                            .send(AppEvent::Connection {
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
                            })
                            .await
                            .is_ok()
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
    });
    Ok(source)
}

fn publish_metrics(
    output: &OutputRouter,
    ui_tx: &tokio::sync::mpsc::Sender<AppEvent>,
    source: &SourceDescriptor,
    sensor_timestamp_ns: u64,
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
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => false,
    }
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
    state.active_sources.lock().await.remove(&source_id);
    if let Some(output) = state.source_outputs.lock().await.remove(&source_id) {
        output.reset_measurement();
    }
    Ok(())
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
) -> CommandResult<SourceDescriptor> {
    let active = active.lock().await;
    for (index, color) in SOURCE_COLORS.iter().enumerate() {
        let id = format!("source-{}", index + 1);
        if !active.contains_key(&id) {
            return Ok(SourceDescriptor {
                slot: id.clone(),
                id,
                label: format!("Source {}", index + 1),
                color,
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
    for (source_id, output) in &outputs {
        let Some(source) = sources.get(source_id) else {
            continue;
        };
        output
            .configure(source_output_config(applied.clone(), source)?)
            .await
            .map_err(|message| CommandError::new("OUTPUT_CONFIGURATION_FAILED", message, true))?;
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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
                preferences_path,
                recording_directory,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_bootstrap,
            migrate_legacy_preferences,
            scan_devices,
            connect_device,
            disconnect_device,
            update_output_config,
            validate_custom_formula,
            open_metric_citation,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Polar Stream");
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
        let polar = SourceDescriptor {
            id: "source-1".into(),
            slot: "source-1".into(),
            label: "Source 1".into(),
            color: SOURCE_COLORS[0],
            input_kind: InputKind::PolarH10,
        };
        let vernier = SourceDescriptor {
            id: "source-2".into(),
            slot: "source-2".into(),
            label: "Source 2".into(),
            color: SOURCE_COLORS[1],
            input_kind: InputKind::VernierGoDirect,
        };

        let polar_config = source_output_config(config.clone(), &polar).unwrap();
        assert_eq!(polar_config.outputs.len(), 2);
        assert!(polar_config.outputs.iter().any(|id| id == "raw_ecg"));
        assert!(polar_config.outputs.iter().any(|id| id == "raw_acc"));
        assert_eq!(polar_config.stream_name, "Polar-H10_source-1");

        let vernier_config = source_output_config(config, &vernier).unwrap();
        assert_eq!(vernier_config.outputs, ["raw_force"]);
        assert_eq!(vernier_config.stream_name, "Polar-H10_source-2");
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
