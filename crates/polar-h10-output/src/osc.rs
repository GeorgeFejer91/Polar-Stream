use std::{collections::HashMap, net::SocketAddr};

use polar_h10_core::AccSample;
use tokio::net::UdpSocket;

use crate::output_stream_name;

pub(crate) const OSC_TARGET: &str = "127.0.0.1:9000";

pub(crate) struct OscPublisher {
    socket: UdpSocket,
    target: SocketAddr,
    paths: HashMap<String, String>,
    packet: Vec<u8>,
}

impl OscPublisher {
    pub(crate) async fn connect(target: &str) -> Result<Self, String> {
        let target = target
            .parse()
            .map_err(|error| format!("Invalid OSC target: {error}"))?;
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|error| format!("Could not open OSC socket: {error}"))?;
        Ok(Self {
            socket,
            target,
            paths: HashMap::new(),
            packet: Vec::with_capacity(1_024),
        })
    }

    pub(crate) fn configure(&mut self, stream_name: &str, outputs: &[String]) {
        self.paths.clear();
        for id in outputs {
            if let Some(name) = output_stream_name(stream_name, id) {
                self.paths.insert(id.clone(), format!("/{name}"));
            }
        }
    }

    pub(crate) fn send_series<I>(
        &mut self,
        metric_id: &str,
        timestamp_ns: u64,
        value_count: usize,
        values: I,
    ) where
        I: IntoIterator<Item = f32>,
    {
        let Some(path) = self.paths.get(metric_id) else {
            return;
        };
        encode_floats_into(&mut self.packet, path, timestamp_ns, value_count, values);
        let _ = self.socket.try_send_to(&self.packet, self.target);
    }

    pub(crate) fn send_accelerometer(&mut self, timestamp_ns: u64, samples: &[AccSample]) {
        let values = samples.iter().flat_map(|sample| {
            [
                f32::from(sample.x_mg),
                f32::from(sample.y_mg),
                f32::from(sample.z_mg),
            ]
        });
        self.send_series("raw_acc", timestamp_ns, samples.len() * 3, values);
    }
}

fn encode_floats_into<I>(
    packet: &mut Vec<u8>,
    path: &str,
    timestamp_ns: u64,
    value_count: usize,
    values: I,
) where
    I: IntoIterator<Item = f32>,
{
    packet.clear();
    packet.reserve(32 + value_count * 5);
    push_string(packet, path);
    packet.extend_from_slice(b",h");
    packet.extend(std::iter::repeat_n(b'f', value_count));
    packet.push(0);
    while !packet.len().is_multiple_of(4) {
        packet.push(0);
    }
    packet.extend_from_slice(&(timestamp_ns as i64).to_be_bytes());
    for value in values {
        packet.extend_from_slice(&value.to_bits().to_be_bytes());
    }
}

fn push_string(buffer: &mut Vec<u8>, value: &str) {
    buffer.extend_from_slice(value.as_bytes());
    buffer.push(0);
    while !buffer.len().is_multiple_of(4) {
        buffer.push(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_is_padded_and_network_endian() {
        let mut packet = Vec::new();
        encode_floats_into(&mut packet, "/polar/ecg", 9, 1, [1.5]);
        assert_eq!(packet.len() % 4, 0);
        assert!(packet.starts_with(b"/polar/ecg\0"));
        assert_eq!(
            &packet[packet.len() - 4..],
            &1.5_f32.to_bits().to_be_bytes()
        );
    }

    #[test]
    fn uses_the_canonical_output_name_as_the_osc_path() {
        let name = output_stream_name("participant_07", "raw_ecg").unwrap();
        let mut packet = Vec::new();
        encode_floats_into(&mut packet, &format!("/{name}"), 9, 1, [1.5]);
        assert!(packet.starts_with(b"/participant_07_rawECG\0"));
    }
}
