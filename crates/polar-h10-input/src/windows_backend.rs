//! Direct Windows WinRT Bluetooth backend.
//!
//! Windows uses an active WinRT advertisement watcher for bounded discovery,
//! then owns one persistent `GattSession`, all characteristic subscriptions,
//! and teardown. Other platforms retain the cross-platform `btleplug` path.

use std::{
    collections::{HashMap, VecDeque},
    future::IntoFuture,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use futures_util::{StreamExt, stream};
use polar_h10_core::{
    ACC_MEASUREMENT, ECG_MEASUREMENT, PmdFrame, decode_heart_rate, decode_pmd,
    start_accelerometer_command, start_ecg_command, stop_command,
};
use tokio::sync::{mpsc, watch};
use windows::{
    Devices::{
        Bluetooth::{
            Advertisement::{
                BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
                BluetoothLEAdvertisementWatcherStatus, BluetoothLEScanningMode,
            },
            BluetoothCacheMode, BluetoothLEDevice, BluetoothLEPreferredConnectionParameters,
            BluetoothLEPreferredConnectionParametersRequest,
            GenericAttributeProfile::{
                GattCharacteristic, GattCharacteristicProperties,
                GattClientCharacteristicConfigurationDescriptorValue, GattCommunicationStatus,
                GattDeviceService, GattSession, GattValueChangedEventArgs,
            },
        },
        Enumeration::DeviceAccessStatus,
    },
    Foundation::TypedEventHandler,
    Storage::Streams::{DataReader, DataWriter, IBuffer},
    core::{GUID, Ref},
};

use super::{
    BATTERY_LEVEL, DeviceSummary, HEART_RATE_MEASUREMENT, HEART_RATE_SERVICE, InputEvent,
    InputManager, PMD_CONTROL_POINT, PMD_DATA, PMD_SERVICE, parse_bluetooth_address,
};

const SCAN_DURATION: Duration = Duration::from_secs(4);
const MAX_SCANNED_DEVICES: usize = 256;
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(8);
const GATT_TIMEOUT: Duration = Duration::from_secs(5);
const FIRST_FRAMES_TIMEOUT: Duration = Duration::from_secs(12);
const EVENT_SEND_TIMEOUT: Duration = Duration::from_secs(2);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(3);
const CHARACTERISTIC_ATTEMPTS: usize = 3;
const RAW_NOTIFICATION_CAPACITY: usize = 128;
const FIRST_FRAME_BUFFER_CAPACITY: usize = 64;
const SCAN_DIAGNOSTICS_ENV: &str = "POLAR_STREAM_H10_SCAN_DIAGNOSTICS";
const PROPERTY_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(5);
const PROPERTY_SWEEP_TIMEOUT: Duration = Duration::from_secs(6);
const PROPERTY_CONFIRMATION_CONCURRENCY: usize = 8;

#[derive(Clone)]
struct ScanObservation {
    name: Option<String>,
    rssi: i16,
    advertised_polar_service: bool,
}

#[derive(Clone)]
struct ScanCandidate {
    address: u64,
    name: Option<String>,
    rssi: i16,
    advertised_polar_service: bool,
}

#[derive(Default)]
struct ScanDiagnostics {
    advertisements: usize,
    malformed_core_fields: usize,
    advertisement_type_readable: usize,
    advertisement_type_unavailable: usize,
    connectable_true: usize,
    connectable_false: usize,
    connectable_unavailable: usize,
    local_name_readable: usize,
    local_name_unavailable: usize,
    local_name_present: usize,
    local_name_missing: usize,
    exact_h10_name_match: usize,
    local_name_nonmatch: usize,
    service_uuids_readable: usize,
    service_uuids_unavailable: usize,
    polar_service_match: usize,
    polar_service_nonmatch: usize,
    manufacturer_sections_present: usize,
    manufacturer_sections_absent: usize,
    manufacturer_sections_unavailable: usize,
    admitted_by_name: usize,
    admitted_by_service: usize,
    admitted_known_duplicate: usize,
    rejected_no_strong_evidence: usize,
    overflow_rejections: usize,
    property_confirmation_attempts: usize,
    property_confirmation_passes: usize,
    property_confirmation_rejections: usize,
    property_confirmation_timeouts: usize,
    returned_candidates: usize,
}

struct AdvertisementEvidence {
    local_name: Option<String>,
    local_name_readable: bool,
    service_uuids_readable: bool,
    advertised_polar_service: bool,
    advertisement_type_readable: bool,
    connectable: Option<bool>,
    manufacturer_section_count: Option<u32>,
}

struct ScanAccumulator {
    devices: HashMap<u64, ScanObservation>,
    diagnostics: ScanDiagnostics,
    diagnostics_enabled: bool,
    overflowed: bool,
}

impl ScanAccumulator {
    fn new(diagnostics_enabled: bool) -> Self {
        Self {
            devices: HashMap::new(),
            diagnostics: ScanDiagnostics::default(),
            diagnostics_enabled,
            overflowed: false,
        }
    }

    fn record_malformed(&mut self) {
        if self.diagnostics_enabled {
            self.diagnostics.advertisements += 1;
            self.diagnostics.malformed_core_fields += 1;
        }
    }

    fn record(&mut self, address: u64, rssi: i16, evidence: AdvertisementEvidence) {
        let exact_name = evidence
            .local_name
            .as_deref()
            .is_some_and(is_polar_h10_name);
        self.observe_predicates(&evidence, exact_name);

        if let Some(existing) = self.devices.get_mut(&address) {
            if exact_name {
                existing.name = evidence.local_name;
            }
            existing.rssi = existing.rssi.max(rssi);
            existing.advertised_polar_service |= evidence.advertised_polar_service;
            if self.diagnostics_enabled {
                self.diagnostics.admitted_known_duplicate += 1;
            }
            return;
        }

        if !exact_name && !evidence.advertised_polar_service {
            if self.diagnostics_enabled {
                self.diagnostics.rejected_no_strong_evidence += 1;
            }
            return;
        }

        if self.devices.len() >= MAX_SCANNED_DEVICES {
            self.overflowed = true;
            if self.diagnostics_enabled {
                self.diagnostics.overflow_rejections += 1;
            }
            return;
        }

        if self.diagnostics_enabled {
            if exact_name {
                self.diagnostics.admitted_by_name += 1;
            } else {
                self.diagnostics.admitted_by_service += 1;
            }
        }
        self.devices.insert(
            address,
            ScanObservation {
                name: evidence.local_name.filter(|_| exact_name),
                rssi,
                advertised_polar_service: evidence.advertised_polar_service,
            },
        );
    }

    fn observe_predicates(&mut self, evidence: &AdvertisementEvidence, exact_name: bool) {
        if !self.diagnostics_enabled {
            return;
        }
        let diagnostics = &mut self.diagnostics;
        diagnostics.advertisements += 1;
        if evidence.advertisement_type_readable {
            diagnostics.advertisement_type_readable += 1;
        } else {
            diagnostics.advertisement_type_unavailable += 1;
        }
        match evidence.connectable {
            Some(true) => diagnostics.connectable_true += 1,
            Some(false) => diagnostics.connectable_false += 1,
            None => diagnostics.connectable_unavailable += 1,
        }
        if evidence.local_name_readable {
            diagnostics.local_name_readable += 1;
            if evidence
                .local_name
                .as_deref()
                .is_some_and(|name| !name.is_empty())
            {
                diagnostics.local_name_present += 1;
                if exact_name {
                    diagnostics.exact_h10_name_match += 1;
                } else {
                    diagnostics.local_name_nonmatch += 1;
                }
            } else {
                diagnostics.local_name_missing += 1;
            }
        } else {
            diagnostics.local_name_unavailable += 1;
        }
        if evidence.service_uuids_readable {
            diagnostics.service_uuids_readable += 1;
            if evidence.advertised_polar_service {
                diagnostics.polar_service_match += 1;
            } else {
                diagnostics.polar_service_nonmatch += 1;
            }
        } else {
            diagnostics.service_uuids_unavailable += 1;
        }
        match evidence.manufacturer_section_count {
            Some(0) => diagnostics.manufacturer_sections_absent += 1,
            Some(_) => diagnostics.manufacturer_sections_present += 1,
            None => diagnostics.manufacturer_sections_unavailable += 1,
        }
    }

    fn finish(self) -> ScanBatch {
        let candidates = self
            .devices
            .into_iter()
            .map(|(address, observation)| ScanCandidate {
                address,
                name: observation.name,
                rssi: observation.rssi,
                advertised_polar_service: observation.advertised_polar_service,
            })
            .collect::<Vec<_>>();
        ScanBatch {
            candidates,
            diagnostics: self.diagnostics,
            diagnostics_enabled: self.diagnostics_enabled,
            overflowed: self.overflowed,
        }
    }
}

struct ScanBatch {
    candidates: Vec<ScanCandidate>,
    diagnostics: ScanDiagnostics,
    diagnostics_enabled: bool,
    overflowed: bool,
}

fn is_polar_h10_name(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    normalized == "polar h10"
        || normalized
            .strip_prefix("polar h10")
            .is_some_and(|suffix| suffix.starts_with(' ') || suffix.starts_with('-'))
}

struct AdvertisementWatcherGuard {
    watcher: BluetoothLEAdvertisementWatcher,
    received_token: Option<i64>,
}

fn watcher_needs_stop(status: BluetoothLEAdvertisementWatcherStatus) -> bool {
    status == BluetoothLEAdvertisementWatcherStatus::Started
}

fn take_received_token(received_token: &mut Option<i64>) -> Option<i64> {
    received_token.take()
}

fn watcher_status_name(status: BluetoothLEAdvertisementWatcherStatus) -> &'static str {
    if status == BluetoothLEAdvertisementWatcherStatus::Created {
        "created"
    } else if status == BluetoothLEAdvertisementWatcherStatus::Started {
        "started"
    } else if status == BluetoothLEAdvertisementWatcherStatus::Stopping {
        "stopping"
    } else if status == BluetoothLEAdvertisementWatcherStatus::Stopped {
        "stopped"
    } else if status == BluetoothLEAdvertisementWatcherStatus::Aborted {
        "aborted"
    } else {
        "unknown"
    }
}

