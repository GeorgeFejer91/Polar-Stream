//! Bounded, per-device Bluetooth sessions for Vernier Go Direct sensors.
//!
//! Every admitted device owns its peripheral, notification stream, decoder,
//! queue, sequence, and cancellation path. The pool lock is lifecycle-only and
//! is never taken by the notification hot path.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use btleplug::{
    api::{
        Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter,
        WriteType,
    },
    platform::{Manager, Peripheral},
};
use futures_util::{Stream, StreamExt};
use serde::Serialize;
use tokio::sync::{Mutex, mpsc, watch};
use vernier_gdx_core::{
    COMMAND_CHARACTERISTIC, Command, CommandCounter, DeviceModel, Frame, FrameAccumulator,
    GET_AVAILABLE_SENSOR_MASK, Measurement, RESPONSE_CHARACTERISTIC, SensorInfo,
    available_sensor_numbers, classify_device_model, decode_device_info_response, decode_frame,
    decode_sensor_info_response, decode_sensor_mask_response, decode_status_response,
    select_respiration_force_sensor,
};

const BLE_TIMEOUT: Duration = Duration::from_secs(6);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(8);
const SCAN_DURATION: Duration = Duration::from_secs(6);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(3);
const PERIPHERAL_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const EVENT_CAPACITY: usize = 256;
const ATT_PAYLOAD_BYTES: usize = 20;
pub const MAX_GDX_SESSIONS: usize = 8;
pub const DEFAULT_PERIOD_US: u32 = 100_000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    pub id: String,
    pub name: String,
    pub rssi: Option<i16>,
    pub model_code: &'static str,
    pub model_name: &'static str,
    pub respiration_belt_candidate: bool,
    pub adapter_info: String,
}

#[derive(Clone, Copy, Debug)]
pub struct SessionConfig {
    pub period_us: u32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            period_us: DEFAULT_PERIOD_US,
        }
    }
}

#[derive(Clone, Debug)]
pub enum InputEvent {
    Status {
        phase: &'static str,
        message: String,
    },
    Connected {
        device_name: String,
        model_code: String,
        sensor_number: u8,
        sensor_name: String,
        sensor_unit: String,
        sample_period_us: u32,
        main_firmware_version: String,
        battery_percent: u8,
    },
    Samples {
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
    },
}

struct ActiveSession {
    device_id: String,
    cancel: watch::Sender<bool>,
    finished: watch::Receiver<bool>,
}

pub struct InputSessionPool {
    devices: Mutex<HashMap<String, Peripheral>>,
    sessions: Mutex<HashMap<String, ActiveSession>>,
    operation_gate: Mutex<()>,
    max_sessions: usize,
}

impl Default for InputSessionPool {
    fn default() -> Self {
        Self::new(MAX_GDX_SESSIONS)
    }
}

impl InputSessionPool {
    pub fn new(max_sessions: usize) -> Self {
        Self {
            devices: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            operation_gate: Mutex::new(()),
            max_sessions: max_sessions.clamp(1, MAX_GDX_SESSIONS),
        }
    }

    pub async fn scan(&self) -> Result<Vec<DeviceSummary>, String> {
        let _operation = self.operation_gate.lock().await;
        self.prune_finished_sessions().await;
        if !self.sessions.lock().await.is_empty() {
            return Err("Disconnect all Go Direct sessions before scanning again.".into());
        }
        let manager = timed("Bluetooth initialization", Manager::new()).await?;
        let adapters = timed("Bluetooth adapter enumeration", manager.adapters()).await?;
        let adapter = adapters
            .into_iter()
            .next()
            .ok_or_else(|| "No Bluetooth Low Energy adapter was found.".to_string())?;
        let adapter_info = adapter
            .adapter_info()
            .await
            .unwrap_or_else(|_| "unavailable".to_string());
        timed(
            "Bluetooth scan startup",
            adapter.start_scan(ScanFilter::default()),
        )
        .await?;
        tokio::time::sleep(SCAN_DURATION).await;
        let peripherals = timed("Bluetooth scan result enumeration", adapter.peripherals()).await;
        let stop = timed("Bluetooth scan cleanup", adapter.stop_scan()).await;
        let peripherals = peripherals?;
        stop?;

        let mut found = Vec::new();
        let mut devices = HashMap::new();
        for peripheral in peripherals {
            let Ok(Some(properties)) =
                timed("Bluetooth property read", peripheral.properties()).await
            else {
                continue;
            };
            let Some(name) = properties.local_name.filter(|name| {
                name.to_ascii_uppercase().starts_with("GDX")
                    || name.to_ascii_lowercase().contains("go direct")
            }) else {
                continue;
            };
            let id = peripheral.id().to_string();
            let model = classify_device_model(&name, "", "");
            found.push(DeviceSummary {
                id: id.clone(),
                name,
                rssi: properties.rssi,
                model_code: model.code(),
                model_name: model.label(),
                respiration_belt_candidate: model == DeviceModel::RespirationBelt,
                adapter_info: adapter_info.clone(),
            });
            devices.insert(id, peripheral);
        }
        found.sort_by(|left, right| {
            right
                .rssi
                .cmp(&left.rssi)
                .then_with(|| left.name.cmp(&right.name))
        });
        *self.devices.lock().await = devices;
        Ok(found)
    }

