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
    ACC_MEASUREMENT, ECG_MEASUREMENT, PMD_GET_SETTINGS_OPCODE, PMD_START_STREAM_OPCODE,
    PmdControlResponse, PmdFrame, decode_heart_rate, decode_pmd, decode_pmd_control_response,
    request_settings_command, start_accelerometer_command, start_ecg_command, stop_command,
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
    windows_session_lifecycle::{
        FirstFrameKind, FirstFrameStages, SessionCleanup, SessionStage, StageControl,
        StageReporter, StageResultClass, SubscriptionKind, run_controlled_stage, run_sync_stage,
    },
};

const SCAN_DURATION: Duration = Duration::from_secs(4);
const MAX_SCANNED_DEVICES: usize = 256;
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(8);
const GATT_TIMEOUT: Duration = Duration::from_secs(5);
const PMD_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const FIRST_STREAM_FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const SESSION_SETUP_TIMEOUT: Duration = Duration::from_secs(45);
const EVENT_SEND_TIMEOUT: Duration = Duration::from_secs(2);
const CLEANUP_OPERATION_TIMEOUT: Duration = Duration::from_millis(500);
const CHARACTERISTIC_ATTEMPTS: usize = 3;
const RAW_NOTIFICATION_CAPACITY: usize = 128;
const FIRST_FRAME_BUFFER_CAPACITY: usize = 64;
const SCAN_DIAGNOSTICS_ENV: &str = "POLAR_STREAM_H10_SCAN_DIAGNOSTICS";
const SESSION_DIAGNOSTICS_ENV: &str = "POLAR_STREAM_H10_SESSION_DIAGNOSTICS";
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

impl NotificationSource {
    const fn subscription_kind(self) -> SubscriptionKind {
        match self {
            Self::PmdControl => SubscriptionKind::PmdControl,
            Self::PmdData => SubscriptionKind::PmdData,
            Self::HeartRate => SubscriptionKind::HeartRate,
        }
    }
}

