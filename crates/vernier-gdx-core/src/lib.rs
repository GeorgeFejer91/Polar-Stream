//! Platform-neutral Vernier Go Direct BLE framing and measurement decoding.
//!
//! This is an independent interoperability implementation. Bluetooth ownership,
//! timing, queues, output transports, and UI state deliberately live elsewhere.

use thiserror::Error;
use uuid::Uuid;

pub const COMMAND_CHARACTERISTIC: Uuid = Uuid::from_u128(0xf4bf14a6_c7d5_4b6d_8aa8_df1a7c83adcb);
pub const RESPONSE_CHARACTERISTIC: Uuid = Uuid::from_u128(0xb41e6675_a329_40e0_aa01_44d2f444babe);

pub const GET_STATUS: u8 = 0x10;
pub const START_MEASUREMENTS: u8 = 0x18;
pub const STOP_MEASUREMENTS: u8 = 0x19;
pub const INITIALIZE: u8 = 0x1a;
pub const SET_MEASUREMENT_PERIOD: u8 = 0x1b;
pub const GET_SENSOR_INFO: u8 = 0x50;
pub const GET_AVAILABLE_SENSOR_MASK: u8 = 0x51;
pub const DISCONNECT: u8 = 0x54;
pub const GET_DEVICE_INFO: u8 = 0x55;
pub const GET_DEFAULT_SENSOR_MASK: u8 = 0x56;

const COMMAND_HEADER: u8 = 0x58;
const MEASUREMENT_HEADER: u8 = 0x20;
const FRAME_KIND_MASK: u8 = 0x1f;
const MAX_FRAME_BYTES: usize = 255;
const MAX_ACCUMULATED_BYTES: usize = MAX_FRAME_BYTES * 2;

const DEVICE_ORDER_CODE_RANGE: std::ops::Range<usize> = 6..22;
const DEVICE_NAME_RANGE: std::ops::Range<usize> = 38..70;
const DEVICE_DESCRIPTION_RANGE: std::ops::Range<usize> = 94..158;
const STATUS_RESPONSE_MINIMUM_BYTES: usize = 18;
const SENSOR_DESCRIPTION_RANGE: std::ops::Range<usize> = 14..74;
const SENSOR_UNIT_RANGE: std::ops::Range<usize> = 74..106;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceModel {
    RespirationBelt,
    UnknownGoDirect,
}