fn report_scan_status(
    diagnostics_enabled: bool,
    transition: &str,
    watcher: &BluetoothLEAdvertisementWatcher,
) {
    if diagnostics_enabled {
        let status = watcher
            .Status()
            .map(watcher_status_name)
            .unwrap_or("unavailable");
        eprintln!("POLAR_H10_SCAN_DIAGNOSTIC transition={transition} status={status}");
    }
}

impl Drop for AdvertisementWatcherGuard {
    fn drop(&mut self) {
        if let Some(token) = take_received_token(&mut self.received_token) {
            let _ = self.watcher.RemoveReceived(token);
        }
        if self.watcher.Status().is_ok_and(watcher_needs_stop) {
            let _ = self.watcher.Stop();
        }
    }
}

pub(super) async fn scan() -> Result<Vec<DeviceSummary>, String> {
    let mut batch = tokio::task::spawn_blocking(scan_blocking)
        .await
        .map_err(|error| format!("Windows WinRT scan worker failed: {error}"))??;
    if batch.overflowed {
        report_scan_predicates(&batch);
        return Err(format!(
            "Windows WinRT scan exceeded its {MAX_SCANNED_DEVICES}-device bound"
        ));
    }
    let devices = confirm_scan_candidates(&mut batch).await;
    batch.diagnostics.returned_candidates = devices.len();
    report_scan_predicates(&batch);
    Ok(devices)
}

fn scan_blocking() -> Result<ScanBatch, String> {
    let diagnostics_enabled = std::env::var_os(SCAN_DIAGNOSTICS_ENV).is_some();
    let watcher = BluetoothLEAdvertisementWatcher::new()
        .map_err(|error| stage_error("scan initialization", error))?;
    report_scan_status(diagnostics_enabled, "created", &watcher);
    watcher
        .SetScanningMode(BluetoothLEScanningMode::Active)
        .map_err(|error| stage_error("scan mode", error))?;

    let observations = Arc::new(Mutex::new(ScanAccumulator::new(diagnostics_enabled)));
    let callback_observations = observations.clone();
    let handler = TypedEventHandler::<
        BluetoothLEAdvertisementWatcher,
        BluetoothLEAdvertisementReceivedEventArgs,
    >::new(
        move |_, args: Ref<BluetoothLEAdvertisementReceivedEventArgs>| {
            let Ok(args) = args.ok() else {
                record_malformed_callback(&callback_observations);
                return Ok(());
            };
            let (Ok(address), Ok(rssi), Ok(advertisement)) = (
                args.BluetoothAddress(),
                args.RawSignalStrengthInDBm(),
                args.Advertisement(),
            ) else {
                record_malformed_callback(&callback_observations);
                return Ok(());
            };

            let local_name = advertisement.LocalName();
            let local_name_readable = local_name.is_ok();
            let local_name = local_name.ok().map(|name| name.to_string());
            let service_uuids = advertised_polar_service(&advertisement);
            let service_uuids_readable = service_uuids.is_ok();
            let advertised_polar_service = service_uuids.unwrap_or(false);
            let advertisement_type_readable = if diagnostics_enabled {
                args.AdvertisementType().is_ok()
            } else {
                false
            };
            let connectable = diagnostics_enabled
                .then(|| args.IsConnectable().ok())
                .flatten();
            let manufacturer_section_count = diagnostics_enabled
                .then(|| {
                    advertisement
                        .ManufacturerData()
                        .and_then(|sections| sections.Size())
                        .ok()
                })
                .flatten();
            let evidence = AdvertisementEvidence {
                local_name,
                local_name_readable,
                service_uuids_readable,
                advertised_polar_service,
                advertisement_type_readable,
                connectable,
                manufacturer_section_count,
            };
            callback_observations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .record(address, rssi, evidence);
            Ok(())
        },
    );
    let received_token = watcher
        .Received(&handler)
        .map_err(|error| stage_error("scan handler", error))?;
    let mut guard = AdvertisementWatcherGuard {
        watcher,
        received_token: Some(received_token),
    };
    guard
        .watcher
        .Start()
        .map_err(|error| stage_error("scan start", error))?;
    report_scan_status(diagnostics_enabled, "started", &guard.watcher);
    std::thread::sleep(SCAN_DURATION);
    report_scan_status(diagnostics_enabled, "before-stop", &guard.watcher);
    guard
        .watcher
        .Stop()
        .map_err(|error| stage_error("scan stop", error))?;
    report_scan_status(diagnostics_enabled, "after-stop", &guard.watcher);
    if let Some(token) = take_received_token(&mut guard.received_token) {
        guard
            .watcher
            .RemoveReceived(token)
            .map_err(|error| stage_error("scan handler cleanup", error))?;
    }
    drop(handler);
    let observations = {
        let mut observations = observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::replace(
            &mut *observations,
            ScanAccumulator::new(diagnostics_enabled),
        )
    };
    Ok(observations.finish())
}

fn record_malformed_callback(observations: &Arc<Mutex<ScanAccumulator>>) {
    observations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .record_malformed();
}

fn advertised_polar_service(
    advertisement: &windows::Devices::Bluetooth::Advertisement::BluetoothLEAdvertisement,
) -> windows::core::Result<bool> {
    let services = advertisement.ServiceUuids()?;
    let heart_rate = GUID::from_u128(HEART_RATE_SERVICE.as_u128());
    let pmd = GUID::from_u128(PMD_SERVICE.as_u128());
    for index in 0..services.Size()? {
        let service = services.GetAt(index)?;
        if service == heart_rate || service == pmd {
            return Ok(true);
        }
    }
    Ok(false)
}

