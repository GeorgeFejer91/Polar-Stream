//! Thin application coordinator. Protocol decoding, Bluetooth input, and
//! network output are independent crates below `crates/`.

use std::{path::PathBuf, sync::Arc};

use polar_h10_core::AccSample;
use polar_h10_input::{DeviceSummary, InputEvent, InputManager};
use polar_h10_metrics::{
    BreathingSettings, METRIC_CATALOG, MetricDefinition, MetricEngine, MetricSample,
};
use polar_h10_output::{MetricValue, OutputConfig, OutputHealth, OutputRouter};
use serde::Serialize;
use tauri::{Manager, State, ipc::Channel, path::BaseDirectory};

struct AppState {
    input: Arc<InputManager>,
    output: Arc<OutputRouter>,
    breathing_settings: tokio::sync::watch::Sender<BreathingSettings>,
}

impl AppState {
    fn new(bundled_lsl: Option<PathBuf>) -> Self {
        let (breathing_settings, _) = tokio::sync::watch::channel(BreathingSettings::default());
        Self {
            input: Arc::new(InputManager::new()),
            output: Arc::new(OutputRouter::with_bundled_lsl(bundled_lsl)),
            breathing_settings,
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
        message: String,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    config: OutputConfig,
    platform: &'static str,
    metric_catalog: Vec<MetricDefinition>,
}

#[tauri::command]
fn get_bootstrap(state: State<'_, Arc<AppState>>) -> Bootstrap {
    Bootstrap {
        config: state.output.config(),
        platform: std::env::consts::OS,
        metric_catalog: METRIC_CATALOG.to_vec(),
    }
}

#[tauri::command]
async fn scan_devices(state: State<'_, Arc<AppState>>) -> Result<Vec<DeviceSummary>, String> {
    state.input.scan().await
}

#[tauri::command]
async fn connect_device(
    state: State<'_, Arc<AppState>>,
    device_id: String,
    events: Channel<AppEvent>,
) -> Result<(), String> {
    let mut input_events = state.input.connect(&device_id).await?;
    let output = state.output.clone();
    let mut breathing_settings = state.breathing_settings.subscribe();
    output.reset_measurement();
    tauri::async_runtime::spawn(async move {
        let mut metrics_engine = MetricEngine::default();
        metrics_engine.apply_breathing_settings(*breathing_settings.borrow_and_update());
        while let Some(event) = input_events.recv().await {
            if breathing_settings.has_changed().unwrap_or(false) {
                metrics_engine.apply_breathing_settings(*breathing_settings.borrow_and_update());
            }
            let continue_streaming = match event {
                InputEvent::Status { phase, message } => events
                    .send(AppEvent::Status {
                        phase: phase.into(),
                        message,
                    })
                    .is_ok(),
                InputEvent::Connected {
                    device_name,
                    battery_percent,
                } => events
                    .send(AppEvent::Connection {
                        connected: true,
                        streaming: true,
                        device_name,
                        battery_percent,
                        message: "Raw ECG and accelerometer are streaming".into(),
                    })
                    .is_ok(),
                InputEvent::Ecg {
                    sensor_timestamp_ns,
                    microvolts,
                } => {
                    output.publish_ecg(sensor_timestamp_ns, &microvolts);
                    let derived = metrics_engine.process_ecg(&microvolts);
                    if events
                        .send(AppEvent::Ecg {
                            sensor_timestamp_ns,
                            microvolts,
                        })
                        .is_err()
                    {
                        false
                    } else {
                        publish_metrics(&output, &events, derived)
                    }
                }
                InputEvent::Accelerometer {
                    sensor_timestamp_ns,
                    samples,
                } => {
                    output.publish_accelerometer(sensor_timestamp_ns, &samples);
                    let derived = metrics_engine.process_accelerometer(&samples);
                    if events
                        .send(AppEvent::Accelerometer {
                            sensor_timestamp_ns,
                            samples,
                        })
                        .is_err()
                    {
                        false
                    } else {
                        publish_metrics(&output, &events, derived)
                    }
                }
                InputEvent::HeartRate {
                    beats_per_minute,
                    rr_intervals_ms,
                } => publish_metrics(
                    &output,
                    &events,
                    metrics_engine.process_heart_rate(beats_per_minute, &rr_intervals_ms),
                ),
                InputEvent::Error(message) => events.send(AppEvent::Error { message }).is_ok(),
                InputEvent::Disconnected {
                    device_name,
                    battery_percent,
                } => {
                    let phase_sent = publish_metrics(
                        &output,
                        &events,
                        vec![MetricSample {
                            id: "breathing_phase",
                            value: -2.0,
                        }],
                    );
                    phase_sent
                        && events
                            .send(AppEvent::Connection {
                                connected: false,
                                streaming: false,
                                device_name,
                                battery_percent,
                                message: "Disconnected".into(),
                            })
                            .is_ok()
                }
            };
            if !continue_streaming {
                break;
            }
        }
    });
    Ok(())
}

fn publish_metrics(
    output: &OutputRouter,
    events: &Channel<AppEvent>,
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
    output.publish_metrics(&routed);
    events.send(AppEvent::Metrics { values }).is_ok()
}

#[tauri::command]
async fn disconnect_device(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.input.disconnect().await
}

#[tauri::command]
async fn update_output_config(
    state: State<'_, Arc<AppState>>,
    config: OutputConfig,
) -> Result<OutputHealth, String> {
    let breathing_settings = config
        .metric_options
        .get("breathing_phase")
        .and_then(|options| options.processing.breathing_phase)
        .unwrap_or_default()
        .clamped();
    let health = state.output.configure(config).await?;
    state.breathing_settings.send_replace(breathing_settings);
    Ok(health)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let bundled_lsl = app
                .path()
                .resolve(lsl_resource_path(), BaseDirectory::Resource)
                .ok()
                .filter(|path| path.is_file());
            app.manage(Arc::new(AppState::new(bundled_lsl)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_bootstrap,
            scan_devices,
            connect_device,
            disconnect_device,
            update_output_config,
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