impl DeviceModel {
    pub const fn code(self) -> &'static str {
        match self {
            Self::RespirationBelt => "GDX-RB",
            Self::UnknownGoDirect => "GDX",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::RespirationBelt => "Go Direct Respiration Belt",
            Self::UnknownGoDirect => "Go Direct device",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceInfo {
    pub order_code: String,
    pub name: String,
    pub description: String,
    pub model: DeviceModel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceStatus {
    pub main_firmware_version: String,
    pub battery_percent: u8,
    pub charger_state: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumericMeasurementType {
    Real,
    Integer,
    Unknown(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SamplingMode {
    Periodic,
    Aperiodic,
    Unknown(u8),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SensorInfo {
    pub number: u8,
    pub sensor_id: u32,
    pub numeric_type: NumericMeasurementType,
    pub sampling_mode: SamplingMode,
    pub description: String,
    pub unit: String,
    pub uncertainty: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub minimum_period_us: u32,
    pub maximum_period_us: u64,
    pub typical_period_us: u32,
    pub period_granularity_us: u32,
    pub mutual_exclusion_mask: u32,
}

impl SensorInfo {
    pub fn is_respiration_force(&self) -> bool {
        self.description.trim().eq_ignore_ascii_case("force")
            && matches!(self.unit.trim(), "N" | "n")
            && self.sampling_mode == SamplingMode::Periodic
    }

    pub fn has_supported_measurement_shape(&self) -> bool {
        matches!(
            self.numeric_type,
            NumericMeasurementType::Real | NumericMeasurementType::Integer
        ) && matches!(
            self.sampling_mode,
            SamplingMode::Periodic | SamplingMode::Aperiodic
        )
    }

    pub fn has_recording_identity(&self) -> bool {
        self.sensor_id != 0
            && !self.description.trim().is_empty()
            && !self.unit.trim().is_empty()
            && !self.description.contains('\0')
            && !self.unit.contains('\0')
    }
}

pub fn classify_device_model(
    advertised_name: &str,
    order_code: &str,
    description: &str,
) -> DeviceModel {
    if [advertised_name, order_code, description]
        .into_iter()
        .any(|value| {
            let upper = value.to_ascii_uppercase();
            upper.contains("GDX-RB") || upper.contains("RESPIRATION BELT")
        })
    {
        DeviceModel::RespirationBelt
    } else {
        DeviceModel::UnknownGoDirect
    }
}

pub fn available_sensor_numbers(mask: u32) -> impl Iterator<Item = u8> {
    (0..32)
        .filter(move |number| mask & (1_u32 << number) != 0)
        .map(|number| number as u8)
}

pub fn decode_sensor_mask_response(bytes: &[u8], command_id: u8) -> Result<u32, ProtocolError> {
    ensure_response(bytes, command_id, 10)?;
    Ok(u32::from_le_bytes(bytes[6..10].try_into().unwrap()))
}

pub fn decode_device_info_response(
    bytes: &[u8],
    advertised_name: &str,
) -> Result<DeviceInfo, ProtocolError> {
    ensure_response(bytes, GET_DEVICE_INFO, DEVICE_DESCRIPTION_RANGE.end)?;
    let order_code = fixed_text(bytes, DEVICE_ORDER_CODE_RANGE);
    let protocol_name = fixed_text(bytes, DEVICE_NAME_RANGE);
    let description = fixed_text(bytes, DEVICE_DESCRIPTION_RANGE);
    let model = classify_device_model(advertised_name, &order_code, &description);
    Ok(DeviceInfo {
        order_code,
        name: if protocol_name.is_empty() {
            advertised_name.trim().to_string()
        } else {
            protocol_name
        },
        description,
        model,
    })
}

/// Decode the stable status fields documented by Vernier's BSD-licensed
/// Go Direct JavaScript implementation. The main firmware contract is exposed
/// as major.minor because the public JavaScript API does not expose a build
/// component for that processor.
pub fn decode_status_response(bytes: &[u8]) -> Result<DeviceStatus, ProtocolError> {
    ensure_response(bytes, GET_STATUS, STATUS_RESPONSE_MINIMUM_BYTES)?;
    Ok(DeviceStatus {
        main_firmware_version: format!("{}.{}", bytes[8], bytes[9]),
        battery_percent: bytes[16],
        charger_state: bytes[17],
    })
}

pub fn decode_sensor_info_response(bytes: &[u8]) -> Result<SensorInfo, ProtocolError> {
    ensure_response(bytes, GET_SENSOR_INFO, 154)?;
    let number = bytes[6];
    let sensor_id = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if sensor_id == 0 {
        return Err(ProtocolError::InvalidSensorMetadata(number));
    }
    let numeric_type = match bytes[12] {
        0 => NumericMeasurementType::Real,
        1 => NumericMeasurementType::Integer,
        value => NumericMeasurementType::Unknown(value),
    };
    let sampling_mode = match bytes[13] {
        0 => SamplingMode::Periodic,
        1 => SamplingMode::Aperiodic,
        value => SamplingMode::Unknown(value),
    };
    Ok(SensorInfo {
        number,
        sensor_id,
        numeric_type,
        sampling_mode,
        description: fixed_text(bytes, SENSOR_DESCRIPTION_RANGE),
        unit: fixed_text(bytes, SENSOR_UNIT_RANGE),
        uncertainty: f64::from_le_bytes(bytes[106..114].try_into().unwrap()),
        minimum: f64::from_le_bytes(bytes[114..122].try_into().unwrap()),
        maximum: f64::from_le_bytes(bytes[122..130].try_into().unwrap()),
        minimum_period_us: u32::from_le_bytes(bytes[130..134].try_into().unwrap()),
        maximum_period_us: u64::from_le_bytes(bytes[134..142].try_into().unwrap()),
        typical_period_us: u32::from_le_bytes(bytes[142..146].try_into().unwrap()),
        period_granularity_us: u32::from_le_bytes(bytes[146..150].try_into().unwrap()),
        mutual_exclusion_mask: u32::from_le_bytes(bytes[150..154].try_into().unwrap()),
    })
}

pub fn select_respiration_force_sensor(
    device: &DeviceInfo,
    sensors: &[SensorInfo],
) -> Result<SensorInfo, ProtocolError> {
    if device.model != DeviceModel::RespirationBelt {
        return Err(ProtocolError::UnsupportedDeviceModel(
            device.order_code.clone(),
        ));
    }
    let mut matching = sensors
        .iter()
        .filter(|sensor| sensor.number == 1 && sensor.is_respiration_force());
    let selected = matching
        .next()
        .ok_or(ProtocolError::RespirationForceChannelMissing)?;
    if matching.next().is_some() {
        return Err(ProtocolError::RespirationForceChannelAmbiguous);
    }
    Ok(selected.clone())
}

/// Validates and returns every numeric measurement channel exposed by a
/// GDX-RB. The complete available-sensor mask is a recording contract: an
/// unsupported, duplicate, or mutually exclusive channel fails setup instead
/// of being omitted silently.
pub fn select_respiration_belt_sensors(
    device: &DeviceInfo,
    sensors: &[SensorInfo],
) -> Result<Vec<SensorInfo>, ProtocolError> {
    let force = select_respiration_force_sensor(device, sensors)?;
    let mut selected = sensors.to_vec();
    selected.sort_by_key(|sensor| sensor.number);

    let mut selected_mask = 0_u32;
    for sensor in &selected {
        if sensor.number >= 32 {
            return Err(ProtocolError::SensorNumberOutOfRange(sensor.number));
        }
        if !sensor.has_supported_measurement_shape() {
            return Err(ProtocolError::UnsupportedSensorShape(sensor.number));
        }
        if !sensor.has_recording_identity() {
            return Err(ProtocolError::InvalidRecordingMetadata(sensor.number));
        }
        let bit = 1_u32 << sensor.number;
        if selected_mask & bit != 0 {
            return Err(ProtocolError::DuplicateSensorNumber(sensor.number));
        }
        selected_mask |= bit;
    }
    for sensor in &selected {
        let own_bit = 1_u32 << sensor.number;
        let conflicts = sensor.mutual_exclusion_mask & selected_mask & !own_bit;
        if conflicts != 0 {
            return Err(ProtocolError::MutuallyExclusiveSensors {
                sensor: sensor.number,
                conflicts,
            });
        }
    }
    if !selected.iter().any(|sensor| sensor.number == force.number) {
        return Err(ProtocolError::RespirationForceChannelMissing);
    }
    Ok(selected)
}

fn ensure_response(bytes: &[u8], command_id: u8, minimum: usize) -> Result<(), ProtocolError> {
    validate_frame(bytes)?;
    if bytes[0] == MEASUREMENT_HEADER || bytes[4] != command_id {
        return Err(ProtocolError::UnexpectedResponse {
            expected: command_id,
            actual: bytes[4],
        });
    }
    if bytes.len() < minimum {
        return Err(ProtocolError::ResponseTooShort {
            command: command_id,
            expected: minimum,
            actual: bytes.len(),
        });
    }
    Ok(())
}

fn fixed_text(bytes: &[u8], range: std::ops::Range<usize>) -> String {
    let retained = bytes[range]
        .iter()
        .copied()
        .filter(|byte| *byte != 0)
        .collect::<Vec<_>>();
    String::from_utf8_lossy(&retained).trim().to_string()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandCounter(u8);

impl Default for CommandCounter {
    fn default() -> Self {
        Self(0xff)
    }
}

impl CommandCounter {
    pub fn next_value(&mut self) -> u8 {
        self.0 = self.0.wrapping_sub(1);
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Command {
    pub id: u8,
    pub bytes: Vec<u8>,
}

impl Command {
    pub fn initialize(counter: &mut CommandCounter) -> Self {
        Self::new(
            counter,
            INITIALIZE,
            &[
                0xa5, 0x4a, 0x06, 0x49, 0x07, 0x48, 0x08, 0x47, 0x09, 0x46, 0x0a, 0x45, 0x0b, 0x44,
                0x0c, 0x43, 0x0d, 0x42, 0x0e, 0x41,
            ],
        )
    }

    pub fn get_status(counter: &mut CommandCounter) -> Self {
        Self::new(counter, GET_STATUS, &[])
    }

    pub fn get_device_info(counter: &mut CommandCounter) -> Self {
        Self::new(counter, GET_DEVICE_INFO, &[])
    }

    pub fn get_available_sensor_mask(counter: &mut CommandCounter) -> Self {
        Self::new(counter, GET_AVAILABLE_SENSOR_MASK, &[])
    }

    pub fn get_default_sensor_mask(counter: &mut CommandCounter) -> Self {
        Self::new(counter, GET_DEFAULT_SENSOR_MASK, &[])
    }

    pub fn get_sensor_info(counter: &mut CommandCounter, sensor_number: u8) -> Self {
        Self::new(counter, GET_SENSOR_INFO, &[sensor_number])
    }

    pub fn set_measurement_period(
        counter: &mut CommandCounter,
        period_us: u32,
    ) -> Result<Self, ProtocolError> {
        if !(1_000..=60_000_000).contains(&period_us) {
            return Err(ProtocolError::InvalidPeriod(period_us));
        }
        let mut payload = vec![0xff, 0x00];
        payload.extend(period_us.to_le_bytes());
        payload.extend([0_u8; 4]);
        Ok(Self::new(counter, SET_MEASUREMENT_PERIOD, &payload))
    }

    pub fn start_measurements(counter: &mut CommandCounter, sensor_mask: u32) -> Self {
        let mut payload = vec![0xff, 0x01];
        payload.extend(sensor_mask.to_le_bytes());
        payload.extend([0_u8; 8]);
        Self::new(counter, START_MEASUREMENTS, &payload)
    }

    pub fn stop_measurements(counter: &mut CommandCounter) -> Self {
        Self::new(
            counter,
            STOP_MEASUREMENTS,
            &[0xff, 0x00, 0xff, 0xff, 0xff, 0xff],
        )
    }

    pub fn disconnect(counter: &mut CommandCounter) -> Self {
        Self::new(counter, DISCONNECT, &[])
    }

    pub fn new(counter: &mut CommandCounter, id: u8, payload: &[u8]) -> Self {
        let length = payload.len() + 5;
        assert!(
            length <= MAX_FRAME_BYTES,
            "Go Direct command exceeds one-byte length"
        );
        let mut bytes = Vec::with_capacity(length);
        bytes.extend([COMMAND_HEADER, length as u8, counter.next_value(), 0, id]);
        bytes.extend(payload);
        bytes[3] = checksum(&bytes);
        Self { id, bytes }
    }

    pub fn chunks(&self, mtu_payload: usize) -> impl Iterator<Item = &[u8]> {
        self.bytes.chunks(mtu_payload.max(1))
    }
}

pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SampleEncoding {
    Float32,
    Integer32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SensorSamples {
    pub sensor_number: u8,
    /// Float32 values and Int32 values are both losslessly widened to f64.
    /// This lets a heterogeneous aggregate recording retain the exact device
    /// number without constraining every channel to the narrower LSL format.
    pub values: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Measurement {
    Samples {
        encoding: SampleEncoding,
        sensors: Vec<SensorSamples>,
    },
    StartTime(Vec<u8>),
    Dropped(Vec<u8>),
    Period(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Response(Vec<u8>),
    Measurement(Measurement),
}

#[derive(Default)]
pub struct FrameAccumulator {
    bytes: Vec<u8>,
}

impl FrameAccumulator {
    pub fn push(&mut self, fragment: &[u8]) -> Result<Vec<Vec<u8>>, ProtocolError> {
        if self.bytes.len().saturating_add(fragment.len()) > MAX_ACCUMULATED_BYTES {
            self.bytes.clear();
            return Err(ProtocolError::AccumulatorOverflow);
        }
        self.bytes.extend_from_slice(fragment);
        let mut frames = Vec::new();
        loop {
            if self.bytes.len() < 2 {
                break;
            }
            if !is_frame_header(self.bytes[0]) {
                let invalid = self.bytes.remove(0);
                return Err(ProtocolError::InvalidHeader(invalid));
            }
            let length = self.bytes[1] as usize;
            if length < 5 {
                self.bytes.clear();
                return Err(ProtocolError::InvalidLength(length));
            }
            if self.bytes.len() < length {
                break;
            }
            frames.push(self.bytes.drain(..length).collect());
        }
        Ok(frames)
    }
}

pub fn decode_frame(bytes: &[u8]) -> Result<Frame, ProtocolError> {
    validate_frame(bytes)?;
    if bytes[0] != MEASUREMENT_HEADER {
        return Ok(Frame::Response(bytes.to_vec()));
    }
    let subtype = bytes[4];
    let measurement = match subtype {
        0x06 => decode_masked_f32(bytes, false)?,
        0x07 => decode_masked_f32(bytes, true)?,
        0x08 | 0x0a => decode_single(bytes, SampleEncoding::Float32)?,
        0x09 | 0x0b => decode_single(bytes, SampleEncoding::Integer32)?,
        0x0c => Measurement::StartTime(bytes[5..].to_vec()),
        0x0d => Measurement::Dropped(bytes[5..].to_vec()),
        0x0e => Measurement::Period(bytes[5..].to_vec()),
        other => return Err(ProtocolError::UnsupportedMeasurement(other)),
    };
    Ok(Frame::Measurement(measurement))
}

fn validate_frame(bytes: &[u8]) -> Result<(), ProtocolError> {
    if bytes.len() < 5 {
        return Err(ProtocolError::Truncated);
    }
    if !is_frame_header(bytes[0]) {
        return Err(ProtocolError::InvalidHeader(bytes[0]));
    }
    if bytes[1] as usize != bytes.len() {
        return Err(ProtocolError::LengthMismatch {
            declared: bytes[1] as usize,
            actual: bytes.len(),
        });
    }
    Ok(())
}

fn is_frame_header(header: u8) -> bool {
    header == MEASUREMENT_HEADER || header & FRAME_KIND_MASK == COMMAND_HEADER & FRAME_KIND_MASK
}

fn decode_masked_f32(bytes: &[u8], wide: bool) -> Result<Measurement, ProtocolError> {
    let (mask, count_index, values_index) = if wide {
        require(bytes, 11)?;
        (u32::from_le_bytes(bytes[5..9].try_into().unwrap()), 9, 11)
    } else {
        require(bytes, 9)?;
        (
            u16::from_le_bytes(bytes[5..7].try_into().unwrap()) as u32,
            7,
            9,
        )
    };
    let sensor_numbers = (0..32)
        .filter(|number| mask & (1_u32 << number) != 0)
        .map(|number| number as u8)
        .collect::<Vec<_>>();
    if sensor_numbers.is_empty() {
        return Err(ProtocolError::EmptySensorMask);
    }
    let value_count = (bytes[count_index] as usize)
        .checked_mul(sensor_numbers.len())
        .ok_or(ProtocolError::Truncated)?;
    let raw = parse_values(&bytes[values_index..], value_count, SampleEncoding::Float32)?;
    if raw.len() % sensor_numbers.len() != 0 {
        return Err(ProtocolError::UnevenInterleave {
            values: raw.len(),
            sensors: sensor_numbers.len(),
        });
    }
    let mut sensors = sensor_numbers
        .into_iter()
        .map(|sensor_number| SensorSamples {
            sensor_number,
            values: Vec::with_capacity(raw.len()),
        })
        .collect::<Vec<_>>();
    let sensor_count = sensors.len();
    for (index, value) in raw.into_iter().enumerate() {
        sensors[index % sensor_count].values.push(value);
    }
    Ok(Measurement::Samples {
        encoding: SampleEncoding::Float32,
        sensors,
    })
}

fn decode_single(bytes: &[u8], encoding: SampleEncoding) -> Result<Measurement, ProtocolError> {
    require(bytes, 8)?;
    let sensor_number = bytes[6];
    let value_count = bytes[7] as usize;
    let values = parse_values(&bytes[8..], value_count, encoding)?;
    Ok(Measurement::Samples {
        encoding,
        sensors: vec![SensorSamples {
            sensor_number,
            values,
        }],
    })
}

fn parse_values(
    bytes: &[u8],
    value_count: usize,
    encoding: SampleEncoding,
) -> Result<Vec<f64>, ProtocolError> {
    let expected = value_count.checked_mul(4).ok_or(ProtocolError::Truncated)?;
    if bytes.len() < expected {
        return Err(ProtocolError::Truncated);
    }
    Ok(bytes[..expected]
        .chunks_exact(4)
        .map(|chunk| {
            let raw: [u8; 4] = chunk.try_into().unwrap();
            match encoding {
                SampleEncoding::Float32 => f64::from(f32::from_le_bytes(raw)),
                SampleEncoding::Integer32 => f64::from(i32::from_le_bytes(raw)),
            }
        })
        .collect())
}

fn require(bytes: &[u8], minimum: usize) -> Result<(), ProtocolError> {
    if bytes.len() < minimum {
        Err(ProtocolError::Truncated)
    } else {
        Ok(())
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ProtocolError {
    #[error("Go Direct frame is truncated")]
    Truncated,
    #[error("Go Direct frame header 0x{0:02x} is invalid")]
    InvalidHeader(u8),
    #[error("Go Direct frame length {0} is invalid")]
    InvalidLength(usize),
    #[error("Go Direct frame declares {declared} bytes but contains {actual}")]
    LengthMismatch { declared: usize, actual: usize },
    #[error("Go Direct response accumulator exceeded its bounded capacity")]
    AccumulatorOverflow,
    #[error("Go Direct measurement subtype 0x{0:02x} is unsupported")]
    UnsupportedMeasurement(u8),
    #[error("Go Direct measurement has an empty sensor mask")]
    EmptySensorMask,
    #[error("Go Direct measurement has {values} values for {sensors} interleaved sensors")]
    UnevenInterleave { values: usize, sensors: usize },
    #[error("Go Direct measurement period {0} microseconds is outside the supported bound")]
    InvalidPeriod(u32),
    #[error("expected Go Direct response 0x{expected:02x}, received 0x{actual:02x}")]
    UnexpectedResponse { expected: u8, actual: u8 },
    #[error(
        "Go Direct response 0x{command:02x} requires at least {expected} bytes, received {actual}"
    )]
    ResponseTooShort {
        command: u8,
        expected: usize,
        actual: usize,
    },
    #[error("Go Direct sensor {0} returned invalid metadata")]
    InvalidSensorMetadata(u8),
    #[error("Go Direct model '{0}' is not a supported respiration belt")]
    UnsupportedDeviceModel(String),
    #[error("the GDX-RB did not report channel 1 as periodic Force (N)")]
    RespirationForceChannelMissing,
    #[error("the GDX-RB reported channel 1 more than once")]
    RespirationForceChannelAmbiguous,
    #[error("Go Direct sensor {0} has an unsupported numeric type or sampling mode")]
    UnsupportedSensorShape(u8),
    #[error("Go Direct sensor {0} has no stable recording label, unit, or sensor ID")]
    InvalidRecordingMetadata(u8),
    #[error("Go Direct sensor number {0} is outside the 32-bit measurement mask")]
    SensorNumberOutOfRange(u8),
    #[error("Go Direct sensor number {0} was reported more than once")]
    DuplicateSensorNumber(u8),
    #[error(
        "Go Direct sensor {sensor} excludes another requested sensor in mask 0x{conflicts:08x}"
    )]
    MutuallyExclusiveSensors { sensor: u8, conflicts: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measurement_frame(subtype: u8, body: &[u8]) -> Vec<u8> {
        let mut frame = vec![MEASUREMENT_HEADER, 0, 0, 0, subtype];
        frame.extend(body);
        frame[1] = frame.len() as u8;
        frame
    }

    fn response_frame(command: u8, length: usize) -> Vec<u8> {
        let mut frame = vec![0_u8; length];
        frame[0] = COMMAND_HEADER;
        frame[1] = length as u8;
        frame[4] = command;
        frame
    }

    #[test]
    fn decodes_main_firmware_and_battery_from_status() {
        let mut response = response_frame(GET_STATUS, STATUS_RESPONSE_MINIMUM_BYTES);
        response[8] = 2;
        response[9] = 7;
        response[16] = 83;
        response[17] = 1;

        assert_eq!(
            decode_status_response(&response).unwrap(),
            DeviceStatus {
                main_firmware_version: "2.7".into(),
                battery_percent: 83,
                charger_state: 1,
            }
        );
    }

    #[test]
    fn rejects_truncated_status_metadata() {
        let response = response_frame(GET_STATUS, STATUS_RESPONSE_MINIMUM_BYTES - 1);
        assert!(matches!(
            decode_status_response(&response),
            Err(ProtocolError::ResponseTooShort {
                command: GET_STATUS,
                expected: STATUS_RESPONSE_MINIMUM_BYTES,
                actual: 17,
            })
        ));
    }

    fn write_fixed(frame: &mut [u8], range: std::ops::Range<usize>, value: &str) {
        let bytes = value.as_bytes();
        let count = bytes.len().min(range.len());
        frame[range.start..range.start + count].copy_from_slice(&bytes[..count]);
    }

    fn respiration_sensor_response(number: u8, description: &str, unit: &str) -> Vec<u8> {
        let mut frame = response_frame(GET_SENSOR_INFO, 154);
        frame[6] = number;
        frame[8..12].copy_from_slice(&42_u32.to_le_bytes());
        write_fixed(&mut frame, SENSOR_DESCRIPTION_RANGE, description);
        write_fixed(&mut frame, SENSOR_UNIT_RANGE, unit);
        frame[106..114].copy_from_slice(&0.01_f64.to_le_bytes());
        frame[122..130].copy_from_slice(&50.0_f64.to_le_bytes());
        frame[130..134].copy_from_slice(&50_000_u32.to_le_bytes());
        frame[134..142].copy_from_slice(&60_000_000_u64.to_le_bytes());
        frame[142..146].copy_from_slice(&100_000_u32.to_le_bytes());
        frame[146..150].copy_from_slice(&1_000_u32.to_le_bytes());
        frame
    }

    #[test]
    fn command_counter_descends_and_checksum_covers_the_complete_packet() {
        let mut counter = CommandCounter::default();
        let command = Command::get_status(&mut counter);
        assert_eq!(command.bytes, [0x58, 5, 0xfe, 0x6b, GET_STATUS]);
        assert_eq!(checksum(&command.bytes), 0xd6);
        assert_eq!(Command::get_device_info(&mut counter).bytes[2], 0xfd);
    }

    #[test]
    fn long_initialization_packet_is_split_without_changing_bytes() {
        let command = Command::initialize(&mut CommandCounter::default());
        let chunks = command.chunks(20).collect::<Vec<_>>();
        assert_eq!(command.bytes.len(), 25);
        assert_eq!(
            chunks.iter().map(|chunk| chunk.len()).collect::<Vec<_>>(),
            [20, 5]
        );
        assert_eq!(chunks.concat(), command.bytes);
    }

    #[test]
    fn fragmented_and_coalesced_notifications_are_reassembled() {
        let first = measurement_frame(0x08, &[0, 1, 1, 0, 0, 0, 0]);
        let second = measurement_frame(0x08, &[0, 1, 1, 0, 0, 128, 63]);
        let mut accumulator = FrameAccumulator::default();
        assert!(accumulator.push(&first[..4]).unwrap().is_empty());
        let mut tail = first[4..].to_vec();
        tail.extend(&second);
        assert_eq!(accumulator.push(&tail).unwrap(), vec![first, second]);
    }

    #[test]
    fn physical_ble_response_header_is_accepted() {
        let response = vec![
            0xb8, 0x1a, 0x00, 0xe0, INITIALIZE, 0xfe, 0x55, 0xaa, 0x56, 0xa9, 0x57, 0xa8, 0x58,
            0xa7, 0x59, 0xa6, 0x5a, 0xa5, 0x5b, 0xa4, 0x5c, 0xa3, 0x5d, 0xa2, 0x5e, 0xa1,
        ];
        let mut accumulator = FrameAccumulator::default();
        assert_eq!(accumulator.push(&response).unwrap(), vec![response.clone()]);
        assert!(matches!(decode_frame(&response), Ok(Frame::Response(_))));
    }

    #[test]
    fn response_header_family_accepts_status_and_rejects_unrelated_bytes() {
        assert!(is_frame_header(0x58));
        assert!(is_frame_header(0x98));
        assert!(is_frame_header(0xb8));
        assert!(is_frame_header(MEASUREMENT_HEADER));
        assert!(!is_frame_header(0x00));
        assert!(!is_frame_header(0xff));
    }

    #[test]
    fn normal_float_packet_deinterleaves_selected_sensors() {
        let mut body = vec![0b0000_0110, 0, 2, 0];
        for value in [1.0_f32, 10.0, 2.0, 20.0] {
            body.extend(value.to_le_bytes());
        }
        let decoded = decode_frame(&measurement_frame(0x06, &body)).unwrap();
        assert_eq!(
            decoded,
            Frame::Measurement(Measurement::Samples {
                encoding: SampleEncoding::Float32,
                sensors: vec![
                    SensorSamples {
                        sensor_number: 1,
                        values: vec![1.0, 2.0]
                    },
                    SensorSamples {
                        sensor_number: 2,
                        values: vec![10.0, 20.0]
                    },
                ],
            })
        );
    }

    #[test]
    fn single_integer_packet_is_converted_without_alignment_assumptions() {
        let mut body = vec![0, 7, 2];
        body.extend((-2_i32).to_le_bytes());
        body.extend(42_i32.to_le_bytes());
        let decoded = decode_frame(&measurement_frame(0x09, &body)).unwrap();
        assert_eq!(
            decoded,
            Frame::Measurement(Measurement::Samples {
                encoding: SampleEncoding::Integer32,
                sensors: vec![SensorSamples {
                    sensor_number: 7,
                    values: vec![-2.0, 42.0]
                }],
            })
        );
    }

    #[test]
    fn device_info_identifies_the_respiration_belt_without_a_name_guess() {
        let mut response = response_frame(GET_DEVICE_INFO, 158);
        write_fixed(&mut response, DEVICE_ORDER_CODE_RANGE, "GDX-RB");
        write_fixed(&mut response, DEVICE_NAME_RANGE, "GDX-RB 123456");
        write_fixed(
            &mut response,
            DEVICE_DESCRIPTION_RANGE,
            "Go Direct Respiration Belt",
        );

        let info = decode_device_info_response(&response, "unhelpful advertisement").unwrap();
        assert_eq!(info.model, DeviceModel::RespirationBelt);
        assert_eq!(info.order_code, "GDX-RB");
        assert_eq!(info.name, "GDX-RB 123456");
    }

    #[test]
    fn sensor_metadata_preserves_force_units_periods_and_bounds() {
        let info =
            decode_sensor_info_response(&respiration_sensor_response(1, "Force", "N")).unwrap();
        assert!(info.is_respiration_force());
        assert_eq!(info.number, 1);
        assert_eq!(info.typical_period_us, 100_000);
        assert_eq!(info.minimum_period_us, 50_000);
        assert_eq!(info.maximum_period_us, 60_000_000);
        assert_eq!(info.maximum, 50.0);
    }

    #[test]
    fn respiration_selection_rejects_wrong_models_and_wrong_units() {
        let belt = DeviceInfo {
            order_code: "GDX-RB".into(),
            name: "GDX-RB TEST".into(),
            description: "Go Direct Respiration Belt".into(),
            model: DeviceModel::RespirationBelt,
        };
        let force =
            decode_sensor_info_response(&respiration_sensor_response(1, "Force", "N")).unwrap();
        assert_eq!(
            select_respiration_force_sensor(&belt, &[force])
                .unwrap()
                .number,
            1
        );

        let other = DeviceInfo {
            model: DeviceModel::UnknownGoDirect,
            ..belt.clone()
        };
        assert!(matches!(
            select_respiration_force_sensor(&other, &[]),
            Err(ProtocolError::UnsupportedDeviceModel(_))
        ));
        let wrong =
            decode_sensor_info_response(&respiration_sensor_response(1, "Force", "m/s²")).unwrap();
        assert!(matches!(
            select_respiration_force_sensor(&belt, &[wrong]),
            Err(ProtocolError::RespirationForceChannelMissing)
        ));
    }

    #[test]
    fn respiration_belt_selection_preserves_every_compatible_channel_in_number_order() {
        let belt = DeviceInfo {
            order_code: "GDX-RB".into(),
            name: "GDX-RB TEST".into(),
            description: "Go Direct Respiration Belt".into(),
            model: DeviceModel::RespirationBelt,
        };
        let force =
            decode_sensor_info_response(&respiration_sensor_response(1, "Force", "N")).unwrap();
        let mut rate = respiration_sensor_response(2, "Respiration Rate", "breaths/min");
        rate[13] = 1;
        let rate = decode_sensor_info_response(&rate).unwrap();
        let mut steps = respiration_sensor_response(3, "Steps", "count");
        steps[12] = 1;
        steps[13] = 1;
        let steps = decode_sensor_info_response(&steps).unwrap();

        let selected = select_respiration_belt_sensors(&belt, &[steps, force, rate]).unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|sensor| (sensor.number, sensor.description.as_str()))
                .collect::<Vec<_>>(),
            [(1, "Force"), (2, "Respiration Rate"), (3, "Steps")]
        );
    }

    #[test]
    fn respiration_belt_selection_rejects_silent_channel_omission() {
        let belt = DeviceInfo {
            order_code: "GDX-RB".into(),
            name: "GDX-RB TEST".into(),
            description: "Go Direct Respiration Belt".into(),
            model: DeviceModel::RespirationBelt,
        };
        let force =
            decode_sensor_info_response(&respiration_sensor_response(1, "Force", "N")).unwrap();
        let mut unknown = respiration_sensor_response(2, "Respiration Rate", "breaths/min");
        unknown[12] = 9;
        let unknown = decode_sensor_info_response(&unknown).unwrap();
        assert!(matches!(
            select_respiration_belt_sensors(&belt, &[force.clone(), unknown]),
            Err(ProtocolError::UnsupportedSensorShape(2))
        ));

        let mut unlabeled = respiration_sensor_response(2, "Respiration Rate", "");
        unlabeled[13] = 1;
        let unlabeled = decode_sensor_info_response(&unlabeled).unwrap();
        assert!(matches!(
            select_respiration_belt_sensors(&belt, &[force.clone(), unlabeled]),
            Err(ProtocolError::InvalidRecordingMetadata(2))
        ));

        let mut out_of_range = force.clone();
        out_of_range.number = 32;
        assert!(matches!(
            select_respiration_belt_sensors(&belt, &[force.clone(), out_of_range]),
            Err(ProtocolError::SensorNumberOutOfRange(32))
        ));

        let mut exclusive = force.clone();
        exclusive.mutual_exclusion_mask = 1 << 2;
        let rate = decode_sensor_info_response(&respiration_sensor_response(
            2,
            "Respiration Rate",
            "breaths/min",
        ))
        .unwrap();
        assert!(matches!(
            select_respiration_belt_sensors(&belt, &[exclusive, rate]),
            Err(ProtocolError::MutuallyExclusiveSensors {
                sensor: 1,
                conflicts: 4
            })
        ));
    }

    #[test]
    fn integer_measurements_are_losslessly_widened() {
        let value = 2_000_000_001_i32;
        let mut body = vec![0, 3, 1];
        body.extend(value.to_le_bytes());
        let decoded = decode_frame(&measurement_frame(0x09, &body)).unwrap();
        let Frame::Measurement(Measurement::Samples { sensors, .. }) = decoded else {
            panic!("expected samples");
        };
        assert_eq!(sensors[0].values, [f64::from(value)]);
    }
}