    pub async fn connect(
        &self,
        slot: &str,
        device_id: &str,
        config: SessionConfig,
    ) -> Result<mpsc::Receiver<InputEvent>, String> {
        validate_slot(slot)?;
        if !(1_000..=60_000_000).contains(&config.period_us) {
            return Err("Go Direct period must be between 1 ms and 60 s.".into());
        }
        let _operation = self.operation_gate.lock().await;
        self.prune_finished_sessions().await;
        let peripheral = self
            .devices
            .lock()
            .await
            .get(device_id)
            .cloned()
            .ok_or_else(|| "That Go Direct sensor is not in the latest scan.".to_string())?;
        {
            let sessions = self.sessions.lock().await;
            if sessions.len() >= self.max_sessions {
                return Err(format!(
                    "The bounded Go Direct pool already owns {} sessions.",
                    self.max_sessions
                ));
            }
            if sessions.contains_key(slot) {
                return Err("That input slot is already active.".into());
            }
            if sessions
                .values()
                .any(|session| session.device_id == device_id)
            {
                return Err("That Go Direct sensor is already active.".into());
            }
        }

        let name = timed("Bluetooth property read", peripheral.properties())
            .await
            .ok()
            .flatten()
            .and_then(|properties| properties.local_name)
            .unwrap_or_else(|| "Vernier Go Direct".into());
        let (events_tx, events_rx) = mpsc::channel(EVENT_CAPACITY);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (finished_tx, finished_rx) = watch::channel(false);
        self.sessions.lock().await.insert(
            slot.to_string(),
            ActiveSession {
                device_id: device_id.to_string(),
                cancel: cancel_tx,
                finished: finished_rx,
            },
        );
        tokio::spawn(run_session(
            peripheral,
            name,
            config,
            events_tx,
            cancel_rx,
            finished_tx,
        ));
        Ok(events_rx)
    }

    pub async fn disconnect(&self, slot: &str) -> Result<(), String> {
        validate_slot(slot)?;
        let _operation = self.operation_gate.lock().await;
        let Some(mut session) = self.sessions.lock().await.remove(slot) else {
            return Ok(());
        };
        let _ = session.cancel.send(true);
        tokio::time::timeout(CLEANUP_TIMEOUT, session.finished.wait_for(|done| *done))
            .await
            .map_err(|_| "Go Direct session cleanup timed out.".to_string())?
            .map_err(|_| {
                "Go Direct session owner ended before cleanup confirmation.".to_string()
            })?;
        Ok(())
    }