enum PropertyConfirmation {
    Confirmed(String),
    Rejected,
    TimedOut,
}

async fn confirm_scan_candidates(batch: &mut ScanBatch) -> Vec<DeviceSummary> {
    let mut devices = Vec::new();
    let mut provisional = Vec::new();
    for candidate in std::mem::take(&mut batch.candidates) {
        if let Some(device) = confirmed_device_summary(&candidate, None) {
            devices.push(device);
        } else if candidate.advertised_polar_service {
            provisional.push(candidate);
        }
    }

    batch.diagnostics.property_confirmation_attempts = provisional.len();
    let total_provisional = provisional.len();
    let confirmations = stream::iter(provisional)
        .map(|candidate| async move {
            let confirmation = confirm_h10_property(candidate.address).await;
            (candidate, confirmation)
        })
        .buffer_unordered(PROPERTY_CONFIRMATION_CONCURRENCY);
    tokio::pin!(confirmations);
    let deadline = tokio::time::Instant::now() + PROPERTY_SWEEP_TIMEOUT;
    let mut completed = 0_usize;
    while let Ok(Some((candidate, confirmation))) =
        tokio::time::timeout_at(deadline, confirmations.next()).await
    {
        completed += 1;
        if let Some(device) =
            apply_property_confirmation(&candidate, confirmation, &mut batch.diagnostics)
        {
            devices.push(device);
        }
    }
    batch.diagnostics.property_confirmation_timeouts += total_provisional.saturating_sub(completed);
    devices.sort_by(|left, right| {
        right
            .rssi
            .cmp(&left.rssi)
            .then_with(|| left.name.cmp(&right.name))
    });
    devices
}

fn apply_property_confirmation(
    candidate: &ScanCandidate,
    confirmation: PropertyConfirmation,
    diagnostics: &mut ScanDiagnostics,
) -> Option<DeviceSummary> {
    match confirmation {
        PropertyConfirmation::Confirmed(name) => {
            let device = confirmed_device_summary(candidate, Some(name));
            if device.is_some() {
                diagnostics.property_confirmation_passes += 1;
            } else {
                diagnostics.property_confirmation_rejections += 1;
            }
            device
        }
        PropertyConfirmation::Rejected => {
            diagnostics.property_confirmation_rejections += 1;
            None
        }
        PropertyConfirmation::TimedOut => {
            diagnostics.property_confirmation_timeouts += 1;
            None
        }
    }
}

fn confirmed_device_summary(
    candidate: &ScanCandidate,
    confirmed_property_name: Option<String>,
) -> Option<DeviceSummary> {
    let name = candidate
        .name
        .clone()
        .filter(|name| is_polar_h10_name(name))
        .or_else(|| confirmed_property_name.filter(|name| is_polar_h10_name(name)))?;
    Some(DeviceSummary {
        id: format!("{:012X}", candidate.address),
        name,
        rssi: Some(candidate.rssi),
    })
}

async fn confirm_h10_property(address: u64) -> PropertyConfirmation {
    let Ok(operation) = BluetoothLEDevice::FromBluetoothAddressAsync(address) else {
        return PropertyConfirmation::Rejected;
    };
    let device =
        match tokio::time::timeout(PROPERTY_CONFIRMATION_TIMEOUT, operation.into_future()).await {
            Ok(Ok(device)) => device,
            Ok(Err(_)) => return PropertyConfirmation::Rejected,
            Err(_) => return PropertyConfirmation::TimedOut,
        };
    let name = device.Name().ok().map(|name| name.to_string());
    let _ = device.Close();
    match name.filter(|name| is_polar_h10_name(name)) {
        Some(name) => PropertyConfirmation::Confirmed(name),
        None => PropertyConfirmation::Rejected,
    }
}

fn report_scan_predicates(batch: &ScanBatch) {
    if !batch.diagnostics_enabled {
        return;
    }
    let d = &batch.diagnostics;
    eprintln!(
        "POLAR_H10_SCAN_DIAGNOSTIC predicates advertisements={} malformed_core={} advertisement_type_readable={} advertisement_type_unavailable={} connectable_true={} connectable_false={} connectable_unavailable={} local_name_readable={} local_name_unavailable={} local_name_present={} local_name_missing={} exact_h10_name_match={} local_name_nonmatch={} service_uuids_readable={} service_uuids_unavailable={} polar_service_match={} polar_service_nonmatch={} manufacturer_present={} manufacturer_absent={} manufacturer_unavailable={}",
        d.advertisements,
        d.malformed_core_fields,
        d.advertisement_type_readable,
        d.advertisement_type_unavailable,
        d.connectable_true,
        d.connectable_false,
        d.connectable_unavailable,
        d.local_name_readable,
        d.local_name_unavailable,
        d.local_name_present,
        d.local_name_missing,
        d.exact_h10_name_match,
        d.local_name_nonmatch,
        d.service_uuids_readable,
        d.service_uuids_unavailable,
        d.polar_service_match,
        d.polar_service_nonmatch,
        d.manufacturer_sections_present,
        d.manufacturer_sections_absent,
        d.manufacturer_sections_unavailable,
    );
    eprintln!(
        "POLAR_H10_SCAN_DIAGNOSTIC admission by_name={} by_service={} known_duplicate={} rejected_no_strong_evidence={} overflow_rejections={} property_attempts={} property_passes={} property_rejections={} property_timeouts={} returned_candidates={}",
        d.admitted_by_name,
        d.admitted_by_service,
        d.admitted_known_duplicate,
        d.rejected_no_strong_evidence,
        d.overflow_rejections,
        d.property_confirmation_attempts,
        d.property_confirmation_passes,
        d.property_confirmation_rejections,
        d.property_confirmation_timeouts,
        d.returned_candidates,
    );
}

pub(super) struct PreparedConnection {
    session: WinrtSession,
    raw_rx: mpsc::Receiver<RawNotification>,
    fault_rx: watch::Receiver<Option<String>>,
    buffered_events: VecDeque<InputEvent>,
    event_tx: mpsc::Sender<InputEvent>,
    device_name: String,
    battery_percent: Option<u8>,
}

