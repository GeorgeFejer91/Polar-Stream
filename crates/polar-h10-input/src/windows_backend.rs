//! Direct Windows WinRT Bluetooth backend.
//!
//! Windows uses an active WinRT advertisement watcher for bounded discovery,
//! then owns one persistent `GattSession`, all characteristic subscriptions,
//! and teardown. Other platforms retain the cross-platform `btleplug` path.

use std::{
    collections::{HashMap, VecDeque},
    ffi::OsStr,
    future::IntoFuture,
    marker::PhantomData,
    rc::Rc,
    sync::{Arc, Mutex, Weak},
    thread,
    time::{Duration, Instant},
};

use futures_util::{StreamExt, stream};
use polar_h10_core::{
    ACC_MEASUREMENT, ECG_MEASUREMENT, PMD_GET_SETTINGS_OPCODE, PMD_START_STREAM_OPCODE,
    PmdControlResponse, PmdFrame, decode_heart_rate, decode_pmd, decode_pmd_control_response,
    request_settings_command, start_accelerometer_command, start_ecg_command, stop_command,
};
use tokio::sync::{mpsc, oneshot, watch};
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
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    core::{GUID, Ref},
};
use windows_future::IAsyncOperation;

use super::{
    BATTERY_LEVEL, DeviceSummary, HEART_RATE_MEASUREMENT, HEART_RATE_SERVICE, InputEvent,
    InputManager, PMD_CONTROL_POINT, PMD_DATA, PMD_SERVICE, parse_bluetooth_address,
    windows_session_lifecycle::{
        FirstFrameKind, FirstFrameStages, SessionCleanup, SessionStage, StageControl,
        StageReporter, StageResultClass, SubscriptionKind, run_controlled_stage, run_sync_stage,
    },
};

const SCAN_DURATION: Duration = Duration::from_secs(15);
const SCAN_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_SCANNED_DEVICES: usize = 256;
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECTION_SETTLE: Duration = Duration::from_millis(500);
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
const SESSION_PROFILE_ENV: &str = "POLAR_STREAM_H10_SESSION_PROFILE";
const PMD_ONLY_PROFILE: &str = "pmd-only-differential";
const PMD_RETAIN_SUCCESS_PROFILE: &str = "pmd-only-retain-successful-gatt-operations";
const PMD_WHEN_COMPLETION_PROFILE: &str = "pmd-only-winrt-when-completion";
const PROPERTY_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(5);
const PROPERTY_SWEEP_TIMEOUT: Duration = Duration::from_secs(6);
const PROPERTY_CONFIRMATION_CONCURRENCY: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionProfile {
    ReferenceCompatible,
    PmdOnlyDifferential,
    PmdOnlyRetainSuccessfulGattOperations,
    PmdOnlyWinrtWhenCompletion,
}

impl SessionProfile {
    fn parse(value: Option<&OsStr>) -> Result<Self, String> {
        match value {
            None => Ok(Self::ReferenceCompatible),
            Some(value) if value == OsStr::new(PMD_ONLY_PROFILE) => Ok(Self::PmdOnlyDifferential),
            Some(value) if value == OsStr::new(PMD_RETAIN_SUCCESS_PROFILE) => {
                Ok(Self::PmdOnlyRetainSuccessfulGattOperations)
            }
            Some(value) if value == OsStr::new(PMD_WHEN_COMPLETION_PROFILE) => {
                Ok(Self::PmdOnlyWinrtWhenCompletion)
            }
            Some(_) => Err(format!(
                "Windows H10 session profile is invalid; {SESSION_PROFILE_ENV} must be exactly {PMD_ONLY_PROFILE}, {PMD_RETAIN_SUCCESS_PROFILE}, or {PMD_WHEN_COMPLETION_PROFILE} when set"
            )),
        }
    }

    fn from_environment() -> Result<Self, String> {
        let value = std::env::var_os(SESSION_PROFILE_ENV);
        Self::parse(value.as_deref())
    }

    const fn heart_rate_enabled(self) -> bool {
        matches!(self, Self::ReferenceCompatible)
    }

    const fn close_successful_gatt_operations(self) -> bool {
        !matches!(
            self,
            Self::PmdOnlyRetainSuccessfulGattOperations | Self::PmdOnlyWinrtWhenCompletion
        )
    }

    const fn use_winrt_when_completion(self) -> bool {
        matches!(self, Self::PmdOnlyWinrtWhenCompletion)
    }

