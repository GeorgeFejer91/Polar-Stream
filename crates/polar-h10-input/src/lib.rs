//! Bluetooth input boundary for Polar H10 sensors.
//!
//! This crate emits decoded input events. It has no dependency on Tauri, LSL,
//! OSC, charts, or any application state outside the Bluetooth connection.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_os = "windows")]
mod windows_backend;
#[cfg(any(target_os = "windows", test))]
mod windows_session_lifecycle;

#[cfg(not(target_os = "windows"))]
use btleplug::api::WriteType;
#[cfg(not(target_os = "windows"))]
use btleplug::{
    api::{Central, Manager as _, Peripheral as _, ScanFilter},
    platform::{Manager, Peripheral},
};
#[cfg(not(target_os = "windows"))]
use futures_util::StreamExt;
use polar_h10_core::AccSample;
#[cfg(not(target_os = "windows"))]
use polar_h10_core::{
    ACC_MEASUREMENT, ECG_MEASUREMENT, PmdFrame, decode_heart_rate, decode_pmd,
    start_accelerometer_command, start_ecg_command, stop_command,
};
#[cfg(not(target_os = "windows"))]
use polar_stream_time::monotonic_now_ns;
use serde::Serialize;
use tokio::sync::{Mutex, mpsc, watch};
use uuid::Uuid;

const HEART_RATE_SERVICE: Uuid = Uuid::from_u128(0x0000180d_0000_1000_8000_00805f9b34fb);
const HEART_RATE_MEASUREMENT: Uuid = Uuid::from_u128(0x00002a37_0000_1000_8000_00805f9b34fb);
const BATTERY_LEVEL: Uuid = Uuid::from_u128(0x00002a19_0000_1000_8000_00805f9b34fb);
const PMD_SERVICE: Uuid = Uuid::from_u128(0xfb005c80_02e7_f387_1cad_8acd2d8df0c8);
const PMD_CONTROL_POINT: Uuid = Uuid::from_u128(0xfb005c81_02e7_f387_1cad_8acd2d8df0c8);
const PMD_DATA: Uuid = Uuid::from_u128(0xfb005c82_02e7_f387_1cad_8acd2d8df0c8);
#[cfg(not(target_os = "windows"))]
const BLE_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(not(target_os = "windows"))]
const SCAN_DURATION: Duration = Duration::from_secs(4);
#[cfg(not(target_os = "windows"))]
const PROPERTY_SWEEP_TIMEOUT: Duration = Duration::from_secs(4);
#[cfg(not(target_os = "windows"))]
const PROPERTY_READ_CONCURRENCY: usize = 16;
const DEVICE_CACHE_TTL: Duration = Duration::from_secs(45);

#[cfg(target_os = "windows")]
type ScannedDevice = String;
#[cfg(not(target_os = "windows"))]
type ScannedDevice = Peripheral;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    pub id: String,
    pub name: String,
    pub rssi: Option<i16>,
}

#[derive(Clone, Debug)]
pub enum InputEvent {
    Status {
        phase: &'static str,
        message: String,
    },
    Connected {
        device_name: String,
        battery_percent: Option<u8>,
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
    Error(String),
    Disconnected {
        device_name: String,
        battery_percent: Option<u8>,
    },
}

struct ActiveConnection {
    #[cfg(target_os = "windows")]
    generation: u64,
    cancel: watch::Sender<bool>,
    #[cfg(target_os = "windows")]
    finished: watch::Receiver<bool>,
    #[cfg(not(target_os = "windows"))]
    id: String,
    #[cfg(not(target_os = "windows"))]
    peripheral: Peripheral,
}

pub struct InputManager {
    devices: Mutex<HashMap<String, ScannedDevice>>,
    active: Mutex<Option<ActiveConnection>>,
    #[cfg(target_os = "windows")]
    next_generation: AtomicU64,
}

struct PooledInputSession {
    device_id: String,
    manager: Arc<InputManager>,
}

struct CachedDeviceSummary {
    summary: DeviceSummary,
    seen_at: Instant,
}

/// Owns a bounded set of independent sensor sessions that share one discovery
/// snapshot. Each admitted device still runs through its own [`InputManager`]
/// and platform connection owner.
///
/// Connection and disconnection transitions are serialized so a session
/// cannot be removed while its platform setup is still in flight. Call
/// [`Self::disconnect_all`] before dropping the pool when bounded cleanup must
/// be observed.
pub struct InputSessionPool {
    discovery: Arc<InputManager>,
    candidates: Mutex<HashMap<String, CachedDeviceSummary>>,
    sessions: Mutex<HashMap<String, PooledInputSession>>,
    operation_gate: Mutex<()>,
    max_sessions: usize,
}

impl InputSessionPool {
    /// Construct the production qualification shape for two distinct H10s.
    pub fn two_h10s() -> Self {
        Self::with_max_sessions(2)
    }

