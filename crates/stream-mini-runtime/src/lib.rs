use std::{
    fs,
    path::{Path, PathBuf},
    process::{self, Command},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use polar_h10_core::AccSample;
use polar_h10_input::{InputEvent as PolarInputEvent, InputSessionPool as PolarInputSessionPool};
use polar_h10_metrics::{
    METRIC_CATALOG, MetricDefinition, MetricEngine, MetricSample, MetricSelection,
    MetricSelectionTier, POLAR_MINI_ONLY_IDS, RELEASE_POLAR_RESPIRATION_IDS, TimedAccBatch,
    VernierBreathingProcessor, metric_selection_tier,
};
use polar_h10_output::{
    MetricValue, OutputConfig, OutputRouter, VernierMiniSelection, VernierStreamSchema, normalize_stream_base,
    source_palette,
};
#[cfg(feature = "liblsl-backend")]
use polar_h10_output::{MiniCombinedOutput, MiniStatusOutput};
use polar_stream_time::{SourceClockMapper, monotonic_now_ns};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State, ipc::Channel, path::BaseDirectory};
use vernier_gdx_core::{
    NumericMeasurementType, SampleEncoding, SamplingMode, SensorInfo, SensorSamples,
};
use vernier_gdx_input::{
    InputEvent as GdxInputEvent, InputSessionPool as GdxInputSessionPool, SessionConfig,
};

const MINI_SLOT: &str = "mini-node";
const PREFERENCE_SCHEMA: &str = "polar.stream.mini.preferences.v1";
const LIVE_RECONFIGURED_MESSAGE: &str = "Saved and applied to the active LSL outlets.";
const POLAR_DIRECT_OUTPUTS: &[&str] = &["raw_ecg", "raw_acc", "heart_rate", "rr_interval"];
const VERNIER_FORCE_OUTPUT: &str = "raw_force";
const VERNIER_OUTPUT_IDS: [&str; 4] = ["rawVernier", "rawForce", "vernierBreathing", "signalStatus"];
const VERNIER_PERIOD_US: u32 = 50_000;
const ACC_SAMPLE_PERIOD_NS: u64 = 1_000_000_000 / 200;
const EVENT_INTERVAL: Duration = Duration::from_millis(250);
const TASK_JOIN_TIMEOUT: Duration = Duration::from_secs(4);
const MOCK_ARGUMENT: &str = "--mock";
const POLAR_MOCK_TICK: Duration = Duration::from_millis(10);
const VERNIER_MOCK_TICK: Duration = Duration::from_millis(50);
const MOCK_ECG_FILE: &str = "mock-ecg-60m.i16le";
const MOCK_ECG_SAMPLE_COUNT: usize = 60 * 60 * 130;
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

pub type CommandResult<T> = Result<T, CommandError>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

impl CommandError {
    fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MiniAppKind {
    Polar,
    Vernier,
}

impl MiniAppKind {
    fn product_name(self) -> &'static str {
        match self {
            Self::Polar => "Polar Stream Mini",
            Self::Vernier => "Vernier Stream Mini",
        }
    }

