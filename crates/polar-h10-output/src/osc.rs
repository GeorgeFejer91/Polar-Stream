use std::{collections::HashMap, net::SocketAddr};

use polar_h10_core::AccSample;
use tokio::net::UdpSocket;

use crate::{
    CustomFormulaConfig, VERNIER_BREATHING_OUTLET_KEY, custom_output_stream_name,
    output_stream_name, vernier_breathing_stream_name,
};

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
        socket
            .writable()
            .await
            .map_err(|error| format!("Could not prepare OSC socket: {error}"))?;
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

    pub(crate) fn configure_custom(&mut self, stream_name: &str, formulas: &[CustomFormulaConfig]) {
        for formula in formulas.iter().filter(|formula| formula.enabled) {
            let name = custom_output_stream_name(stream_name, formula);
            self.paths.insert(formula.id.clone(), format!("/{name}"));
        }
    }

    pub(crate) fn configure_vernier_breathing(&mut self, stream_name: &str) {
        let name = vernier_breathing_stream_name(stream_name);
        self.paths
            .insert(VERNIER_BREATHING_OUTLET_KEY.into(), format!("/{name}"));
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

    pub(crate) fn send_vernier_breathing(
        &mut self,
        newest_timestamp_ns: u64,
        values: &[f32],
        sample_period_us: u32,
    ) {
        for (index, value) in values.iter().copied().enumerate() {
            let remaining = values.len().saturating_sub(index + 1) as u64;
            let timestamp_ns = newest_timestamp_ns.saturating_sub(
                remaining
                    .saturating_mul(u64::from(sample_period_us))
                    .saturating_mul(1_000),
            );
            self.send_series(
                VERNIER_BREATHING_OUTLET_KEY,
                timestamp_ns,
                1,
                std::iter::once(value),
            );
        }
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
    use std::time::Duration;

    use super::*;
    use tokio::time::timeout;

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

    #[tokio::test]
    async fn sends_vernier_breathing_with_canonical_path_and_backfilled_timestamps() {
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target = receiver.local_addr().unwrap().to_string();
        let mut publisher = OscPublisher::connect(&target).await.unwrap();
        publisher.configure("participant_source-2", &[]);
        publisher.configure_vernier_breathing("participant_source-2");

        publisher.send_vernier_breathing(1_000_000_000, &[0.25, 0.75], 100_000);

        let mut observed = Vec::new();
        let mut buffer = [0_u8; 256];
        for _ in 0..2 {
            let (length, _) = timeout(Duration::from_secs(1), receiver.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            let packet = &buffer[..length];
            assert!(packet.starts_with(b"/participant_source-2_vernierBreathing\0"));
            let timestamp_offset = packet.len() - 12;
            let timestamp = i64::from_be_bytes(
                packet[timestamp_offset..timestamp_offset + 8]
                    .try_into()
                    .unwrap(),
            );
            let value = f32::from_bits(u32::from_be_bytes(
                packet[packet.len() - 4..].try_into().unwrap(),
            ));
            observed.push((timestamp, value));
        }
        observed.sort_by_key(|(timestamp, _)| *timestamp);
        assert_eq!(observed, vec![(900_000_000, 0.25), (1_000_000_000, 0.75)]);
    }
}