    pub async fn disconnect_all(&self) -> Result<(), String> {
        let slots = self
            .sessions
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut first_error = None;
        for slot in slots {
            if let Err(error) = self.disconnect(&slot).await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn prune_finished_sessions(&self) {
        self.sessions
            .lock()
            .await
            .retain(|_, session| !*session.finished.borrow());
    }
}

async fn run_session(
    peripheral: Peripheral,
    device_name: String,
    config: SessionConfig,
    events: mpsc::Sender<InputEvent>,
    mut cancel: watch::Receiver<bool>,
    finished: watch::Sender<bool>,
) {
    let mut cancellation = cancel.clone();
    let result = tokio::select! {
        result = run_connected_session(&peripheral, &device_name, config, &events, &mut cancel) => result,
        changed = cancellation.changed() => {
            let _ = changed;
            Ok(())
        }
    };
    if let Err(message) = result {
        let _ = events.try_send(InputEvent::Error(message));
    }
    let _ = tokio::time::timeout(PERIPHERAL_DISCONNECT_TIMEOUT, peripheral.disconnect()).await;
    let _ = events.try_send(InputEvent::Disconnected {
        device_name: device_name.clone(),
    });
    finished.send_replace(true);
}

async fn run_connected_session(
    peripheral: &Peripheral,
    device_name: &str,
    config: SessionConfig,
    events: &mpsc::Sender<InputEvent>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    control(events, "connecting", format!("Opening {device_name}")).await?;
    timed("Go Direct connection", peripheral.connect()).await?;
    timed(
        "Go Direct service discovery",
        peripheral.discover_services(),
    )
    .await?;
    let command = characteristic(peripheral, COMMAND_CHARACTERISTIC)
        .ok_or_else(|| "Go Direct command characteristic was not found.".to_string())?;
    let response = characteristic(peripheral, RESPONSE_CHARACTERISTIC)
        .ok_or_else(|| "Go Direct response characteristic was not found.".to_string())?;
    timed("Go Direct subscription", peripheral.subscribe(&response)).await?;
    let mut notifications =
        timed("Go Direct notification stream", peripheral.notifications()).await?;
    let mut accumulator = FrameAccumulator::default();
    let mut counter = CommandCounter::default();

    control(
        events,
        "initializing",
        "Negotiating Go Direct protocol".into(),
    )
    .await?;
    let initialize = Command::initialize(&mut counter);
    write_command(peripheral, &command, &initialize).await?;
    wait_for_response(&mut notifications, &mut accumulator, &initialize).await?;
    let status_command = Command::get_status(&mut counter);
    write_command(peripheral, &command, &status_command).await?;
    let status_response =
        wait_for_response(&mut notifications, &mut accumulator, &status_command).await?;
    let status = decode_status_response(&status_response).map_err(|error| error.to_string())?;
    let device_info_command = Command::get_device_info(&mut counter);
    write_command(peripheral, &command, &device_info_command).await?;
    let device_info_response =
        wait_for_response(&mut notifications, &mut accumulator, &device_info_command).await?;
    let device = decode_device_info_response(&device_info_response, device_name)
        .map_err(|error| error.to_string())?;

    let available = Command::get_available_sensor_mask(&mut counter);
    write_command(peripheral, &command, &available).await?;
    let available_response =
        wait_for_response(&mut notifications, &mut accumulator, &available).await?;
    let available_mask =
        decode_sensor_mask_response(&available_response, GET_AVAILABLE_SENSOR_MASK)
            .map_err(|error| error.to_string())?;
    let mut sensors = Vec::with_capacity(available_mask.count_ones() as usize);
    for sensor_number in available_sensor_numbers(available_mask) {
        let sensor_info = Command::get_sensor_info(&mut counter, sensor_number);
        write_command(peripheral, &command, &sensor_info).await?;
        let sensor_response =
            wait_for_response(&mut notifications, &mut accumulator, &sensor_info).await?;
        match decode_sensor_info_response(&sensor_response) {
            Ok(sensor) => sensors.push(sensor),
            Err(error) => {
                let _ = events.try_send(InputEvent::Error(format!(
                    "Go Direct channel {sensor_number} metadata was ignored: {error}"
                )));
            }
        }
    }
    let selected_sensor = select_respiration_force_sensor(&device, &sensors)
        .map_err(|error| format!("{}: {error}", recognized_device_message(&device)))?;
    let effective_period_us = respiration_period(&selected_sensor, config.period_us)?;
    control(
        events,
        "identified",
        format!(
            "Identified {} · channel {} {} ({}) · {:.1} Hz",
            device.model.label(),
            selected_sensor.number,
            selected_sensor.description,
            selected_sensor.unit,
            1_000_000.0 / f64::from(effective_period_us)
        ),
    )
    .await?;
    let set_period = Command::set_measurement_period(&mut counter, effective_period_us)
        .map_err(|error| error.to_string())?;
    write_command(peripheral, &command, &set_period).await?;
    wait_for_response(&mut notifications, &mut accumulator, &set_period).await?;
    let start = Command::start_measurements(&mut counter, 1_u32 << selected_sensor.number);
    write_command(peripheral, &command, &start).await?;
    wait_for_response(&mut notifications, &mut accumulator, &start).await?;

    let origin = Instant::now();
    let mut sequence = 0_u64;
    let mut notifications_seen = 0_u64;
    let mut samples_seen = 0_u64;
    let mut malformed_frames = 0_u64;
    let mut dropped_batches = 0_u64;
    let mut pending_dropped = 0_u64;
    let mut max_decode_latency_ns = 0_u64;
    let mut queue_high_water = 0_usize;
    let mut decode_latencies = Vec::with_capacity(512);
    let mut connected = false;
    let mut next_health = Instant::now() + Duration::from_secs(5);

    loop {
        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    break;
                }
            }
            notification = notifications.next() => {
                let Some(notification) = notification else {
                    return Err("Go Direct notification stream ended unexpectedly.".into());
                };
                if notification.uuid != RESPONSE_CHARACTERISTIC {
                    continue;
                }
                notifications_seen += 1;
                let received_at = Instant::now();
                let host_receive_timestamp_ns = received_at.duration_since(origin).as_nanos() as u64;
                let frames = match accumulator.push(&notification.value) {
                    Ok(frames) => frames,
                    Err(error) => {
                        malformed_frames += 1;
                        let _ = events.try_send(InputEvent::Error(error.to_string()));
                        continue;
                    }
                };
                for frame in frames {
                    let decoded = match decode_frame(&frame) {
                        Ok(decoded) => decoded,
                        Err(error) => {
                            malformed_frames += 1;
                            let _ = events.try_send(InputEvent::Error(error.to_string()));
                            continue;
                        }
                    };
                    let Frame::Measurement(Measurement::Samples { sensors, .. }) = decoded else {
                        continue;
                    };
                    for sensor_samples in sensors {
                        if sensor_samples.sensor_number != selected_sensor.number || sensor_samples.values.is_empty() {
                            continue;
                        }
                        if !connected {
                            control(events, "streaming", "First validated Go Direct sample received".into()).await?;
                            control_connected(events, &device, &status, &selected_sensor, effective_period_us).await?;
                            connected = true;
                        }
                        let decode_latency_ns = received_at.elapsed().as_nanos() as u64;
                        max_decode_latency_ns = max_decode_latency_ns.max(decode_latency_ns);
                        if decode_latencies.len() < 512 {
                            decode_latencies.push(decode_latency_ns);
                        }
                        samples_seen += sensor_samples.values.len() as u64;
                        let count = sensor_samples.values.len() as u64;
                        let event = InputEvent::Samples {
                            sensor_number: sensor_samples.sensor_number,
                            host_receive_timestamp_ns,
                            sample_period_us: effective_period_us,
                            sequence,
                            values: sensor_samples.values,
                            dropped_before: pending_dropped,
                            decode_latency_ns,
                        };
                        match events.try_send(event) {
                            Ok(()) => pending_dropped = 0,
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                dropped_batches += 1;
                                pending_dropped += count;
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => return Ok(()),
                        }
                        queue_high_water = queue_high_water.max(EVENT_CAPACITY - events.capacity());
                        sequence = sequence.saturating_add(count);
                    }
                }
                if Instant::now() >= next_health {
                    decode_latencies.sort_unstable();
                    let _ = events.try_send(InputEvent::StreamHealth {
                        notifications: notifications_seen,
                        samples: samples_seen,
                        malformed_frames,
                        dropped_batches,
                        queue_high_water,
                        decode_latency_p50_ns: percentile(&decode_latencies, 50),
                        decode_latency_p95_ns: percentile(&decode_latencies, 95),
                        decode_latency_p99_ns: percentile(&decode_latencies, 99),
                        max_decode_latency_ns,
                    });
                    decode_latencies.clear();
                    next_health = Instant::now() + Duration::from_secs(5);
                }
            }
        }
    }

    let stop = Command::stop_measurements(&mut counter);
    let _ = write_command(peripheral, &command, &stop).await;
    let disconnect = Command::disconnect(&mut counter);
    let _ = write_command(peripheral, &command, &disconnect).await;
    Ok(())
}