impl PreparedConnection {
    pub fn spawn(
        self,
        manager: Weak<InputManager>,
        generation: u64,
        cancelled: watch::Receiver<bool>,
        finished: watch::Sender<bool>,
    ) {
        tokio::spawn(run_connection(
            self, manager, generation, cancelled, finished,
        ));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NotificationSource {
    PmdControl,
    PmdData,
    HeartRate,
}

#[derive(Debug)]
struct RawNotification {
    source: NotificationSource,
    value: Vec<u8>,
}

struct Subscription {
    characteristic: GattCharacteristic,
    token: i64,
}

struct WinrtSession {
    device: BluetoothLEDevice,
    gatt_session: GattSession,
    preferred_request: Option<BluetoothLEPreferredConnectionParametersRequest>,
    pmd_service: GattDeviceService,
    heart_rate_service: Option<GattDeviceService>,
    battery_service: Option<GattDeviceService>,
    control: GattCharacteristic,
    pmd_data: GattCharacteristic,
    heart_rate: Option<GattCharacteristic>,
    battery: Option<GattCharacteristic>,
    subscriptions: Vec<Subscription>,
    closed: bool,
}

struct OpeningSession {
    device: Option<BluetoothLEDevice>,
    gatt_session: Option<GattSession>,
    preferred_request: Option<BluetoothLEPreferredConnectionParametersRequest>,
}

struct DiscoveredHandles {
    pmd_service: GattDeviceService,
    heart_rate_service: Option<GattDeviceService>,
    battery_service: Option<GattDeviceService>,
    control: GattCharacteristic,
    pmd_data: GattCharacteristic,
    heart_rate: Option<GattCharacteristic>,
    battery: Option<GattCharacteristic>,
}

impl OpeningSession {
    fn new(
        device: BluetoothLEDevice,
        gatt_session: GattSession,
        preferred_request: Option<BluetoothLEPreferredConnectionParametersRequest>,
    ) -> Self {
        Self {
            device: Some(device),
            gatt_session: Some(gatt_session),
            preferred_request,
        }
    }

    fn device(&self) -> &BluetoothLEDevice {
        self.device
            .as_ref()
            .expect("opening session owns its device until completion")
    }

    fn finish(mut self, handles: DiscoveredHandles) -> WinrtSession {
        WinrtSession {
            device: self
                .device
                .take()
                .expect("opening session device is present"),
            gatt_session: self
                .gatt_session
                .take()
                .expect("opening GATT session is present"),
            preferred_request: self.preferred_request.take(),
            pmd_service: handles.pmd_service,
            heart_rate_service: handles.heart_rate_service,
            battery_service: handles.battery_service,
            control: handles.control,
            pmd_data: handles.pmd_data,
            heart_rate: handles.heart_rate,
            battery: handles.battery,
            subscriptions: Vec::new(),
            closed: false,
        }
    }
}

impl Drop for OpeningSession {
    fn drop(&mut self) {
        if let Some(session) = self.gatt_session.take() {
            let _ = session.SetMaintainConnection(false);
            let _ = session.Close();
        }
        if let Some(request) = self.preferred_request.take() {
            let _ = request.Close();
        }
        if let Some(device) = self.device.take() {
            let _ = device.Close();
        }
    }
}

impl WinrtSession {
    async fn open(device_id: &str, cancelled: &watch::Receiver<bool>) -> Result<Self, String> {
        ensure_active(cancelled, "open")?;
        let address = parse_bluetooth_address(device_id).ok_or_else(|| {
            "Windows WinRT open failed: the Bluetooth address was not recognized".to_string()
        })?;
        let device = await_stage(
            "open",
            OPEN_TIMEOUT,
            BluetoothLEDevice::FromBluetoothAddressAsync(address)
                .map_err(|error| stage_error("open", error))?,
        )
        .await?;
        ensure_active(cancelled, "open")?;
        let bluetooth_device_id = device
            .BluetoothDeviceId()
            .map_err(|error| stage_error("session", error))?;
        let gatt_session = await_stage(
            "session",
            OPEN_TIMEOUT,
            GattSession::FromDeviceIdAsync(&bluetooth_device_id)
                .map_err(|error| stage_error("session", error))?,
        )
        .await?;
        ensure_active(cancelled, "session")?;
        gatt_session
            .SetMaintainConnection(true)
            .map_err(|error| stage_error("session", error))?;

        let preferred_request = BluetoothLEPreferredConnectionParameters::ThroughputOptimized()
            .ok()
            .and_then(|parameters| {
                device
                    .RequestPreferredConnectionParameters(&parameters)
                    .ok()
            });

        let opening = OpeningSession::new(device, gatt_session, preferred_request);
        let pmd_service =
            required_service(opening.device(), PMD_SERVICE, "PMD service", cancelled).await?;
        ensure_active(cancelled, "PMD service discovery")?;
        let control = required_characteristic(
            &pmd_service,
            PMD_CONTROL_POINT,
            "PMD control point",
            cancelled,
        )
        .await?;
        ensure_active(cancelled, "PMD control discovery")?;
        let pmd_data =
            required_characteristic(&pmd_service, PMD_DATA, "PMD data", cancelled).await?;
        ensure_active(cancelled, "PMD data discovery")?;

        let heart_rate_service =
            optional_service(opening.device(), HEART_RATE_SERVICE, cancelled).await;
        ensure_active(cancelled, "heart-rate service discovery")?;
        let heart_rate = if let Some(service) = &heart_rate_service {
            optional_characteristic(service, HEART_RATE_MEASUREMENT, cancelled).await
        } else {
            None
        };
        ensure_active(cancelled, "heart-rate characteristic discovery")?;
        let battery_service = optional_service(
            opening.device(),
            uuid::Uuid::from_u128(0x0000180f_0000_1000_8000_00805f9b34fb),
            cancelled,
        )
        .await;
        ensure_active(cancelled, "battery service discovery")?;
        let battery = if let Some(service) = &battery_service {
            optional_characteristic(service, BATTERY_LEVEL, cancelled).await
        } else {
            None
        };
        ensure_active(cancelled, "battery characteristic discovery")?;

        Ok(opening.finish(DiscoveredHandles {
            pmd_service,
            heart_rate_service,
            battery_service,
            control,
            pmd_data,
            heart_rate,
            battery,
        }))
    }

    fn link_summary(&self) -> String {
        let mtu = self.gatt_session.MaxPduSize().ok();
        let observed = self
            .device
            .GetConnectionParameters()
            .ok()
            .and_then(|parameters| {
                Some((
                    parameters.ConnectionInterval().ok()?,
                    parameters.ConnectionLatency().ok()?,
                ))
            });
        let requested = self
            .preferred_request
            .as_ref()
            .and_then(|request| request.Status().ok())
            .map(|status| format!("{status:?}"))
            .unwrap_or_else(|| "system-managed".to_string());

        match (observed, mtu) {
            (Some((interval, latency)), Some(mtu)) => format!(
                "Direct Windows WinRT GATT · persistent session · throughput request {requested} · observed interval {:.2} ms · latency {latency} · negotiated MTU {mtu} B",
                interval as f64 * 1.25
            ),
            (None, Some(mtu)) => format!(
                "Direct Windows WinRT GATT · persistent session · throughput request {requested} · negotiated MTU {mtu} B"
            ),
            _ => format!(
                "Direct Windows WinRT GATT · persistent session · throughput request {requested}"
            ),
        }
    }

    async fn subscribe(
        &mut self,
        characteristic: &GattCharacteristic,
        source: NotificationSource,
        raw_tx: mpsc::Sender<RawNotification>,
        fault_tx: watch::Sender<Option<String>>,
    ) -> Result<(), String> {
        // The WinRT event source retains the delegate after registration. Keep
        // the non-Send delegate itself inside this synchronous scope so the
        // surrounding Tauri command future remains Send across later awaits.
        let token = {
            let handler = TypedEventHandler::<GattCharacteristic, GattValueChangedEventArgs>::new(
                move |_, args| {
                    let result = (|| {
                        let args = args.ok()?;
                        let value = args.CharacteristicValue()?;
                        let bytes = buffer_to_vec(&value)?;
                        enqueue_notification(
                            &raw_tx,
                            &fault_tx,
                            RawNotification {
                                source,
                                value: bytes,
                            },
                        );
                        Ok::<(), windows::core::Error>(())
                    })();
                    if let Err(error) = result {
                        fault_tx.send_replace(Some(format!(
                            "Windows WinRT notification callback failed: {error}"
                        )));
                    }
                    Ok(())
                },
            );
            characteristic
                .ValueChanged(&handler)
                .map_err(|error| stage_error("notification handler", error))?
        };
        let cccd = if characteristic
            .CharacteristicProperties()
            .map_err(|error| stage_error("notification properties", error))?
            .contains(GattCharacteristicProperties::Indicate)
        {
            GattClientCharacteristicConfigurationDescriptorValue::Indicate
        } else {
            GattClientCharacteristicConfigurationDescriptorValue::Notify
        };
        let status = await_stage(
            "notification subscription",
            GATT_TIMEOUT,
            characteristic
                .WriteClientCharacteristicConfigurationDescriptorAsync(cccd)
                .map_err(|error| stage_error("notification subscription", error))?,
        )
        .await;
        match status {
            Ok(GattCommunicationStatus::Success) => {
                self.subscriptions.push(Subscription {
                    characteristic: characteristic.clone(),
                    token,
                });
                Ok(())
            }
            Ok(status) => {
                let _ = characteristic.RemoveValueChanged(token);
                Err(format!(
                    "Windows WinRT notification subscription failed: {status:?}"
                ))
            }
            Err(error) => {
                let _ = characteristic.RemoveValueChanged(token);
                Err(error)
            }
        }
    }

    async fn write_control(&self, bytes: &[u8], stage: &str) -> Result<(), String> {
        let operation = {
            let writer = DataWriter::new().map_err(|error| stage_error(stage, error))?;
            writer
                .WriteBytes(bytes)
                .map_err(|error| stage_error(stage, error))?;
            let buffer = writer
                .DetachBuffer()
                .map_err(|error| stage_error(stage, error))?;
            self.control
                .WriteValueWithResultAsync(&buffer)
                .map_err(|error| stage_error(stage, error))?
        };
        let result = await_stage(stage, GATT_TIMEOUT, operation).await?;
        let status = result.Status().map_err(|error| stage_error(stage, error))?;
        if status != GattCommunicationStatus::Success {
            return Err(format!("Windows WinRT {stage} failed: {status:?}"));
        }
        Ok(())
    }

    async fn read_battery(&self) -> Option<u8> {
        let characteristic = self.battery.as_ref()?;
        let operation = characteristic
            .ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached)
            .ok()?;
        let result = tokio::time::timeout(GATT_TIMEOUT, operation.into_future())
            .await
            .ok()?
            .ok()?;
        if result.Status().ok()? != GattCommunicationStatus::Success {
            return None;
        }
        let value = result.Value().ok()?;
        buffer_to_vec(&value).ok()?.first().copied()
    }

    async fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;

        // Teardown has one global deadline, rather than a fresh deadline for
        // every best-effort GATT operation. Synchronous handler removal and
        // handle closure still run after that deadline.
        let _ = tokio::time::timeout(CLEANUP_TIMEOUT, async {
            let _ = self
                .write_control(&stop_command(ECG_MEASUREMENT), "stop ECG")
                .await;
            let _ = self
                .write_control(&stop_command(ACC_MEASUREMENT), "stop accelerometer")
                .await;

            while let Some(subscription) = self.subscriptions.pop() {
                let _ = subscription
                    .characteristic
                    .RemoveValueChanged(subscription.token);
                if let Ok(operation) = subscription
                    .characteristic
                    .WriteClientCharacteristicConfigurationDescriptorAsync(
                        GattClientCharacteristicConfigurationDescriptorValue::None,
                    )
                {
                    let _ = operation.await;
                }
            }
        })
        .await;

        for subscription in self.subscriptions.drain(..).rev() {
            let _ = subscription
                .characteristic
                .RemoveValueChanged(subscription.token);
        }

        let _ = self.gatt_session.SetMaintainConnection(false);
        if let Some(request) = self.preferred_request.take() {
            let _ = request.Close();
        }
        if let Some(service) = &self.battery_service {
            let _ = service.Close();
        }
        if let Some(service) = &self.heart_rate_service {
            let _ = service.Close();
        }
        let _ = self.pmd_service.Close();
        let _ = self.gatt_session.Close();
        let _ = self.device.Close();
    }
}

