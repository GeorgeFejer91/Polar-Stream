//! Platform-neutral Polar H10 packet decoding and rolling metrics.

use serde::Serialize;
use thiserror::Error;

pub const ECG_MEASUREMENT: u8 = 0x00;
pub const ACC_MEASUREMENT: u8 = 0x02;
const PMD_HEADER_SIZE: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccSample {
    pub x_mg: i16,
    pub y_mg: i16,
    pub z_mg: i16,
}

impl AccSample {
    pub fn magnitude_g(self) -> f32 {
        let x = f32::from(self.x_mg) / 1_000.0;
        let y = f32::from(self.y_mg) / 1_000.0;
        let z = f32::from(self.z_mg) / 1_000.0;
        (x * x + y * y + z * z).sqrt()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeartRateFrame {
    pub beats_per_minute: u16,
    pub rr_intervals_ms: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PmdFrame {
    Ecg {
        sensor_timestamp_ns: u64,
        microvolts: Vec<i32>,
    },
    Accelerometer {
        sensor_timestamp_ns: u64,
        samples: Vec<AccSample>,
    },
}

#[derive(Debug, Error, PartialEq)]
pub enum ProtocolError {
    #[error("PMD frame is shorter than its 10-byte header")]
    FrameTooShort,
    #[error("ECG payload is not a sequence of signed 24-bit samples")]
    InvalidEcgLength,
    #[error("accelerometer payload has an invalid length")]
    InvalidAccelerometerLength,
    #[error("unsupported PMD measurement or frame type")]
    UnsupportedFrame,
}

pub fn decode_heart_rate(bytes: &[u8]) -> HeartRateFrame {
    if bytes.len() < 2 {
        return HeartRateFrame {
            beats_per_minute: 0,
            rr_intervals_ms: Vec::new(),
        };
    }

    let flags = bytes[0];
    let is_u16 = flags & 0x01 != 0;
    let mut cursor = if is_u16 { 3 } else { 2 };
    let beats_per_minute = if is_u16 && bytes.len() >= 3 {
        u16::from_le_bytes([bytes[1], bytes[2]])
    } else {
        u16::from(bytes[1])
    };

    if flags & 0x08 != 0 {
        cursor += 2;
    }

    let mut rr_intervals_ms = Vec::new();
    if flags & 0x10 != 0 {
        while cursor + 1 < bytes.len() {
            let raw = u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]);
            rr_intervals_ms.push(f32::from(raw) * (1_000.0 / 1_024.0));
            cursor += 2;
        }
    }

    HeartRateFrame {
        beats_per_minute,
        rr_intervals_ms,
    }
}

pub fn decode_pmd(bytes: &[u8]) -> Result<PmdFrame, ProtocolError> {
    if bytes.len() < PMD_HEADER_SIZE {
        return Err(ProtocolError::FrameTooShort);
    }

    let timestamp = u64::from_le_bytes(bytes[1..9].try_into().expect("checked PMD header"));
    let frame_type = bytes[9];

    match bytes[0] {
        ECG_MEASUREMENT if frame_type == 0x00 => decode_ecg(timestamp, &bytes[10..]),
        ACC_MEASUREMENT => decode_accelerometer(timestamp, frame_type, &bytes[10..]),
        _ => Err(ProtocolError::UnsupportedFrame),
    }
}

fn decode_ecg(timestamp: u64, payload: &[u8]) -> Result<PmdFrame, ProtocolError> {
    if !payload.len().is_multiple_of(3) {
        return Err(ProtocolError::InvalidEcgLength);
    }

    let microvolts = payload
        .chunks_exact(3)
        .map(|sample| {
            let raw =
                i32::from(sample[0]) | (i32::from(sample[1]) << 8) | (i32::from(sample[2]) << 16);
            if raw & 0x0080_0000 != 0 {
                raw | !0x00ff_ffff
            } else {
                raw
            }
        })
        .collect();

    Ok(PmdFrame::Ecg {
        sensor_timestamp_ns: timestamp,
        microvolts,
    })
}

fn decode_accelerometer(
    timestamp: u64,
    frame_type: u8,
    payload: &[u8],
) -> Result<PmdFrame, ProtocolError> {
    let compressed = frame_type & 0x80 != 0;
    let frame_type_base = frame_type & 0x7f;
    let samples = if !compressed && frame_type_base == 0x01 {
        if !payload.len().is_multiple_of(6) {
            return Err(ProtocolError::InvalidAccelerometerLength);
        }
        payload
            .chunks_exact(6)
            .map(|sample| AccSample {
                x_mg: i16::from_le_bytes([sample[0], sample[1]]),
                y_mg: i16::from_le_bytes([sample[2], sample[3]]),
                z_mg: i16::from_le_bytes([sample[4], sample[5]]),
            })
            .collect()
    } else {
        decode_compressed_accelerometer(payload)?
    };

    Ok(PmdFrame::Accelerometer {
        sensor_timestamp_ns: timestamp,
        samples,
    })
}

fn decode_compressed_accelerometer(payload: &[u8]) -> Result<Vec<AccSample>, ProtocolError> {
    if payload.len() < 6 {
        return Err(ProtocolError::InvalidAccelerometerLength);
    }

    let mut x = i32::from(i16::from_le_bytes([payload[0], payload[1]]));
    let mut y = i32::from(i16::from_le_bytes([payload[2], payload[3]]));
    let mut z = i32::from(i16::from_le_bytes([payload[4], payload[5]]));
    let mut samples = vec![clamped_acc_sample(x, y, z)];
    let mut bit_offset = 0;
    let delta_data = &payload[6..];

    for _ in 0..((delta_data.len() * 8) / 48) {
        x += read_signed_bits(delta_data, &mut bit_offset, 16);
        y += read_signed_bits(delta_data, &mut bit_offset, 16);
        z += read_signed_bits(delta_data, &mut bit_offset, 16);
        samples.push(clamped_acc_sample(x, y, z));
    }
    Ok(samples)
}

fn clamped_acc_sample(x: i32, y: i32, z: i32) -> AccSample {
    AccSample {
        x_mg: x.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
        y_mg: y.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
        z_mg: z.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
    }
}

fn read_signed_bits(bytes: &[u8], bit_offset: &mut usize, width: usize) -> i32 {
    let mut value = 0_u32;
    for shift in 0..width {
        let absolute = *bit_offset + shift;
        let bit = (bytes[absolute / 8] >> (absolute % 8)) & 1;
        value |= u32::from(bit) << shift;
    }
    *bit_offset += width;
    if value & (1 << (width - 1)) != 0 {
        (value | (!0_u32 << width)) as i32
    } else {
        value as i32
    }
}

pub fn start_ecg_command() -> [u8; 10] {
    [0x02, ECG_MEASUREMENT, 0x00, 0x01, 130, 0, 0x01, 0x01, 14, 0]
}

pub fn start_accelerometer_command() -> [u8; 14] {
    [
        0x02,
        ACC_MEASUREMENT,
        0x02,
        0x01,
        8,
        0,
        0x00,
        0x01,
        200,
        0,
        0x01,
        0x01,
        16,
        0,
    ]
}

pub fn stop_command(measurement: u8) -> [u8; 2] {
    [0x03, measurement]
}

/// Small rolling RR store used by applications that opt into RMSSD output.
/// Acquisition code deliberately does not depend on this derived metric.
#[derive(Default)]
pub struct RrTracker {
    intervals: Vec<f32>,
}

impl RrTracker {
    pub fn push(&mut self, value: f32) {
        if (250.0..=2_500.0).contains(&value) {
            self.intervals.push(value);
            if self.intervals.len() > 60 {
                self.intervals.remove(0);
            }
        }
    }

