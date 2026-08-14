//! Thin application coordinator. Protocol decoding, Bluetooth input, and
//! network output are independent crates below `crates/`.

mod error;
mod preferences;

use std::{path::PathBuf, sync::Arc};

use polar_h10_core::AccSample;
use polar_h10_input::{DeviceSummary, InputEvent, InputManager};
use polar_h10_metrics::{
    BreathingSettings, METRIC_CATALOG, MetricDefinition, MetricEngine, MetricSample,
    MetricSelection,
};
use polar_h10_output::{MetricValue, OutputConfig, OutputHealth, OutputRouter};
use serde::{Deserialize, Serialize};
use tauri::{Manager, State, ipc::Channel, path::BaseDirectory};
use tauri_plugin_opener::OpenerExt;

use error::{CommandError, CommandResult};
use preferences::{PreferencesSnapshot, PreferencesStore, SavedDevice};

struct AppState {
    input: Arc<InputManager>,
    output: Arc<OutputRouter>,
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
            input: Arc::new(InputManager::new()),
            output: Arc::new(OutputRouter::with_bundled_lsl_and_recordings(
                bundled_lsl,
                recording_directory,
            )),
            preferences,
            output_configuration: tokio::sync::Mutex::new(()),
            processing_settings,
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
        let breathing = ["breathing_phase", "acc_breathing_magnitude"]
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
        phase: String,
        message: String,
    },
    Connection {
        connected: bool,
        streaming: bool,
        device_name: String,
        battery_percent: Option<u8>,
        message: String,
    },
    Ecg {
        sensor_timestamp_ns: u64,
        microvolts: Vec<i32>,
    },
    Accelerometer {
        sensor_timestamp_ns: u64,
        samples: Vec<AccSample>,
    },
    Metrics {
        values: Vec<MetricSample>,
    },
    Error {
        code: &'static str,
        message: String,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    config: OutputConfig,
    preferences: PreferencesSnapshot,
    has_saved_preferences: bool,
    platform: &'static str,
    metric_catalog: Vec<MetricDefinition>,
}