    /// Construct a bounded pool. Each session retains an independent platform
    /// owner and hot path; the bound applies only to lifecycle admission.
    pub fn with_max_sessions(max_sessions: usize) -> Self {
        Self {
            discovery: Arc::new(InputManager::new()),
            candidates: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            operation_gate: Mutex::new(()),
            max_sessions: max_sessions.clamp(1, 8),
        }
    }

    /// Refresh the shared candidate registry without disturbing active session
    /// owners. The ordinary add-device workflow seeks one new H10, so Windows
    /// can stop as soon as one non-active exact-name candidate is observed.
    pub async fn scan(&self) -> Result<Vec<DeviceSummary>, String> {
        let _operation = self.operation_gate.lock().await;
        self.prune_finished_sessions().await;
        let active_device_ids = self
            .sessions
            .lock()
            .await
            .values()
            .map(|session| session.device_id.clone())
            .collect::<Vec<_>>();
        #[cfg(target_os = "windows")]
        let found = self
            .discovery
            .scan_for_exact_h10s(1, &active_device_ids)
            .await?;
        #[cfg(not(target_os = "windows"))]
        let found = self.discovery.scan().await?;

        let seen_at = Instant::now();
        let mut candidates = self.candidates.lock().await;
        candidates.retain(|id, candidate| {
            active_device_ids.contains(id)
                || seen_at.saturating_duration_since(candidate.seen_at) <= DEVICE_CACHE_TTL
        });
        for summary in found {
            candidates.insert(summary.id.clone(), CachedDeviceSummary { summary, seen_at });
        }
        let mut available = candidates
            .iter()
            .filter(|(id, _)| !active_device_ids.contains(id))
            .map(|(_, candidate)| candidate.summary.clone())
            .collect::<Vec<_>>();
        let retained_ids = candidates.keys().cloned().collect::<Vec<_>>();
        drop(candidates);
        self.discovery.retain_devices(&retained_ids).await;
        available.sort_by(|left, right| {
            right
                .rssi
                .cmp(&left.rssi)
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(available)
    }

    /// Connect one exact device from the latest shared scan under a stable,
    /// non-identifying slot such as `device-1` or `device-2`.
    pub async fn connect(
        &self,
        slot: &str,
        device_id: &str,
    ) -> Result<mpsc::Receiver<InputEvent>, String> {
        validate_session_slot(slot)?;
        let _operation = self.operation_gate.lock().await;
        self.prune_finished_sessions().await;

        let scanned_device = self
            .discovery
            .devices
            .lock()
            .await
            .get(device_id)
            .cloned()
            .ok_or_else(|| {
                "That sensor is no longer in the shared scan results. Scan again.".to_string()
            })?;

        let manager = Arc::new(InputManager::new());
        manager
            .devices
            .lock()
            .await
            .insert(device_id.to_string(), scanned_device);

        {
            let mut sessions = self.sessions.lock().await;
            validate_session_admission(&sessions, self.max_sessions, slot, device_id)?;
            sessions.insert(
                slot.to_string(),
                PooledInputSession {
                    device_id: device_id.to_string(),
                    manager: manager.clone(),
                },
            );
        }

        match manager.connect(device_id).await {
            Ok(events) => Ok(events),
            Err(error) => {
                let mut sessions = self.sessions.lock().await;
                if sessions
                    .get(slot)
                    .is_some_and(|session| Arc::ptr_eq(&session.manager, &manager))
                {
                    sessions.remove(slot);
                }
                Err(error)
            }
        }
    }

    /// Disconnect one named slot through its ordinary platform owner.
    pub async fn disconnect(&self, slot: &str) -> Result<(), String> {
        validate_session_slot(slot)?;
        let _operation = self.operation_gate.lock().await;
        let session = self.sessions.lock().await.remove(slot);
        if let Some(session) = session {
            session.manager.disconnect().await?;
        }
        Ok(())
    }

    /// Disconnect every admitted session. All slots are released even if one
    /// platform owner reports an error; the first error is returned afterward.
    pub async fn disconnect_all(&self) -> Result<(), String> {
        let _operation = self.operation_gate.lock().await;
        let sessions = {
            let mut sessions = self.sessions.lock().await;
            std::mem::take(&mut *sessions)
        };
        let mut first_error = None;
        for session in sessions.into_values() {
            if let Err(error) = session.manager.disconnect().await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub async fn active_session_count(&self) -> usize {
        self.prune_finished_sessions().await;
        self.sessions.lock().await.len()
    }

    async fn prune_finished_sessions(&self) {
        let sessions = self
            .sessions
            .lock()
            .await
            .iter()
            .map(|(slot, session)| (slot.clone(), session.manager.clone()))
            .collect::<Vec<_>>();
        let mut finished = Vec::new();
        for (slot, manager) in sessions {
            if !manager.is_active().await {
                finished.push(slot);
            }
        }
        if finished.is_empty() {
            return;
        }
        self.sessions
            .lock()
            .await
            .retain(|slot, _| !finished.contains(slot));
    }
}

fn validate_session_slot(slot: &str) -> Result<(), String> {
    if slot.is_empty()
        || slot.len() > 32
        || !slot
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(
            "A sensor session slot must be 1-32 ASCII letters, digits, hyphens, or underscores."
                .into(),
        );
    }
    Ok(())
}

fn validate_session_admission(
    sessions: &HashMap<String, PooledInputSession>,
    max_sessions: usize,
    slot: &str,
    device_id: &str,
) -> Result<(), String> {
    if sessions.contains_key(slot) {
        return Err("That sensor session slot is already active.".into());
    }
    if sessions
        .values()
        .any(|session| session.device_id == device_id)
    {
        return Err("That exact sensor is already active in another session slot.".into());
    }
    if sessions.len() >= max_sessions {
        return Err(format!(
            "The bounded sensor session pool already owns its {max_sessions}-session maximum."
        ));
    }
    Ok(())
}

impl Default for InputManager {
    fn default() -> Self {
        Self::new()
    }
}

impl InputManager {
    pub fn new() -> Self {
        Self {
            devices: Mutex::new(HashMap::new()),
            active: Mutex::new(None),
            #[cfg(target_os = "windows")]
            next_generation: AtomicU64::new(1),
        }
    }

    #[cfg(target_os = "windows")]
    pub async fn scan(&self) -> Result<Vec<DeviceSummary>, String> {
        self.scan_for_exact_h10s(1, &[]).await
    }

    #[cfg(target_os = "windows")]
    async fn scan_for_exact_h10s(
        &self,
        minimum_exact_h10s: usize,
        excluded_device_ids: &[String],
    ) -> Result<Vec<DeviceSummary>, String> {
        let excluded_addresses = excluded_device_ids
            .iter()
            .filter_map(|device_id| parse_bluetooth_address(device_id))
            .collect();
        let found =
            windows_backend::scan_for_exact_h10s(minimum_exact_h10s, excluded_addresses).await?;
        let next_devices: HashMap<String, String> = found
            .iter()
            .map(|device| (device.id.clone(), device.name.clone()))
            .collect();
        self.devices.lock().await.extend(next_devices);
        Ok(found)
    }

    #[cfg(not(target_os = "windows"))]
    pub async fn scan(&self) -> Result<Vec<DeviceSummary>, String> {
        let manager = tokio::time::timeout(BLE_OPERATION_TIMEOUT, Manager::new())
            .await
            .map_err(|_| "Bluetooth initialization timed out.".to_string())?
            .map_err(|error| format!("Could not initialize Bluetooth: {error}"))?;
        let adapters = tokio::time::timeout(BLE_OPERATION_TIMEOUT, manager.adapters())
            .await
            .map_err(|_| "Bluetooth adapter enumeration timed out.".to_string())?
            .map_err(|error| format!("Could not enumerate Bluetooth adapters: {error}"))?;
        let adapter = adapters
            .into_iter()
            .next()
            .ok_or_else(|| "No Bluetooth Low Energy adapter was found.".to_string())?;

        // BlueZ combines service filters between applications, so filter locally.
        tokio::time::timeout(
            BLE_OPERATION_TIMEOUT,
            adapter.start_scan(ScanFilter::default()),
        )
        .await
        .map_err(|_| "Bluetooth scan startup timed out.".to_string())?
        .map_err(|error| format!("Could not start Bluetooth scan: {error}"))?;
        tokio::time::sleep(SCAN_DURATION).await;
        let peripherals = tokio::time::timeout(BLE_OPERATION_TIMEOUT, adapter.peripherals())
            .await
            .map_err(|_| "Bluetooth scan result enumeration timed out.".to_string())?
            .map_err(|error| format!("Could not read scan results: {error}"));
        let stop_result = tokio::time::timeout(BLE_OPERATION_TIMEOUT, adapter.stop_scan())
            .await
            .map_err(|_| "Bluetooth scan cleanup timed out.".to_string())?
            .map_err(|error| format!("Could not stop Bluetooth scan: {error}"));
        let peripherals = peripherals?;
        stop_result?;

        let mut found = Vec::new();
        let mut next_devices = HashMap::new();
        let reads = futures_util::stream::iter(peripherals)
            .map(|peripheral| async move {
                peripheral
                    .properties()
                    .await
                    .ok()
                    .flatten()
                    .map(|properties| (peripheral, properties))
            })
            .buffer_unordered(PROPERTY_READ_CONCURRENCY);
        tokio::pin!(reads);
        let property_deadline = tokio::time::Instant::now() + PROPERTY_SWEEP_TIMEOUT;
        while let Ok(Some(read)) = tokio::time::timeout_at(property_deadline, reads.next()).await {
            let Some((peripheral, properties)) = read else {
                continue;
            };
            let name = properties
                .local_name
                .unwrap_or_else(|| "Unnamed Polar sensor".into());
            let is_polar = name.to_ascii_lowercase().contains("polar")
                || properties.services.contains(&HEART_RATE_SERVICE)
                || properties.services.contains(&PMD_SERVICE);
            if !is_polar {
                continue;
            }

            let id = peripheral.id().to_string();
            found.push(DeviceSummary {
                id: id.clone(),
                name,
                rssi: properties.rssi,
            });
            next_devices.insert(id, peripheral);
        }
        // Peripheral property reads can wait on the operating-system Bluetooth
        // service. Publish the completed snapshot under one short lock instead
        // of blocking connect/disconnect while those reads are in flight.
        self.devices.lock().await.extend(next_devices);
        found.sort_by(|left, right| {
            right
                .rssi
                .cmp(&left.rssi)
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(found)
    }

    /// Connect and return an event receiver owned by the caller. The caller
    /// decides independently whether events go to files, networks, or a UI.
    pub async fn connect(
        self: &Arc<Self>,
        device_id: &str,
    ) -> Result<mpsc::Receiver<InputEvent>, String> {
        self.disconnect().await?;
        #[cfg(target_os = "windows")]
        let device_name = self
            .devices
            .lock()
            .await
            .get(device_id)
            .cloned()
            .ok_or_else(|| {
                "That sensor is no longer in the scan results. Scan again.".to_string()
            })?;
        #[cfg(not(target_os = "windows"))]
        let peripheral = self
            .devices
            .lock()
            .await
            .get(device_id)
            .cloned()
            .ok_or_else(|| {
                "That sensor is no longer in the scan results. Scan again.".to_string()
            })?;
        let (event_tx, event_rx) = mpsc::channel(128);
        send(
            &event_tx,
            InputEvent::Status {
                phase: "connecting",
                message: "Opening the low-energy connection…".into(),
            },
        )
        .await?;

        #[cfg(target_os = "windows")]
        {
            let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
            let (cancel, cancelled) = watch::channel(false);
            let (finished_tx, finished) = watch::channel(false);
            self.active.lock().await.replace(ActiveConnection {
                generation,
                cancel,
                finished,
            });
            if let Err(error) = windows_backend::spawn_session(
                device_id.to_string(),
                device_name,
                event_tx,
                Arc::downgrade(self),
                generation,
                cancelled,
                finished_tx.clone(),
            )
            .await
            {
                self.clear_active(generation).await;
                finished_tx.send_replace(true);
                return Err(error);
            }
            Ok(event_rx)
        }

        #[cfg(not(target_os = "windows"))]
        {
            if !peripheral
                .is_connected()
                .await
                .map_err(|error| format!("Could not read connection state: {error}"))?
            {
                peripheral
                    .connect()
                    .await
                    .map_err(|error| format!("Polar H10 connection failed: {error}"))?;
            }

            send(
                &event_tx,
                InputEvent::Status {
                    phase: "discovering",
                    message: "Connected. Discovering ECG and accelerometer services…".into(),
                },
            )
            .await?;
            peripheral
                .discover_services()
                .await
                .map_err(|error| format!("GATT service discovery failed: {error}"))?;

            let characteristics = peripheral.characteristics();
            let find_characteristic = |uuid| {
                characteristics
                    .iter()
                    .find(|characteristic| characteristic.uuid == uuid)
                    .cloned()
            };
            let pmd_data = find_characteristic(PMD_DATA)
                .ok_or_else(|| "The sensor does not expose Polar PMD data.".to_string())?;
            let control = find_characteristic(PMD_CONTROL_POINT)
                .ok_or_else(|| "The sensor does not expose the PMD control point.".to_string())?;
            let heart_rate = find_characteristic(HEART_RATE_MEASUREMENT);
            let battery = find_characteristic(BATTERY_LEVEL);

            let mut notifications = peripheral
                .notifications()
                .await
                .map_err(|error| format!("Could not open BLE notifications: {error}"))?;
            peripheral
                .subscribe(&pmd_data)
                .await
                .map_err(|error| format!("Could not subscribe to PMD data: {error}"))?;
            peripheral
                .subscribe(&control)
                .await
                .map_err(|error| format!("Could not subscribe to PMD responses: {error}"))?;
            if let Some(characteristic) = &heart_rate {
                peripheral
                    .subscribe(characteristic)
                    .await
                    .map_err(|error| format!("Could not subscribe to heart rate: {error}"))?;
            }

            peripheral
                .write(&control, &start_ecg_command(), WriteType::WithResponse)
                .await
                .map_err(|error| format!("Could not start ECG: {error}"))?;
            peripheral
                .write(
                    &control,
                    &start_accelerometer_command(),
                    WriteType::WithResponse,
                )
                .await
                .map_err(|error| format!("Could not start accelerometer: {error}"))?;

            let battery_percent = if let Some(characteristic) = battery {
                peripheral
                    .read(&characteristic)
                    .await
                    .ok()
                    .and_then(|value| value.first().copied())
            } else {
                None
            };
            let device_name = peripheral
                .properties()
                .await
                .ok()
                .flatten()
                .and_then(|properties| properties.local_name)
                .unwrap_or_else(|| "Polar H10".into());

            let (cancel, mut cancelled) = watch::channel(false);
            self.active.lock().await.replace(ActiveConnection {
                id: device_id.to_string(),
                cancel,
                peripheral: peripheral.clone(),
            });
            send(
                &event_tx,
                InputEvent::Connected {
                    device_name: device_name.clone(),
                    battery_percent,
                },
            )
            .await?;

            let connection_id = device_id.to_string();
            let manager = self.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        changed = cancelled.changed() => {
                            if changed.is_err() || *cancelled.borrow() { break; }
                        }
                        notification = notifications.next() => {
                            let Some(notification) = notification else { break };
                            let host_receive_timestamp_ns = monotonic_now_ns();
                            let event = if notification.uuid == PMD_DATA {
                                match decode_pmd(&notification.value) {
                                    Ok(PmdFrame::Ecg { sensor_timestamp_ns, microvolts }) => {
                                        InputEvent::Ecg {
                                            sensor_timestamp_ns,
                                            host_receive_timestamp_ns,
                                            microvolts,
                                        }
                                    }
                                    Ok(PmdFrame::Accelerometer { sensor_timestamp_ns, samples }) => {
                                        InputEvent::Accelerometer {
                                            sensor_timestamp_ns,
                                            host_receive_timestamp_ns,
                                            samples,
                                        }
                                    }
                                    Err(error) => InputEvent::Error(format!("Skipped malformed PMD frame: {error}")),
                                }
                            } else if notification.uuid == HEART_RATE_MEASUREMENT {
                                let frame = decode_heart_rate(&notification.value);
                                InputEvent::HeartRate {
                                    host_receive_timestamp_ns,
                                    beats_per_minute: frame.beats_per_minute,
                                    rr_intervals_ms: frame.rr_intervals_ms,
                                }
                            } else {
                                continue;
                            };
                            if event_tx.send(event).await.is_err() { break; }
                        }
                    }
                }

                let _ = peripheral
                    .write(
                        &control,
                        &stop_command(ECG_MEASUREMENT),
                        WriteType::WithResponse,
                    )
                    .await;
                let _ = peripheral
                    .write(
                        &control,
                        &stop_command(ACC_MEASUREMENT),
                        WriteType::WithResponse,
                    )
                    .await;
                let _ = peripheral.disconnect().await;
                {
                    let mut active = manager.active.lock().await;
                    if active
                        .as_ref()
                        .is_some_and(|connection| connection.id == connection_id)
                    {
                        active.take();
                    }
                }
                let _ = event_tx
                    .send(InputEvent::Disconnected {
                        device_name,
                        battery_percent,
                    })
                    .await;
            });

            Ok(event_rx)
        }
    }

    pub async fn disconnect(&self) -> Result<(), String> {
        let active = self.active.lock().await.take();
        if let Some(active) = active {
            let _ = active.cancel.send(true);

            #[cfg(target_os = "windows")]
            {
                let mut finished = active.finished;
                if !*finished.borrow() {
                    tokio::time::timeout(Duration::from_secs(16), async {
                        while !*finished.borrow() {
                            if finished.changed().await.is_err() {
                                break;
                            }
                        }
                    })
                    .await
                    .map_err(|_| {
                        "Windows WinRT disconnect timed out during bounded cleanup".to_string()
                    })?;
                }
            }

            #[cfg(not(target_os = "windows"))]
            {
                if active.peripheral.is_connected().await.unwrap_or_default() {
                    active
                        .peripheral
                        .disconnect()
                        .await
                        .map_err(|error| format!("Could not disconnect sensor: {error}"))?;
                }
            }
        }
        Ok(())
    }

    async fn is_active(&self) -> bool {
        self.active.lock().await.is_some()
    }

    async fn retain_devices(&self, retained_ids: &[String]) {
        self.devices
            .lock()
            .await
            .retain(|id, _| retained_ids.contains(id));
    }

    #[cfg(target_os = "windows")]
    async fn clear_active(&self, generation: u64) {
        let mut active = self.active.lock().await;
        if active
            .as_ref()
            .is_some_and(|connection| connection.generation == generation)
        {
            active.take();
        }
    }
}

#[cfg(any(target_os = "windows", test))]
fn parse_bluetooth_address(device_id: &str) -> Option<u64> {
    let mut compact = String::with_capacity(12);
    for character in device_id.chars() {
        if character.is_ascii_hexdigit() {
            compact.push(character);
        } else if !matches!(character, ':' | '-') {
            return None;
        }
    }
    if compact.len() != 12 {
        return None;
    }
    u64::from_str_radix(&compact, 16).ok()
}

async fn send(sender: &mpsc::Sender<InputEvent>, event: InputEvent) -> Result<(), String> {
    sender
        .send(event)
        .await
        .map_err(|_| "Input event receiver closed.".to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        InputManager, InputSessionPool, PooledInputSession, parse_bluetooth_address,
        validate_session_admission, validate_session_slot,
    };
    use std::{collections::HashMap, sync::Arc};

    #[test]
    fn parses_windows_bluetooth_address_forms() {
        assert_eq!(
            parse_bluetooth_address("AA:BB:CC:DD:EE:FF"),
            Some(0xAABBCCDDEEFF)
        );
        assert_eq!(
            parse_bluetooth_address("aa-bb-cc-dd-ee-ff"),
            Some(0xAABBCCDDEEFF)
        );
        assert_eq!(
            parse_bluetooth_address("AABBCCDDEEFF"),
            Some(0xAABBCCDDEEFF)
        );
    }

    #[test]
    fn rejects_non_address_peripheral_ids() {
        assert_eq!(parse_bluetooth_address("hci0/dev_01"), None);
        assert_eq!(parse_bluetooth_address("AABBCC"), None);
    }

    #[test]
    fn session_slots_are_short_identifier_free_labels() {
        assert!(validate_session_slot("device-1").is_ok());
        assert!(validate_session_slot("device_2").is_ok());
        assert!(validate_session_slot("").is_err());
        assert!(validate_session_slot("device 1").is_err());
        assert!(validate_session_slot(&"x".repeat(33)).is_err());
    }

    #[test]
    fn two_session_admission_rejects_duplicate_slots_devices_and_overflow() {
        let mut sessions = HashMap::new();
        sessions.insert(
            "device-1".to_string(),
            PooledInputSession {
                device_id: "first-private-id".to_string(),
                manager: Arc::new(InputManager::new()),
            },
        );
        assert!(validate_session_admission(&sessions, 2, "device-2", "second-private-id").is_ok());
        assert!(validate_session_admission(&sessions, 2, "device-1", "second-private-id").is_err());
        assert!(validate_session_admission(&sessions, 2, "device-2", "first-private-id").is_err());

        sessions.insert(
            "device-2".to_string(),
            PooledInputSession {
                device_id: "second-private-id".to_string(),
                manager: Arc::new(InputManager::new()),
            },
        );
        assert!(validate_session_admission(&sessions, 2, "device-3", "third-private-id").is_err());
    }

    #[tokio::test]
    async fn disconnect_all_drains_every_admitted_slot() {
        let pool = InputSessionPool::two_h10s();
        {
            let mut sessions = pool.sessions.lock().await;
            for (slot, device_id) in [
                ("device-1", "first-private-id"),
                ("device-2", "second-private-id"),
            ] {
                sessions.insert(
                    slot.to_string(),
                    PooledInputSession {
                        device_id: device_id.to_string(),
                        manager: Arc::new(InputManager::new()),
                    },
                );
            }
        }

        assert_eq!(pool.sessions.lock().await.len(), 2);
        pool.disconnect_all().await.unwrap();
        assert_eq!(pool.active_session_count().await, 0);
    }
}