    fn default_stream_name(self) -> &'static str {
        match self {
            Self::Polar => "Polar-H10-Mini",
            Self::Vernier => "Vernier-GDX-Mini",
        }
    }

    fn scan_label(self) -> &'static str {
        match self {
            Self::Polar => "Polar H10",
            Self::Vernier => "Vernier Go Direct",
        }
    }

    fn palette_id(self) -> &'static str {
        match self {
            Self::Polar => "meadow",
            Self::Vernier => "lagoon",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MiniOutputMode {
    #[default]
    SeparateStreams,
    SingleStream,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedMiniDevice {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniPreferencesSnapshot {
    pub schema: String,
    pub stream_name: String,
    pub output_mode: MiniOutputMode,
    pub auto_connect: bool,
    pub polar_outputs: Vec<String>,
    pub vernier_outputs: Vec<String>,
    pub last_device: Option<SavedMiniDevice>,
    pub recent_devices: Vec<SavedMiniDevice>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniPreferencesInput {
    pub stream_name: String,
    pub output_mode: MiniOutputMode,
    pub auto_connect: bool,
    #[serde(default)]
    pub polar_outputs: Vec<String>,
    #[serde(default)]
    pub vernier_outputs: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MiniPreferencesFile {
    stream_name: Option<String>,
    output_mode: Option<MiniOutputMode>,
    auto_connect: Option<bool>,
    polar_outputs: Option<Vec<String>>,
    vernier_outputs: Option<Vec<String>>,
    last_device: Option<SavedMiniDevice>,
    recent_devices: Option<Vec<SavedMiniDevice>>,
}

impl MiniPreferencesSnapshot {
    fn default_for(kind: MiniAppKind) -> Self {
        Self {
            schema: PREFERENCE_SCHEMA.into(),
            stream_name: kind.default_stream_name().into(),
            output_mode: MiniOutputMode::SeparateStreams,
            auto_connect: true,
            polar_outputs: normalize_polar_outputs(None),
            vernier_outputs: default_vernier_outputs(),
            last_device: None,
            recent_devices: Vec::new(),
        }
    }

    fn from_file(kind: MiniAppKind, file: MiniPreferencesFile) -> Self {
        let fallback = Self::default_for(kind);
        let last_device = file.last_device.filter(valid_saved_device);
        let mut recent_devices = file.recent_devices.unwrap_or_default();
        recent_devices.retain(valid_saved_device);
        if let Some(last) = &last_device {
            recent_devices.retain(|device| device.id != last.id);
            recent_devices.insert(0, last.clone());
        }
        let mut seen = std::collections::HashSet::new();
        recent_devices.retain(|device| seen.insert(device.id.clone()));
        recent_devices.truncate(6);
        Self {
            schema: PREFERENCE_SCHEMA.into(),
            stream_name: normalize_stream_base(
                file.stream_name
                    .as_deref()
                    .unwrap_or(kind.default_stream_name()),
            )
            .unwrap_or(fallback.stream_name),
            output_mode: file.output_mode.unwrap_or_default(),
            auto_connect: file.auto_connect.unwrap_or(true),
            polar_outputs: normalize_polar_outputs(file.polar_outputs),
            vernier_outputs: file.vernier_outputs
                .and_then(|ids| validate_vernier_outputs(ids).ok())
                .unwrap_or_else(default_vernier_outputs),
            last_device,
            recent_devices,
        }
    }
}

struct PreferencesStore {
    kind: MiniAppKind,
    path: Option<PathBuf>,
    snapshot: RwLock<MiniPreferencesSnapshot>,
}

impl PreferencesStore {
    fn load(path: PathBuf, kind: MiniAppKind) -> Self {
        let snapshot = Self::read_snapshot(&path, kind);
        Self {
            kind,
            path: Some(path),
            snapshot: RwLock::new(snapshot),
        }
    }

    fn mock_from(path: &Path, kind: MiniAppKind) -> Self {
        let mut snapshot = Self::read_snapshot(path, kind);
        let candidate = format!("{}-Mock-{}", snapshot.stream_name, process::id());
        snapshot.stream_name = normalize_stream_base(&candidate)
            .unwrap_or_else(|_| format!("{}-Mock-{}", kind.default_stream_name(), process::id()));
        snapshot.auto_connect = false;
        snapshot.last_device = None;
        snapshot.recent_devices.clear();
        Self {
            kind,
            path: None,
            snapshot: RwLock::new(snapshot),
        }
    }

    fn read_snapshot(path: &Path, kind: MiniAppKind) -> MiniPreferencesSnapshot {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<MiniPreferencesFile>(&text).ok())
            .map(|file| MiniPreferencesSnapshot::from_file(kind, file))
            .unwrap_or_else(|| MiniPreferencesSnapshot::default_for(kind))
    }

    fn snapshot(&self) -> MiniPreferencesSnapshot {
        self.snapshot
            .read()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_else(|_| MiniPreferencesSnapshot::default_for(self.kind))
    }

    fn save_input(
        &self,
        kind: MiniAppKind,
        input: MiniPreferencesInput,
    ) -> Result<MiniPreferencesSnapshot, String> {
        self.replace(self.snapshot_for_input(kind, input)?)
    }

    fn snapshot_for_input(
        &self,
        kind: MiniAppKind,
        input: MiniPreferencesInput,
    ) -> Result<MiniPreferencesSnapshot, String> {
        let current = self.snapshot();
        let mut snapshot = MiniPreferencesSnapshot {
            schema: PREFERENCE_SCHEMA.into(),
            stream_name: normalize_stream_base(&input.stream_name)?,
            output_mode: input.output_mode,
            auto_connect: input.auto_connect,
            polar_outputs: normalize_polar_outputs(Some(input.polar_outputs)),
            vernier_outputs: if kind == MiniAppKind::Vernier {
                validate_vernier_outputs(input.vernier_outputs.unwrap_or(current.vernier_outputs))?
            } else {
                current.vernier_outputs
            },
            last_device: current.last_device,
            recent_devices: current.recent_devices,
        };
        if kind == MiniAppKind::Vernier {
            snapshot.polar_outputs = normalize_polar_outputs(None);
        }
        Ok(snapshot)
    }

    fn save_last_device(&self, device: SavedMiniDevice) -> Result<MiniPreferencesSnapshot, String> {
        if !valid_saved_device(&device) {
            return Err("Saved device identity was empty or invalid.".into());
        }
        let mut snapshot = self.snapshot();
        snapshot
            .recent_devices
            .retain(|saved| saved.id != device.id);
        snapshot.recent_devices.insert(0, device.clone());
        snapshot.recent_devices.truncate(6);
        snapshot.last_device = Some(device);
        self.replace(snapshot)
    }

    fn replace(
        &self,
        snapshot: MiniPreferencesSnapshot,
    ) -> Result<MiniPreferencesSnapshot, String> {
        if let Some(path) = &self.path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let payload =
                serde_json::to_vec_pretty(&snapshot).map_err(|error| error.to_string())?;
            fs::write(path, payload).map_err(|error| error.to_string())?;
        }
        let mut guard = self
            .snapshot
            .write()
            .map_err(|_| "Preference state lock failed".to_string())?;
        *guard = snapshot.clone();
        Ok(snapshot)
    }
}

pub struct MiniAppState {
    kind: MiniAppKind,
    mock_mode: bool,
    polar_input: Arc<PolarInputSessionPool>,
    gdx_input: Arc<GdxInputSessionPool>,
    active: tokio::sync::Mutex<Option<ActiveMiniSession>>,
    configuration: Arc<tokio::sync::Mutex<()>>,
    vernier_schema: Arc<tokio::sync::Mutex<Option<VernierStreamSchema>>>,
    bundled_lsl: Option<PathBuf>,
    recording_directory: PathBuf,
    preferences: Arc<PreferencesStore>,
    display: Arc<DisplayEndpoint>,
    shutdown_started: AtomicBool,
}

impl MiniAppState {
    fn new(
        kind: MiniAppKind,
        bundled_lsl: Option<PathBuf>,
        preferences_path: PathBuf,
        recording_directory: PathBuf,
        mock_mode: bool,
    ) -> Self {
        let preferences = if mock_mode {
            PreferencesStore::mock_from(&preferences_path, kind)
        } else {
            PreferencesStore::load(preferences_path, kind)
        };
        Self {
            kind,
            mock_mode,
            polar_input: Arc::new(PolarInputSessionPool::with_max_sessions(1)),
            gdx_input: Arc::new(GdxInputSessionPool::new(1)),
            active: tokio::sync::Mutex::new(None),
            configuration: Arc::new(tokio::sync::Mutex::new(())),
            vernier_schema: Arc::new(tokio::sync::Mutex::new(None)),
            bundled_lsl,
            recording_directory,
            preferences: Arc::new(preferences),
            display: Arc::new(DisplayEndpoint::default()),
            shutdown_started: AtomicBool::new(false),
        }
    }

    pub fn begin_shutdown(&self) -> bool {
        !self.shutdown_started.swap(true, Ordering::AcqRel)
    }
}

struct ActiveMiniSession {
    slot: String,
    device_id: String,
    device_name: Option<String>,
    output_mode: MiniOutputMode,
    stream_name: String,
    polar_outputs: Vec<String>,
    vernier_outputs: Vec<String>,
    output: tokio::sync::watch::Sender<MiniOutputHandle>,
    settings: Option<tokio::sync::watch::Sender<PolarMiniSettings>>,
    mock: bool,
    connected: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    cancel: tokio::sync::watch::Sender<bool>,
    task: tauri::async_runtime::JoinHandle<()>,
}

#[derive(Clone)]
enum MiniOutputHandle {
    Separate(
        Arc<OutputRouter>,
        #[cfg(feature = "liblsl-backend")] Option<Arc<MiniStatusOutput>>,
    ),
    #[cfg(feature = "liblsl-backend")]
    Single(Arc<MiniCombinedOutput>),
}

struct VernierRawBatch<'a> {
    host_receive_timestamp_ns: u64,
    sample_period_us: u32,
    sequence: u64,
    dropped_before: u64,
    device_drop_reports_before: u64,
    decode_latency_ns: u64,
    encoding: SampleEncoding,
    sensors: &'a [SensorSamples],
}

impl MiniOutputHandle {
    fn publish_signal_state(&self, restored: bool) {
        match self {
            #[cfg(feature = "liblsl-backend")]
            Self::Separate(_, status) => {
                if let Some(status) = status { status.publish(restored); }
            },
            #[cfg(not(feature = "liblsl-backend"))]
            Self::Separate(..) => {}
            #[cfg(feature = "liblsl-backend")]
            Self::Single(output) => output.publish_signal_state(restored),
        }
    }

    fn health(&self) -> String {
        match self {
            #[cfg(feature = "liblsl-backend")]
            Self::Separate(output, status) => {
                let health = output.health().lsl;
                let marker = status.as_ref().map(|marker| marker.health());
                let count = |text: &str| text.strip_prefix("Publishing ")
                    .and_then(|suffix| suffix.split_whitespace().next())
                    .and_then(|number| number.parse::<usize>().ok());
                match (count(&health), marker.as_deref().and_then(count)) {
                    (Some(main), Some(extra)) => format!("Publishing {} stream(s)", main + extra),
                    (Some(_), _) => health,
                    (_, Some(_)) => marker.unwrap_or(health),
                    _ => health,
                }
            }
            #[cfg(not(feature = "liblsl-backend"))]
            Self::Separate(output) => output.health().lsl,
            #[cfg(feature = "liblsl-backend")]
            Self::Single(output) => output.health(),
        }
    }

    fn publish_polar_ecg(&self, sensor_timestamp_ns: u64, samples: &[i32]) -> Option<String> {
        match self {
            Self::Separate(output, ..) => output.publish_ecg(sensor_timestamp_ns, samples),
            #[cfg(feature = "liblsl-backend")]
            Self::Single(output) => {
                output.publish_polar_ecg(sensor_timestamp_ns, samples);
                None
            }
        }
    }

    fn publish_polar_accelerometer(
        &self,
        sensor_timestamp_ns: u64,
        samples: &[polar_h10_core::AccSample],
    ) -> Option<String> {
        match self {
            Self::Separate(output, ..) => {
                output.publish_accelerometer(sensor_timestamp_ns, samples)
            }
            #[cfg(feature = "liblsl-backend")]
            Self::Single(output) => {
                output.publish_polar_accelerometer(sensor_timestamp_ns, samples);
                None
            }
        }
    }

    fn publish_polar_heart_rate(
        &self,
        beats_per_minute: u16,
        rr_intervals_ms: &[f32],
    ) -> Option<String> {
        match self {
            Self::Separate(output, ..) => {
                output.publish_heart_rate(beats_per_minute, rr_intervals_ms)
            }
            #[cfg(feature = "liblsl-backend")]
            Self::Single(output) => {
                output.publish_polar_heart_rate(beats_per_minute, rr_intervals_ms);
                None
            }
        }
    }

    fn publish_polar_metrics_at(
        &self,
        sensor_timestamp_ns: u64,
        values: &[MetricValue<'_>],
    ) -> Option<String> {
        match self {
            Self::Separate(output, ..) => output.publish_metrics_at(sensor_timestamp_ns, values),
            #[cfg(feature = "liblsl-backend")]
            Self::Single(output) => {
                output.publish_polar_metrics_at(sensor_timestamp_ns, values);
                None
            }
        }
    }

    async fn configure_vernier_streams(
        &self,
        model_code: &str,
        sample_period_us: u32,
        sensors: &[SensorInfo],
    ) -> Result<VernierStreamSchema, String> {
        match self {
            Self::Separate(output, ..) => {
                output.configure_vernier_streams(model_code, sample_period_us, sensors)
            }
            #[cfg(feature = "liblsl-backend")]
            Self::Single(output) => {
                output.configure_vernier_streams(model_code, sample_period_us, sensors)
            }
        }
    }

    fn publish_vernier_raw(&self, batch: VernierRawBatch<'_>) {
        match self {
            Self::Separate(output, ..) => output.publish_vernier_raw(
                batch.host_receive_timestamp_ns,
                batch.sample_period_us,
                batch.sequence,
                batch.dropped_before,
                batch.device_drop_reports_before,
                batch.decode_latency_ns,
                batch.encoding,
                batch.sensors,
            ),
            #[cfg(feature = "liblsl-backend")]
            Self::Single(output) => output.publish_vernier_raw(
                batch.host_receive_timestamp_ns,
                batch.sample_period_us,
                batch.sequence,
                batch.dropped_before,
                batch.device_drop_reports_before,
                batch.decode_latency_ns,
                batch.encoding,
                batch.sensors,
            ),
        }
    }

    fn publish_force(
        &self,
        host_receive_timestamp_ns: u64,
        values: &[f32],
        sample_period_us: u32,
    ) -> Option<String> {
        match self {
            Self::Separate(output, ..) => {
                output.publish_force(host_receive_timestamp_ns, values, sample_period_us)
            }
            #[cfg(feature = "liblsl-backend")]
            Self::Single(_) => None,
        }
    }

    fn publish_vernier_breathing(
        &self,
        host_receive_timestamp_ns: u64,
        values_01: &[f32],
        sample_period_us: u32,
    ) -> Option<String> {
        match self {
            Self::Separate(output, ..) => output.publish_vernier_breathing(
                host_receive_timestamp_ns,
                values_01,
                sample_period_us,
            ),
            #[cfg(feature = "liblsl-backend")]
            Self::Single(output) => {
                output.publish_vernier_breathing(
                    host_receive_timestamp_ns,
                    values_01,
                    sample_period_us,
                );
                None
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PolarMiniSettings {
    selection: MetricSelection,
}

impl PolarMiniSettings {
    fn from_outputs(outputs: &[String]) -> Self {
        Self {
            selection: MetricSelection::from_ids(outputs.iter().map(String::as_str)),
        }
    }
}

#[derive(Default)]
struct DisplayEndpoint {
    state: std::sync::Mutex<DisplayEndpointState>,
}

#[derive(Default)]
struct DisplayEndpointState {
    channel: Option<Channel<MiniEvent>>,
    connection_snapshot: Option<MiniEvent>,
    samples_snapshot: Option<MiniEvent>,
}

impl DisplayEndpoint {
    fn attach(&self, channel: Channel<MiniEvent>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.channel = Some(channel);
        for snapshot in [
            state.connection_snapshot.clone(),
            state.samples_snapshot.clone(),
        ]
        .into_iter()
        .flatten()
        {
            if state
                .channel
                .as_ref()
                .is_some_and(|channel| channel.send(snapshot).is_err())
            {
                state.channel = None;
                break;
            }
        }
    }

    fn send(&self, event: MiniEvent) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        match &event {
            MiniEvent::Connection { .. } => state.connection_snapshot = Some(event.clone()),
            MiniEvent::Samples { .. } => state.samples_snapshot = Some(event.clone()),
            _ => {}
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

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MiniEvent {
    Status {
        phase: String,
        message: String,
    },
    Connection {
        connected: bool,
        streaming: bool,
        device_name: Option<String>,
        battery_percent: Option<u8>,
        model_code: Option<String>,
        message: String,
    },
    Samples {
        ecg_samples: u64,
        acc_samples: u64,
        heart_rate_packets: u64,
        metric_samples: u64,
        vernier_rows: u64,
        dropped_batches: u64,
        queue_high_water: usize,
        lsl: String,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniBootstrap {
    pub kind: MiniAppKind,
    pub product_name: &'static str,
    pub scan_label: &'static str,
    pub preferences: MiniPreferencesSnapshot,
    pub metrics: Vec<MiniMetricOption>,
    pub session: Option<MiniSessionSnapshot>,
    pub mock_mode: bool,
    pub lsl_resource_present: bool,
    pub lsl_resource_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniMetricOption {
    pub id: &'static str,
    pub stream_suffix: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
    pub unit: &'static str,
    pub category: &'static str,
    pub default_included: bool,
    pub direct: bool,
    pub release_tier: MetricSelectionTier,
}

impl MiniMetricOption {
    fn from_definition(metric: MetricDefinition) -> Self {
        let direct = POLAR_DIRECT_OUTPUTS.contains(&metric.id);
        Self {
            id: metric.id,
            stream_suffix: metric.stream_suffix,
            label: metric.label,
            detail: metric.detail,
            unit: metric.unit,
            category: metric.category,
            default_included: direct,
            direct,
            release_tier: metric_selection_tier(metric.id),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniDeviceSummary {
    pub id: String,
    pub name: String,
    pub rssi: Option<i16>,
    pub detail: String,
    pub model_code: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniSessionSnapshot {
    pub connected: bool,
    pub device_id: String,
    pub device_name: Option<String>,
    pub stream_name: String,
    pub output_mode: MiniOutputMode,
    pub lsl: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniSaveResult {
    pub preferences: MiniPreferencesSnapshot,
    pub applied: bool,
    pub reconnect_required: bool,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewNodeLaunch {
    pub launched: bool,
    pub message: String,
}

pub fn setup_mini_state(app: &mut tauri::App, kind: MiniAppKind) -> tauri::Result<()> {
    let mock_mode = std::env::args_os().any(|argument| argument == MOCK_ARGUMENT);
    let bundled_lsl = app
        .path()
        .resolve(lsl_resource_path(), BaseDirectory::Resource)
        .ok()
        .filter(|path| path.is_file());
    let preferences_path = app.path().app_config_dir()?.join("preferences.json");
    let recording_directory = app
        .path()
        .download_dir()
        .map(|path| path.join(kind.product_name()))
        .unwrap_or_else(|_| {
            app.path()
                .app_data_dir()
                .unwrap_or_else(|_| {
                    preferences_path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .to_path_buf()
                })
                .join("recordings")
        });
    app.manage(MiniAppState::new(
        kind,
        bundled_lsl,
        preferences_path,
        recording_directory,
        mock_mode,
    ));
    Ok(())
}

pub async fn get_bootstrap(state: State<'_, MiniAppState>) -> CommandResult<MiniBootstrap> {
    let metrics = if state.kind == MiniAppKind::Polar {
        polar_metric_options()
    } else {
        Vec::new()
    };
    let session = active_snapshot(&state).await;
    Ok(MiniBootstrap {
        kind: state.kind,
        product_name: state.kind.product_name(),
        scan_label: state.kind.scan_label(),
        preferences: state.preferences.snapshot(),
        metrics,
        session,
        mock_mode: state.mock_mode,
        lsl_resource_present: state.bundled_lsl.is_some(),
        lsl_resource_path: state
            .bundled_lsl
            .as_ref()
            .map(|path| path.display().to_string()),
    })
}

pub fn attach_events(state: State<'_, MiniAppState>, events: Channel<MiniEvent>) {
    state.display.attach(events);
}

pub async fn scan_devices(state: State<'_, MiniAppState>) -> CommandResult<Vec<MiniDeviceSummary>> {
    if state.mock_mode {
        return Err(CommandError::new(
            "MOCK_MODE_ACTIVE",
            "Mock windows do not scan for Bluetooth devices.",
            false,
        ));
    }
    scan_devices_internal(&state)
        .await
        .map_err(|message| CommandError::new("BLUETOOTH_SCAN_FAILED", message, true))
}

pub async fn save_preferences(
    state: State<'_, MiniAppState>,
    preferences: MiniPreferencesInput,
) -> CommandResult<MiniSaveResult> {
    save_preferences_inner(&state, preferences).await
}

async fn save_preferences_inner(
    state: &MiniAppState,
    preferences: MiniPreferencesInput,
) -> CommandResult<MiniSaveResult> {
    let _configuration = state.configuration.lock().await;
    let snapshot = state
        .preferences
        .snapshot_for_input(state.kind, preferences)
        .map_err(|message| CommandError::new("INVALID_PREFERENCES", message, false))?;
    let active = {
        let active = state.active.lock().await;
        active.as_ref().map(|session| {
            (
                session.output_mode,
                session.stream_name.clone(),
                session.polar_outputs.clone(),
                session.vernier_outputs.clone(),
                session.output.clone(),
                session.settings.clone(),
                session.connected.load(Ordering::Acquire),
            )
        })
    };

    let Some((mode, stream_name, polar_outputs, vernier_outputs, output, settings, connected)) = active else {
        state
            .preferences
            .replace(snapshot.clone())
            .map_err(|message| CommandError::new("PREFERENCES_WRITE_FAILED", message, false))?;
        return Ok(MiniSaveResult {
            preferences: snapshot,
            applied: false,
            reconnect_required: false,
            message: "Saved.".into(),
        });
    };
    let contract_changed = mode != snapshot.output_mode
        || stream_name != snapshot.stream_name
        || (state.kind == MiniAppKind::Polar && polar_outputs != snapshot.polar_outputs)
        || (state.kind == MiniAppKind::Vernier && vernier_outputs != snapshot.vernier_outputs);
    if !contract_changed {
        state
            .preferences
            .replace(snapshot.clone())
            .map_err(|message| CommandError::new("PREFERENCES_WRITE_FAILED", message, false))?;
        return Ok(MiniSaveResult {
            preferences: snapshot,
            applied: false,
            reconnect_required: false,
            message: "Saved.".into(),
        });
    }

    let replacement = build_output(
        state.kind,
        &snapshot,
        state.bundled_lsl.clone(),
        state.recording_directory.clone(),
    )
    .await
    .map_err(|message| CommandError::new("OUTPUT_RECONFIGURE_FAILED", message, false))?;
    if let Some(schema) = state.vernier_schema.lock().await.as_ref() {
        replacement
            .configure_vernier_streams(
                schema.model_code(),
                schema.sample_period_us(),
                schema.channels(),
            )
            .await
            .map_err(|message| CommandError::new("OUTPUT_RECONFIGURE_FAILED", message, false))?;
    }
    let health = replacement.health();
    if output.borrow().health().starts_with("Publishing ") && !health.starts_with("Publishing ") {
        return Err(CommandError::new(
            "OUTPUT_RECONFIGURE_FAILED",
            format!("The requested LSL outlets could not be opened: {health}"),
            true,
        ));
    }
    state
        .preferences
        .replace(snapshot.clone())
        .map_err(|message| CommandError::new("PREFERENCES_WRITE_FAILED", message, false))?;
    if !connected {
        replacement.publish_signal_state(false);
    }
    output.send_replace(replacement);
    if let Some(settings) = settings {
        let _ = settings.send(PolarMiniSettings::from_outputs(&snapshot.polar_outputs));
    }
    if let Some(session) = state.active.lock().await.as_mut() {
        session.output_mode = snapshot.output_mode;
        session.stream_name = snapshot.stream_name.clone();
        session.polar_outputs = snapshot.polar_outputs.clone();
        session.vernier_outputs = snapshot.vernier_outputs.clone();
    }
    state.display.send(MiniEvent::Status {
        phase: "output".into(),
        message: health,
    });
    Ok(MiniSaveResult {
        preferences: snapshot,
        applied: true,
        reconnect_required: false,
        message: LIVE_RECONFIGURED_MESSAGE.into(),
    })
}

pub async fn connect_device(
    state: State<'_, MiniAppState>,
    device_id: String,
    preferences: MiniPreferencesInput,
) -> CommandResult<MiniSessionSnapshot> {
    if state.mock_mode {
        return Err(CommandError::new(
            "MOCK_MODE_ACTIVE",
            "Use the mock source control in this window.",
            false,
        ));
    }
    let snapshot = state
        .preferences
        .save_input(state.kind, preferences)
        .map_err(|message| CommandError::new("PREFERENCES_WRITE_FAILED", message, false))?;
    connect_with_snapshot(&state, device_id, snapshot, true).await
}

pub async fn connect_remembered(
    state: State<'_, MiniAppState>,
) -> CommandResult<MiniSessionSnapshot> {
    if state.mock_mode {
        return Err(CommandError::new(
            "MOCK_MODE_ACTIVE",
            "Mock windows do not reconnect Bluetooth devices.",
            false,
        ));
    }
    let snapshot = state.preferences.snapshot();
    let Some(saved) = snapshot.last_device.clone() else {
        return Err(CommandError::new(
            "NO_REMEMBERED_DEVICE",
            "No previously connected sensor is saved yet.",
            false,
        ));
    };
    connect_with_snapshot(&state, saved.id, snapshot, false).await
}

pub async fn start_mock_stream(
    state: State<'_, MiniAppState>,
    preferences: MiniPreferencesInput,
) -> CommandResult<MiniSessionSnapshot> {
    if !state.mock_mode {
        return Err(CommandError::new(
            "MOCK_MODE_REQUIRED",
            "Create a mock stream window before starting synthetic data.",
            false,
        ));
    }
    let snapshot = state
        .preferences
        .save_input(state.kind, preferences)
        .map_err(|message| CommandError::new("PREFERENCES_WRITE_FAILED", message, false))?;
    start_mock_with_snapshot(&state, snapshot).await
}

pub async fn disconnect_device(
    state: State<'_, MiniAppState>,
) -> CommandResult<MiniSessionSnapshot> {
    disconnect_active(&state).await?;
    Ok(MiniSessionSnapshot {
        connected: false,
        device_id: String::new(),
        device_name: None,
        stream_name: state.preferences.snapshot().stream_name,
        output_mode: state.preferences.snapshot().output_mode,
        lsl: "Off".into(),
    })
}

pub fn open_new_node() -> CommandResult<NewNodeLaunch> {
    let executable = std::env::current_exe().map_err(|error| {
        CommandError::new(
            "NEW_NODE_PATH_FAILED",
            format!("Could not resolve the running mini app executable: {error}"),
            false,
        )
    })?;
    Command::new(executable).spawn().map_err(|error| {
        CommandError::new(
            "NEW_NODE_LAUNCH_FAILED",
            format!("Could not open a new mini node instance: {error}"),
            false,
        )
    })?;
    Ok(NewNodeLaunch {
        launched: true,
        message: "Opened a new mini node instance.".into(),
    })
}

pub fn open_mock_node() -> CommandResult<NewNodeLaunch> {
    let executable = std::env::current_exe().map_err(|error| {
        CommandError::new(
            "MOCK_NODE_PATH_FAILED",
            format!("Could not resolve the running mini app executable: {error}"),
            false,
        )
    })?;
    Command::new(executable)
        .arg(MOCK_ARGUMENT)
        .spawn()
        .map_err(|error| {
            CommandError::new(
                "MOCK_NODE_LAUNCH_FAILED",
                format!("Could not open a mock stream instance: {error}"),
                false,
            )
        })?;
    Ok(NewNodeLaunch {
        launched: true,
        message: "Opened an independent mock stream applet.".into(),
    })
}

pub fn close_app(app: AppHandle) {
    app.exit(0);
}

pub async fn graceful_shutdown(app: AppHandle) {
    let state = app.state::<MiniAppState>();
    let _ = disconnect_active(&state).await;
}

async fn connect_with_snapshot(
    state: &MiniAppState,
    device_id: String,
    snapshot: MiniPreferencesSnapshot,
    connect_now: bool,
) -> CommandResult<MiniSessionSnapshot> {
    let active = state.active.lock().await.as_ref().map(|session| {
        (
            session.running.load(Ordering::Acquire),
            session.connected.load(Ordering::Acquire),
            session.device_id.clone(),
            session_snapshot(session),
        )
    });
    if let Some((running, connected, active_id, session)) = active {
        if running && (!connect_now || connected || active_id == device_id) {
            return Ok(session);
        }
        disconnect_active(state).await?;
    }
    let _configuration = state.configuration.lock().await;

    let output = build_output(
        state.kind,
        &snapshot,
        state.bundled_lsl.clone(),
        state.recording_directory.clone(),
    )
    .await
    .map_err(|message| CommandError::new("OUTPUT_CONFIGURE_FAILED", message, false))?;
    let settings = (state.kind == MiniAppKind::Polar).then(|| {
        tokio::sync::watch::channel(PolarMiniSettings::from_outputs(&snapshot.polar_outputs))
    });
    state.display.send(MiniEvent::Status {
        phase: "connect".into(),
        message: format!("Connecting to {}", state.kind.scan_label()),
    });

    let target = snapshot
        .recent_devices
        .iter()
        .find(|saved| saved.id == device_id)
        .cloned()
        .unwrap_or_else(|| SavedMiniDevice {
            id: device_id.clone(),
            name: device_id.clone(),
        });
    let (output_tx, output_rx) = tokio::sync::watch::channel(output.clone());
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let connected = Arc::new(AtomicBool::new(false));
    let running = Arc::new(AtomicBool::new(true));
    let display = state.display.clone();
    let preferences = state.preferences.clone();
    let slot = MINI_SLOT.to_string();
    let task = match state.kind {
        MiniAppKind::Polar => {
            let input_events = if connect_now {
                match state
                    .polar_input
                    .connect_low_latency(&slot, &device_id)
                    .await
                {
                    Ok(events) => Some(events),
                    Err(message) => {
                        display.send(MiniEvent::Status {
                            phase: "reconnect".into(),
                            message,
                        });
                        None
                    }
                }
            } else {
                None
            };
            let settings_rx = settings
                .as_ref()
                .map(|(_, receiver)| receiver.clone())
                .expect("Polar mini settings channel is created above");
            let target_for_task = target.clone();
            let configuration = state.configuration.clone();
            let input = state.polar_input.clone();
            let connected_for_task = connected.clone();
            let running_for_task = running.clone();
            tauri::async_runtime::spawn(async move {
                supervise_polar_session(
                    input_events,
                    output_rx,
                    settings_rx,
                    display,
                    preferences,
                    target_for_task,
                    configuration,
                    input,
                    cancel_rx,
                    connected_for_task,
                )
                .await;
                running_for_task.store(false, Ordering::Release);
            })
        }
        MiniAppKind::Vernier => {
            let input_events = if connect_now {
                match state
                    .gdx_input
                    .connect(&slot, &device_id, mini_vernier_config())
                    .await
                {
                    Ok(events) => Some(events),
                    Err(message) => {
                        display.send(MiniEvent::Status {
                            phase: "reconnect".into(),
                            message,
                        });
                        None
                    }
                }
            } else {
                None
            };
            let target_for_task = target.clone();
            let configuration = state.configuration.clone();
            let vernier_schema = state.vernier_schema.clone();
            let input = state.gdx_input.clone();
            let connected_for_task = connected.clone();
            let running_for_task = running.clone();
            tauri::async_runtime::spawn(async move {
                supervise_vernier_session(
                    input_events,
                    output_rx,
                    display,
                    preferences,
                    target_for_task,
                    configuration,
                    vernier_schema,
                    input,
                    cancel_rx,
                    connected_for_task,
                )
                .await;
                running_for_task.store(false, Ordering::Release);
            })
        }
    };
    let settings_tx = settings.map(|(sender, _)| sender);
    let session = ActiveMiniSession {
        slot,
        device_id: device_id.clone(),
        device_name: None,
        output_mode: snapshot.output_mode,
        stream_name: snapshot.stream_name.clone(),
        polar_outputs: snapshot.polar_outputs.clone(),
        vernier_outputs: snapshot.vernier_outputs.clone(),
        output: output_tx,
        settings: settings_tx,
        mock: false,
        connected,
        running,
        cancel: cancel_tx,
        task,
    };
    let result = session_snapshot(&session);
    *state.active.lock().await = Some(session);
    Ok(result)
}

async fn start_mock_with_snapshot(
    state: &MiniAppState,
    snapshot: MiniPreferencesSnapshot,
) -> CommandResult<MiniSessionSnapshot> {
    let _configuration = state.configuration.lock().await;
    if let Some(session) = active_snapshot(state).await {
        return Ok(session);
    }

    let mock_ecg = if state.kind == MiniAppKind::Polar {
        let bundled_lsl = state.bundled_lsl.clone();
        Some(
            tokio::task::spawn_blocking(move || load_mock_ecg(bundled_lsl.as_deref()))
                .await
                .map_err(|_| {
                    CommandError::new(
                        "MOCK_RECORDING_UNAVAILABLE",
                        "The bundled 60-minute NeuroKit ECG recording could not be loaded.",
                        false,
                    )
                })?
                .map_err(|message| {
                    CommandError::new("MOCK_RECORDING_UNAVAILABLE", message, false)
                })?,
        )
    } else {
        None
    };

    let output = build_output(
        state.kind,
        &snapshot,
        state.bundled_lsl.clone(),
        state.recording_directory.clone(),
    )
    .await
    .map_err(|message| CommandError::new("OUTPUT_CONFIGURE_FAILED", message, false))?;
    if state.kind == MiniAppKind::Vernier {
        let schema = output
            .configure_vernier_streams("GDX-RB-MOCK", VERNIER_PERIOD_US, &mock_vernier_sensors())
            .await
            .map_err(|message| CommandError::new("OUTPUT_CONFIGURE_FAILED", message, false))?;
        *state.vernier_schema.lock().await = Some(schema);
    }

    let device_name = match state.kind {
        MiniAppKind::Polar => "Mock Polar H10",
        MiniAppKind::Vernier => "Mock Vernier GDX-RB",
    };
    let settings = (state.kind == MiniAppKind::Polar).then(|| {
        tokio::sync::watch::channel(PolarMiniSettings::from_outputs(&snapshot.polar_outputs))
    });
    let (output_tx, output_rx) = tokio::sync::watch::channel(output.clone());
    let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);
    let display = state.display.clone();
    let task = match state.kind {
        MiniAppKind::Polar => {
            let settings_rx = settings
                .as_ref()
                .map(|(_, receiver)| receiver.clone())
                .expect("Polar mock settings channel is created above");
            let mock_ecg = mock_ecg.expect("Polar mock recording was loaded above");
            tauri::async_runtime::spawn(async move {
                run_polar_mock(output_rx, settings_rx, display, mock_ecg).await;
            })
        }
        MiniAppKind::Vernier => tauri::async_runtime::spawn(async move {
            run_vernier_mock(output_rx, display).await;
        }),
    };
    let session = ActiveMiniSession {
        slot: MINI_SLOT.into(),
        device_id: format!("mock://{}-{}", state.kind.product_name(), process::id()),
        device_name: Some(device_name.into()),
        output_mode: snapshot.output_mode,
        stream_name: snapshot.stream_name.clone(),
        polar_outputs: snapshot.polar_outputs.clone(),
        vernier_outputs: snapshot.vernier_outputs.clone(),
        output: output_tx,
        settings: settings.map(|(sender, _)| sender),
        mock: true,
        connected: Arc::new(AtomicBool::new(true)),
        running: Arc::new(AtomicBool::new(true)),
        cancel: cancel_tx,
        task,
    };
    let result = session_snapshot(&session);
    *state.active.lock().await = Some(session);
    state.display.send(MiniEvent::Connection {
        connected: true,
        streaming: output.health().starts_with("Publishing "),
        device_name: Some(device_name.into()),
        battery_percent: None,
        model_code: Some(
            match state.kind {
                MiniAppKind::Polar => "H10-MOCK",
                MiniAppKind::Vernier => "GDX-RB-MOCK",
            }
            .into(),
        ),
        message: format!(
            "Publishing synthetic {} data to LSL",
            state.kind.scan_label()
        ),
    });
    Ok(result)
}

async fn build_output(
    kind: MiniAppKind,
    snapshot: &MiniPreferencesSnapshot,
    bundled_lsl: Option<PathBuf>,
    recording_directory: PathBuf,
) -> Result<MiniOutputHandle, String> {
    match snapshot.output_mode {
        MiniOutputMode::SeparateStreams => {
            #[cfg(feature = "liblsl-backend")]
            let bundled_lsl_for_status = bundled_lsl.clone();
            let output = Arc::new(OutputRouter::with_bundled_lsl_and_recordings(
                bundled_lsl,
                recording_directory,
            ));
            output.configure(mini_output_config(kind, snapshot)).await?;
            #[cfg(feature = "liblsl-backend")]
            {
                let include_status = kind == MiniAppKind::Polar
                    || snapshot.vernier_outputs.iter().any(|id| id == "signalStatus");
                let status = if include_status {
                    Some(Arc::new(MiniStatusOutput::new(
                        bundled_lsl_for_status,
                        &snapshot.stream_name,
                        kind == MiniAppKind::Polar,
                    )?))
                } else { None };
                Ok(MiniOutputHandle::Separate(output, status))
            }
            #[cfg(not(feature = "liblsl-backend"))]
            {
                Ok(MiniOutputHandle::Separate(output))
            }
        }
        MiniOutputMode::SingleStream => build_single_output(kind, snapshot, bundled_lsl),
    }
}

#[cfg(feature = "liblsl-backend")]
fn build_single_output(
    kind: MiniAppKind,
    snapshot: &MiniPreferencesSnapshot,
    bundled_lsl: Option<PathBuf>,
) -> Result<MiniOutputHandle, String> {
    match kind {
        MiniAppKind::Polar => Ok(MiniOutputHandle::Single(Arc::new(
            MiniCombinedOutput::polar(bundled_lsl, &snapshot.stream_name, &snapshot.polar_outputs)?,
        ))),
        MiniAppKind::Vernier => Ok(MiniOutputHandle::Single(Arc::new(
            MiniCombinedOutput::vernier(bundled_lsl, &snapshot.stream_name, &snapshot.vernier_outputs)?,
        ))),
    }
}

#[cfg(not(feature = "liblsl-backend"))]
fn build_single_output(
    _kind: MiniAppKind,
    _snapshot: &MiniPreferencesSnapshot,
    _bundled_lsl: Option<PathBuf>,
) -> Result<MiniOutputHandle, String> {
    Err("Single stream mode requires the packaged liblsl backend.".into())
}

fn mini_output_config(kind: MiniAppKind, snapshot: &MiniPreferencesSnapshot) -> OutputConfig {
    let vernier = VernierMiniSelection::from_ids(Some(&snapshot.vernier_outputs));
    OutputConfig {
        stream_name: snapshot.stream_name.clone(),
        lsl_enabled: true,
        osc_enabled: false,
        csv_enabled: false,
        audio_enabled: false,
        source_palette: source_palette(kind.palette_id()),
        outputs: match kind {
            MiniAppKind::Polar => snapshot.polar_outputs.clone(),
            MiniAppKind::Vernier => if vernier.raw_force { vec![VERNIER_FORCE_OUTPUT.into()] } else { Vec::new() },
        },
        vernier_outputs: (kind == MiniAppKind::Vernier).then(|| snapshot.vernier_outputs.clone()),
        ..Default::default()
    }
}

fn mini_vernier_config() -> SessionConfig {
    SessionConfig {
        period_us: VERNIER_PERIOD_US,
        prefer_low_latency_link: true,
    }
}

async fn scan_devices_internal(state: &MiniAppState) -> Result<Vec<MiniDeviceSummary>, String> {
    match state.kind {
        MiniAppKind::Polar => Ok(state
            .polar_input
            .scan_all()
            .await?
            .into_iter()
            .map(|device| MiniDeviceSummary {
                id: device.id,
                name: device.name,
                rssi: device.rssi,
                detail: "Polar H10 ECG, HR/RR, and accelerometer".into(),
                model_code: "H10".into(),
            })
            .collect()),
        MiniAppKind::Vernier => Ok(state
            .gdx_input
            .scan()
            .await?
            .into_iter()
            .map(|device| MiniDeviceSummary {
                id: device.id,
                name: device.name,
                rssi: device.rssi,
                detail: device.model_name.into(),
                model_code: device.model_code.into(),
            })
            .collect()),
    }
}

async fn active_snapshot(state: &MiniAppState) -> Option<MiniSessionSnapshot> {
    state.active.lock().await.as_ref().map(session_snapshot)
}

fn session_snapshot(session: &ActiveMiniSession) -> MiniSessionSnapshot {
    MiniSessionSnapshot {
        connected: session.connected.load(Ordering::Acquire),
        device_id: session.device_id.clone(),
        device_name: session.device_name.clone(),
        stream_name: session.stream_name.clone(),
        output_mode: session.output_mode,
        lsl: session.output.borrow().health(),
    }
}

async fn disconnect_active(state: &MiniAppState) -> CommandResult<()> {
    let Some(mut session) = state.active.lock().await.take() else {
        state.display.send(MiniEvent::Connection {
            connected: false,
            streaming: false,
            device_name: None,
            battery_percent: None,
            model_code: None,
            message: "Disconnected".into(),
        });
        return Ok(());
    };
    let _ = session.cancel.send(true);
    if session.mock {
        session.task.abort();
    } else {
        match state.kind {
            MiniAppKind::Polar => state.polar_input.disconnect(&session.slot).await,
            MiniAppKind::Vernier => state.gdx_input.disconnect(&session.slot).await,
        }
        .map_err(|message| CommandError::new("DISCONNECT_FAILED", message, true))?;
    }
    if tokio::time::timeout(TASK_JOIN_TIMEOUT, &mut session.task)
        .await
        .is_err()
    {
        session.task.abort();
    }
    *state.vernier_schema.lock().await = None;
    state.display.send(MiniEvent::Connection {
        connected: false,
        streaming: false,
        device_name: session.device_name,
        battery_percent: None,
        model_code: None,
        message: "Disconnected".into(),
    });
    Ok(())
}

fn mock_vernier_sensors() -> Vec<SensorInfo> {
    vec![SensorInfo {
        number: 1,
        sensor_id: 1,
        numeric_type: NumericMeasurementType::Real,
        sampling_mode: SamplingMode::Periodic,
        description: "Force".into(),
        unit: "N".into(),
        uncertainty: 0.01,
        minimum: 0.0,
        maximum: 50.0,
        minimum_period_us: VERNIER_PERIOD_US,
        maximum_period_us: 60_000_000,
        typical_period_us: VERNIER_PERIOD_US,
        period_granularity_us: 1_000,
        mutual_exclusion_mask: 0,
    }]
}

async fn run_polar_mock(
    output_rx: tokio::sync::watch::Receiver<MiniOutputHandle>,
    mut settings_rx: tokio::sync::watch::Receiver<PolarMiniSettings>,
    display: Arc<DisplayEndpoint>,
    mock_ecg: Vec<i16>,
) {
    const ACC_SAMPLES_PER_TICK: u64 = 2;

    let mut interval = tokio::time::interval(POLAR_MOCK_TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut metrics_engine = MetricEngine::with_selection(settings_rx.borrow().selection);
    let mut source_clock = SourceClockMapper::default();
    let mut counts = MiniCounters::default();
    let mut last_event = Instant::now();
    let mut ecg_index = 0_u64;
    let mut acc_index = 0_u64;
    let mut tick = 0_u64;

    loop {
        interval.tick().await;
        let output = output_rx.borrow().clone();
        if settings_rx.has_changed().unwrap_or(false) {
            metrics_engine.apply_selection(settings_rx.borrow_and_update().selection);
        }

        let host_receive_timestamp_ns = monotonic_now_ns().max(1);
        let ecg_samples_this_tick = (tick + 1) * 13 / 10 - tick * 13 / 10;
        let ecg = (0..ecg_samples_this_tick)
            .map(|offset| {
                let index = (ecg_index.saturating_add(offset) % mock_ecg.len() as u64) as usize;
                i32::from(mock_ecg[index])
            })
            .collect::<Vec<_>>();
        output_warning(
            &display,
            output.publish_polar_ecg(host_receive_timestamp_ns, &ecg),
        );
        counts.ecg_samples = counts.ecg_samples.saturating_add(ecg_samples_this_tick);
        let derived = metrics_engine.process_ecg(&ecg);
        publish_metric_samples(
            &display,
            &output,
            host_receive_timestamp_ns,
            &derived,
            &mut counts,
        );
        ecg_index = ecg_index.saturating_add(ecg_samples_this_tick);

        let acc = (0..ACC_SAMPLES_PER_TICK)
            .map(|offset| mock_acc_sample(acc_index.saturating_add(offset)))
            .collect::<Vec<_>>();
        output_warning(
            &display,
            output.publish_polar_accelerometer(host_receive_timestamp_ns, &acc),
        );
        counts.acc_samples = counts.acc_samples.saturating_add(ACC_SAMPLES_PER_TICK);
        let mapping =
            source_clock.observe_and_map(host_receive_timestamp_ns, host_receive_timestamp_ns);
        let derived = metrics_engine.process_accelerometer_timed(
            &acc,
            TimedAccBatch {
                newest_sensor_timestamp_ns: host_receive_timestamp_ns,
                sample_period_ns: ACC_SAMPLE_PERIOD_NS,
                clock_revision: mapping.revision,
                clock_reset: mapping.reset,
                gap_before: mapping.reset,
            },
        );
        publish_metric_samples(
            &display,
            &output,
            host_receive_timestamp_ns,
            &derived,
            &mut counts,
        );
        acc_index = acc_index.saturating_add(ACC_SAMPLES_PER_TICK);

        if tick.is_multiple_of(100) {
            let rr_intervals_ms = [833.33_f32];
            output_warning(
                &display,
                output.publish_polar_heart_rate(72, &rr_intervals_ms),
            );
            counts.heart_rate_packets = counts.heart_rate_packets.saturating_add(1);
            let derived = metrics_engine.process_heart_rate(72, &rr_intervals_ms);
            publish_metric_samples(
                &display,
                &output,
                host_receive_timestamp_ns,
                &derived,
                &mut counts,
            );
        }
        tick = tick.saturating_add(1);
        send_counts_if_due(&display, &output, &counts, &mut last_event);
    }
}

async fn run_vernier_mock(
    output_rx: tokio::sync::watch::Receiver<MiniOutputHandle>,
    display: Arc<DisplayEndpoint>,
) {
    const SAMPLES_PER_TICK: u64 = 1;

    let mut interval = tokio::time::interval(VERNIER_MOCK_TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut breathing = VernierBreathingProcessor::default();
    let mut counts = MiniCounters::default();
    let mut last_event = Instant::now();
    let mut sample_index = 0_u64;
    let mut sequence = 0_u64;

    loop {
        interval.tick().await;
        let output = output_rx.borrow().clone();
        let host_receive_timestamp_ns = monotonic_now_ns().max(1);
        let force_values = (0..SAMPLES_PER_TICK)
            .map(|offset| mock_force_sample(sample_index.saturating_add(offset)))
            .collect::<Vec<_>>();
        let sensors = [SensorSamples {
            sensor_number: 1,
            values: force_values.clone(),
        }];
        output.publish_vernier_raw(VernierRawBatch {
            host_receive_timestamp_ns,
            sample_period_us: VERNIER_PERIOD_US,
            sequence,
            dropped_before: 0,
            device_drop_reports_before: 0,
            decode_latency_ns: 0,
            encoding: SampleEncoding::Float32,
            sensors: &sensors,
        });
        let force_values_f32 = force_values
            .iter()
            .map(|value| *value as f32)
            .collect::<Vec<_>>();
        output_warning(
            &display,
            output.publish_force(
                host_receive_timestamp_ns,
                &force_values_f32,
                VERNIER_PERIOD_US,
            ),
        );
        let waveform = breathing.push(&force_values, VERNIER_PERIOD_US);
        output_warning(
            &display,
            output.publish_vernier_breathing(
                host_receive_timestamp_ns,
                &waveform,
                VERNIER_PERIOD_US,
            ),
        );
        counts.vernier_rows = counts.vernier_rows.saturating_add(SAMPLES_PER_TICK);
        counts.metric_samples = counts
            .metric_samples
            .saturating_add(u64::try_from(waveform.len()).unwrap_or(u64::MAX));
        sample_index = sample_index.saturating_add(SAMPLES_PER_TICK);
        sequence = sequence.saturating_add(1);
        send_counts_if_due(&display, &output, &counts, &mut last_event);
    }
}

fn load_mock_ecg(bundled_lsl: Option<&Path>) -> Result<Vec<i16>, &'static str> {
    const ERROR: &str = "The bundled 60-minute NeuroKit ECG recording is missing or invalid.";
    let path = bundled_lsl
        .and_then(Path::parent)
        .map(|directory| directory.join(MOCK_ECG_FILE))
        .ok_or(ERROR)?;
    let expected_bytes = MOCK_ECG_SAMPLE_COUNT * std::mem::size_of::<i16>();
    if fs::metadata(&path).map_err(|_| ERROR)?.len() != expected_bytes as u64 {
        return Err(ERROR);
    }
    let bytes = fs::read(path).map_err(|_| ERROR)?;
    if bytes.len() != expected_bytes {
        return Err(ERROR);
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
        .collect())
}

fn mock_acc_sample(index: u64) -> AccSample {
    let phase = index as f64 * std::f64::consts::TAU * 0.22 / 200.0;
    AccSample {
        x_mg: (26.0 * phase.sin()).round() as i16,
        y_mg: (18.0 * (phase + 0.8).sin()).round() as i16,
        z_mg: (1_000.0 + 42.0 * phase.sin()).round() as i16,
    }
}

fn mock_force_sample(index: u64) -> f64 {
    let phase = index as f64 * std::f64::consts::TAU * 0.22 / 20.0;
    12.0 + 2.4 * phase.sin()
}

async fn wait_to_retry(cancel: &mut tokio::sync::watch::Receiver<bool>) -> bool {
    if *cancel.borrow() {
        return false;
    }
    tokio::select! {
        _ = tokio::time::sleep(RECONNECT_DELAY) => !*cancel.borrow(),
        _ = cancel.changed() => !*cancel.borrow(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn supervise_polar_session(
    mut events: Option<tokio::sync::mpsc::Receiver<PolarInputEvent>>,
    output_rx: tokio::sync::watch::Receiver<MiniOutputHandle>,
    settings_rx: tokio::sync::watch::Receiver<PolarMiniSettings>,
    display: Arc<DisplayEndpoint>,
    preferences: Arc<PreferencesStore>,
    target: SavedMiniDevice,
    configuration: Arc<tokio::sync::Mutex<()>>,
    input: Arc<PolarInputSessionPool>,
    mut cancel: tokio::sync::watch::Receiver<bool>,
    connected: Arc<AtomicBool>,
) {
    let mut ever_connected = false;
    let mut direct_attempted = false;
    let mut current_device_id = target.id.clone();
    loop {
        if *cancel.borrow() {
            break;
        }
        if let Some(input_events) = events.take() {
            let connected_this_run = run_polar_session(
                input_events,
                output_rx.clone(),
                settings_rx.clone(),
                display.clone(),
                preferences.clone(),
                current_device_id.clone(),
                configuration.clone(),
                ever_connected,
                &connected,
            )
            .await;
            ever_connected |= connected_this_run;
            direct_attempted = !connected_this_run;
            connected.store(false, Ordering::Release);
            let _ = input.disconnect(MINI_SLOT).await;
        }
        if *cancel.borrow() || !preferences.snapshot().auto_connect {
            break;
        }
        if !wait_to_retry(&mut cancel).await {
            break;
        }
        let saved = if ever_connected {
            preferences
                .snapshot()
                .last_device
                .unwrap_or_else(|| target.clone())
        } else {
            target.clone()
        };
        if ever_connected
            && !direct_attempted
            && !*cancel.borrow()
            && preferences.snapshot().auto_connect
        {
            if let Ok(receiver) = input.connect_low_latency(MINI_SLOT, &saved.id).await {
                current_device_id = saved.id.clone();
                events = Some(receiver);
                continue;
            }
            direct_attempted = true;
        }
        display.send(MiniEvent::Status {
            phase: "reconnect".into(),
            message: format!("Looking for {}", saved.name),
        });
        match input.scan_all().await {
            Ok(devices) => {
                if let Some(id) = devices
                    .iter()
                    .find(|device| device.id == saved.id)
                    .or_else(|| {
                        devices
                            .iter()
                            .find(|device| device.name.eq_ignore_ascii_case(&saved.name))
                    })
                    .map(|device| device.id.clone())
                {
                    if !preferences.snapshot().auto_connect || *cancel.borrow() {
                        break;
                    }
                    match input.connect_low_latency(MINI_SLOT, &id).await {
                        Ok(receiver) => {
                            current_device_id = id;
                            events = Some(receiver);
                        }
                        Err(message) => display.send(MiniEvent::Status {
                            phase: "reconnect".into(),
                            message,
                        }),
                    }
                }
            }
            Err(message) => display.send(MiniEvent::Status {
                phase: "reconnect".into(),
                message,
            }),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn supervise_vernier_session(
    mut events: Option<tokio::sync::mpsc::Receiver<GdxInputEvent>>,
    output_rx: tokio::sync::watch::Receiver<MiniOutputHandle>,
    display: Arc<DisplayEndpoint>,
    preferences: Arc<PreferencesStore>,
    target: SavedMiniDevice,
    configuration: Arc<tokio::sync::Mutex<()>>,
    vernier_schema: Arc<tokio::sync::Mutex<Option<VernierStreamSchema>>>,
    input: Arc<GdxInputSessionPool>,
    mut cancel: tokio::sync::watch::Receiver<bool>,
    connected: Arc<AtomicBool>,
) {
    let mut ever_connected = false;
    let mut direct_attempted = false;
    let mut current_device_id = target.id.clone();
    loop {
        if *cancel.borrow() {
            break;
        }
        if let Some(input_events) = events.take() {
            let connected_this_run = run_vernier_session(
                input_events,
                output_rx.clone(),
                display.clone(),
                preferences.clone(),
                current_device_id.clone(),
                configuration.clone(),
                vernier_schema.clone(),
                ever_connected,
                &connected,
            )
            .await;
            ever_connected |= connected_this_run;
            direct_attempted = !connected_this_run;
            connected.store(false, Ordering::Release);
            let _ = input.disconnect(MINI_SLOT).await;
        }
        if *cancel.borrow() || !preferences.snapshot().auto_connect {
            break;
        }
        if !wait_to_retry(&mut cancel).await {
            break;
        }
        let saved = if ever_connected {
            preferences
                .snapshot()
                .last_device
                .unwrap_or_else(|| target.clone())
        } else {
            target.clone()
        };
        if ever_connected
            && !direct_attempted
            && !*cancel.borrow()
            && preferences.snapshot().auto_connect
        {
            if let Ok(receiver) = input
                .connect(MINI_SLOT, &saved.id, mini_vernier_config())
                .await
            {
                current_device_id = saved.id.clone();
                events = Some(receiver);
                continue;
            }
            direct_attempted = true;
        }
        display.send(MiniEvent::Status {
            phase: "reconnect".into(),
            message: format!("Looking for {}", saved.name),
        });
        match input.scan().await {
            Ok(devices) => {
                if let Some(id) = devices
                    .iter()
                    .find(|device| device.id == saved.id)
                    .or_else(|| {
                        devices
                            .iter()
                            .find(|device| device.name.eq_ignore_ascii_case(&saved.name))
                    })
                    .map(|device| device.id.clone())
                {
                    if !preferences.snapshot().auto_connect || *cancel.borrow() {
                        break;
                    }
                    match input.connect(MINI_SLOT, &id, mini_vernier_config()).await {
                        Ok(receiver) => {
                            current_device_id = id;
                            events = Some(receiver);
                        }
                        Err(message) => display.send(MiniEvent::Status {
                            phase: "reconnect".into(),
                            message,
                        }),
                    }
                }
            }
            Err(message) => display.send(MiniEvent::Status {
                phase: "reconnect".into(),
                message,
            }),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_polar_session(
    mut input_events: tokio::sync::mpsc::Receiver<PolarInputEvent>,
    output_rx: tokio::sync::watch::Receiver<MiniOutputHandle>,
    mut settings_rx: tokio::sync::watch::Receiver<PolarMiniSettings>,
    display: Arc<DisplayEndpoint>,
    preferences: Arc<PreferencesStore>,
    device_id: String,
    configuration: Arc<tokio::sync::Mutex<()>>,
    restoring: bool,
    connected: &AtomicBool,
) -> bool {
    let mut metrics_engine = MetricEngine::with_selection(settings_rx.borrow().selection);
    let mut source_clock = SourceClockMapper::default();
    let mut counts = MiniCounters::default();
    let mut last_event = Instant::now();
    let mut had_connection = false;
    let mut loss_reported = false;
    while let Some(event) = input_events.recv().await {
        let output = output_rx.borrow().clone();
        if settings_rx.has_changed().unwrap_or(false) {
            metrics_engine.apply_selection(settings_rx.borrow_and_update().selection);
        }
        match event {
            PolarInputEvent::Status { phase, message } => display.send(MiniEvent::Status {
                phase: phase.into(),
                message,
            }),
            PolarInputEvent::Connected {
                device_name,
                battery_percent,
            } => {
                had_connection = true;
                connected.store(true, Ordering::Release);
                if restoring {
                    output.publish_signal_state(true);
                }
                let _configuration = configuration.lock().await;
                let _ = preferences.save_last_device(SavedMiniDevice {
                    id: device_id.clone(),
                    name: device_name.clone(),
                });
                display.send(MiniEvent::Connection {
                    connected: true,
                    streaming: true,
                    device_name: Some(device_name),
                    battery_percent,
                    model_code: Some("H10".into()),
                    message: "Publishing H10 ECG, HR/RR, and accelerometer to LSL".into(),
                });
            }
            PolarInputEvent::Ecg {
                sensor_timestamp_ns,
                microvolts,
                ..
            } => {
                output_warning(
                    &display,
                    output.publish_polar_ecg(sensor_timestamp_ns, &microvolts),
                );
                counts.ecg_samples = counts
                    .ecg_samples
                    .saturating_add(u64::try_from(microvolts.len()).unwrap_or(u64::MAX));
                let derived = metrics_engine.process_ecg(&microvolts);
                publish_metric_samples(
                    &display,
                    &output,
                    sensor_timestamp_ns,
                    &derived,
                    &mut counts,
                );
            }
            PolarInputEvent::Accelerometer {
                sensor_timestamp_ns,
                host_receive_timestamp_ns,
                samples,
            } => {
                output_warning(
                    &display,
                    output.publish_polar_accelerometer(sensor_timestamp_ns, &samples),
                );
                counts.acc_samples = counts
                    .acc_samples
                    .saturating_add(u64::try_from(samples.len()).unwrap_or(u64::MAX));
                let mapping =
                    source_clock.observe_and_map(sensor_timestamp_ns, host_receive_timestamp_ns);
                let derived = metrics_engine.process_accelerometer_timed(
                    &samples,
                    TimedAccBatch {
                        newest_sensor_timestamp_ns: sensor_timestamp_ns,
                        sample_period_ns: ACC_SAMPLE_PERIOD_NS,
                        clock_revision: mapping.revision,
                        clock_reset: mapping.reset,
                        gap_before: mapping.reset,
                    },
                );
                publish_metric_samples(
                    &display,
                    &output,
                    sensor_timestamp_ns,
                    &derived,
                    &mut counts,
                );
            }
            PolarInputEvent::HeartRate {
                beats_per_minute,
                rr_intervals_ms,
                ..
            } => {
                output_warning(
                    &display,
                    output.publish_polar_heart_rate(beats_per_minute, &rr_intervals_ms),
                );
                counts.heart_rate_packets = counts.heart_rate_packets.saturating_add(1);
                let derived = metrics_engine.process_heart_rate(beats_per_minute, &rr_intervals_ms);
                publish_metric_samples(&display, &output, 0, &derived, &mut counts);
            }
            PolarInputEvent::Error(message) => display.send(MiniEvent::Error {
                code: "SENSOR_DATA_WARNING".into(),
                message,
            }),
            PolarInputEvent::Disconnected {
                device_name,
                battery_percent,
            } => {
                connected.store(false, Ordering::Release);
                if had_connection {
                    output.publish_signal_state(false);
                    loss_reported = true;
                }
                display.send(MiniEvent::Connection {
                    connected: false,
                    streaming: false,
                    device_name: Some(device_name),
                    battery_percent,
                    model_code: Some("H10".into()),
                    message: "Disconnected".into(),
                });
                break;
            }
        }
        send_counts_if_due(&display, &output, &counts, &mut last_event);
    }
    if had_connection && !loss_reported {
        output_rx.borrow().publish_signal_state(false);
        display.send(MiniEvent::Connection {
            connected: false,
            streaming: false,
            device_name: None,
            battery_percent: None,
            model_code: Some("H10".into()),
            message: "Signal lost; reconnecting".into(),
        });
    }
    had_connection
}

#[allow(clippy::too_many_arguments)]
async fn run_vernier_session(
    mut input_events: tokio::sync::mpsc::Receiver<GdxInputEvent>,
    output_rx: tokio::sync::watch::Receiver<MiniOutputHandle>,
    display: Arc<DisplayEndpoint>,
    preferences: Arc<PreferencesStore>,
    device_id: String,
    configuration: Arc<tokio::sync::Mutex<()>>,
    vernier_schema: Arc<tokio::sync::Mutex<Option<VernierStreamSchema>>>,
    restoring: bool,
    connected: &AtomicBool,
) -> bool {
    let mut breathing = VernierBreathingProcessor::default();
    let mut schema: Option<VernierStreamSchema> = None;
    let mut counts = MiniCounters::default();
    let mut last_event = Instant::now();
    let mut had_connection = false;
    let mut loss_reported = false;
    while let Some(event) = input_events.recv().await {
        let output = output_rx.borrow().clone();
        match event {
            GdxInputEvent::Status { phase, message } => display.send(MiniEvent::Status {
                phase: phase.into(),
                message,
            }),
            GdxInputEvent::Connected {
                device_name,
                model_code,
                sample_period_us,
                battery_percent,
                sensors,
                ..
            } => {
                had_connection = true;
                connected.store(true, Ordering::Release);
                let _configuration = configuration.lock().await;
                let active_output = output_rx.borrow().clone();
                match active_output
                    .configure_vernier_streams(&model_code, sample_period_us, &sensors)
                    .await
                {
                    Ok(configured) => {
                        *vernier_schema.lock().await = Some(configured.clone());
                        schema = Some(configured);
                        breathing = VernierBreathingProcessor::default();
                        if restoring {
                            active_output.publish_signal_state(true);
                        }
                    }
                    Err(message) => display.send(MiniEvent::Error {
                        code: "OUTPUT_CONFIGURE_FAILED".into(),
                        message,
                    }),
                }
                let _ = preferences.save_last_device(SavedMiniDevice {
                    id: device_id.clone(),
                    name: device_name.clone(),
                });
                display.send(MiniEvent::Connection {
                    connected: true,
                    streaming: true,
                    device_name: Some(device_name),
                    battery_percent: Some(battery_percent),
                    model_code: Some(model_code),
                    message: "Publishing Vernier raw channels and breathing to LSL".into(),
                });
            }
            GdxInputEvent::Samples {
                encoding,
                sensors,
                host_receive_timestamp_ns,
                sample_period_us,
                sequence,
                dropped_before,
                device_drop_reports_before,
                decode_latency_ns,
            } => {
                output.publish_vernier_raw(VernierRawBatch {
                    host_receive_timestamp_ns,
                    sample_period_us,
                    sequence,
                    dropped_before,
                    device_drop_reports_before,
                    decode_latency_ns,
                    encoding,
                    sensors: &sensors,
                });
                counts.vernier_rows = counts
                    .vernier_rows
                    .saturating_add(sample_row_count(&sensors));
                counts.dropped_batches = counts.dropped_batches.saturating_add(dropped_before);
                let force_sensor = schema
                    .as_ref()
                    .and_then(VernierStreamSchema::force_sensor_number)
                    .unwrap_or(1);
                if let Some(force) = sensors
                    .iter()
                    .find(|samples| samples.sensor_number == force_sensor)
                {
                    let force_values = force
                        .values
                        .iter()
                        .map(|value| *value as f32)
                        .collect::<Vec<_>>();
                    output_warning(
                        &display,
                        output.publish_force(
                            host_receive_timestamp_ns,
                            &force_values,
                            sample_period_us,
                        ),
                    );
                    let waveform = breathing.push(&force.values, sample_period_us);
                    output_warning(
                        &display,
                        output.publish_vernier_breathing(
                            host_receive_timestamp_ns,
                            &waveform,
                            sample_period_us,
                        ),
                    );
                    counts.metric_samples = counts
                        .metric_samples
                        .saturating_add(u64::try_from(waveform.len()).unwrap_or(u64::MAX));
                }
            }
            GdxInputEvent::StreamHealth {
                dropped_batches,
                queue_high_water,
                ..
            } => {
                counts.dropped_batches = dropped_batches;
                counts.queue_high_water = queue_high_water;
            }
            GdxInputEvent::Error(message) => display.send(MiniEvent::Error {
                code: "SENSOR_DATA_WARNING".into(),
                message,
            }),
            GdxInputEvent::Disconnected { device_name } => {
                connected.store(false, Ordering::Release);
                if had_connection {
                    output.publish_signal_state(false);
                    loss_reported = true;
                }
                display.send(MiniEvent::Connection {
                    connected: false,
                    streaming: false,
                    device_name: Some(device_name),
                    battery_percent: None,
                    model_code: None,
                    message: "Disconnected".into(),
                });
                break;
            }
        }
        send_counts_if_due(&display, &output, &counts, &mut last_event);
    }
    if had_connection && !loss_reported {
        output_rx.borrow().publish_signal_state(false);
        display.send(MiniEvent::Connection {
            connected: false,
            streaming: false,
            device_name: None,
            battery_percent: None,
            model_code: None,
            message: "Signal lost; reconnecting".into(),
        });
    }
    had_connection
}

#[derive(Default)]
struct MiniCounters {
    ecg_samples: u64,
    acc_samples: u64,
    heart_rate_packets: u64,
    metric_samples: u64,
    vernier_rows: u64,
    dropped_batches: u64,
    queue_high_water: usize,
}

fn publish_metric_samples(
    display: &DisplayEndpoint,
    output: &MiniOutputHandle,
    sensor_timestamp_ns: u64,
    values: &[MetricSample],
    counts: &mut MiniCounters,
) {
    if values.is_empty() {
        return;
    }
    let values = values
        .iter()
        .map(|sample| MetricValue {
            id: sample.id,
            value: sample.value,
        })
        .collect::<Vec<_>>();
    output_warning(
        display,
        output.publish_polar_metrics_at(sensor_timestamp_ns, &values),
    );
    counts.metric_samples = counts
        .metric_samples
        .saturating_add(u64::try_from(values.len()).unwrap_or(u64::MAX));
}

fn send_counts_if_due(
    display: &DisplayEndpoint,
    output: &MiniOutputHandle,
    counts: &MiniCounters,
    last_event: &mut Instant,
) {
    if last_event.elapsed() < EVENT_INTERVAL {
        return;
    }
    *last_event = Instant::now();
    display.send(MiniEvent::Samples {
        ecg_samples: counts.ecg_samples,
        acc_samples: counts.acc_samples,
        heart_rate_packets: counts.heart_rate_packets,
        metric_samples: counts.metric_samples,
        vernier_rows: counts.vernier_rows,
        dropped_batches: counts.dropped_batches,
        queue_high_water: counts.queue_high_water,
        lsl: output.health(),
    });
}

fn output_warning(display: &DisplayEndpoint, message: Option<String>) {
    if let Some(message) = message {
        display.send(MiniEvent::Error {
            code: "OUTPUT_WARNING".into(),
            message,
        });
    }
}

fn sample_row_count(sensors: &[SensorSamples]) -> u64 {
    sensors
        .iter()
        .map(|sensor| sensor.values.len())
        .max()
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(0)
}

fn polar_metric_options() -> Vec<MiniMetricOption> {
    METRIC_CATALOG
        .iter()
        .copied()
        .filter(|metric| metric.id != VERNIER_FORCE_OUTPUT)
        .filter(|metric| {
            POLAR_DIRECT_OUTPUTS.contains(&metric.id)
                || metric_selection_tier(metric.id) == MetricSelectionTier::Release
                || POLAR_MINI_ONLY_IDS.contains(&metric.id)
                || matches!(metric.category, "Breathing" | "Breathing dynamics")
        })
        .map(MiniMetricOption::from_definition)
        .collect()
}

fn normalize_polar_outputs(input: Option<Vec<String>>) -> Vec<String> {
    let mut outputs = POLAR_DIRECT_OUTPUTS
        .iter()
        .map(|id| (*id).to_string())
        .collect::<Vec<_>>();
    for id in input.unwrap_or_default() {
        if outputs.iter().any(|known| known == &id) || id == VERNIER_FORCE_OUTPUT {
            continue;
        }
        let Some(metric) = MetricDefinition::for_id(&id) else {
            continue;
        };
        if metric_selection_tier(metric.id) == MetricSelectionTier::Release
            || POLAR_MINI_ONLY_IDS.contains(&metric.id)
            || matches!(metric.category, "Breathing" | "Breathing dynamics")
        {
            outputs.push(id);
        }
    }
    if RELEASE_POLAR_RESPIRATION_IDS
        .iter()
        .any(|id| outputs.iter().any(|selected| selected == id))
    {
        for id in RELEASE_POLAR_RESPIRATION_IDS {
            if !outputs.iter().any(|selected| selected == id) {
                outputs.push((*id).into());
            }
        }
    }
    outputs
}

fn default_vernier_outputs() -> Vec<String> {
    VERNIER_OUTPUT_IDS.iter().map(|id| (*id).into()).collect()
}

fn validate_vernier_outputs(input: Vec<String>) -> Result<Vec<String>, String> {
    if input.is_empty() || input.len() > VERNIER_OUTPUT_IDS.len()
        || input.iter().any(|id| !VERNIER_OUTPUT_IDS.contains(&id.as_str()))
    {
        return Err("Select at least one known Vernier LSL output.".into());
    }
    let mut unique = std::collections::HashSet::new();
    if input.iter().any(|id| !unique.insert(id)) {
        return Err("Vernier LSL outputs cannot be duplicated.".into());
    }
    Ok(VERNIER_OUTPUT_IDS.iter().filter(|id| input.iter().any(|selected| selected == *id)).map(|id| (*id).into()).collect())
}

fn valid_saved_device(device: &SavedMiniDevice) -> bool {
    !device.id.trim().is_empty()
        && !device.name.trim().is_empty()
        && !device.id.contains('\0')
        && !device.name.contains('\0')
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

    #[test]
    fn recent_devices_survive_restart_and_keep_last_device_first() {
        let path = std::env::temp_dir().join(format!("vernier-mini-recent-{}.json", process::id()));
        let _ = fs::remove_file(&path);
        let store = PreferencesStore::load(path.clone(), MiniAppKind::Vernier);
        for id in ["belt-a", "belt-b", "belt-a"] {
            store
                .save_last_device(SavedMiniDevice {
                    id: id.into(),
                    name: format!("GDX-RB {id}"),
                })
                .unwrap();
        }
        let restored = PreferencesStore::load(path.clone(), MiniAppKind::Vernier).snapshot();
        assert_eq!(restored.last_device.as_ref().unwrap().id, "belt-a");
        assert_eq!(restored.recent_devices.len(), 2);
        assert_eq!(restored.recent_devices[0].id, "belt-a");
        assert_eq!(restored.recent_devices[1].id, "belt-b");
        assert!(
            PreferencesStore::mock_from(&path, MiniAppKind::Vernier)
                .snapshot()
                .recent_devices
                .is_empty()
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn acc_breathing_outputs_are_mini_selectable_and_survive_preferences() {
        let options = polar_metric_options();
        let ids = METRIC_CATALOG
            .iter()
            .filter(|metric| matches!(metric.category, "Breathing" | "Breathing dynamics"))
            .map(|metric| metric.id)
            .collect::<Vec<_>>();
        for id in &ids {
            let option = options.iter().find(|option| option.id == *id).unwrap();
            assert!(!option.direct);
        }
        let outputs = normalize_polar_outputs(Some(ids.iter().map(|id| (*id).into()).collect()));
        for id in ids {
            assert!(outputs.iter().any(|selected| selected == id), "{id}");
        }
        assert_eq!(normalize_polar_outputs(Some(outputs.clone())), outputs);
    }

    #[cfg(all(target_os = "windows", feature = "liblsl-backend"))]
    #[tokio::test]
    async fn selecting_another_device_replaces_a_pending_recovery_target() {
        let bundled_lsl = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/polar-stream-mini/resources/lsl.dll");
        if !bundled_lsl.is_file() {
            return;
        }
        let scratch = std::env::temp_dir().join(format!("mini-device-switch-{}", process::id()));
        let state = MiniAppState::new(
            MiniAppKind::Polar,
            Some(bundled_lsl),
            scratch.join("preferences.json"),
            scratch.join("recordings"),
            true,
        );
        let snapshot = state.preferences.snapshot();
        let first = connect_with_snapshot(&state, "device-a".into(), snapshot.clone(), false)
            .await
            .unwrap();
        assert_eq!(first.device_id, "device-a");
        assert!(!first.connected);
        let second = connect_with_snapshot(&state, "device-b".into(), snapshot, true)
            .await
            .unwrap();
        assert_eq!(second.device_id, "device-b");
        disconnect_active(&state).await.unwrap();
    }

    #[cfg(all(target_os = "windows", feature = "liblsl-backend"))]
    #[tokio::test]
    async fn sensor_loss_keeps_each_mini_outlet_open_in_both_modes() {
        let bundled_lsl = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/polar-stream-mini/resources/lsl.dll");
        if !bundled_lsl.is_file() {
            return;
        }
        for kind in [MiniAppKind::Polar, MiniAppKind::Vernier] {
            for mode in [
                MiniOutputMode::SeparateStreams,
                MiniOutputMode::SingleStream,
            ] {
                let scratch = std::env::temp_dir()
                    .join(format!("mini-gap-test-{}-{kind:?}-{mode:?}", process::id()));
                let state = MiniAppState::new(
                    kind,
                    Some(bundled_lsl.clone()),
                    scratch.join("preferences.json"),
                    scratch.join("recordings"),
                    true,
                );
                let mut snapshot = state.preferences.snapshot();
                snapshot.output_mode = mode;
                snapshot.stream_name = format!("GapTest-{}-{kind:?}-{mode:?}", process::id());
                let output = build_output(kind, &snapshot, Some(bundled_lsl.clone()), scratch)
                    .await
                    .unwrap();
                let (_output_tx, output_rx) = tokio::sync::watch::channel(output.clone());
                let connected = AtomicBool::new(false);
                match kind {
                    MiniAppKind::Polar => {
                        let (tx, rx) = tokio::sync::mpsc::channel(4);
                        tx.send(PolarInputEvent::Connected {
                            device_name: "Test H10".into(),
                            battery_percent: None,
                        })
                        .await
                        .unwrap();
                        tx.send(PolarInputEvent::Disconnected {
                            device_name: "Test H10".into(),
                            battery_percent: None,
                        })
                        .await
                        .unwrap();
                        drop(tx);
                        let (_settings_tx, settings_rx) = tokio::sync::watch::channel(
                            PolarMiniSettings::from_outputs(&snapshot.polar_outputs),
                        );
                        assert!(
                            run_polar_session(
                                rx,
                                output_rx,
                                settings_rx,
                                state.display.clone(),
                                state.preferences.clone(),
                                "test-h10".into(),
                                state.configuration.clone(),
                                false,
                                &connected,
                            )
                            .await
                        );
                    }
                    MiniAppKind::Vernier => {
                        let (tx, rx) = tokio::sync::mpsc::channel(4);
                        tx.send(GdxInputEvent::Connected {
                            device_name: "Test GDX-RB".into(),
                            model_code: "GDX-RB".into(),
                            sensor_number: 1,
                            sensor_name: "Force".into(),
                            sensor_unit: "N".into(),
                            sample_period_us: VERNIER_PERIOD_US,
                            main_firmware_version: "test".into(),
                            battery_percent: 100,
                            sensors: mock_vernier_sensors(),
                        })
                        .await
                        .unwrap();
                        tx.send(GdxInputEvent::Disconnected {
                            device_name: "Test GDX-RB".into(),
                        })
                        .await
                        .unwrap();
                        drop(tx);
                        assert!(
                            run_vernier_session(
                                rx,
                                output_rx,
                                state.display.clone(),
                                state.preferences.clone(),
                                "test-gdx".into(),
                                state.configuration.clone(),
                                state.vernier_schema.clone(),
                                false,
                                &connected,
                            )
                            .await
                        );
                    }
                }
                assert!(!connected.load(Ordering::Acquire));
                assert!(
                    output.health().starts_with("Publishing "),
                    "{}",
                    output.health()
                );
            }
        }
    }

    #[cfg(all(target_os = "windows", feature = "liblsl-backend"))]
    async fn mock_samples(state: &MiniAppState) -> u64 {
        tokio::time::sleep(Duration::from_millis(320)).await;
        let display = state.display.state.lock().unwrap();
        match display.samples_snapshot.as_ref().unwrap() {
            MiniEvent::Samples {
                ecg_samples,
                vernier_rows,
                ..
            } => ecg_samples + vernier_rows,
            _ => panic!("expected a sample event"),
        }
    }

    #[cfg(all(target_os = "windows", feature = "liblsl-backend"))]
    #[tokio::test]
    async fn live_mode_changes_keep_both_mini_mock_sources_publishing() {
        let bundled_lsl = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/polar-stream-mini/resources/lsl.dll");
        if !bundled_lsl.is_file() {
            eprintln!("skipping bundled LSL test because the local DLL is not staged");
            return;
        }
        for kind in [MiniAppKind::Polar, MiniAppKind::Vernier] {
            let scratch = std::env::temp_dir().join(format!(
                "stream-mini-live-mode-test-{}-{kind:?}",
                process::id()
            ));
            let state = MiniAppState::new(
                kind,
                Some(bundled_lsl.clone()),
                scratch.join("preferences.json"),
                scratch.join("recordings"),
                true,
            );
            let mut initial = state.preferences.snapshot();
            if kind == MiniAppKind::Polar {
                initial
                    .polar_outputs
                    .extend(POLAR_MINI_ONLY_IDS.iter().map(|id| (*id).into()));
            }
            let session = start_mock_with_snapshot(&state, initial.clone())
                .await
                .unwrap();
            assert!(session.lsl.starts_with("Publishing "), "{}", session.lsl);
            assert_ne!(session.lsl, "Publishing 1 stream(s)");
            if kind == MiniAppKind::Polar {
                assert_eq!(
                    session.lsl,
                    format!("Publishing {} stream(s)", initial.polar_outputs.len() + 1)
                );
            }
            let first_samples = mock_samples(&state).await;
            assert!(first_samples > 0);

            let input = |mode, stream_name: String| MiniPreferencesInput {
                stream_name,
                output_mode: mode,
                auto_connect: false,
                polar_outputs: initial.polar_outputs.clone(),
                vernier_outputs: Some(initial.vernier_outputs.clone()),
            };
            let single = save_preferences_inner(
                &state,
                input(MiniOutputMode::SingleStream, initial.stream_name.clone()),
            )
            .await
            .unwrap();
            assert!(single.applied && !single.reconnect_required);
            let session = active_snapshot(&state).await.unwrap();
            assert_eq!(session.output_mode, MiniOutputMode::SingleStream);
            assert_eq!(session.lsl, "Publishing 1 stream(s)");
            let single_samples = mock_samples(&state).await;
            assert!(single_samples > first_samples);

            let separate = save_preferences_inner(
                &state,
                input(
                    MiniOutputMode::SeparateStreams,
                    format!("{}-Renamed", initial.stream_name),
                ),
            )
            .await
            .unwrap();
            assert!(separate.applied && !separate.reconnect_required);
            let session = active_snapshot(&state).await.unwrap();
            assert_eq!(session.output_mode, MiniOutputMode::SeparateStreams);
            assert!(session.lsl.starts_with("Publishing "));
            assert_ne!(session.lsl, "Publishing 1 stream(s)");
            assert!(mock_samples(&state).await > single_samples);
            if kind == MiniAppKind::Vernier {
                let force_only = MiniPreferencesInput {
                    vernier_outputs: Some(vec!["rawForce".into()]),
                    ..input(MiniOutputMode::SeparateStreams, initial.stream_name.clone())
                };
                let result = save_preferences_inner(&state, force_only).await.unwrap();
                assert!(result.applied && !result.reconnect_required);
                let active = active_snapshot(&state).await.unwrap();
                assert!(active.connected);
                assert_eq!(active.lsl, "Publishing 1 stream(s)");
                assert!(mock_samples(&state).await > single_samples);
                let marker_only = MiniPreferencesInput {
                    vernier_outputs: Some(vec!["signalStatus".into()]),
                    ..input(MiniOutputMode::SeparateStreams, initial.stream_name.clone())
                };
                let result = save_preferences_inner(&state, marker_only).await.unwrap();
                assert!(result.applied && !result.reconnect_required);
                assert_eq!(active_snapshot(&state).await.unwrap().lsl, "Publishing 1 stream(s)");
            }
            let invalid = input(MiniOutputMode::SingleStream, String::new());
            assert!(save_preferences_inner(&state, invalid).await.is_err());
            assert_eq!(
                active_snapshot(&state).await.unwrap().output_mode,
                MiniOutputMode::SeparateStreams
            );
            disconnect_active(&state).await.unwrap();
        }
    }

    #[test]
    fn vernier_output_selection_requires_distinct_known_ids() {
        assert_eq!(validate_vernier_outputs(vec!["rawForce".into()]).unwrap(), vec!["rawForce"]);
        assert!(validate_vernier_outputs(Vec::new()).is_err());
        assert!(validate_vernier_outputs(vec!["rawForce".into(), "rawForce".into()]).is_err());
        assert!(validate_vernier_outputs(vec!["rawAcceleration".into()]).is_err());
    }

    #[test]
    fn mock_vernier_contract_uses_the_fixed_fast_force_channel() {
        let sensors = mock_vernier_sensors();
        assert_eq!(sensors.len(), 1);
        assert_eq!(sensors[0].number, 1);
        assert!(sensors[0].is_respiration_force());
        assert_eq!(sensors[0].minimum_period_us, VERNIER_PERIOD_US);
        assert_eq!(sensors[0].typical_period_us, VERNIER_PERIOD_US);
    }

    #[test]
    fn mock_waveforms_are_bounded_and_non_constant() {
        let bundled_lsl = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/polar-stream-mini/resources/lsl.dll");
        let ecg = load_mock_ecg(Some(&bundled_lsl)).unwrap();
        let acc = (0..200).map(mock_acc_sample).collect::<Vec<_>>();
        let force = (0..20).map(mock_force_sample).collect::<Vec<_>>();

        assert_eq!(ecg.len(), MOCK_ECG_SAMPLE_COUNT);
        assert!(ecg.iter().all(|value| (-500..=500).contains(value)));
        assert!(load_mock_ecg(None).is_err());
        assert!(
            acc.iter()
                .all(|sample| sample.z_mg > 900 && sample.z_mg < 1_100)
        );
        assert!(force.iter().all(|value| (9.0..=15.0).contains(value)));
        assert!(ecg.windows(2).any(|pair| pair[0] != pair[1]));
        assert!(force.windows(2).any(|pair| pair[0] != pair[1]));
    }
}
