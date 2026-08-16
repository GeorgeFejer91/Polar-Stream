//! Identifier-free Windows PMD projection probe.
//!
//! This diagnostic deliberately excludes discovery, Tauri, Tokio, LSL, heart
//! rate, and application queues. The selected Bluetooth address is accepted
//! only through a private environment variable and is never emitted.

use std::{process::ExitCode, time::Duration};

#[cfg(any(target_os = "windows", test))]
use std::sync::mpsc::{Receiver, RecvTimeoutError};

use polar_h10_core::decode_pmd_control_response;

const ADDRESS_ENV: &str = "POLAR_H10_PROBE_ADDRESS";
const OPERATION_TIMEOUT: Duration = Duration::from_secs(8);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const FRAME_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(any(target_os = "windows", test))]
const CANCEL_GRACE: Duration = Duration::from_millis(250);

fn parse_private_address(value: &str) -> Option<u64> {
    let compact: String = value
        .chars()
        .filter(|character| !matches!(character, ':' | '-'))
        .collect();
    if compact.len() != 12 || !compact.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let address = u64::from_str_radix(&compact, 16).ok()?;
    (address != 0).then_some(address)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResultClass {
    InvalidInput,
    NativeError,
    Timeout,
    Rejected,
    Missing,
    Protocol,
}

impl ResultClass {
    const fn name(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid-input",
            Self::NativeError => "native-error",
            Self::Timeout => "timeout",
            Self::Rejected => "rejected",
            Self::Missing => "missing",
            Self::Protocol => "protocol-error",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct ProbeFailure {
    stage: &'static str,
    class: ResultClass,
}

impl ProbeFailure {
    const fn new(stage: &'static str, class: ResultClass) -> Self {
        Self { stage, class }
    }

    fn report(&self) {
        eprintln!(
            "POLAR_H10_PROBE_FAILURE stage={} class={}",
            self.stage,
            self.class.name()
        );
    }
}

#[derive(Default, Debug, Eq, PartialEq)]
struct Observation {
    control_callbacks: usize,
    data_callbacks: usize,
    ecg_frames: usize,
    ecg_samples: usize,
    acc_frames: usize,
    acc_samples: usize,
}

#[cfg(any(target_os = "windows", test))]
fn receive_completion<T>(
    rx: &Receiver<Result<T, ResultClass>>,
    timeout: Duration,
    cancel: impl FnOnce(),
) -> Result<T, ResultClass> {
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => {
            cancel();
            let _ = rx.recv_timeout(CANCEL_GRACE);
            Err(ResultClass::Timeout)
        }
        Err(RecvTimeoutError::Disconnected) => Err(ResultClass::NativeError),
    }
}

#[cfg(any(target_os = "windows", test))]
#[derive(Default)]
struct CleanupGate {
    started: bool,
}

#[cfg(any(target_os = "windows", test))]
impl CleanupGate {
    fn begin(&mut self) -> bool {
        if self.started {
            false
        } else {
            self.started = true;
            true
        }
    }
}

fn validate_control_response(
    bytes: &[u8],
    expected_opcode: u8,
    expected_measurement: u8,
) -> Result<(), ResultClass> {
    let response = decode_pmd_control_response(bytes).map_err(|_| ResultClass::Protocol)?;
    if response.opcode != expected_opcode
        || response.measurement != expected_measurement
        || response.error_code != 0
    {
        return Err(ResultClass::Protocol);
    }
    Ok(())
}

fn stage<T>(
    name: &'static str,
    action: impl FnOnce() -> Result<T, ProbeFailure>,
) -> Result<T, ProbeFailure> {
    let started = std::time::Instant::now();
    println!("POLAR_H10_PROBE_STAGE stage={name} result=entered");
    match action() {
        Ok(value) => {
            println!(
                "POLAR_H10_PROBE_STAGE stage={name} result=success duration_ms={}",
                started.elapsed().as_millis()
            );
            Ok(value)
        }
        Err(error) => {
            println!(
                "POLAR_H10_PROBE_STAGE stage={name} result={} duration_ms={}",
                error.class.name(),
                started.elapsed().as_millis()
            );
            Err(error)
        }
    }
}

fn main() -> ExitCode {
    let address = match std::env::var(ADDRESS_ENV)
        .ok()
        .as_deref()
        .and_then(parse_private_address)
    {
        Some(address) => address,
        None => {
            ProbeFailure::new("input", ResultClass::InvalidInput).report();
            return ExitCode::from(2);
        }
    };

    match platform::run(address) {
        Ok(observation) => {
            println!(
                "POLAR_H10_PROBE_RESULT result=success control_callbacks={} data_callbacks={} ecg_frames={} ecg_samples={} acc_frames={} acc_samples={}",
                observation.control_callbacks,
                observation.data_callbacks,
                observation.ecg_frames,
                observation.ecg_samples,
                observation.acc_frames,
                observation.acc_samples,
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            error.report();
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::{Observation, ProbeFailure, ResultClass};

    pub(super) fn run(_address: u64) -> Result<Observation, ProbeFailure> {
        Err(ProbeFailure::new("platform", ResultClass::Rejected))
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::{
        sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
        thread,
        time::{Duration, Instant},
    };

    use polar_h10_core::{
        ACC_MEASUREMENT, ECG_MEASUREMENT, PMD_GET_SETTINGS_OPCODE, PMD_START_STREAM_OPCODE,
        PmdFrame, decode_pmd, request_settings_command, start_accelerometer_command,
        start_ecg_command, stop_command,
    };
    use windows::{
        Devices::{
            Bluetooth::{
                BluetoothCacheMode, BluetoothLEDevice,
                GenericAttributeProfile::{
                    GattCharacteristic, GattClientCharacteristicConfigurationDescriptorValue,
                    GattCommunicationStatus, GattDeviceService, GattSession,
                    GattValueChangedEventArgs,
                },
            },
            Enumeration::DeviceAccessStatus,
        },
        Foundation::TypedEventHandler,
        Storage::Streams::{DataReader, DataWriter, IBuffer},
        Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        core::{GUID, Result as WinResult},
    };
    use windows_future::IAsyncOperation;

    use super::{
        CleanupGate, FRAME_TIMEOUT, OPERATION_TIMEOUT, Observation, ProbeFailure, RESPONSE_TIMEOUT,
        ResultClass, receive_completion, stage, validate_control_response,
    };

    const PMD_SERVICE: GUID = GUID::from_u128(0xfb005c80_02e7_f387_1cad_8acd2d8df0c8);
    const PMD_CONTROL_POINT: GUID = GUID::from_u128(0xfb005c81_02e7_f387_1cad_8acd2d8df0c8);
    const PMD_DATA: GUID = GUID::from_u128(0xfb005c82_02e7_f387_1cad_8acd2d8df0c8);
    #[derive(Clone, Copy)]
    enum Source {
        Control,
        Data,
    }

    struct Notification {
        source: Source,
        bytes: Vec<u8>,
    }

    struct ComApartment;

    impl ComApartment {
        fn initialize() -> Result<Self, ProbeFailure> {
            unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
                .map_err(|_| ProbeFailure::new("com-initialize", ResultClass::NativeError))?;
            Ok(Self)
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            unsafe { RoUninitialize() };
        }
    }

    fn wait_operation<T: windows::core::RuntimeType + Send + 'static>(
        stage_name: &'static str,
        operation: IAsyncOperation<T>,
        timeout: Duration,
    ) -> Result<T, ProbeFailure> {
        let cancel = operation.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        operation
            .when(move |result| {
                let _ = tx.send(result.map_err(|_| ResultClass::NativeError));
            })
            .map_err(|_| ProbeFailure::new(stage_name, ResultClass::NativeError))?;

        receive_completion(&rx, timeout, || {
            let _ = cancel.Cancel();
        })
        .map_err(|class| ProbeFailure::new(stage_name, class))
    }

    fn buffer_to_vec(buffer: &IBuffer) -> WinResult<Vec<u8>> {
        let reader = DataReader::FromBuffer(buffer)?;
        let mut bytes = vec![0; buffer.Length()? as usize];
        reader.ReadBytes(&mut bytes)?;
        Ok(bytes)
    }

    fn first_service(device: &BluetoothLEDevice) -> Result<GattDeviceService, ProbeFailure> {
        let result = wait_operation(
            "pmd-service-discovery",
            device
                .GetGattServicesForUuidWithCacheModeAsync(PMD_SERVICE, BluetoothCacheMode::Uncached)
                .map_err(|_| {
                    ProbeFailure::new("pmd-service-discovery", ResultClass::NativeError)
                })?,
            OPERATION_TIMEOUT,
        )?;
        if result.Status().ok() != Some(GattCommunicationStatus::Success) {
            return Err(ProbeFailure::new(
                "pmd-service-discovery",
                ResultClass::Rejected,
            ));
        }
        let services = result
            .Services()
            .map_err(|_| ProbeFailure::new("pmd-service-discovery", ResultClass::NativeError))?;
        if services.Size().ok().unwrap_or(0) != 1 {
            return Err(ProbeFailure::new(
                "pmd-service-discovery",
                ResultClass::Missing,
            ));
        }
        services
            .GetAt(0)
            .map_err(|_| ProbeFailure::new("pmd-service-discovery", ResultClass::NativeError))
    }

    fn first_characteristic(
        service: &GattDeviceService,
        guid: GUID,
        stage_name: &'static str,
    ) -> Result<GattCharacteristic, ProbeFailure> {
        let result = wait_operation(
            stage_name,
            service
                .GetCharacteristicsForUuidWithCacheModeAsync(guid, BluetoothCacheMode::Uncached)
                .map_err(|_| ProbeFailure::new(stage_name, ResultClass::NativeError))?,
            OPERATION_TIMEOUT,
        )?;
        if result.Status().ok() != Some(GattCommunicationStatus::Success) {
            return Err(ProbeFailure::new(stage_name, ResultClass::Rejected));
        }
        let characteristics = result
            .Characteristics()
            .map_err(|_| ProbeFailure::new(stage_name, ResultClass::NativeError))?;
        if characteristics.Size().ok().unwrap_or(0) != 1 {
            return Err(ProbeFailure::new(stage_name, ResultClass::Missing));
        }
        characteristics
            .GetAt(0)
            .map_err(|_| ProbeFailure::new(stage_name, ResultClass::NativeError))
    }

    fn configure_cccd(
        characteristic: &GattCharacteristic,
        mode: GattClientCharacteristicConfigurationDescriptorValue,
        stage_name: &'static str,
    ) -> Result<(), ProbeFailure> {
        let status = wait_operation(
            stage_name,
            characteristic
                .WriteClientCharacteristicConfigurationDescriptorAsync(mode)
                .map_err(|_| ProbeFailure::new(stage_name, ResultClass::NativeError))?,
            OPERATION_TIMEOUT,
        )?;
        if status != GattCommunicationStatus::Success {
            return Err(ProbeFailure::new(stage_name, ResultClass::Rejected));
        }
        Ok(())
    }

    fn attach_handler(
        characteristic: &GattCharacteristic,
        source: Source,
        tx: SyncSender<Notification>,
        stage_name: &'static str,
    ) -> Result<
        (
            i64,
            TypedEventHandler<GattCharacteristic, GattValueChangedEventArgs>,
        ),
        ProbeFailure,
    > {
        let handler = TypedEventHandler::new(
            move |_, args: windows::core::Ref<'_, GattValueChangedEventArgs>| {
                let result = (|| {
                    let value = args.ok()?.CharacteristicValue()?;
                    let bytes = buffer_to_vec(&value)?;
                    let _ = tx.try_send(Notification { source, bytes });
                    Ok::<(), windows::core::Error>(())
                })();
                let _ = result;
                Ok(())
            },
        );
        let token = characteristic
            .ValueChanged(&handler)
            .map_err(|_| ProbeFailure::new(stage_name, ResultClass::NativeError))?;
        Ok((token, handler))
    }

    fn write_control(
        characteristic: &GattCharacteristic,
        bytes: &[u8],
        stage_name: &'static str,
    ) -> Result<(), ProbeFailure> {
        let writer = DataWriter::new()
            .map_err(|_| ProbeFailure::new(stage_name, ResultClass::NativeError))?;
        writer
            .WriteBytes(bytes)
            .map_err(|_| ProbeFailure::new(stage_name, ResultClass::NativeError))?;
        let buffer = writer
            .DetachBuffer()
            .map_err(|_| ProbeFailure::new(stage_name, ResultClass::NativeError))?;
        let result = wait_operation(
            stage_name,
            characteristic
                .WriteValueWithResultAsync(&buffer)
                .map_err(|_| ProbeFailure::new(stage_name, ResultClass::NativeError))?,
            OPERATION_TIMEOUT,
        )?;
        if result.Status().ok() != Some(GattCommunicationStatus::Success) {
            return Err(ProbeFailure::new(stage_name, ResultClass::Rejected));
        }
        Ok(())
    }

    struct Inbox {
        rx: Receiver<Notification>,
        observation: Observation,
    }

    impl Inbox {
        fn new(rx: Receiver<Notification>) -> Self {
            Self {
                rx,
                observation: Observation::default(),
            }
        }

        fn observe(&mut self, notification: Notification) -> Option<Vec<u8>> {
            match notification.source {
                Source::Control => {
                    self.observation.control_callbacks += 1;
                    Some(notification.bytes)
                }
                Source::Data => {
                    self.observation.data_callbacks += 1;
                    match decode_pmd(&notification.bytes) {
                        Ok(PmdFrame::Ecg { microvolts, .. }) if !microvolts.is_empty() => {
                            self.observation.ecg_frames += 1;
                            self.observation.ecg_samples += microvolts.len();
                        }
                        Ok(PmdFrame::Accelerometer { samples, .. }) if !samples.is_empty() => {
                            self.observation.acc_frames += 1;
                            self.observation.acc_samples += samples.len();
                        }
                        _ => {}
                    }
                    None
                }
            }
        }

        fn wait_for_control(
            &mut self,
            opcode: u8,
            measurement: u8,
            stage_name: &'static str,
        ) -> Result<(), ProbeFailure> {
            let deadline = Instant::now() + RESPONSE_TIMEOUT;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match self.rx.recv_timeout(remaining) {
                    Ok(notification) => {
                        if let Some(bytes) = self.observe(notification) {
                            validate_control_response(&bytes, opcode, measurement)
                                .map_err(|class| ProbeFailure::new(stage_name, class))?;
                            return Ok(());
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        return Err(ProbeFailure::new(stage_name, ResultClass::Timeout));
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        return Err(ProbeFailure::new(stage_name, ResultClass::NativeError));
                    }
                }
            }
        }

        fn wait_for_frame(
            &mut self,
            measurement: u8,
            stage_name: &'static str,
        ) -> Result<(), ProbeFailure> {
            let already_seen = |observation: &Observation| match measurement {
                ECG_MEASUREMENT => observation.ecg_frames > 0,
                ACC_MEASUREMENT => observation.acc_frames > 0,
                _ => false,
            };
            if already_seen(&self.observation) {
                return Ok(());
            }
            let deadline = Instant::now() + FRAME_TIMEOUT;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match self.rx.recv_timeout(remaining) {
                    Ok(notification) => {
                        self.observe(notification);
                        if already_seen(&self.observation) {
                            return Ok(());
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        return Err(ProbeFailure::new(stage_name, ResultClass::Timeout));
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        return Err(ProbeFailure::new(stage_name, ResultClass::NativeError));
                    }
                }
            }
        }
    }

    struct ProbeSession {
        device: BluetoothLEDevice,
        gatt_session: GattSession,
        service: GattDeviceService,
        control: GattCharacteristic,
        data: GattCharacteristic,
        control_token: Option<i64>,
        data_token: Option<i64>,
        control_handler: Option<TypedEventHandler<GattCharacteristic, GattValueChangedEventArgs>>,
        data_handler: Option<TypedEventHandler<GattCharacteristic, GattValueChangedEventArgs>>,
        control_cccd: bool,
        data_cccd: bool,
        cleanup: CleanupGate,
    }

    impl ProbeSession {
        fn cleanup(&mut self) {
            if !self.cleanup.begin() {
                return;
            }
            let _ = write_control(
                &self.control,
                &stop_command(ECG_MEASUREMENT),
                "cleanup-stop-ecg",
            );
            let _ = write_control(
                &self.control,
                &stop_command(ACC_MEASUREMENT),
                "cleanup-stop-acc",
            );
            if let Some(token) = self.data_token.take() {
                let _ = self.data.RemoveValueChanged(token);
            }
            self.data_handler.take();
            if self.data_cccd {
                let _ = configure_cccd(
                    &self.data,
                    GattClientCharacteristicConfigurationDescriptorValue::None,
                    "cleanup-data-cccd",
                );
            }
            if let Some(token) = self.control_token.take() {
                let _ = self.control.RemoveValueChanged(token);
            }
            self.control_handler.take();
            if self.control_cccd {
                let _ = configure_cccd(
                    &self.control,
                    GattClientCharacteristicConfigurationDescriptorValue::None,
                    "cleanup-control-cccd",
                );
            }
            let _ = self.gatt_session.SetMaintainConnection(false);
            let _ = self.service.Close();
            let _ = self.gatt_session.Close();
            let _ = self.device.Close();
        }
    }

    impl Drop for ProbeSession {
        fn drop(&mut self) {
            self.cleanup();
        }
    }

    pub(super) fn run(address: u64) -> Result<Observation, ProbeFailure> {
        let _apartment = stage("com-initialize", ComApartment::initialize)?;
        let device = stage("address-to-device", || {
            wait_operation(
                "address-to-device",
                BluetoothLEDevice::FromBluetoothAddressAsync(address).map_err(|_| {
                    ProbeFailure::new("address-to-device", ResultClass::NativeError)
                })?,
                OPERATION_TIMEOUT,
            )
        })?;
        let bluetooth_device_id = device
            .BluetoothDeviceId()
            .map_err(|_| ProbeFailure::new("gatt-session-create", ResultClass::NativeError))?;
        let gatt_session = stage("gatt-session-create", || {
            wait_operation(
                "gatt-session-create",
                GattSession::FromDeviceIdAsync(&bluetooth_device_id).map_err(|_| {
                    ProbeFailure::new("gatt-session-create", ResultClass::NativeError)
                })?,
                OPERATION_TIMEOUT,
            )
        })?;
        stage("maintain-connection", || {
            gatt_session
                .SetMaintainConnection(true)
                .map_err(|_| ProbeFailure::new("maintain-connection", ResultClass::NativeError))
        })?;
        stage("connection-settle", || {
            thread::sleep(Duration::from_millis(500));
            Ok(())
        })?;
        let service = stage("pmd-service-discovery", || first_service(&device))?;
        stage("pmd-service-access", || {
            let access = wait_operation(
                "pmd-service-access",
                service.RequestAccessAsync().map_err(|_| {
                    ProbeFailure::new("pmd-service-access", ResultClass::NativeError)
                })?,
                OPERATION_TIMEOUT,
            )?;
            if access != DeviceAccessStatus::Allowed {
                return Err(ProbeFailure::new(
                    "pmd-service-access",
                    ResultClass::Rejected,
                ));
            }
            Ok(())
        })?;
        let control = stage("pmd-control-discovery", || {
            first_characteristic(&service, PMD_CONTROL_POINT, "pmd-control-discovery")
        })?;
        let data = stage("pmd-data-discovery", || {
            first_characteristic(&service, PMD_DATA, "pmd-data-discovery")
        })?;
        let (tx, rx) = mpsc::sync_channel(128);
        let mut session = ProbeSession {
            device,
            gatt_session,
            service,
            control,
            data,
            control_token: None,
            data_token: None,
            control_handler: None,
            data_handler: None,
            control_cccd: false,
            data_cccd: false,
            cleanup: CleanupGate::default(),
        };

        stage("pmd-control-cccd", || {
            configure_cccd(
                &session.control,
                GattClientCharacteristicConfigurationDescriptorValue::Indicate,
                "pmd-control-cccd",
            )
        })?;
        session.control_cccd = true;
        let (token, handler) = stage("pmd-control-handler", || {
            attach_handler(
                &session.control,
                Source::Control,
                tx.clone(),
                "pmd-control-handler",
            )
        })?;
        session.control_token = Some(token);
        session.control_handler = Some(handler);

        stage("pmd-data-cccd", || {
            configure_cccd(
                &session.data,
                GattClientCharacteristicConfigurationDescriptorValue::Notify,
                "pmd-data-cccd",
            )
        })?;
        session.data_cccd = true;
        let (token, handler) = stage("pmd-data-handler", || {
            attach_handler(&session.data, Source::Data, tx, "pmd-data-handler")
        })?;
        session.data_token = Some(token);
        session.data_handler = Some(handler);

        let mut inbox = Inbox::new(rx);
        stage("request-ecg-settings", || {
            write_control(
                &session.control,
                &request_settings_command(ECG_MEASUREMENT),
                "request-ecg-settings",
            )
        })?;
        stage("ecg-settings-response", || {
            inbox.wait_for_control(
                PMD_GET_SETTINGS_OPCODE,
                ECG_MEASUREMENT,
                "ecg-settings-response",
            )
        })?;
        stage("start-ecg", || {
            write_control(&session.control, &start_ecg_command(), "start-ecg")
        })?;
        stage("start-ecg-response", || {
            inbox.wait_for_control(
                PMD_START_STREAM_OPCODE,
                ECG_MEASUREMENT,
                "start-ecg-response",
            )
        })?;
        stage("first-ecg-frame", || {
            inbox.wait_for_frame(ECG_MEASUREMENT, "first-ecg-frame")
        })?;
        stage("start-acc", || {
            write_control(
                &session.control,
                &start_accelerometer_command(),
                "start-acc",
            )
        })?;
        stage("start-acc-response", || {
            inbox.wait_for_control(
                PMD_START_STREAM_OPCODE,
                ACC_MEASUREMENT,
                "start-acc-response",
            )
        })?;
        stage("first-acc-frame", || {
            inbox.wait_for_frame(ACC_MEASUREMENT, "first-acc-frame")
        })?;

        session.cleanup();
        Ok(inbox.observation)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    use super::*;

    #[test]
    fn private_address_accepts_common_exact_shapes() {
        assert_eq!(
            parse_private_address("12:34:56:78:9A:BC"),
            Some(0x123456789abc)
        );
        assert_eq!(
            parse_private_address("12-34-56-78-9a-bc"),
            Some(0x123456789abc)
        );
        assert_eq!(parse_private_address("123456789ABC"), Some(0x123456789abc));
    }

    #[test]
    fn private_address_rejects_damaged_or_ambiguous_shapes() {
        for damaged in [
            "",
            "12:34:56:78:9A",
            "12:34:56:78:9A:BC:00",
            "12:34:56:78:9A:GG",
            "12 34 56 78 9A BC",
            "00:00:00:00:00:00",
        ] {
            assert_eq!(parse_private_address(damaged), None, "{damaged}");
        }
    }

    #[test]
    fn failure_report_vocabulary_is_identifier_free_and_closed() {
        assert_eq!(ResultClass::InvalidInput.name(), "invalid-input");
        assert_eq!(ResultClass::NativeError.name(), "native-error");
        assert_eq!(ResultClass::Timeout.name(), "timeout");
        assert_eq!(ResultClass::Rejected.name(), "rejected");
        assert_eq!(ResultClass::Missing.name(), "missing");
        assert_eq!(ResultClass::Protocol.name(), "protocol-error");
    }

    #[test]
    fn control_response_validation_is_exact_and_fail_closed() {
        assert_eq!(
            validate_control_response(&[0xf0, 0x02, 0x00, 0x00], 0x02, 0x00),
            Ok(())
        );
        for damaged in [
            &[0xf0, 0x02, 0x00][..],
            &[0x00, 0x02, 0x00, 0x00],
            &[0xf0, 0x01, 0x00, 0x00],
            &[0xf0, 0x02, 0x02, 0x00],
            &[0xf0, 0x02, 0x00, 0x05],
        ] {
            assert_eq!(
                validate_control_response(damaged, 0x02, 0x00),
                Err(ResultClass::Protocol)
            );
        }
    }

    #[test]
    fn observation_starts_empty() {
        assert_eq!(
            Observation::default(),
            Observation {
                control_callbacks: 0,
                data_callbacks: 0,
                ecg_frames: 0,
                ecg_samples: 0,
                acc_frames: 0,
                acc_samples: 0,
            }
        );
    }

    #[test]
    fn completion_wait_covers_success_error_disconnect_and_timeout_cancel() {
        let (success_tx, success_rx) = mpsc::sync_channel(1);
        success_tx.send(Ok(7_u8)).unwrap();
        assert_eq!(
            receive_completion(&success_rx, Duration::from_millis(10), || {}),
            Ok(7)
        );

        let (error_tx, error_rx) = mpsc::sync_channel::<Result<u8, ResultClass>>(1);
        error_tx.send(Err(ResultClass::NativeError)).unwrap();
        assert_eq!(
            receive_completion(&error_rx, Duration::from_millis(10), || {}),
            Err(ResultClass::NativeError)
        );

        let (disconnected_tx, disconnected_rx) = mpsc::sync_channel::<Result<u8, ResultClass>>(1);
        drop(disconnected_tx);
        assert_eq!(
            receive_completion(&disconnected_rx, Duration::from_millis(10), || {}),
            Err(ResultClass::NativeError)
        );

        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_for_wait = cancelled.clone();
        let (_timeout_tx, timeout_rx) = mpsc::sync_channel::<Result<u8, ResultClass>>(1);
        assert_eq!(
            receive_completion(&timeout_rx, Duration::from_millis(1), move || {
                cancelled_for_wait.store(true, Ordering::SeqCst);
            }),
            Err(ResultClass::Timeout)
        );
        assert!(cancelled.load(Ordering::SeqCst));
    }

    #[test]
    fn cleanup_gate_starts_exactly_once() {
        let mut gate = CleanupGate::default();
        assert!(gate.begin());
        assert!(!gate.begin());
        assert!(!gate.begin());
    }
}