impl Drop for WinrtSession {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        for subscription in self.subscriptions.drain(..).rev() {
            let _ = subscription
                .characteristic
                .RemoveValueChanged(subscription.token);
        }
        let _ = self.gatt_session.SetMaintainConnection(false);
        if let Some(request) = self.preferred_request.take() {
            let _ = request.Close();
        }
        if let Some(service) = &self.battery_service {
            let _ = service.Close();
        }
        if let Some(service) = &self.heart_rate_service {
            let _ = service.Close();
        }
        let _ = self.pmd_service.Close();
        let _ = self.gatt_session.Close();
        let _ = self.device.Close();
    }
}

pub(super) async fn prepare(
    device_id: &str,
    device_name: String,
    event_tx: mpsc::Sender<InputEvent>,
    cancelled: &mut watch::Receiver<bool>,
) -> Result<PreparedConnection, String> {
    let (raw_tx, mut raw_rx) = mpsc::channel(RAW_NOTIFICATION_CAPACITY);
    let (fault_tx, mut fault_rx) = watch::channel(None::<String>);
    let mut session = WinrtSession::open(device_id, cancelled).await?;

    if let Err(error) = async {
        ensure_active(cancelled, "status publication")?;
        send_status(&event_tx, "optimizing", session.link_summary()).await?;
        send_status(
            &event_tx,
            "authorizing",
            "Windows is using the direct WinRT service-access and notification path.".to_string(),
        )
        .await?;

        if let Some(heart_rate) = session.heart_rate.clone() {
            ensure_active(cancelled, "heart-rate subscription")?;
            session
                .subscribe(
                    &heart_rate,
                    NotificationSource::HeartRate,
                    raw_tx.clone(),
                    fault_tx.clone(),
                )
                .await?;
        }
        ensure_active(cancelled, "PMD control subscription")?;
        session
            .subscribe(
                &session.control.clone(),
                NotificationSource::PmdControl,
                raw_tx.clone(),
                fault_tx.clone(),
            )
            .await?;
        ensure_active(cancelled, "PMD control subscription")?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        ensure_active(cancelled, "PMD data subscription")?;
        session
            .subscribe(
                &session.pmd_data.clone(),
                NotificationSource::PmdData,
                raw_tx,
                fault_tx,
            )
            .await?;
        ensure_active(cancelled, "PMD data subscription")?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        ensure_active(cancelled, "PMD setup")?;

        send_status(
            &event_tx,
            "starting",
            "Starting ECG at 130 Hz and three-axis accelerometer at 200 Hz…".to_string(),
        )
        .await?;
        session
            .write_control(&start_ecg_command(), "start ECG")
            .await?;
        ensure_active(cancelled, "start ECG")?;
        session
            .write_control(&start_accelerometer_command(), "start accelerometer")
            .await?;
        ensure_active(cancelled, "start accelerometer")?;
        Ok::<(), String>(())
    }
    .await
    {
        session.shutdown().await;
        return Err(error);
    }

    let mut gate = FirstFrameGate::default();
    let first_frames = tokio::time::timeout(FIRST_FRAMES_TIMEOUT, async {
        loop {
            tokio::select! {
                changed = cancelled.changed() => {
                    if changed.is_err() || *cancelled.borrow() {
                        return Err("Windows WinRT setup was cancelled during first-frame qualification".to_string());
                    }
                }
                changed = fault_rx.changed() => {
                    if changed.is_err() {
                        return Err("Windows WinRT notification fault channel closed".to_string());
                    }
                    if let Some(error) = fault_rx.borrow().clone() {
                        return Err(error);
                    }
                }
                raw = raw_rx.recv() => {
                    let raw = raw.ok_or_else(|| "Windows WinRT notifications ended before first ECG/ACC frames".to_string())?;
                    if let Some(event) = decode_notification(raw)
                        && gate.push(event)? {
                        return Ok(());
                    }
                }
            }
        }
    })
    .await;

    match first_frames {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            session.shutdown().await;
            return Err(error);
        }
        Err(_) => {
            session.shutdown().await;
            return Err(
                "Windows WinRT first-frame qualification timed out before both ECG and ACC arrived"
                    .to_string(),
            );
        }
    }

    if let Err(error) = ensure_active(cancelled, "battery read") {
        session.shutdown().await;
        return Err(error);
    }
    let battery_percent = session.read_battery().await;
    if let Err(error) = ensure_active(cancelled, "battery read") {
        session.shutdown().await;
        return Err(error);
    }
    Ok(PreparedConnection {
        session,
        raw_rx,
        fault_rx,
        buffered_events: gate.into_events(),
        event_tx,
        device_name,
        battery_percent,
    })
}