    pub fn rmssd(&self) -> Option<f32> {
        if self.intervals.len() < 2 {
            return None;
        }
        let squared_sum: f32 = self
            .intervals
            .windows(2)
            .map(|pair| {
                let difference = pair[1] - pair[0];
                difference * difference
            })
            .sum();
        Some((squared_sum / (self.intervals.len() - 1) as f32).sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_signed_24_bit_ecg() {
        let mut frame = vec![ECG_MEASUREMENT];
        frame.extend_from_slice(&42_u64.to_le_bytes());
        frame.push(0);
        frame.extend_from_slice(&[1, 0, 0, 0xff, 0xff, 0xff, 0, 0, 0x80]);

        assert_eq!(
            decode_pmd(&frame).unwrap(),
            PmdFrame::Ecg {
                sensor_timestamp_ns: 42,
                microvolts: vec![1, -1, -8_388_608],
            }
        );
    }

    #[test]
    fn decodes_uncompressed_accelerometer() {
        let mut frame = vec![ACC_MEASUREMENT];
        frame.extend_from_slice(&7_u64.to_le_bytes());
        frame.push(1);
        for value in [1_i16, -2, 3, 4, 5, -6] {
            frame.extend_from_slice(&value.to_le_bytes());
        }

        let PmdFrame::Accelerometer { samples, .. } = decode_pmd(&frame).unwrap() else {
            panic!("expected accelerometer frame")
        };
        assert_eq!(samples.len(), 2);
        assert_eq!(
            samples[0],
            AccSample {
                x_mg: 1,
                y_mg: -2,
                z_mg: 3
            }
        );
        assert_eq!(
            samples[1],
            AccSample {
                x_mg: 4,
                y_mg: 5,
                z_mg: -6
            }
        );
    }

    #[test]
    fn decodes_heart_rate_and_rr() {
        let sample = decode_heart_rate(&[0x10, 60, 0x00, 0x04]);
        assert_eq!(sample.beats_per_minute, 60);
        assert_eq!(sample.rr_intervals_ms, vec![1_000.0]);
    }

    #[test]
    fn commands_match_h10_settings() {
        assert_eq!(start_ecg_command()[..2], [0x02, ECG_MEASUREMENT]);
        assert_eq!(
            start_accelerometer_command()[..6],
            [0x02, ACC_MEASUREMENT, 0x02, 0x01, 8, 0]
        );
        assert_eq!(stop_command(ACC_MEASUREMENT), [0x03, ACC_MEASUREMENT]);
    }

    #[test]
    fn computes_rmssd_from_accepted_intervals() {
        let mut tracker = RrTracker::default();
        for value in [1_000.0, 1_020.0, 980.0] {
            tracker.push(value);
        }
        assert!((tracker.rmssd().unwrap() - 31.622_776).abs() < 0.001);
    }

    #[test]
    fn rejects_implausible_rr_values() {
        let mut tracker = RrTracker::default();
        for value in [100.0, 1_000.0, 3_000.0, 1_010.0] {
            tracker.push(value);
        }
        assert_eq!(tracker.intervals, [1_000.0, 1_010.0]);
    }
}