fn percentile(sorted: &[u64], percent: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = (sorted.len() - 1).saturating_mul(percent).div_ceil(100);
    sorted[index.min(sorted.len() - 1)]
}

fn respiration_period(sensor: &SensorInfo, requested_period_us: u32) -> Result<u32, String> {
    let within_minimum =
        sensor.minimum_period_us == 0 || requested_period_us >= sensor.minimum_period_us;
    let within_maximum =
        sensor.maximum_period_us == 0 || u64::from(requested_period_us) <= sensor.maximum_period_us;
    let on_granularity = sensor.period_granularity_us == 0
        || requested_period_us.is_multiple_of(sensor.period_granularity_us);
    if (1_000..=60_000_000).contains(&requested_period_us)
        && within_minimum
        && within_maximum
        && on_granularity
    {
        return Ok(requested_period_us);
    }
    let typical = sensor.typical_period_us;
    if (1_000..=60_000_000).contains(&typical) {
        return Ok(typical);
    }
    Err(format!(
        "GDX-RB Force (N) does not accept the requested {requested_period_us} µs period and did not report a valid fallback."
    ))
}

fn recognized_device_message(device: &vernier_gdx_core::DeviceInfo) -> String {
    if device.order_code.is_empty() {
        format!("Recognized {}", device.model.label())
    } else {
        format!(
            "Recognized {} ({})",
            device.model.label(),
            device.order_code
        )
    }
}