async fn run_connection(
    mut prepared: PreparedConnection,
    manager: Weak<InputManager>,
    generation: u64,
    mut cancelled: watch::Receiver<bool>,
    finished: watch::Sender<bool>,
) {
    if *cancelled.borrow() {
        prepared.session.shutdown().await;
        if let Some(manager) = manager.upgrade() {
            manager.clear_active(generation).await;
        }
        let _ = send_event(
            &prepared.event_tx,
            InputEvent::Disconnected {
                device_name: prepared.device_name,
                battery_percent: prepared.battery_percent,
            },
        )
        .await;
        finished.send_replace(true);
        return;
    }

    let connected = InputEvent::Connected {
        device_name: prepared.device_name.clone(),
        battery_percent: prepared.battery_percent,
    };
    let mut output_open = send_event(&prepared.event_tx, connected).await.is_ok();
    if output_open {
        while let Some(event) = prepared.buffered_events.pop_front() {
            if send_event(&prepared.event_tx, event).await.is_err() {
                output_open = false;
                break;
            }
        }
    }

    if output_open {
        loop {
            tokio::select! {
                changed = cancelled.changed() => {
                    if changed.is_err() || *cancelled.borrow() {
                        break;
                    }
                }
                changed = prepared.fault_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let error = { prepared.fault_rx.borrow().clone() };
                    if let Some(error) = error {
                        let _ = send_event(&prepared.event_tx, InputEvent::Error(error)).await;
                        break;
                    }
                }
                raw = prepared.raw_rx.recv() => {
                    let Some(raw) = raw else { break };
                    if let Some(event) = decode_notification(raw)
                        && send_event(&prepared.event_tx, event).await.is_err()
                    {
                        break;
                    }
                }
            }
        }
    }

    prepared.session.shutdown().await;
    if let Some(manager) = manager.upgrade() {
        manager.clear_active(generation).await;
    }
    let _ = send_event(
        &prepared.event_tx,
        InputEvent::Disconnected {
            device_name: prepared.device_name,
            battery_percent: prepared.battery_percent,
        },
    )
    .await;
    finished.send_replace(true);
}

async fn required_service(
    device: &BluetoothLEDevice,
    uuid: uuid::Uuid,
    label: &str,
    cancelled: &watch::Receiver<bool>,
) -> Result<GattDeviceService, String> {
    service(device, uuid, cancelled)
        .await?
        .ok_or_else(|| format!("Windows WinRT discovery failed: {label} was not exposed"))
}

async fn optional_service(
    device: &BluetoothLEDevice,
    uuid: uuid::Uuid,
    cancelled: &watch::Receiver<bool>,
) -> Option<GattDeviceService> {
    service(device, uuid, cancelled).await.ok().flatten()
}

async fn service(
    device: &BluetoothLEDevice,
    uuid: uuid::Uuid,
    cancelled: &watch::Receiver<bool>,
) -> Result<Option<GattDeviceService>, String> {
    ensure_active(cancelled, "service discovery")?;
    let result = await_stage(
        "service discovery",
        DISCOVERY_TIMEOUT,
        device
            .GetGattServicesForUuidWithCacheModeAsync(
                GUID::from_u128(uuid.as_u128()),
                BluetoothCacheMode::Uncached,
            )
            .map_err(|error| stage_error("service discovery", error))?,
    )
    .await?;
    let status = result
        .Status()
        .map_err(|error| stage_error("service discovery", error))?;
    if status != GattCommunicationStatus::Success {
        return Err(format!(
            "Windows WinRT service discovery failed: {status:?}"
        ));
    }
    let services = result
        .Services()
        .map_err(|error| stage_error("service enumeration", error))?;
    if services
        .Size()
        .map_err(|error| stage_error("service enumeration", error))?
        == 0
    {
        return Ok(None);
    }
    services
        .GetAt(0)
        .map(Some)
        .map_err(|error| stage_error("service enumeration", error))
}

async fn required_characteristic(
    service: &GattDeviceService,
    uuid: uuid::Uuid,
    label: &str,
    cancelled: &watch::Receiver<bool>,
) -> Result<GattCharacteristic, String> {
    characteristic(service, uuid, cancelled)
        .await?
        .ok_or_else(|| format!("Windows WinRT discovery failed: {label} was not exposed"))
}

async fn optional_characteristic(
    service: &GattDeviceService,
    uuid: uuid::Uuid,
    cancelled: &watch::Receiver<bool>,
) -> Option<GattCharacteristic> {
    characteristic(service, uuid, cancelled)
        .await
        .ok()
        .flatten()
}

async fn characteristic(
    service: &GattDeviceService,
    uuid: uuid::Uuid,
    cancelled: &watch::Receiver<bool>,
) -> Result<Option<GattCharacteristic>, String> {
    let guid = GUID::from_u128(uuid.as_u128());
    let mut last_error = "characteristic did not become reachable".to_string();

    for attempt in 0..CHARACTERISTIC_ATTEMPTS {
        ensure_active(cancelled, "characteristic discovery")?;
        let access = await_stage(
            "service access",
            GATT_TIMEOUT,
            service
                .RequestAccessAsync()
                .map_err(|error| stage_error("service access", error))?,
        )
        .await?;
        if access != DeviceAccessStatus::Allowed {
            last_error = format!("service access returned {access:?}");
        } else {
            let result = await_stage(
                "characteristic discovery",
                GATT_TIMEOUT,
                service
                    .GetCharacteristicsForUuidWithCacheModeAsync(guid, BluetoothCacheMode::Uncached)
                    .map_err(|error| stage_error("characteristic discovery", error))?,
            )
            .await?;
            let status = result
                .Status()
                .map_err(|error| stage_error("characteristic discovery", error))?;
            if status == GattCommunicationStatus::Success {
                let discovered =
                    {
                        let characteristics = result
                            .Characteristics()
                            .map_err(|error| stage_error("characteristic enumeration", error))?;
                        if characteristics
                            .Size()
                            .map_err(|error| stage_error("characteristic enumeration", error))?
                            > 0
                        {
                            Some(characteristics.GetAt(0).map_err(|error| {
                                stage_error("characteristic enumeration", error)
                            })?)
                        } else {
                            None
                        }
                    };
                if discovered.is_some() {
                    return Ok(discovered);
                }
                if let Some(cached) = cached_characteristic(service, guid, cancelled).await? {
                    return Ok(Some(cached));
                }
                return Ok(None);
            }
            last_error = format!("characteristic discovery returned {status:?}");
            if !retryable_gatt_status(status) {
                return Err(format!(
                    "Windows WinRT characteristic discovery failed: {last_error}"
                ));
            }
        }

        if attempt + 1 < CHARACTERISTIC_ATTEMPTS {
            ensure_active(cancelled, "characteristic discovery retry")?;
            tokio::time::sleep(Duration::from_millis(200 * (attempt as u64 + 1))).await;
        }
    }

    Err(format!(
        "Windows WinRT characteristic discovery failed after {CHARACTERISTIC_ATTEMPTS} attempts: {last_error}"
    ))
}

async fn cached_characteristic(
    service: &GattDeviceService,
    guid: GUID,
    cancelled: &watch::Receiver<bool>,
) -> Result<Option<GattCharacteristic>, String> {
    ensure_active(cancelled, "cached characteristic discovery")?;
    let result = await_stage(
        "cached characteristic discovery",
        GATT_TIMEOUT,
        service
            .GetCharacteristicsForUuidWithCacheModeAsync(guid, BluetoothCacheMode::Cached)
            .map_err(|error| stage_error("cached characteristic discovery", error))?,
    )
    .await?;
    if result
        .Status()
        .map_err(|error| stage_error("cached characteristic discovery", error))?
        != GattCommunicationStatus::Success
    {
        return Ok(None);
    }
    let characteristics = result
        .Characteristics()
        .map_err(|error| stage_error("cached characteristic enumeration", error))?;
    if characteristics
        .Size()
        .map_err(|error| stage_error("cached characteristic enumeration", error))?
        == 0
    {
        return Ok(None);
    }
    characteristics
        .GetAt(0)
        .map(Some)
        .map_err(|error| stage_error("cached characteristic enumeration", error))
}