    const fn name(self) -> &'static str {
        match self {
            Self::ReferenceCompatible => "reference-compatible",
            Self::PmdOnlyDifferential => PMD_ONLY_PROFILE,
            Self::PmdOnlyRetainSuccessfulGattOperations => PMD_RETAIN_SUCCESS_PROFILE,
            Self::PmdOnlyWinrtWhenCompletion => PMD_WHEN_COMPLETION_PROFILE,
        }
    }
}

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

    fn has_exact_name_candidate(&self) -> bool {
        self.devices
            .values()
            .any(|device| device.name.as_deref().is_some_and(is_polar_h10_name))
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
    let scan_deadline = Instant::now() + SCAN_DURATION;
    loop {
        let exact_name_observed = observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .has_exact_name_candidate();
        if exact_name_observed {
            break;
        }
        let remaining = scan_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(SCAN_POLL_INTERVAL.min(remaining));
    }
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

struct WinrtApartment(PhantomData<Rc<()>>);

impl WinrtApartment {
    fn initialize_mta() -> Result<Self, String> {
        // SAFETY: this runs once at the start of a newly created, dedicated
        // session thread. `WinrtApartment::drop` balances the successful call
        // on that same thread after every WinRT handle and callback is closed.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .map(|()| Self(PhantomData))
            .map_err(|error| format!("Could not initialize the Windows Runtime apartment: {error}"))
    }
}

impl Drop for WinrtApartment {
    fn drop(&mut self) {
        // SAFETY: this guard is neither Send nor moved after construction; it
        // is dropped on the same dedicated thread that called `RoInitialize`.
        unsafe { RoUninitialize() };
    }
}

struct SessionThreadFinished(watch::Sender<bool>);

impl Drop for SessionThreadFinished {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

fn build_session_runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("Could not initialize the Windows H10 session runtime: {error}"))
}

pub(super) async fn spawn_session(
    device_id: String,
    device_name: String,
    event_tx: mpsc::Sender<InputEvent>,
    manager: Weak<InputManager>,
    generation: u64,
    mut cancelled: watch::Receiver<bool>,
    finished: watch::Sender<bool>,
) -> Result<(), String> {
    let (setup_tx, setup_rx) = oneshot::channel();
    thread::Builder::new()
        .name("polar-h10-winrt-session".to_string())
        .spawn(move || {
            let _finished_guard = SessionThreadFinished(finished.clone());
            let _apartment = match WinrtApartment::initialize_mta() {
                Ok(apartment) => apartment,
                Err(error) => {
                    let _ = setup_tx.send(Err(error));
                    return;
                }
            };
            let runtime = match build_session_runtime() {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = setup_tx.send(Err(error));
                    return;
                }
            };
            runtime.block_on(async move {
                match prepare(&device_id, device_name, event_tx, &mut cancelled).await {
                    Ok(mut prepared) => {
                        if setup_tx.send(Ok(())).is_err() {
                            prepared.session.shutdown().await;
                            if let Some(manager) = manager.upgrade() {
                                manager.clear_active(generation).await;
                            }
                            return;
                        }
                        run_connection(prepared, manager, generation, cancelled, finished).await;
                    }
                    Err(error) => {
                        if let Some(manager) = manager.upgrade() {
                            manager.clear_active(generation).await;
                        }
                        let _ = setup_tx.send(Err(error));
                    }
                }
            });
        })
        .map_err(|error| format!("Could not start the Windows H10 session owner: {error}"))?;

    setup_rx
        .await
        .map_err(|_| "The Windows H10 session owner stopped during setup".to_string())?
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

    const fn diagnostic_index(self) -> usize {
        match self {
            Self::PmdControl => 0,
            Self::PmdData => 1,
            Self::HeartRate => 2,
        }
    }

    const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::PmdControl => "pmd-control",
            Self::PmdData => "pmd-data",
            Self::HeartRate => "heart-rate",
        }
    }
}

const NOTIFICATION_SOURCES: [NotificationSource; 3] = [
    NotificationSource::PmdControl,
    NotificationSource::PmdData,
    NotificationSource::HeartRate,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SubscriptionMode {
    Notify,
    Indicate,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SubscriptionActivation {
    cccd_committed: bool,
    handler_attached: bool,
}

impl SubscriptionActivation {
    fn commit_cccd(&mut self) -> Result<(), &'static str> {
        if self.cccd_committed {
            return Err("CCCD was committed more than once");
        }
        if self.handler_attached {
            return Err("handler was attached before the CCCD commit");
        }
        self.cccd_committed = true;
        Ok(())
    }

    fn attach_handler(&mut self) -> Result<(), &'static str> {
        if !self.cccd_committed {
            return Err("handler attachment preceded the CCCD commit");
        }
        if self.handler_attached {
            return Err("handler was attached more than once");
        }
        self.handler_attached = true;
        Ok(())
    }

    const fn rollback_required(self) -> bool {
        self.cccd_committed && !self.handler_attached
    }
}