fn characteristic(peripheral: &Peripheral, uuid: uuid::Uuid) -> Option<Characteristic> {
    peripheral
        .characteristics()
        .into_iter()
        .find(|characteristic| characteristic.uuid == uuid)
}

async fn write_command(
    peripheral: &Peripheral,
    characteristic: &Characteristic,
    command: &Command,
) -> Result<(), String> {
    for chunk in command.chunks(ATT_PAYLOAD_BYTES) {
        timed(
            "Go Direct command write",
            peripheral.write(
                characteristic,
                chunk,
                command_write_type(characteristic.properties)?,
            ),
        )
        .await?;
    }
    Ok(())
}

fn command_write_type(properties: CharPropFlags) -> Result<WriteType, String> {
    if properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE) {
        return Ok(WriteType::WithoutResponse);
    }
    if properties.contains(CharPropFlags::WRITE) {
        return Ok(WriteType::WithResponse);
    }
    Err("Go Direct command characteristic is not writable.".into())
}

async fn wait_for_response<S>(
    notifications: &mut S,
    accumulator: &mut FrameAccumulator,
    command: &Command,
) -> Result<Vec<u8>, String>
where
    S: Stream<Item = btleplug::api::ValueNotification> + Unpin,
{
    tokio::time::timeout(STARTUP_TIMEOUT, async {
        loop {
            let notification = notifications
                .next()
                .await
                .ok_or_else(|| "Go Direct notification stream ended during startup.".to_string())?;
            if notification.uuid != RESPONSE_CHARACTERISTIC {
                continue;
            }
            for bytes in accumulator
                .push(&notification.value)
                .map_err(|error| error.to_string())?
            {
                if matches!(decode_frame(&bytes), Ok(Frame::Response(_)))
                    && bytes.get(4) == Some(&command.id)
                    && bytes.get(5) == command.bytes.get(2)
                {
                    return Ok(bytes);
                }
            }
        }
    })
    .await
    .map_err(|_| "Go Direct command response timed out.".to_string())?
}