fn retryable_gatt_status(status: GattCommunicationStatus) -> bool {
    matches!(
        status,
        GattCommunicationStatus::AccessDenied | GattCommunicationStatus::Unreachable
    )
}

fn decode_notification(raw: RawNotification) -> Option<InputEvent> {
    match raw.source {
        NotificationSource::PmdControl => None,
        NotificationSource::PmdData => Some(match decode_pmd(&raw.value) {
            Ok(PmdFrame::Ecg {
                sensor_timestamp_ns,
                microvolts,
            }) => InputEvent::Ecg {
                sensor_timestamp_ns,
                microvolts,
            },
            Ok(PmdFrame::Accelerometer {
                sensor_timestamp_ns,
                samples,
            }) => InputEvent::Accelerometer {
                sensor_timestamp_ns,
                samples,
            },
            Err(error) => InputEvent::Error(format!("Skipped malformed PMD frame: {error}")),
        }),
        NotificationSource::HeartRate => {
            let frame = decode_heart_rate(&raw.value);
            Some(InputEvent::HeartRate {
                beats_per_minute: frame.beats_per_minute,
                rr_intervals_ms: frame.rr_intervals_ms,
            })
        }
    }
}

#[derive(Default)]
struct FirstFrameGate {
    saw_ecg: bool,
    saw_acc: bool,
    buffered: VecDeque<InputEvent>,
}

impl FirstFrameGate {
    fn push(&mut self, event: InputEvent) -> Result<bool, String> {
        if let InputEvent::Error(error) = event {
            return Err(format!(
                "Windows WinRT received invalid sensor data before first-frame qualification: {error}"
            ));
        }
        if self.buffered.len() >= FIRST_FRAME_BUFFER_CAPACITY {
            return Err(format!(
                "Windows WinRT first-frame buffer reached its {FIRST_FRAME_BUFFER_CAPACITY}-event bound"
            ));
        }
        self.saw_ecg |= matches!(
            &event,
            InputEvent::Ecg { microvolts, .. } if !microvolts.is_empty()
        );
        self.saw_acc |= matches!(
            &event,
            InputEvent::Accelerometer { samples, .. } if !samples.is_empty()
        );
        self.buffered.push_back(event);
        Ok(self.saw_ecg && self.saw_acc)
    }

    fn into_events(self) -> VecDeque<InputEvent> {
        self.buffered
    }
}

fn enqueue_notification(
    raw_tx: &mpsc::Sender<RawNotification>,
    fault_tx: &watch::Sender<Option<String>>,
    raw: RawNotification,
) {
    match raw_tx.try_send(raw) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            fault_tx.send_replace(Some(format!(
                "Windows WinRT notification queue reached its {RAW_NOTIFICATION_CAPACITY}-batch bound; acquisition stopped without silent loss"
            )));
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {}
    }
}

fn buffer_to_vec(buffer: &IBuffer) -> windows::core::Result<Vec<u8>> {
    let reader = DataReader::FromBuffer(buffer)?;
    let mut bytes = vec![0; reader.UnconsumedBufferLength()? as usize];
    reader.ReadBytes(&mut bytes)?;
    Ok(bytes)
}

async fn send_status(
    event_tx: &mpsc::Sender<InputEvent>,
    phase: &'static str,
    message: String,
) -> Result<(), String> {
    event_tx
        .send(InputEvent::Status { phase, message })
        .await
        .map_err(|_| "Input event receiver closed during Windows setup".to_string())
}

async fn send_event(event_tx: &mpsc::Sender<InputEvent>, event: InputEvent) -> Result<(), String> {
    match tokio::time::timeout(EVENT_SEND_TIMEOUT, event_tx.send(event)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err("Input event receiver closed".to_string()),
        Err(_) => Err("Input event receiver stalled for two seconds".to_string()),
    }
}

fn ensure_active(cancelled: &watch::Receiver<bool>, stage: &str) -> Result<(), String> {
    if *cancelled.borrow() {
        Err(format!("Windows WinRT setup was cancelled during {stage}"))
    } else {
        Ok(())
    }
}

async fn await_stage<T, O>(stage: &str, duration: Duration, operation: O) -> Result<T, String>
where
    O: IntoFuture<Output = windows::core::Result<T>>,
{
    match tokio::time::timeout(duration, operation.into_future()).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(stage_error(stage, error)),
        Err(_) => Err(format!("Windows WinRT {stage} timed out")),
    }
}