#[tauri::command]
fn get_bootstrap(state: State<'_, AppState>) -> Bootstrap {
    let preferences = state.preferences.snapshot();
    Bootstrap {
        config: preferences.output_config.clone(),
        preferences,
        has_saved_preferences: state.preferences.has_saved_preferences(),
        platform: std::env::consts::OS,
        metric_catalog: METRIC_CATALOG.to_vec(),
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
async fn scan_devices(state: State<'_, AppState>) -> CommandResult<Vec<DeviceSummary>> {
    state
        .input
        .scan()
        .await
        .map_err(|message| CommandError::new("BLUETOOTH_SCAN_FAILED", message, true))
}

#[tauri::command]
async fn connect_device(
    state: State<'_, AppState>,
    device_id: String,
    events: Channel<AppEvent>,
) -> CommandResult<()> {
    let mut input_events = state
        .input
        .connect(&device_id)
        .await
        .map_err(|message| CommandError::new("POLAR_CONNECTION_FAILED", message, true))?;
    let output = state.output.clone();
    let preferences = state.preferences.clone();
    let mut processing_settings = state.processing_settings.subscribe();
    output.reset_measurement();
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
                InputEvent::Status { phase, message } => ui_tx
                    .send(AppEvent::Status {
                        phase: phase.into(),
                        message,
                    })
                    .await
                    .is_ok(),
                InputEvent::Connected {
                    device_name,
                    battery_percent,
                } => {
                    let save_preferences = preferences.clone();
                    let save_events = ui_tx.clone();
                    let saved_device = SavedDevice {
                        id: device_id.clone(),
                        name: device_name.clone(),
                    };
                    tauri::async_runtime::spawn(async move {
                        if let Err(message) = save_preferences.save_last_device(saved_device).await
                        {
                            let _ = save_events
                                .send(AppEvent::Error {
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
                            connected: true,
                            streaming: true,
                            device_name,
                            battery_percent,
                            message: "Raw ECG and accelerometer are streaming".into(),
                        })
                        .await
                        .is_ok()
                }
                InputEvent::Ecg {
                    sensor_timestamp_ns,
                    microvolts,
                } => {
                    let output_open = forward_output_warning(
                        &ui_tx,
                        output.publish_ecg(sensor_timestamp_ns, &microvolts),
                    );
                    let derived = metrics_engine.process_ecg(&microvolts);
                    if !output_open
                        || !forward_display_event(
                            &ui_tx,
                            AppEvent::Ecg {
                                sensor_timestamp_ns,
                                microvolts,
                            },
                        )
                    {
                        false
                    } else {
                        publish_metrics(&output, &ui_tx, derived)
                    }
                }
                InputEvent::Accelerometer {
                    sensor_timestamp_ns,
                    samples,
                } => {
                    let output_open = forward_output_warning(
                        &ui_tx,
                        output.publish_accelerometer(sensor_timestamp_ns, &samples),
                    );
                    let derived = metrics_engine.process_accelerometer(&samples);
                    if !output_open
                        || !forward_display_event(
                            &ui_tx,
                            AppEvent::Accelerometer {
                                sensor_timestamp_ns,
                                samples,
                            },
                        )
                    {
                        false
                    } else {
                        publish_metrics(&output, &ui_tx, derived)
                    }
                }
                InputEvent::HeartRate {
                    beats_per_minute,
                    rr_intervals_ms,
                } => {
                    let output_open = forward_output_warning(
                        &ui_tx,
                        output.publish_heart_rate(beats_per_minute, &rr_intervals_ms),
                    );
                    output_open
                        && publish_metrics(
                            &output,
                            &ui_tx,
                            metrics_engine.process_heart_rate(beats_per_minute, &rr_intervals_ms),
                        )
                }
                InputEvent::Error(message) => forward_display_event(
                    &ui_tx,
                    AppEvent::Error {
                        code: "SENSOR_DATA_WARNING",
                        message,
                    },
                ),
                InputEvent::Disconnected {
                    device_name,
                    battery_percent,
                } => {
                    let phase_sent = publish_metrics(
                        &output,
                        &ui_tx,
                        vec![MetricSample {
                            id: "breathing_phase",
                            value: 0.0,
                        }],
                    );
                    phase_sent
                        && ui_tx
                            .send(AppEvent::Connection {
                                connected: false,
                                streaming: false,
                                device_name,
                                battery_percent,
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
        drop(ui_tx);
        let _ = ui_task.await;
    });
    Ok(())
}

fn publish_metrics(
    output: &OutputRouter,
    ui_tx: &tokio::sync::mpsc::Sender<AppEvent>,
    values: Vec<MetricSample>,
) -> bool {
    if values.is_empty() {
        return true;
    }
    let routed = values
        .iter()
        .map(|metric| MetricValue {
            id: metric.id,
            value: metric.value,
        })
        .collect::<Vec<_>>();
    let output_open = forward_output_warning(ui_tx, output.publish_metrics(&routed));
    output_open && forward_display_event(ui_tx, AppEvent::Metrics { values })
}

fn forward_output_warning(
    ui_tx: &tokio::sync::mpsc::Sender<AppEvent>,
    message: Option<String>,
) -> bool {
    message.is_none_or(|message| {
        forward_display_event(
            ui_tx,
            AppEvent::Error {
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
async fn disconnect_device(state: State<'_, AppState>) -> CommandResult<()> {
    state
        .input
        .disconnect()
        .await
        .map_err(|message| CommandError::new("POLAR_DISCONNECT_FAILED", message, true))
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
    let health = state
        .output
        .configure(config)
        .await
        .map_err(|message| CommandError::new("OUTPUT_CONFIGURATION_FAILED", message, true))?;
    let applied = state.output.config();
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
fn open_metric_citation(app: tauri::AppHandle, metric_id: String) -> CommandResult<()> {
    let metric = MetricDefinition::for_id(&metric_id).ok_or_else(|| {
        CommandError::new(
            "UNKNOWN_METRIC",
            "That metric is not in the catalog.",
            false,
        )
    })?;
    if !metric.citation_url.starts_with("https://") {
        return Err(CommandError::new(
            "UNSAFE_CITATION_URL",
            "The citation URL was not opened because it is not HTTPS.",
            false,
        ));
    }
    app.opener()
        .open_url(metric.citation_url, None::<&str>)
        .map_err(|message| {
            CommandError::new(
                "CITATION_OPEN_FAILED",
                format!("Could not open the citation in the system browser: {message}"),
                true,
            )
        })
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