impl SubscriptionMode {
    const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Notify => "notify",
            Self::Indicate => "indicate",
        }
    }

    const fn cccd(self) -> GattClientCharacteristicConfigurationDescriptorValue {
        match self {
            Self::Notify => GattClientCharacteristicConfigurationDescriptorValue::Notify,
            Self::Indicate => GattClientCharacteristicConfigurationDescriptorValue::Indicate,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CharacteristicPropertyShape {
    notify: bool,
    indicate: bool,
    read: bool,
    write: bool,
    write_without_response: bool,
}

impl CharacteristicPropertyShape {
    fn from_winrt(properties: GattCharacteristicProperties) -> Self {
        Self {
            notify: properties.contains(GattCharacteristicProperties::Notify),
            indicate: properties.contains(GattCharacteristicProperties::Indicate),
            read: properties.contains(GattCharacteristicProperties::Read),
            write: properties.contains(GattCharacteristicProperties::Write),
            write_without_response: properties
                .contains(GattCharacteristicProperties::WriteWithoutResponse),
        }
    }
}

fn select_subscription_mode(shape: CharacteristicPropertyShape) -> SubscriptionMode {
    // Preserve the established Windows behavior while making the choice
    // observable. The physical differential run will show whether the PMD
    // data characteristic actually advertises both modes.
    if shape.indicate {
        SubscriptionMode::Indicate
    } else {
        SubscriptionMode::Notify
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SourceNotificationDiagnostics {
    properties: Option<CharacteristicPropertyShape>,
    mode: Option<SubscriptionMode>,
    cccd_committed: usize,
    handlers_attached: usize,
    handlers_removed: usize,
    handler_remove_failures: usize,
    callbacks_entered: usize,
    callbacks_decoded: usize,
    callbacks_enqueued: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct NotificationDiagnosticSnapshot {
    sources: [SourceNotificationDiagnostics; 3],
    callback_faults: usize,
    queue_full: usize,
    queue_closed: usize,
}

struct NotificationDiagnostics {
    enabled: bool,
    state: Mutex<NotificationDiagnosticSnapshot>,
}

impl NotificationDiagnostics {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            state: Mutex::new(NotificationDiagnosticSnapshot::default()),
        }
    }

    fn mutate(&self, operation: impl FnOnce(&mut NotificationDiagnosticSnapshot)) {
        if !self.enabled {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        operation(&mut state);
    }

    fn record_properties(
        &self,
        source: NotificationSource,
        properties: CharacteristicPropertyShape,
        mode: SubscriptionMode,
    ) {
        self.mutate(|state| {
            let source = &mut state.sources[source.diagnostic_index()];
            source.properties = Some(properties);
            source.mode = Some(mode);
        });
    }

    fn record_cccd_committed(&self, source: NotificationSource) {
        self.mutate(|state| {
            state.sources[source.diagnostic_index()].cccd_committed += 1;
        });
    }

    fn record_handler_attached(&self, source: NotificationSource) {
        self.mutate(|state| {
            let source = &mut state.sources[source.diagnostic_index()];
            debug_assert_eq!(source.cccd_committed, source.handlers_attached + 1);
            source.handlers_attached += 1;
        });
    }

    fn record_handler_removed(&self, source: NotificationSource) {
        self.mutate(|state| state.sources[source.diagnostic_index()].handlers_removed += 1);
    }

    fn record_handler_remove_failure(&self, source: NotificationSource) {
        self.mutate(|state| {
            state.sources[source.diagnostic_index()].handler_remove_failures += 1;
        });
    }

    fn record_callback_entered(&self, source: NotificationSource) {
        self.mutate(|state| state.sources[source.diagnostic_index()].callbacks_entered += 1);
    }

    fn record_callback_decoded(&self, source: NotificationSource) {
        self.mutate(|state| state.sources[source.diagnostic_index()].callbacks_decoded += 1);
    }

    fn record_callback_enqueued(&self, source: NotificationSource) {
        self.mutate(|state| state.sources[source.diagnostic_index()].callbacks_enqueued += 1);
    }

    fn record_callback_fault(&self) {
        self.mutate(|state| state.callback_faults += 1);
    }

    fn record_queue_full(&self) {
        self.mutate(|state| state.queue_full += 1);
    }

    fn record_queue_closed(&self) {
        self.mutate(|state| state.queue_closed += 1);
    }

    fn snapshot(&self) -> NotificationDiagnosticSnapshot {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn summary(&self) -> String {
        let snapshot = self.snapshot();
        let sources = NOTIFICATION_SOURCES.map(|source| {
            let state = snapshot.sources[source.diagnostic_index()];
            let properties = state.properties.unwrap_or_default();
            let mode = state
                .mode
                .map(SubscriptionMode::diagnostic_name)
                .unwrap_or("none");
            format!(
                "{}:properties-notify={},properties-indicate={},properties-read={},properties-write={},properties-write-without-response={},mode={},cccd-committed={},handlers-attached={},handlers-removed={},handler-remove-failures={},callbacks-entered={},callbacks-decoded={},callbacks-enqueued={}",
                source.diagnostic_name(),
                properties.notify,
                properties.indicate,
                properties.read,
                properties.write,
                properties.write_without_response,
                mode,
                state.cccd_committed,
                state.handlers_attached,
                state.handlers_removed,
                state.handler_remove_failures,
                state.callbacks_entered,
                state.callbacks_decoded,
                state.callbacks_enqueued,
            )
        });
        format!(
            "{} {} {} callback-faults={} queue-full={} queue-closed={}",
            sources[0],
            sources[1],
            sources[2],
            snapshot.callback_faults,
            snapshot.queue_full,
            snapshot.queue_closed,
        )
    }

    fn report(&self, checkpoint: &str) {
        if self.enabled {
            eprintln!(
                "POLAR_H10_NOTIFICATION_DIAGNOSTIC checkpoint={checkpoint} {}",
                self.summary()
            );
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
    diagnostics: Arc<NotificationDiagnostics>,
}

struct Subscription {
    characteristic: GattCharacteristic,
    source: SubscriptionKind,
    notification_source: NotificationSource,
    diagnostics: Arc<NotificationDiagnostics>,
    token: Option<i64>,
    _handler: TypedEventHandler<GattCharacteristic, GattValueChangedEventArgs>,
}

impl Subscription {
    fn remove_handler(&mut self) {
        if let Some(token) = take_subscription_token(&mut self.token) {
            if self.characteristic.RemoveValueChanged(token).is_ok() {
                self.diagnostics
                    .record_handler_removed(self.notification_source);
            } else {
                self.diagnostics
                    .record_handler_remove_failure(self.notification_source);
            }
            self.diagnostics.report("handler-removed");
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
    close_successful_gatt_operations: bool,
    use_winrt_when_completion: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LinkDiagnosticSnapshot {
    connected: Option<bool>,
    max_pdu_size: Option<u16>,
    connection_interval: Option<u16>,
    connection_latency: Option<u16>,
}

impl LinkDiagnosticSnapshot {
    fn summary(self) -> String {
        let connected = self
            .connected
            .map(|value| if value { "true" } else { "false" })
            .unwrap_or("unavailable");
        let max_pdu_size = self
            .max_pdu_size
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unavailable".to_string());
        let connection_interval = self
            .connection_interval
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unavailable".to_string());
        let connection_latency = self
            .connection_latency
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unavailable".to_string());
        format!(
            "connected={connected} max-pdu-size={max_pdu_size} connection-interval-units={connection_interval} connection-latency={connection_latency}"
        )
    }
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

    fn finish(
        mut self,
        handles: DiscoveredHandles,
        close_successful_gatt_operations: bool,
        use_winrt_when_completion: bool,
    ) -> WinrtSession {
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
            close_successful_gatt_operations,
            use_winrt_when_completion,
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
    close_after_success: bool,
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

    fn close_after_success(&self) -> bool {
        self.close_after_success
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
    await_winrt_stage_with_success_close(call, operation, cancel, close, true, cancelled).await
}

async fn await_winrt_stage_with_success_close<T, O, C, X>(
    call: WinrtStageCall,
    operation: windows::core::Result<O>,
    cancel: C,
    close: X,
    close_after_success: bool,
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
            close_after_success,
        },
    )
    .await
    .map_err(|error| error.to_string())
}

async fn await_winrt_stage_when<T>(
    call: WinrtStageCall,
    operation: windows::core::Result<IAsyncOperation<T>>,
    close_after_success: bool,
    cancelled: &mut watch::Receiver<bool>,
) -> Result<T, String>
where
    T: windows::core::RuntimeType + Send + 'static,
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
    let control_operation = operation.clone();
    let (completion_tx, completion_rx) = oneshot::channel();
    run_controlled_stage(
        call.reporter,
        call.stage,
        call.attempt,
        call.timeout,
        cancelled,
        async move {
            operation
                .when(move |result| {
                    let _ = completion_tx.send(result.map_err(|error| error.to_string()));
                })
                .map_err(|error| error.to_string())?;
            completion_rx
                .await
                .map_err(|_| "Windows WinRT completion callback closed".to_string())?
        },
        WinrtStageControl {
            operation: control_operation,
            cancel: |operation: &IAsyncOperation<T>| operation.Cancel(),
            close: |operation: &IAsyncOperation<T>| operation.Close(),
            close_after_success,
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
        profile: SessionProfile,
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

        let settle_span = reporter.enter(SessionStage::ConnectionSettle, 1);
        if *cancelled.borrow() {
            settle_span.finish(StageResultClass::Cancelled);
            return Err("Windows WinRT connection-settle was cancelled".to_string());
        }
        tokio::select! {
            () = tokio::time::sleep(CONNECTION_SETTLE) => {
                settle_span.finish(StageResultClass::Success);
            }
            changed = cancelled.changed() => {
                settle_span.finish(StageResultClass::Cancelled);
                return Err(if changed.is_err() {
                    "Windows WinRT connection-settle cancellation owner closed".to_string()
                } else {
                    "Windows WinRT connection-settle was cancelled".to_string()
                });
            }
        }

        // The normal profile matches the black-box lifecycle of the exact
        // published Windows reference: settle the persistent session and
        // resolve optional heart rate before required PMD. The verifier-only
        // PMD differential skips that optional branch so its next physical run
        // changes exactly one production-session behavior. Battery discovery
        // remains outside required sensor qualification. Throughput
        // optimization is requested only after both first frames.
        let opening = OpeningSession::new(device, gatt_session, None);
        let (heart_rate_service, heart_rate) = if profile.heart_rate_enabled() {
            let service = optional_service(
                opening.device(),
                HEART_RATE_SERVICE,
                SessionStage::HeartRateServiceDiscovery,
                reporter,
                cancelled,
            )
            .await;
            ensure_active(cancelled, "heart-rate service discovery")?;
            let characteristic = if let Some(service) = &service {
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
            (service, characteristic)
        } else {
            (None, None)
        };

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

        Ok(opening.finish(
            DiscoveredHandles {
                pmd_service,
                heart_rate_service,
                battery_service: None,
                control,
                pmd_data,
                heart_rate,
                battery: None,
            },
            profile.close_successful_gatt_operations(),
            profile.use_winrt_when_completion(),
        ))
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

    fn link_diagnostic_snapshot(&self) -> LinkDiagnosticSnapshot {
        let connected = self.device.ConnectionStatus().ok().map(|status| {
            status == windows::Devices::Bluetooth::BluetoothConnectionStatus::Connected
        });
        let parameters = self.device.GetConnectionParameters().ok();
        LinkDiagnosticSnapshot {
            connected,
            max_pdu_size: self.gatt_session.MaxPduSize().ok(),
            connection_interval: parameters
                .as_ref()
                .and_then(|parameters| parameters.ConnectionInterval().ok()),
            connection_latency: parameters
                .as_ref()
                .and_then(|parameters| parameters.ConnectionLatency().ok()),
        }
    }

    fn report_link_diagnostic(&self, checkpoint: &str) {
        if std::env::var_os(SESSION_DIAGNOSTICS_ENV).is_some() {
            eprintln!(
                "POLAR_H10_LINK_DIAGNOSTIC checkpoint={checkpoint} {}",
                self.link_diagnostic_snapshot().summary()
            );
        }
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
        let mut activation = SubscriptionActivation::default();
        let NotificationSink {
            raw_tx,
            fault_tx,
            diagnostics,
        } = sink;
        let properties = characteristic
            .CharacteristicProperties()
            .map_err(|error| stage_error("notification properties", error))?;
        let property_shape = CharacteristicPropertyShape::from_winrt(properties);
        let mode = select_subscription_mode(property_shape);
        diagnostics.record_properties(source, property_shape, mode);
        diagnostics.report("properties-read");
        let call = WinrtStageCall::new(reporter, stage, 1, GATT_TIMEOUT);
        let operation =
            characteristic.WriteClientCharacteristicConfigurationDescriptorAsync(mode.cccd());
        let status = if self.use_winrt_when_completion {
            await_winrt_stage_when(
                call,
                operation,
                self.close_successful_gatt_operations,
                cancelled,
            )
            .await
        } else {
            await_winrt_stage_with_success_close(
                call,
                operation,
                |operation| operation.Cancel(),
                |operation| operation.Close(),
                self.close_successful_gatt_operations,
                cancelled,
            )
            .await
        };
        match status {
            Ok(GattCommunicationStatus::Success) => {
                activation.commit_cccd().map_err(|error| {
                    format!("Windows WinRT subscription invariant failed: {error}")
                })?;
                diagnostics.record_cccd_committed(source);
                diagnostics.report("cccd-committed");
                // Match the proven Windows reference projection exactly:
                // perform one successful CCCD write and immediately attach a
                // directly retained handler before any PMD command can produce
                // data. Descriptor readback is intentionally omitted because
                // the passing reference does not interpose that operation.
                let token_result = {
                    let callback_diagnostics = diagnostics.clone();
                    let handler =
                        TypedEventHandler::<GattCharacteristic, GattValueChangedEventArgs>::new(
                            move |_, args| {
                                callback_diagnostics.record_callback_entered(source);
                                let result = (|| {
                                    let args = args.ok()?;
                                    let value = args.CharacteristicValue()?;
                                    let bytes = buffer_to_vec(&value)?;
                                    callback_diagnostics.record_callback_decoded(source);
                                    enqueue_notification(
                                        &raw_tx,
                                        &fault_tx,
                                        &callback_diagnostics,
                                        RawNotification {
                                            source,
                                            value: bytes,
                                        },
                                    );
                                    Ok::<(), windows::core::Error>(())
                                })();
                                if let Err(error) = result {
                                    callback_diagnostics.record_callback_fault();
                                    callback_diagnostics.report("callback-fault");
                                    fault_tx.send_replace(Some(format!(
                                        "Windows WinRT notification callback failed: {error}"
                                    )));
                                }
                                Ok(())
                            },
                        );
                    characteristic
                        .ValueChanged(&handler)
                        .map(|token| (token, handler))
                };
                let (token, handler) = match token_result {
                    Ok(value) => value,
                    Err(error) => {
                        if activation.rollback_required() {
                            Self::disable_cccd(characteristic).await;
                        }
                        return Err(stage_error("notification handler", error));
                    }
                };
                if let Err(error) = activation.attach_handler() {
                    let _ = characteristic.RemoveValueChanged(token);
                    Self::disable_cccd(characteristic).await;
                    return Err(format!(
                        "Windows WinRT subscription invariant failed: {error}"
                    ));
                }
                diagnostics.record_handler_attached(source);
                self.subscriptions.push(Subscription {
                    characteristic: characteristic.clone(),
                    source: source.subscription_kind(),
                    notification_source: source,
                    diagnostics: diagnostics.clone(),
                    token: Some(token),
                    _handler: handler,
                });
                self.cleanup.record_subscription(source.subscription_kind());
                diagnostics.report("handler-attached");
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
        let call = WinrtStageCall::new(reporter, stage, 1, GATT_TIMEOUT);
        let result = if self.use_winrt_when_completion {
            await_winrt_stage_when(
                call,
                operation,
                self.close_successful_gatt_operations,
                cancelled,
            )
            .await?
        } else {
            await_winrt_stage_with_success_close(
                call,
                operation,
                |operation| operation.Cancel(),
                |operation| operation.Close(),
                self.close_successful_gatt_operations,
                cancelled,
            )
            .await?
        };
        let status = result
            .Status()
            .map_err(|error| stage_error(stage.name(), error))?;
        if status != GattCommunicationStatus::Success {
            return Err(format!("Windows WinRT {} failed: {status:?}", stage.name()));
        }
        Ok(())
    }

    async fn read_battery(
        &mut self,
        reporter: StageReporter,
        cancelled: &mut watch::Receiver<bool>,
    ) -> Option<u8> {
        if self.battery.is_none() {
            let service = optional_service(
                &self.device,
                uuid::Uuid::from_u128(0x0000180f_0000_1000_8000_00805f9b34fb),
                SessionStage::BatteryServiceDiscovery,
                reporter,
                cancelled,
            )
            .await;
            ensure_active(cancelled, "battery service discovery").ok()?;
            let characteristic = if let Some(service) = &service {
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
            ensure_active(cancelled, "battery characteristic discovery").ok()?;
            self.battery_service = service;
            self.battery = characteristic;
        }

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

    async fn disable_cccd(characteristic: &GattCharacteristic) {
        let operation = characteristic.WriteClientCharacteristicConfigurationDescriptorAsync(
            GattClientCharacteristicConfigurationDescriptorValue::None,
        );
        let _ = await_cleanup_operation(
            operation,
            |operation| operation.Cancel(),
            |operation| operation.Close(),
        )
        .await;
    }

    async fn disable_subscription(subscription: &mut Subscription) {
        subscription.remove_handler();
        Self::disable_cccd(&subscription.characteristic).await;
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
    let profile = SessionProfile::from_environment()?;
    let diagnostics_enabled = std::env::var_os(SESSION_DIAGNOSTICS_ENV).is_some();
    if diagnostics_enabled {
        eprintln!(
            "POLAR_H10_SESSION_PROFILE name={} heart-rate-enabled={} close-successful-gatt-operations={} winrt-when-completion={}",
            profile.name(),
            profile.heart_rate_enabled(),
            profile.close_successful_gatt_operations(),
            profile.use_winrt_when_completion()
        );
    }
    let reporter = StageReporter::new(
        diagnostics_enabled,
        Some(tokio::time::Instant::now() + SESSION_SETUP_TIMEOUT),
    );
    let notification_diagnostics = Arc::new(NotificationDiagnostics::new(diagnostics_enabled));
    let (raw_tx, mut raw_rx) = mpsc::channel(RAW_NOTIFICATION_CAPACITY);
    let (fault_tx, mut fault_rx) = watch::channel(None::<String>);
    let mut session = WinrtSession::open(device_id, profile, reporter, cancelled).await?;
    session.report_link_diagnostic("after-discovery");
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
                        diagnostics: notification_diagnostics.clone(),
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
                    diagnostics: notification_diagnostics.clone(),
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
                NotificationSink {
                    raw_tx,
                    fault_tx,
                    diagnostics: notification_diagnostics.clone(),
                },
            )
            .await?;
        ensure_active(cancelled, "PMD data subscription")?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        session.report_link_diagnostic("before-pmd-setup");
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
            diagnostics: &notification_diagnostics,
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
            diagnostics: &notification_diagnostics,
            gate: &mut gate,
            frame_stages: &mut frame_stages,
        }
        .pmd_control_response(
            SessionStage::StartEcgResponse,
            ExpectedControlResponse::ECG_START,
            PMD_RESPONSE_TIMEOUT,
        )
        .await?;
        session.report_link_diagnostic("after-ecg-start-response");
        SetupWaitContext {
            raw_rx: &mut raw_rx,
            fault_rx: &mut fault_rx,
            cancelled: &mut *cancelled,
            reporter,
            diagnostics: &notification_diagnostics,
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
            diagnostics: &notification_diagnostics,
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
            diagnostics: &notification_diagnostics,
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
        session.report_link_diagnostic("setup-failure");
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
    diagnostics: &'a NotificationDiagnostics,
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
                        self.diagnostics.report(stage.name());
                        return Ok(());
                    }
                }
                () = &mut deadline => {
                    span.finish(StageResultClass::Timeout);
                    self.diagnostics.report(stage.name());
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
            self.diagnostics.report(stage.name());
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
                        Ok(None) if self.gate.saw(kind) => {
                            self.diagnostics.report(stage.name());
                            return Ok(());
                        }
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
                    self.diagnostics.report(stage.name());
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
    diagnostics: &NotificationDiagnostics,
    raw: RawNotification,
) {
    let source = raw.source;
    match raw_tx.try_send(raw) {
        Ok(()) => diagnostics.record_callback_enqueued(source),
        Err(mpsc::error::TrySendError::Full(_)) => {
            diagnostics.record_queue_full();
            diagnostics.report("queue-full");
            fault_tx.send_replace(Some(format!(
                "Windows WinRT notification queue reached its {RAW_NOTIFICATION_CAPACITY}-batch bound; acquisition stopped without silent loss"
            )));
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            diagnostics.record_queue_closed();
            diagnostics.report("queue-closed");
        }
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

    #[test]
    fn session_profile_is_closed_and_differentials_are_exact() {
        let reference = SessionProfile::parse(None).unwrap();
        assert_eq!(reference, SessionProfile::ReferenceCompatible);
        assert_eq!(reference.name(), "reference-compatible");
        assert!(reference.heart_rate_enabled());
        assert!(reference.close_successful_gatt_operations());
        assert!(!reference.use_winrt_when_completion());

        let pmd_only = SessionProfile::parse(Some(OsStr::new(PMD_ONLY_PROFILE))).unwrap();
        assert_eq!(pmd_only, SessionProfile::PmdOnlyDifferential);
        assert_eq!(pmd_only.name(), PMD_ONLY_PROFILE);
        assert!(!pmd_only.heart_rate_enabled());
        assert!(pmd_only.close_successful_gatt_operations());
        assert!(!pmd_only.use_winrt_when_completion());

        let retained = SessionProfile::parse(Some(OsStr::new(PMD_RETAIN_SUCCESS_PROFILE))).unwrap();
        assert_eq!(
            retained,
            SessionProfile::PmdOnlyRetainSuccessfulGattOperations
        );
        assert_eq!(retained.name(), PMD_RETAIN_SUCCESS_PROFILE);
        assert!(!retained.heart_rate_enabled());
        assert!(!retained.close_successful_gatt_operations());
        assert!(!retained.use_winrt_when_completion());

        let when = SessionProfile::parse(Some(OsStr::new(PMD_WHEN_COMPLETION_PROFILE))).unwrap();
        assert_eq!(when, SessionProfile::PmdOnlyWinrtWhenCompletion);
        assert_eq!(when.name(), PMD_WHEN_COMPLETION_PROFILE);
        assert!(!when.heart_rate_enabled());
        assert!(!when.close_successful_gatt_operations());
        assert!(when.use_winrt_when_completion());

        let error = SessionProfile::parse(Some(OsStr::new("pmd-only"))).unwrap_err();
        assert!(error.contains(SESSION_PROFILE_ENV));
        assert!(error.contains(PMD_ONLY_PROFILE));
        assert!(error.contains(PMD_RETAIN_SUCCESS_PROFILE));
        assert!(error.contains(PMD_WHEN_COMPLETION_PROFILE));
    }

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
    fn session_owner_keeps_one_initialized_thread_across_async_yields() {
        let (before, after) = thread::spawn(|| {
            let _apartment = WinrtApartment::initialize_mta().unwrap();
            let runtime = build_session_runtime().unwrap();
            let before = thread::current().id();
            let after = runtime.block_on(async {
                tokio::task::yield_now().await;
                tokio::time::sleep(Duration::from_millis(1)).await;
                thread::current().id()
            });
            (before, after)
        })
        .join()
        .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn session_owner_completion_is_signalled_even_during_unwind_cleanup() {
        let (finished_tx, finished_rx) = watch::channel(false);
        {
            let _guard = SessionThreadFinished(finished_tx);
            assert!(!*finished_rx.borrow());
        }
        assert!(*finished_rx.borrow());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn session_owner_reports_setup_failure_without_blocking_the_caller_runtime() {
        let manager = Arc::new(InputManager::new());
        let (event_tx, _event_rx) = mpsc::channel(4);
        let (_cancel_tx, cancelled) = watch::channel(false);
        let (finished_tx, mut finished_rx) = watch::channel(false);
        let heartbeat = tokio::spawn(async {
            tokio::task::yield_now().await;
            "caller-responsive"
        });

        let error = spawn_session(
            "not-a-bluetooth-address".to_string(),
            "identifier-free-test-device".to_string(),
            event_tx,
            Arc::downgrade(&manager),
            1,
            cancelled,
            finished_tx,
        )
        .await
        .unwrap_err();

        assert_eq!(heartbeat.await.unwrap(), "caller-responsive");
        assert!(error.contains("Bluetooth address was not recognized"));
        while !*finished_rx.borrow() {
            finished_rx.changed().await.unwrap();
        }
    }

    #[test]
    fn exact_reference_name_does_not_depend_on_service_uuid_readback() {
        let mut scan = ScanAccumulator::new(true);
        assert!(!scan.has_exact_name_candidate());
        scan.record(1, -45, evidence(Some("Polar H10 TEST"), true, false, false));
        assert!(scan.has_exact_name_candidate());
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
        assert!(!scan.has_exact_name_candidate());

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

    #[test]
    fn reference_subscription_activation_is_single_write_then_single_handler() {
        let mut activation = SubscriptionActivation::default();
        assert!(!activation.rollback_required());
        assert_eq!(
            activation.attach_handler(),
            Err("handler attachment preceded the CCCD commit")
        );

        activation.commit_cccd().expect("first CCCD commit");
        assert!(activation.rollback_required());
        assert_eq!(
            activation.commit_cccd(),
            Err("CCCD was committed more than once")
        );

        activation.attach_handler().expect("first handler attach");
        assert!(!activation.rollback_required());
        assert_eq!(
            activation.attach_handler(),
            Err("handler was attached more than once")
        );
    }

    #[test]
    fn link_diagnostic_summary_is_identifier_free_and_deterministic() {
        assert_eq!(
            LinkDiagnosticSnapshot {
                connected: Some(true),
                max_pdu_size: Some(247),
                connection_interval: Some(12),
                connection_latency: Some(0),
            }
            .summary(),
            "connected=true max-pdu-size=247 connection-interval-units=12 connection-latency=0"
        );
        assert_eq!(
            LinkDiagnosticSnapshot {
                connected: None,
                max_pdu_size: None,
                connection_interval: None,
                connection_latency: None,
            }
            .summary(),
            "connected=unavailable max-pdu-size=unavailable connection-interval-units=unavailable connection-latency=unavailable"
        );
    }

    #[test]
    fn subscription_mode_and_identifier_free_diagnostics_are_deterministic() {
        let both = CharacteristicPropertyShape {
            notify: true,
            indicate: true,
            read: false,
            write: false,
            write_without_response: false,
        };
        assert_eq!(select_subscription_mode(both), SubscriptionMode::Indicate);
        assert_eq!(
            select_subscription_mode(CharacteristicPropertyShape {
                notify: true,
                ..CharacteristicPropertyShape::default()
            }),
            SubscriptionMode::Notify
        );

        let diagnostics = NotificationDiagnostics::new(true);
        diagnostics.record_properties(
            NotificationSource::PmdData,
            both,
            SubscriptionMode::Indicate,
        );
        diagnostics.record_cccd_committed(NotificationSource::PmdData);
        diagnostics.record_handler_attached(NotificationSource::PmdData);
        diagnostics.record_callback_entered(NotificationSource::PmdData);
        diagnostics.record_callback_decoded(NotificationSource::PmdData);
        diagnostics.record_callback_enqueued(NotificationSource::PmdData);
        diagnostics.record_handler_removed(NotificationSource::PmdData);

        let snapshot = diagnostics.snapshot();
        assert_eq!(
            snapshot.sources[NotificationSource::PmdData.diagnostic_index()],
            SourceNotificationDiagnostics {
                properties: Some(both),
                mode: Some(SubscriptionMode::Indicate),
                cccd_committed: 1,
                handlers_attached: 1,
                handlers_removed: 1,
                handler_remove_failures: 0,
                callbacks_entered: 1,
                callbacks_decoded: 1,
                callbacks_enqueued: 1,
            }
        );
        assert_eq!(
            diagnostics.summary(),
            "pmd-control:properties-notify=false,properties-indicate=false,properties-read=false,properties-write=false,properties-write-without-response=false,mode=none,cccd-committed=0,handlers-attached=0,handlers-removed=0,handler-remove-failures=0,callbacks-entered=0,callbacks-decoded=0,callbacks-enqueued=0 pmd-data:properties-notify=true,properties-indicate=true,properties-read=false,properties-write=false,properties-write-without-response=false,mode=indicate,cccd-committed=1,handlers-attached=1,handlers-removed=1,handler-remove-failures=0,callbacks-entered=1,callbacks-decoded=1,callbacks-enqueued=1 heart-rate:properties-notify=false,properties-indicate=false,properties-read=false,properties-write=false,properties-write-without-response=false,mode=none,cccd-committed=0,handlers-attached=0,handlers-removed=0,handler-remove-failures=0,callbacks-entered=0,callbacks-decoded=0,callbacks-enqueued=0 callback-faults=0 queue-full=0 queue-closed=0"
        );
    }

    #[test]
    fn closed_notification_queue_is_counted_without_becoming_a_fault() {
        let (raw_tx, raw_rx) = mpsc::channel(1);
        let (fault_tx, fault_rx) = watch::channel(None);
        let diagnostics = NotificationDiagnostics::new(true);
        drop(raw_rx);
        enqueue_notification(
            &raw_tx,
            &fault_tx,
            &diagnostics,
            RawNotification {
                source: NotificationSource::PmdData,
                value: vec![1],
            },
        );
        assert_eq!(diagnostics.snapshot().queue_closed, 1);
        assert!(fault_rx.borrow().is_none());
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
            let diagnostics = NotificationDiagnostics::new(false);
            let mut stages = FirstFrameStages::new(reporter);
            SetupWaitContext {
                raw_rx: &mut raw_rx,
                fault_rx: &mut fault_rx,
                cancelled: &mut cancelled,
                reporter,
                diagnostics: &diagnostics,
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
            let diagnostics = NotificationDiagnostics::new(false);
            let mut gate = FirstFrameGate::default();
            let mut stages = FirstFrameStages::new(reporter);
            SetupWaitContext {
                raw_rx: &mut raw_rx,
                fault_rx: &mut fault_rx,
                cancelled: &mut cancelled,
                reporter,
                diagnostics: &diagnostics,
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
        let diagnostics = NotificationDiagnostics::new(true);
        enqueue_notification(
            &raw_tx,
            &fault_tx,
            &diagnostics,
            RawNotification {
                source: NotificationSource::PmdControl,
                value: vec![1],
            },
        );
        enqueue_notification(
            &raw_tx,
            &fault_tx,
            &diagnostics,
            RawNotification {
                source: NotificationSource::PmdControl,
                value: vec![2],
            },
        );

        assert!(fault_rx.borrow().as_deref().is_some_and(|message| {
            message.contains("acquisition stopped without silent loss")
        }));
        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.sources[0].callbacks_enqueued, 1);
        assert_eq!(snapshot.queue_full, 1);
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