#[derive(Debug)]
struct RawNotification {
    source: NotificationSource,
    value: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExpectedControlResponse {
    opcode: u8,
    measurement: u8,
}

impl ExpectedControlResponse {
    const ECG_SETTINGS: Self = Self {
        opcode: PMD_GET_SETTINGS_OPCODE,
        measurement: ECG_MEASUREMENT,
    };
    const ECG_START: Self = Self {
        opcode: PMD_START_STREAM_OPCODE,
        measurement: ECG_MEASUREMENT,
    };
    const ACC_START: Self = Self {
        opcode: PMD_START_STREAM_OPCODE,
        measurement: ACC_MEASUREMENT,
    };
}

struct NotificationSink {
    raw_tx: mpsc::Sender<RawNotification>,
    fault_tx: watch::Sender<Option<String>>,
}

struct Subscription {
    characteristic: GattCharacteristic,
    source: SubscriptionKind,
    token: Option<i64>,
}

impl Subscription {
    fn remove_handler(&mut self) {
        if let Some(token) = take_subscription_token(&mut self.token) {
            let _ = self.characteristic.RemoveValueChanged(token);
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.remove_handler();
    }
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
    cleanup: SessionCleanup,
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
            cleanup: SessionCleanup::default(),
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

struct WinrtStageControl<O, C, X> {
    operation: O,
    cancel: C,
    close: X,
}

#[derive(Clone, Copy)]
struct WinrtStageCall {
    reporter: StageReporter,
    stage: SessionStage,
    attempt: usize,
    timeout: Duration,
}

#[derive(Clone, Copy)]
struct CharacteristicStages {
    access: SessionStage,
    uncached: SessionStage,
    cached: SessionStage,
}

impl WinrtStageCall {
    const fn new(
        reporter: StageReporter,
        stage: SessionStage,
        attempt: usize,
        timeout: Duration,
    ) -> Self {
        Self {
            reporter,
            stage,
            attempt,
            timeout,
        }
    }
}

impl<O, C, X> StageControl for WinrtStageControl<O, C, X>
where
    C: Fn(&O) -> windows::core::Result<()>,
    X: Fn(&O) -> windows::core::Result<()>,
{
    fn cancel(&self) -> Result<(), String> {
        (self.cancel)(&self.operation).map_err(|error| error.to_string())
    }

    fn close(&self) -> Result<(), String> {
        (self.close)(&self.operation).map_err(|error| error.to_string())
    }
}

async fn await_winrt_stage<T, O, C, X>(
    call: WinrtStageCall,
    operation: windows::core::Result<O>,
    cancel: C,
    close: X,
    cancelled: &mut watch::Receiver<bool>,
) -> Result<T, String>
where
    O: Clone + IntoFuture<Output = windows::core::Result<T>>,
    C: Fn(&O) -> windows::core::Result<()>,
    X: Fn(&O) -> windows::core::Result<()>,
{
    let operation = match operation {
        Ok(operation) => operation,
        Err(error) => {
            call.reporter
                .record_immediate(call.stage, call.attempt, StageResultClass::NativeError);
            return Err(format!(
                "Windows WinRT {} failed: {error}",
                call.stage.name()
            ));
        }
    };
    let future_operation = operation.clone();
    run_controlled_stage(
        call.reporter,
        call.stage,
        call.attempt,
        call.timeout,
        cancelled,
        async move {
            future_operation
                .into_future()
                .await
                .map_err(|error| error.to_string())
        },
        WinrtStageControl {
            operation,
            cancel,
            close,
        },
    )
    .await
    .map_err(|error| error.to_string())
}

async fn await_cleanup_operation<T, O, C, X>(
    operation: windows::core::Result<O>,
    cancel: C,
    close: X,
) -> Result<T, String>
where
    O: Clone + IntoFuture<Output = windows::core::Result<T>>,
    C: Fn(&O) -> windows::core::Result<()>,
    X: Fn(&O) -> windows::core::Result<()>,
{
    let operation = operation.map_err(|error| error.to_string())?;
    let future_operation = operation.clone();
    let result =
        tokio::time::timeout(CLEANUP_OPERATION_TIMEOUT, future_operation.into_future()).await;
    if result.is_err() {
        let _ = cancel(&operation);
    }
    let _ = close(&operation);
    result
        .map_err(|_| "cleanup operation timed out and was cancelled".to_string())?
        .map_err(|error| error.to_string())
}

impl WinrtSession {
    async fn open(
        device_id: &str,
        reporter: StageReporter,
        cancelled: &mut watch::Receiver<bool>,
    ) -> Result<Self, String> {
        ensure_active(cancelled, "address-to-device")?;
        let address = parse_bluetooth_address(device_id).ok_or_else(|| {
            reporter.record_immediate(
                SessionStage::AddressToDevice,
                1,
                StageResultClass::NativeError,
            );
            "Windows WinRT address-to-device failed: the Bluetooth address was not recognized"
                .to_string()
        })?;
        let device = await_winrt_stage(
            WinrtStageCall::new(reporter, SessionStage::AddressToDevice, 1, OPEN_TIMEOUT),
            BluetoothLEDevice::FromBluetoothAddressAsync(address),
            |operation| operation.Cancel(),
            |operation| operation.Close(),
            cancelled,
        )
        .await?;
        ensure_active(cancelled, "address-to-device")?;
        let bluetooth_device_id = device
            .BluetoothDeviceId()
            .map_err(|error| stage_error(SessionStage::GattSessionCreate.name(), error))?;
        let gatt_session = await_winrt_stage(
            WinrtStageCall::new(reporter, SessionStage::GattSessionCreate, 1, OPEN_TIMEOUT),
            GattSession::FromDeviceIdAsync(&bluetooth_device_id),
            |operation| operation.Cancel(),
            |operation| operation.Close(),
            cancelled,
        )
        .await?;
        ensure_active(cancelled, "gatt-session-create")?;
        run_sync_stage(reporter, SessionStage::MaintainConnection, || {
            gatt_session
                .SetMaintainConnection(true)
                .map_err(|error| error.to_string())
        })
        .map_err(|error| error.to_string())?;

        // Keep PMD qualification on Windows' system-managed connection timing,
        // matching the exact reference that delivers physical ECG and ACC.
        // Throughput optimization is requested only after both first frames.
        let opening = OpeningSession::new(device, gatt_session, None);
        let pmd_service = required_service(
            opening.device(),
            PMD_SERVICE,
            "PMD service",
            SessionStage::PmdServiceDiscovery,
            reporter,
            cancelled,
        )
        .await?;
        ensure_active(cancelled, "PMD service discovery")?;
        let control = required_characteristic(
            &pmd_service,
            PMD_CONTROL_POINT,
            "PMD control point",
            CharacteristicStages {
                access: SessionStage::PmdControlAccess,
                uncached: SessionStage::PmdControlDiscoveryUncached,
                cached: SessionStage::PmdControlDiscoveryCached,
            },
            reporter,
            cancelled,
        )
        .await?;
        ensure_active(cancelled, "PMD control discovery")?;
        let pmd_data = required_characteristic(
            &pmd_service,
            PMD_DATA,
            "PMD data",
            CharacteristicStages {
                access: SessionStage::PmdDataAccess,
                uncached: SessionStage::PmdDataDiscoveryUncached,
                cached: SessionStage::PmdDataDiscoveryCached,
            },
            reporter,
            cancelled,
        )
        .await?;
        ensure_active(cancelled, "PMD data discovery")?;

        let heart_rate_service = optional_service(
            opening.device(),
            HEART_RATE_SERVICE,
            SessionStage::HeartRateServiceDiscovery,
            reporter,
            cancelled,
        )
        .await;
        ensure_active(cancelled, "heart-rate service discovery")?;
        let heart_rate = if let Some(service) = &heart_rate_service {
            optional_characteristic(
                service,
                HEART_RATE_MEASUREMENT,
                CharacteristicStages {
                    access: SessionStage::HeartRateAccess,
                    uncached: SessionStage::HeartRateDiscoveryUncached,
                    cached: SessionStage::HeartRateDiscoveryCached,
                },
                reporter,
                cancelled,
            )
            .await
        } else {
            None
        };
        ensure_active(cancelled, "heart-rate characteristic discovery")?;
        let battery_service = optional_service(
            opening.device(),
            uuid::Uuid::from_u128(0x0000180f_0000_1000_8000_00805f9b34fb),
            SessionStage::BatteryServiceDiscovery,
            reporter,
            cancelled,
        )
        .await;
        ensure_active(cancelled, "battery service discovery")?;
        let battery = if let Some(service) = &battery_service {
            optional_characteristic(
                service,
                BATTERY_LEVEL,
                CharacteristicStages {
                    access: SessionStage::BatteryAccess,
                    uncached: SessionStage::BatteryDiscoveryUncached,
                    cached: SessionStage::BatteryDiscoveryCached,
                },
                reporter,
                cancelled,
            )
            .await
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

    fn request_preferred_connection(&mut self, reporter: StageReporter) -> Result<(), String> {
        let request = run_sync_stage(reporter, SessionStage::PreferredConnectionRequest, || {
            Ok(
                BluetoothLEPreferredConnectionParameters::ThroughputOptimized()
                    .ok()
                    .and_then(|parameters| {
                        self.device
                            .RequestPreferredConnectionParameters(&parameters)
                            .ok()
                    }),
            )
        })
        .map_err(|error| error.to_string())?;
        self.preferred_request = request;
        Ok(())
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
        stage: SessionStage,
        reporter: StageReporter,
        cancelled: &mut watch::Receiver<bool>,
        sink: NotificationSink,
    ) -> Result<(), String> {
        let NotificationSink { raw_tx, fault_tx } = sink;
        let cccd = if characteristic
            .CharacteristicProperties()
            .map_err(|error| stage_error("notification properties", error))?
            .contains(GattCharacteristicProperties::Indicate)
        {
            GattClientCharacteristicConfigurationDescriptorValue::Indicate
        } else {
            GattClientCharacteristicConfigurationDescriptorValue::Notify
        };
        let status = await_winrt_stage(
            WinrtStageCall::new(reporter, stage, 1, GATT_TIMEOUT),
            characteristic.WriteClientCharacteristicConfigurationDescriptorAsync(cccd),
            |operation| operation.Cancel(),
            |operation| operation.Close(),
            cancelled,
        )
        .await;
        match status {
            Ok(GattCommunicationStatus::Success) => {
                // Match the proven Windows reference lifecycle: commit the
                // CCCD first, then attach the WinRT event handler before any
                // PMD command can produce data. Registering the handler first
                // yielded accepted PMD start responses but no data callbacks
                // on a physical H10. The non-Send delegate remains confined to
                // this synchronous scope so later awaits stay Send.
                let token_result = {
                    let handler =
                        TypedEventHandler::<GattCharacteristic, GattValueChangedEventArgs>::new(
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
                    characteristic.ValueChanged(&handler)
                };
                let token = match token_result {
                    Ok(token) => token,
                    Err(error) => {
                        let rollback = characteristic
                            .WriteClientCharacteristicConfigurationDescriptorAsync(
                                GattClientCharacteristicConfigurationDescriptorValue::None,
                            );
                        let _ = await_cleanup_operation(
                            rollback,
                            |operation| operation.Cancel(),
                            |operation| operation.Close(),
                        )
                        .await;
                        return Err(stage_error("notification handler", error));
                    }
                };
                self.subscriptions.push(Subscription {
                    characteristic: characteristic.clone(),
                    source: source.subscription_kind(),
                    token: Some(token),
                });
                self.cleanup.record_subscription(source.subscription_kind());
                Ok(())
            }
            Ok(status) => Err(format!(
                "Windows WinRT notification subscription failed: {status:?}"
            )),
            Err(error) => Err(error),
        }
    }

    async fn write_control(
        &self,
        bytes: &[u8],
        stage: SessionStage,
        reporter: StageReporter,
        cancelled: &mut watch::Receiver<bool>,
    ) -> Result<(), String> {
        let operation = (|| {
            let writer = DataWriter::new()?;
            writer.WriteBytes(bytes)?;
            let buffer = writer.DetachBuffer()?;
            self.control.WriteValueWithResultAsync(&buffer)
        })();
        let result = await_winrt_stage(
            WinrtStageCall::new(reporter, stage, 1, GATT_TIMEOUT),
            operation,
            |operation| operation.Cancel(),
            |operation| operation.Close(),
            cancelled,
        )
        .await?;
        let status = result
            .Status()
            .map_err(|error| stage_error(stage.name(), error))?;
        if status != GattCommunicationStatus::Success {
            return Err(format!("Windows WinRT {} failed: {status:?}", stage.name()));
        }
        Ok(())
    }

    async fn read_battery(
        &self,
        reporter: StageReporter,
        cancelled: &mut watch::Receiver<bool>,
    ) -> Option<u8> {
        let characteristic = self.battery.as_ref()?;
        let result = await_winrt_stage(
            WinrtStageCall::new(reporter, SessionStage::BatteryRead, 1, GATT_TIMEOUT),
            characteristic.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached),
            |operation| operation.Cancel(),
            |operation| operation.Close(),
            cancelled,
        )
        .await
        .ok()?;
        if result.Status().ok()? != GattCommunicationStatus::Success {
            return None;
        }
        let value = result.Value().ok()?;
        buffer_to_vec(&value).ok()?.first().copied()
    }

    async fn write_control_cleanup(&self, bytes: &[u8]) {
        let operation = (|| {
            let writer = DataWriter::new()?;
            writer.WriteBytes(bytes)?;
            let buffer = writer.DetachBuffer()?;
            self.control.WriteValueWithResultAsync(&buffer)
        })();
        let _ = await_cleanup_operation(
            operation,
            |operation| operation.Cancel(),
            |operation| operation.Close(),
        )
        .await;
    }

    async fn disable_subscription(subscription: &mut Subscription) {
        subscription.remove_handler();
        let operation = subscription
            .characteristic
            .WriteClientCharacteristicConfigurationDescriptorAsync(
                GattClientCharacteristicConfigurationDescriptorValue::None,
            );
        let _ = await_cleanup_operation(
            operation,
            |operation| operation.Cancel(),
            |operation| operation.Close(),
        )
        .await;
    }

    fn close_native_handles(&mut self) {
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

    async fn shutdown(&mut self) {
        let Some(plan) = self.cleanup.begin() else {
            return;
        };
        debug_assert_eq!(
            plan.subscriptions,
            self.subscriptions
                .iter()
                .rev()
                .map(|subscription| subscription.source)
                .collect::<Vec<_>>()
        );
        debug_assert!(plan.close_session);

        self.write_control_cleanup(&stop_command(ECG_MEASUREMENT))
            .await;
        self.write_control_cleanup(&stop_command(ACC_MEASUREMENT))
            .await;
        while let Some(mut subscription) = self.subscriptions.pop() {
            Self::disable_subscription(&mut subscription).await;
        }
        self.close_native_handles();
    }
}

impl Drop for WinrtSession {
    fn drop(&mut self) {
        let Some(plan) = self.cleanup.begin() else {
            return;
        };
        debug_assert_eq!(
            plan.subscriptions,
            self.subscriptions
                .iter()
                .rev()
                .map(|subscription| subscription.source)
                .collect::<Vec<_>>()
        );
        debug_assert!(plan.close_session);
        self.subscriptions.clear();
        self.close_native_handles();
    }
}

pub(super) async fn prepare(
    device_id: &str,
    device_name: String,
    event_tx: mpsc::Sender<InputEvent>,
    cancelled: &mut watch::Receiver<bool>,
) -> Result<PreparedConnection, String> {
    let reporter = StageReporter::new(
        std::env::var_os(SESSION_DIAGNOSTICS_ENV).is_some(),
        Some(tokio::time::Instant::now() + SESSION_SETUP_TIMEOUT),
    );
    let (raw_tx, mut raw_rx) = mpsc::channel(RAW_NOTIFICATION_CAPACITY);
    let (fault_tx, mut fault_rx) = watch::channel(None::<String>);
    let mut session = WinrtSession::open(device_id, reporter, cancelled).await?;
    let mut gate = FirstFrameGate::default();
    let mut frame_stages = FirstFrameStages::new(reporter);

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
                    SessionStage::HeartRateNotification,
                    reporter,
                    cancelled,
                    NotificationSink {
                        raw_tx: raw_tx.clone(),
                        fault_tx: fault_tx.clone(),
                    },
                )
                .await?;
        }
        ensure_active(cancelled, "PMD control subscription")?;
        session
            .subscribe(
                &session.control.clone(),
                NotificationSource::PmdControl,
                SessionStage::PmdControlNotification,
                reporter,
                cancelled,
                NotificationSink {
                    raw_tx: raw_tx.clone(),
                    fault_tx: fault_tx.clone(),
                },
            )
            .await?;
        ensure_active(cancelled, "PMD control subscription")?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        ensure_active(cancelled, "PMD data subscription")?;
        session
            .subscribe(
                &session.pmd_data.clone(),
                NotificationSource::PmdData,
                SessionStage::PmdDataNotification,
                reporter,
                cancelled,
                NotificationSink { raw_tx, fault_tx },
            )
            .await?;
        ensure_active(cancelled, "PMD data subscription")?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        ensure_active(cancelled, "PMD setup")?;

        send_status(
            &event_tx,
            "starting",
            "Qualifying ECG at 130 Hz before starting the three-axis accelerometer at 200 Hz…"
                .to_string(),
        )
        .await?;
        session
            .write_control(
                &request_settings_command(ECG_MEASUREMENT),
                SessionStage::RequestEcgSettings,
                reporter,
                cancelled,
            )
            .await?;
        SetupWaitContext {
            raw_rx: &mut raw_rx,
            fault_rx: &mut fault_rx,
            cancelled: &mut *cancelled,
            reporter,
            gate: &mut gate,
            frame_stages: &mut frame_stages,
        }
        .pmd_control_response(
            SessionStage::EcgSettingsResponse,
            ExpectedControlResponse::ECG_SETTINGS,
            PMD_RESPONSE_TIMEOUT,
        )
        .await?;
        session
            .write_control(
                &start_ecg_command(),
                SessionStage::StartEcg,
                reporter,
                cancelled,
            )
            .await?;
        SetupWaitContext {
            raw_rx: &mut raw_rx,
            fault_rx: &mut fault_rx,
            cancelled: &mut *cancelled,
            reporter,
            gate: &mut gate,
            frame_stages: &mut frame_stages,
        }
        .pmd_control_response(
            SessionStage::StartEcgResponse,
            ExpectedControlResponse::ECG_START,
            PMD_RESPONSE_TIMEOUT,
        )
        .await?;
        SetupWaitContext {
            raw_rx: &mut raw_rx,
            fault_rx: &mut fault_rx,
            cancelled: &mut *cancelled,
            reporter,
            gate: &mut gate,
            frame_stages: &mut frame_stages,
        }
        .first_frame(FirstFrameKind::Ecg, FIRST_STREAM_FRAME_TIMEOUT)
        .await?;
        ensure_active(cancelled, "start accelerometer")?;
        session
            .write_control(
                &start_accelerometer_command(),
                SessionStage::StartAcc,
                reporter,
                cancelled,
            )
            .await?;
        SetupWaitContext {
            raw_rx: &mut raw_rx,
            fault_rx: &mut fault_rx,
            cancelled: &mut *cancelled,
            reporter,
            gate: &mut gate,
            frame_stages: &mut frame_stages,
        }
        .pmd_control_response(
            SessionStage::StartAccResponse,
            ExpectedControlResponse::ACC_START,
            PMD_RESPONSE_TIMEOUT,
        )
        .await?;
        SetupWaitContext {
            raw_rx: &mut raw_rx,
            fault_rx: &mut fault_rx,
            cancelled: &mut *cancelled,
            reporter,
            gate: &mut gate,
            frame_stages: &mut frame_stages,
        }
        .first_frame(FirstFrameKind::Acc, FIRST_STREAM_FRAME_TIMEOUT)
        .await?;
        ensure_active(cancelled, "preferred connection request")?;
        session.request_preferred_connection(reporter)?;
        send_status(&event_tx, "optimizing", session.link_summary()).await?;
        Ok::<(), String>(())
    }
    .await
    {
        frame_stages.finish_pending(StageResultClass::NativeError);
        session.shutdown().await;
        return Err(error);
    }

    if let Err(error) = ensure_active(cancelled, "battery read") {
        session.shutdown().await;
        return Err(error);
    }
    let battery_percent = session.read_battery(reporter, cancelled).await;
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
    stage: SessionStage,
    reporter: StageReporter,
    cancelled: &mut watch::Receiver<bool>,
) -> Result<GattDeviceService, String> {
    service(device, uuid, stage, reporter, cancelled)
        .await?
        .ok_or_else(|| format!("Windows WinRT discovery failed: {label} was not exposed"))
}

async fn optional_service(
    device: &BluetoothLEDevice,
    uuid: uuid::Uuid,
    stage: SessionStage,
    reporter: StageReporter,
    cancelled: &mut watch::Receiver<bool>,
) -> Option<GattDeviceService> {
    service(device, uuid, stage, reporter, cancelled)
        .await
        .ok()
        .flatten()
}

async fn service(
    device: &BluetoothLEDevice,
    uuid: uuid::Uuid,
    stage: SessionStage,
    reporter: StageReporter,
    cancelled: &mut watch::Receiver<bool>,
) -> Result<Option<GattDeviceService>, String> {
    ensure_active(cancelled, "service discovery")?;
    let result = await_winrt_stage(
        WinrtStageCall::new(reporter, stage, 1, DISCOVERY_TIMEOUT),
        device.GetGattServicesForUuidWithCacheModeAsync(
            GUID::from_u128(uuid.as_u128()),
            BluetoothCacheMode::Uncached,
        ),
        |operation| operation.Cancel(),
        |operation| operation.Close(),
        cancelled,
    )
    .await?;
    let status = result
        .Status()
        .map_err(|error| stage_error(stage.name(), error))?;
    if status != GattCommunicationStatus::Success {
        return Err(format!("Windows WinRT {} failed: {status:?}", stage.name()));
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
    stages: CharacteristicStages,
    reporter: StageReporter,
    cancelled: &mut watch::Receiver<bool>,
) -> Result<GattCharacteristic, String> {
    characteristic(service, uuid, stages, reporter, cancelled)
        .await?
        .ok_or_else(|| format!("Windows WinRT discovery failed: {label} was not exposed"))
}

async fn optional_characteristic(
    service: &GattDeviceService,
    uuid: uuid::Uuid,
    stages: CharacteristicStages,
    reporter: StageReporter,
    cancelled: &mut watch::Receiver<bool>,
) -> Option<GattCharacteristic> {
    characteristic(service, uuid, stages, reporter, cancelled)
        .await
        .ok()
        .flatten()
}

async fn characteristic(
    service: &GattDeviceService,
    uuid: uuid::Uuid,
    stages: CharacteristicStages,
    reporter: StageReporter,
    cancelled: &mut watch::Receiver<bool>,
) -> Result<Option<GattCharacteristic>, String> {
    let guid = GUID::from_u128(uuid.as_u128());
    let mut last_error = "characteristic did not become reachable".to_string();

    for attempt in 0..CHARACTERISTIC_ATTEMPTS {
        ensure_active(cancelled, "characteristic discovery")?;
        let access = await_winrt_stage(
            WinrtStageCall::new(reporter, stages.access, attempt + 1, GATT_TIMEOUT),
            service.RequestAccessAsync(),
            |operation| operation.Cancel(),
            |operation| operation.Close(),
            cancelled,
        )
        .await?;
        if access != DeviceAccessStatus::Allowed {
            last_error = format!("service access returned {access:?}");
        } else {
            let result = await_winrt_stage(
                WinrtStageCall::new(reporter, stages.uncached, attempt + 1, GATT_TIMEOUT),
                service.GetCharacteristicsForUuidWithCacheModeAsync(
                    guid,
                    BluetoothCacheMode::Uncached,
                ),
                |operation| operation.Cancel(),
                |operation| operation.Close(),
                cancelled,
            )
            .await?;
            let status = result
                .Status()
                .map_err(|error| stage_error(stages.uncached.name(), error))?;
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
                if let Some(cached) =
                    cached_characteristic(service, guid, stages.cached, reporter, cancelled).await?
                {
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
    stage: SessionStage,
    reporter: StageReporter,
    cancelled: &mut watch::Receiver<bool>,
) -> Result<Option<GattCharacteristic>, String> {
    ensure_active(cancelled, "cached characteristic discovery")?;
    let result = await_winrt_stage(
        WinrtStageCall::new(reporter, stage, 1, GATT_TIMEOUT),
        service.GetCharacteristicsForUuidWithCacheModeAsync(guid, BluetoothCacheMode::Cached),
        |operation| operation.Cancel(),
        |operation| operation.Close(),
        cancelled,
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

fn take_subscription_token(token: &mut Option<i64>) -> Option<i64> {
    token.take()
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

fn validate_control_response(
    expected: ExpectedControlResponse,
    response: PmdControlResponse,
) -> Result<(), String> {
    if response.opcode != expected.opcode || response.measurement != expected.measurement {
        return Err(format!(
            "Windows WinRT PMD control response arrived out of order: expected opcode 0x{:02x}/measurement 0x{:02x}, received 0x{:02x}/0x{:02x}",
            expected.opcode, expected.measurement, response.opcode, response.measurement
        ));
    }
    if response.error_code != 0 {
        return Err(format!(
            "Windows WinRT PMD control response rejected opcode 0x{:02x}/measurement 0x{:02x} with error 0x{:02x}",
            response.opcode, response.measurement, response.error_code
        ));
    }
    Ok(())
}

fn observe_setup_notification(
    raw: RawNotification,
    gate: &mut FirstFrameGate,
    frame_stages: &mut FirstFrameStages,
) -> Result<Option<PmdControlResponse>, String> {
    if raw.source == NotificationSource::PmdControl {
        return decode_pmd_control_response(&raw.value)
            .map(Some)
            .map_err(|error| {
                format!("Windows WinRT received a malformed PMD control response: {error}")
            });
    }

    let event = decode_notification(raw).ok_or_else(|| {
        "Windows WinRT notification did not map to a setup observation".to_string()
    })?;
    match &event {
        InputEvent::Ecg { microvolts, .. } if !microvolts.is_empty() => {
            frame_stages.observe(FirstFrameKind::Ecg);
        }
        InputEvent::Accelerometer { samples, .. } if !samples.is_empty() => {
            frame_stages.observe(FirstFrameKind::Acc);
        }
        _ => {}
    }
    gate.push(event)?;
    Ok(None)
}

struct SetupWaitContext<'a> {
    raw_rx: &'a mut mpsc::Receiver<RawNotification>,
    fault_rx: &'a mut watch::Receiver<Option<String>>,
    cancelled: &'a mut watch::Receiver<bool>,
    reporter: StageReporter,
    gate: &'a mut FirstFrameGate,
    frame_stages: &'a mut FirstFrameStages,
}

impl SetupWaitContext<'_> {
    async fn pmd_control_response(
        self,
        stage: SessionStage,
        expected: ExpectedControlResponse,
        timeout: Duration,
    ) -> Result<(), String> {
        let span = self.reporter.enter(stage, 1);
        if *self.cancelled.borrow() {
            span.finish(StageResultClass::Cancelled);
            return Err(
                "Windows WinRT setup was cancelled while awaiting a PMD control response"
                    .to_string(),
            );
        }
        let deadline = tokio::time::sleep(self.reporter.limit(timeout));
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                changed = self.cancelled.changed() => {
                    span.finish(StageResultClass::Cancelled);
                    return Err(if changed.is_err() {
                        "Windows WinRT setup cancellation owner closed while awaiting a PMD control response".to_string()
                    } else {
                        "Windows WinRT setup was cancelled while awaiting a PMD control response".to_string()
                    });
                }
                changed = self.fault_rx.changed() => {
                    if changed.is_err() {
                        span.finish(StageResultClass::NativeError);
                        return Err("Windows WinRT notification fault channel closed while awaiting a PMD control response".to_string());
                    }
                    if let Some(error) = self.fault_rx.borrow().clone() {
                        span.finish(StageResultClass::NativeError);
                        return Err(error);
                    }
                }
                raw = self.raw_rx.recv() => {
                    let Some(raw) = raw else {
                        span.finish(StageResultClass::NativeError);
                        return Err("Windows WinRT notifications ended while awaiting a PMD control response".to_string());
                    };
                    let response = match observe_setup_notification(raw, self.gate, self.frame_stages) {
                        Ok(response) => response,
                        Err(error) => {
                            span.finish(StageResultClass::NativeError);
                            return Err(error);
                        }
                    };
                    if let Some(response) = response {
                        if let Err(error) = validate_control_response(expected, response) {
                            span.finish(StageResultClass::NativeError);
                            return Err(error);
                        }
                        span.finish(StageResultClass::Success);
                        return Ok(());
                    }
                }
                () = &mut deadline => {
                    span.finish(StageResultClass::Timeout);
                    return Err(format!("Windows WinRT {} timed out", stage.name()));
                }
            }
        }
    }

    async fn first_frame(self, kind: FirstFrameKind, timeout: Duration) -> Result<(), String> {
        let stage = match kind {
            FirstFrameKind::Ecg => SessionStage::FirstEcgFrame,
            FirstFrameKind::Acc => SessionStage::FirstAccFrame,
        };
        self.frame_stages.begin(kind);
        if self.gate.saw(kind) {
            return Ok(());
        }
        if *self.cancelled.borrow() {
            self.frame_stages.finish(kind, StageResultClass::Cancelled);
            return Err(format!(
                "Windows WinRT setup was cancelled during {}",
                stage.name()
            ));
        }

        let deadline = tokio::time::sleep(self.reporter.limit(timeout));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                changed = self.cancelled.changed() => {
                    self.frame_stages.finish(kind, StageResultClass::Cancelled);
                    return Err(if changed.is_err() {
                        format!("Windows WinRT setup cancellation owner closed during {}", stage.name())
                    } else {
                        format!("Windows WinRT setup was cancelled during {}", stage.name())
                    });
                }
                changed = self.fault_rx.changed() => {
                    if changed.is_err() {
                        self.frame_stages.finish(kind, StageResultClass::NativeError);
                        return Err(format!("Windows WinRT notification fault channel closed during {}", stage.name()));
                    }
                    if let Some(error) = self.fault_rx.borrow().clone() {
                        self.frame_stages.finish(kind, StageResultClass::NativeError);
                        return Err(error);
                    }
                }
                raw = self.raw_rx.recv() => {
                    let Some(raw) = raw else {
                        self.frame_stages.finish(kind, StageResultClass::NativeError);
                        return Err(format!("Windows WinRT notifications ended during {}", stage.name()));
                    };
                    match observe_setup_notification(raw, self.gate, self.frame_stages) {
                        Ok(None) if self.gate.saw(kind) => return Ok(()),
                        Ok(None) => {}
                        Ok(Some(response)) => {
                            self.frame_stages.finish(kind, StageResultClass::NativeError);
                            return Err(format!(
                                "Windows WinRT received an unexpected PMD control response during {}: opcode 0x{:02x}/measurement 0x{:02x}",
                                stage.name(), response.opcode, response.measurement
                            ));
                        }
                        Err(error) => {
                            self.frame_stages.finish(kind, StageResultClass::NativeError);
                            return Err(error);
                        }
                    }
                }
                () = &mut deadline => {
                    self.frame_stages.finish(kind, StageResultClass::Timeout);
                    return Err(format!("Windows WinRT {} timed out", stage.name()));
                }
            }
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

    fn saw(&self, kind: FirstFrameKind) -> bool {
        match kind {
            FirstFrameKind::Ecg => self.saw_ecg,
            FirstFrameKind::Acc => self.saw_acc,
        }
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

    #[test]
    fn notification_callback_token_is_removed_exactly_once() {
        let mut token = Some(17);
        assert_eq!(take_subscription_token(&mut token), Some(17));
        assert_eq!(take_subscription_token(&mut token), None);
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

    fn control_response(expected: ExpectedControlResponse, error_code: u8) -> RawNotification {
        RawNotification {
            source: NotificationSource::PmdControl,
            value: vec![
                polar_h10_core::PMD_RESPONSE_FRAME,
                expected.opcode,
                expected.measurement,
                error_code,
            ],
        }
    }

    #[test]
    fn pmd_control_responses_reject_error_and_wrong_order() {
        for expected in [
            ExpectedControlResponse::ECG_SETTINGS,
            ExpectedControlResponse::ECG_START,
            ExpectedControlResponse::ACC_START,
        ] {
            assert!(
                validate_control_response(
                    expected,
                    PmdControlResponse {
                        opcode: expected.opcode,
                        measurement: expected.measurement,
                        error_code: 0,
                    }
                )
                .is_ok()
            );
            assert!(
                validate_control_response(
                    expected,
                    PmdControlResponse {
                        opcode: expected.opcode,
                        measurement: expected.measurement,
                        error_code: 0x0a,
                    }
                )
                .unwrap_err()
                .contains("error 0x0a")
            );
        }

        assert!(
            validate_control_response(
                ExpectedControlResponse::ECG_START,
                PmdControlResponse {
                    opcode: PMD_START_STREAM_OPCODE,
                    measurement: ACC_MEASUREMENT,
                    error_code: 0,
                }
            )
            .unwrap_err()
            .contains("out of order")
        );
    }

    #[tokio::test]
    async fn pmd_response_wait_covers_success_malformed_order_error_timeout_and_cancel() {
        async fn run(
            raw: Option<RawNotification>,
            cancelled_initially: bool,
        ) -> Result<(), String> {
            let (raw_tx, mut raw_rx) = mpsc::channel(2);
            if let Some(raw) = raw {
                raw_tx.send(raw).await.unwrap();
            }
            let (_fault_tx, mut fault_rx) = watch::channel(None);
            let (cancel_tx, mut cancelled) = watch::channel(false);
            if cancelled_initially {
                cancel_tx.send_replace(true);
            }
            let mut gate = FirstFrameGate::default();
            let reporter = StageReporter::new(false, None);
            let mut stages = FirstFrameStages::new(reporter);
            SetupWaitContext {
                raw_rx: &mut raw_rx,
                fault_rx: &mut fault_rx,
                cancelled: &mut cancelled,
                reporter,
                gate: &mut gate,
                frame_stages: &mut stages,
            }
            .pmd_control_response(
                SessionStage::StartEcgResponse,
                ExpectedControlResponse::ECG_START,
                Duration::from_millis(1),
            )
            .await
        }

        assert!(
            run(
                Some(control_response(ExpectedControlResponse::ECG_START, 0)),
                false
            )
            .await
            .is_ok()
        );
        assert!(
            run(
                Some(RawNotification {
                    source: NotificationSource::PmdControl,
                    value: vec![0xf0, PMD_START_STREAM_OPCODE],
                }),
                false
            )
            .await
            .unwrap_err()
            .contains("malformed")
        );
        assert!(
            run(
                Some(control_response(ExpectedControlResponse::ACC_START, 0)),
                false
            )
            .await
            .unwrap_err()
            .contains("out of order")
        );
        assert!(
            run(
                Some(control_response(ExpectedControlResponse::ECG_START, 0x0a)),
                false
            )
            .await
            .unwrap_err()
            .contains("error 0x0a")
        );
        assert!(run(None, false).await.unwrap_err().contains("timed out"));
        assert!(run(None, true).await.unwrap_err().contains("cancelled"));
    }

    #[tokio::test]
    async fn first_frame_wait_accepts_sensor_data_and_rejects_control_reordering() {
        async fn run(raw: RawNotification) -> Result<(), String> {
            let (raw_tx, mut raw_rx) = mpsc::channel(1);
            raw_tx.send(raw).await.unwrap();
            let (_fault_tx, mut fault_rx) = watch::channel(None);
            let (_cancel_tx, mut cancelled) = watch::channel(false);
            let reporter = StageReporter::new(false, None);
            let mut gate = FirstFrameGate::default();
            let mut stages = FirstFrameStages::new(reporter);
            SetupWaitContext {
                raw_rx: &mut raw_rx,
                fault_rx: &mut fault_rx,
                cancelled: &mut cancelled,
                reporter,
                gate: &mut gate,
                frame_stages: &mut stages,
            }
            .first_frame(FirstFrameKind::Ecg, Duration::from_millis(5))
            .await
        }

        let mut ecg = vec![ECG_MEASUREMENT];
        ecg.extend_from_slice(&1_u64.to_le_bytes());
        ecg.push(0);
        ecg.extend_from_slice(&[1, 0, 0]);
        assert!(
            run(RawNotification {
                source: NotificationSource::PmdData,
                value: ecg,
            })
            .await
            .is_ok()
        );
        assert!(
            run(control_response(ExpectedControlResponse::ACC_START, 0))
                .await
                .unwrap_err()
                .contains("unexpected PMD control response")
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