async fn timed<T, E>(
    operation: &'static str,
    future: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, String>
where
    E: std::fmt::Display,
{
    tokio::time::timeout(BLE_TIMEOUT, future)
        .await
        .map_err(|_| format!("{operation} timed out."))?
        .map_err(|error| format!("{operation} failed: {error}"))
}

async fn control(
    events: &mpsc::Sender<InputEvent>,
    phase: &'static str,
    message: String,
) -> Result<(), String> {
    events
        .send(InputEvent::Status { phase, message })
        .await
        .map_err(|_| "Go Direct event receiver closed.".to_string())
}

async fn control_connected(
    events: &mpsc::Sender<InputEvent>,
    device: &vernier_gdx_core::DeviceInfo,
    status: &vernier_gdx_core::DeviceStatus,
    sensor: &SensorInfo,
    sample_period_us: u32,
) -> Result<(), String> {
    events
        .send(InputEvent::Connected {
            device_name: device.name.clone(),
            model_code: device.model.code().to_string(),
            sensor_number: sensor.number,
            sensor_name: sensor.description.clone(),
            sensor_unit: sensor.unit.clone(),
            sample_period_us,
            main_firmware_version: status.main_firmware_version.clone(),
            battery_percent: status.battery_percent,
        })
        .await
        .map_err(|_| "Go Direct event receiver closed.".to_string())
}

fn validate_slot(slot: &str) -> Result<(), String> {
    if slot.is_empty()
        || slot.len() > 32
        || !slot
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(
            "An input slot must be 1-32 ASCII letters, digits, hyphens, or underscores.".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_capacity_is_explicitly_bounded() {
        assert_eq!(InputSessionPool::new(0).max_sessions, 1);
        assert_eq!(InputSessionPool::new(99).max_sessions, MAX_GDX_SESSIONS);
    }

    #[test]
    fn slots_are_stable_nonidentifying_routing_keys() {
        assert!(validate_slot("source-8").is_ok());
        assert!(validate_slot("contains a space").is_err());
    }

    #[tokio::test]
    async fn completed_sessions_are_pruned_before_new_lifecycle_operations() {
        let pool = InputSessionPool::new(2);
        let (cancel, _) = watch::channel(false);
        let (finished, finished_rx) = watch::channel(false);
        pool.sessions.lock().await.insert(
            "source-1".into(),
            ActiveSession {
                device_id: "fixture".into(),
                cancel,
                finished: finished_rx,
            },
        );
        finished.send_replace(true);
        pool.prune_finished_sessions().await;
        assert!(pool.sessions.lock().await.is_empty());
    }

    #[test]
    fn respiration_period_prefers_respyra_compatible_ten_hertz() {
        let sensor = SensorInfo {
            number: 1,
            sensor_id: 1,
            numeric_type: vernier_gdx_core::NumericMeasurementType::Real,
            sampling_mode: vernier_gdx_core::SamplingMode::Periodic,
            description: "Force".into(),
            unit: "N".into(),
            uncertainty: 0.01,
            minimum: 0.0,
            maximum: 50.0,
            minimum_period_us: 50_000,
            maximum_period_us: 60_000_000,
            typical_period_us: 200_000,
            period_granularity_us: 1_000,
            mutual_exclusion_mask: 0,
        };
        assert_eq!(
            respiration_period(&sensor, DEFAULT_PERIOD_US).unwrap(),
            100_000
        );
    }

    #[test]
    fn respiration_period_falls_back_to_valid_sensor_metadata() {
        let sensor = SensorInfo {
            number: 1,
            sensor_id: 1,
            numeric_type: vernier_gdx_core::NumericMeasurementType::Real,
            sampling_mode: vernier_gdx_core::SamplingMode::Periodic,
            description: "Force".into(),
            unit: "N".into(),
            uncertainty: 0.01,
            minimum: 0.0,
            maximum: 50.0,
            minimum_period_us: 200_000,
            maximum_period_us: 60_000_000,
            typical_period_us: 200_000,
            period_granularity_us: 1_000,
            mutual_exclusion_mask: 0,
        };
        assert_eq!(
            respiration_period(&sensor, DEFAULT_PERIOD_US).unwrap(),
            200_000
        );
    }

    #[test]
    fn command_writes_without_response_when_supported() {
        assert!(matches!(
            command_write_type(CharPropFlags::WRITE_WITHOUT_RESPONSE),
            Ok(WriteType::WithoutResponse)
        ));
    }

    #[test]
    fn command_write_falls_back_to_write_with_response() {
        assert!(matches!(
            command_write_type(CharPropFlags::WRITE),
            Ok(WriteType::WithResponse)
        ));
    }

    #[test]
    fn command_write_prefers_without_response_when_both_are_supported() {
        assert!(matches!(
            command_write_type(CharPropFlags::WRITE_WITHOUT_RESPONSE | CharPropFlags::WRITE),
            Ok(WriteType::WithoutResponse)
        ));
    }

    #[test]
    fn command_write_rejects_a_nonwritable_characteristic() {
        assert!(command_write_type(CharPropFlags::READ).is_err());
    }
}