fn stage_error(stage: &str, error: impl std::fmt::Display) -> String {
    format!("Windows WinRT {stage} failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use polar_h10_core::AccSample;

    fn evidence(
        name: Option<&str>,
        local_name_readable: bool,
        service_uuids_readable: bool,
        advertised_polar_service: bool,
    ) -> AdvertisementEvidence {
        AdvertisementEvidence {
            local_name: name.map(str::to_owned),
            local_name_readable,
            service_uuids_readable,
            advertised_polar_service,
            advertisement_type_readable: true,
            connectable: Some(true),
            manufacturer_section_count: Some(1),
        }
    }

    #[test]
    fn exact_reference_name_does_not_depend_on_service_uuid_readback() {
        let mut scan = ScanAccumulator::new(true);
        scan.record(1, -45, evidence(Some("Polar H10 TEST"), true, false, false));
        let batch = scan.finish();
        assert_eq!(batch.candidates.len(), 1);
        assert_eq!(batch.candidates[0].name.as_deref(), Some("Polar H10 TEST"));
        assert_eq!(batch.diagnostics.service_uuids_unavailable, 1);
        assert_eq!(batch.diagnostics.admitted_by_name, 1);
    }

    #[test]
    fn scan_accumulator_rejects_weak_shapes_and_coalesces_known_packets() {
        let mut scan = ScanAccumulator::new(true);
        scan.record(1, -20, evidence(Some("Unrelated"), true, true, false));
        scan.record(2, -20, evidence(None, true, true, false));
        scan.record(3, -20, evidence(Some("Polar H100"), true, true, false));
        scan.record(4, -20, evidence(Some("Fake Polar H10"), true, true, false));
        assert!(scan.devices.is_empty());

        scan.record(5, -80, evidence(Some("Polar H10 TEST"), true, true, false));
        scan.record(5, -55, evidence(None, true, true, false));
        scan.record(5, -70, evidence(Some("Unrelated"), true, true, false));
        let batch = scan.finish();
        assert_eq!(batch.candidates.len(), 1);
        assert_eq!(batch.candidates[0].address, 5);
        assert_eq!(batch.candidates[0].name.as_deref(), Some("Polar H10 TEST"));
        assert_eq!(batch.candidates[0].rssi, -55);
        assert_eq!(batch.diagnostics.admitted_known_duplicate, 2);
        assert_eq!(batch.diagnostics.rejected_no_strong_evidence, 4);
    }

    #[test]
    fn missing_name_service_candidate_requires_exact_property_confirmation() {
        let mut scan = ScanAccumulator::new(true);
        scan.record(7, -60, evidence(None, true, true, true));
        let batch = scan.finish();
        assert_eq!(batch.candidates.len(), 1);
        let candidate = &batch.candidates[0];
        assert!(confirmed_device_summary(candidate, None).is_none());
        assert!(confirmed_device_summary(candidate, Some("Polar Verity Sense".into())).is_none());
        let confirmed =
            confirmed_device_summary(candidate, Some("Polar H10 CONFIRMED".into())).unwrap();
        assert_eq!(confirmed.name, "Polar H10 CONFIRMED");
        assert_eq!(confirmed.id, "000000000007");
        assert_eq!(batch.diagnostics.admitted_by_service, 1);
    }

    #[test]
    fn property_confirmation_outcomes_are_counted_and_fail_closed() {
        let candidate = ScanCandidate {
            address: 9,
            name: None,
            rssi: -50,
            advertised_polar_service: true,
        };
        let mut diagnostics = ScanDiagnostics::default();
        assert!(
            apply_property_confirmation(
                &candidate,
                PropertyConfirmation::Confirmed("Polar H10 confirmed".into()),
                &mut diagnostics,
            )
            .is_some()
        );
        assert!(
            apply_property_confirmation(
                &candidate,
                PropertyConfirmation::Confirmed("Polar Verity Sense".into()),
                &mut diagnostics,
            )
            .is_none()
        );
        assert!(
            apply_property_confirmation(
                &candidate,
                PropertyConfirmation::Rejected,
                &mut diagnostics,
            )
            .is_none()
        );
        assert!(
            apply_property_confirmation(
                &candidate,
                PropertyConfirmation::TimedOut,
                &mut diagnostics,
            )
            .is_none()
        );
        assert_eq!(diagnostics.property_confirmation_passes, 1);
        assert_eq!(diagnostics.property_confirmation_rejections, 2);
        assert_eq!(diagnostics.property_confirmation_timeouts, 1);
    }

    #[test]
    fn scan_accumulator_fails_closed_at_unique_device_bound() {
        let mut scan = ScanAccumulator::new(true);
        for address in 0..MAX_SCANNED_DEVICES as u64 {
            scan.record(
                address,
                -60,
                evidence(Some("Polar H10 bounded"), true, true, false),
            );
        }
        scan.record(
            MAX_SCANNED_DEVICES as u64,
            -60,
            evidence(Some("Polar H10 overflow"), true, true, false),
        );
        assert_eq!(scan.devices.len(), MAX_SCANNED_DEVICES);
        assert!(scan.overflowed);
        assert_eq!(scan.diagnostics.overflow_rejections, 1);
    }

    #[test]
    fn watcher_cleanup_stops_only_an_active_scan() {
        let mut received_token = Some(42);
        assert_eq!(take_received_token(&mut received_token), Some(42));
        assert_eq!(take_received_token(&mut received_token), None);
        assert!(watcher_needs_stop(
            BluetoothLEAdvertisementWatcherStatus::Started
        ));
        assert!(!watcher_needs_stop(
            BluetoothLEAdvertisementWatcherStatus::Created
        ));
        assert!(!watcher_needs_stop(
            BluetoothLEAdvertisementWatcherStatus::Stopping
        ));
        assert!(!watcher_needs_stop(
            BluetoothLEAdvertisementWatcherStatus::Stopped
        ));
        assert!(!watcher_needs_stop(
            BluetoothLEAdvertisementWatcherStatus::Aborted
        ));
        assert_eq!(
            watcher_status_name(BluetoothLEAdvertisementWatcherStatus::Created),
            "created"
        );
        assert_eq!(
            watcher_status_name(BluetoothLEAdvertisementWatcherStatus::Started),
            "started"
        );
        assert_eq!(
            watcher_status_name(BluetoothLEAdvertisementWatcherStatus::Stopping),
            "stopping"
        );
        assert_eq!(
            watcher_status_name(BluetoothLEAdvertisementWatcherStatus::Stopped),
            "stopped"
        );
        assert_eq!(
            watcher_status_name(BluetoothLEAdvertisementWatcherStatus::Aborted),
            "aborted"
        );
    }

    fn ecg_event() -> InputEvent {
        InputEvent::Ecg {
            sensor_timestamp_ns: 10,
            microvolts: vec![1, -1],
        }
    }

    fn acc_event() -> InputEvent {
        InputEvent::Accelerometer {
            sensor_timestamp_ns: 20,
            samples: vec![AccSample {
                x_mg: 1,
                y_mg: -2,
                z_mg: 3,
            }],
        }
    }

    #[test]
    fn first_frame_gate_requires_both_streams_in_either_order() {
        let mut gate = FirstFrameGate::default();
        assert!(!gate.push(acc_event()).unwrap());
        assert!(gate.push(ecg_event()).unwrap());

        let mut gate = FirstFrameGate::default();
        assert!(!gate.push(ecg_event()).unwrap());
        assert!(gate.push(acc_event()).unwrap());
    }

    #[test]
    fn malformed_and_control_notifications_do_not_satisfy_first_frame_gate() {
        assert!(
            decode_notification(RawNotification {
                source: NotificationSource::PmdControl,
                value: vec![0x02, 0x00],
            })
            .is_none()
        );
        let event = decode_notification(RawNotification {
            source: NotificationSource::PmdData,
            value: vec![ECG_MEASUREMENT],
        })
        .expect("malformed PMD becomes an observable error");
        assert!(matches!(event, InputEvent::Error(_)));

        let mut gate = FirstFrameGate::default();
        assert!(
            gate.push(event)
                .unwrap_err()
                .contains("invalid sensor data")
        );
    }

    #[test]
    fn full_notification_queue_fails_closed() {
        let (raw_tx, _raw_rx) = mpsc::channel(1);
        let (fault_tx, fault_rx) = watch::channel(None);
        enqueue_notification(
            &raw_tx,
            &fault_tx,
            RawNotification {
                source: NotificationSource::PmdControl,
                value: vec![1],
            },
        );
        enqueue_notification(
            &raw_tx,
            &fault_tx,
            RawNotification {
                source: NotificationSource::PmdControl,
                value: vec![2],
            },
        );

        assert!(fault_rx.borrow().as_deref().is_some_and(|message| {
            message.contains("acquisition stopped without silent loss")
        }));
    }

    #[test]
    fn retry_status_is_narrow() {
        assert!(retryable_gatt_status(GattCommunicationStatus::AccessDenied));
        assert!(retryable_gatt_status(GattCommunicationStatus::Unreachable));
        assert!(!retryable_gatt_status(
            GattCommunicationStatus::ProtocolError
        ));
        assert!(!retryable_gatt_status(GattCommunicationStatus::Success));
    }

    #[test]
    fn setup_cancellation_is_observed_at_stage_boundaries() {
        let (cancel, cancelled) = watch::channel(false);
        assert!(ensure_active(&cancelled, "test stage").is_ok());
        cancel.send_replace(true);
        let error = ensure_active(&cancelled, "test stage").unwrap_err();
        assert_eq!(error, "Windows WinRT setup was cancelled during test stage");
    }

    #[test]
    fn first_frame_buffer_is_strictly_bounded() {
        let mut gate = FirstFrameGate::default();
        for _ in 0..FIRST_FRAME_BUFFER_CAPACITY {
            assert!(
                !gate
                    .push(InputEvent::HeartRate {
                        beats_per_minute: 60,
                        rr_intervals_ms: vec![1_000.0],
                    })
                    .unwrap()
            );
        }
        assert!(
            gate.push(InputEvent::HeartRate {
                beats_per_minute: 60,
                rr_intervals_ms: vec![1_000.0],
            })
            .unwrap_err()
            .contains("first-frame buffer reached")
        );
    }

    #[test]
    fn empty_sensor_batches_do_not_qualify() {
        let mut gate = FirstFrameGate::default();
        assert!(
            !gate
                .push(InputEvent::Ecg {
                    sensor_timestamp_ns: 1,
                    microvolts: Vec::new(),
                })
                .unwrap()
        );
        assert!(
            !gate
                .push(InputEvent::Accelerometer {
                    sensor_timestamp_ns: 2,
                    samples: Vec::new(),
                })
                .unwrap()
        );
    }
}
