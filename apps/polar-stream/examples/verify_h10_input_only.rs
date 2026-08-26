use std::{sync::Arc, time::Duration};

use polar_h10_input::{DeviceSummary, InputEvent, InputManager};
use serde_json::json;

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Default)]
struct StreamObservation {
    frames: u64,
    samples: u64,
    first_timestamp_ns: Option<u64>,
    last_timestamp_ns: Option<u64>,
}

impl StreamObservation {
    fn observe(&mut self, timestamp_ns: u64, samples: usize) {
        self.frames = self.frames.saturating_add(1);
        self.samples = self
            .samples
            .saturating_add(u64::try_from(samples).unwrap_or(u64::MAX));
        self.first_timestamp_ns.get_or_insert(timestamp_ns);
        self.last_timestamp_ns = Some(timestamp_ns);
    }

    fn advanced(&self) -> bool {
        self.first_timestamp_ns
            .zip(self.last_timestamp_ns)
            .is_some_and(|(first, last)| last > first)
    }
}

fn select_h10(devices: &[DeviceSummary]) -> Result<DeviceSummary, String> {
    let matches = devices
        .iter()
        .filter(|device| device.name.to_ascii_lowercase().contains("polar h10"))
        .cloned()
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [selected] => Ok(selected.clone()),
        [] => Err("input-only verifier found no exact H10 candidate".to_string()),
        _ => Err(format!(
            "input-only verifier found {} exact H10 candidates and requires one",
            matches.len()
        )),
    }
}

async fn capture(manager: &Arc<InputManager>) -> Result<serde_json::Value, String> {
    let devices = manager.scan().await?;
    let selected = select_h10(&devices)?;
    println!(
        "POLAR_H10_INPUT_ONLY_SELECTED {}",
        json!({"exact_candidate_count": 1})
    );

    let mut events = manager.connect(&selected.id).await?;
    let deadline = tokio::time::Instant::now() + CAPTURE_TIMEOUT;
    let mut ecg = StreamObservation::default();
    let mut acc = StreamObservation::default();
    let mut connected = false;
    let mut each_axis_nonzero = [false; 3];

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("input-only verifier capture deadline elapsed".to_string());
        }
        let event = tokio::time::timeout(remaining, events.recv())
            .await
            .map_err(|_| "input-only verifier capture timed out".to_string())?
            .ok_or_else(|| "input-only verifier event stream closed".to_string())?;
        match event {
            InputEvent::Status { phase, message } => {
                println!(
                    "POLAR_H10_INPUT_ONLY_STATUS {}",
                    json!({"phase": phase, "message": message})
                );
            }
            InputEvent::Connected { .. } => connected = true,
            InputEvent::Ecg {
                sensor_timestamp_ns,
                microvolts,
                ..
            } => ecg.observe(sensor_timestamp_ns, microvolts.len()),
            InputEvent::Accelerometer {
                sensor_timestamp_ns,
                samples,
                ..
            } => {
                acc.observe(sensor_timestamp_ns, samples.len());
                for sample in samples {
                    each_axis_nonzero[0] |= sample.x_mg != 0;
                    each_axis_nonzero[1] |= sample.y_mg != 0;
                    each_axis_nonzero[2] |= sample.z_mg != 0;
                }
            }
            InputEvent::HeartRate { .. } => {}
            InputEvent::Error(error) => {
                return Err(format!(
                    "input-only verifier received an input error: {error}"
                ));
            }
            InputEvent::Disconnected { .. } => {
                return Err("input-only verifier disconnected before qualification".to_string());
            }
        }

        if connected
            && ecg.frames >= 2
            && acc.frames >= 2
            && ecg.advanced()
            && acc.advanced()
            && each_axis_nonzero.into_iter().all(|observed| observed)
        {
            return Ok(json!({
                "schema": "polar.stream.h10_input_only_differential.v1",
                "ecg": {"frames": ecg.frames, "samples": ecg.samples, "timestamp_advanced": true},
                "acc": {"frames": acc.frames, "samples": acc.samples, "timestamp_advanced": true},
                "each_acc_axis_nonzero": true,
                "output_transport_initialized": false,
                "result": "pass"
            }));
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let manager = Arc::new(InputManager::new());
    let result = capture(&manager).await;
    let cleanup = manager.disconnect().await;
    match (result, cleanup) {
        (Ok(evidence), Ok(())) => {
            println!("POLAR_H10_INPUT_ONLY_COMPLETE {evidence}");
            println!("POLAR_H10_INPUT_ONLY_STOPPED");
            Ok(())
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(format!("input-only verifier cleanup failed: {error}")),
        (Err(error), Err(cleanup)) => Err(format!(
            "{error}; input-only verifier cleanup also failed: {cleanup}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, name: &str) -> DeviceSummary {
        DeviceSummary {
            id: id.to_string(),
            name: name.to_string(),
            rssi: None,
        }
    }

    #[test]
    fn selection_requires_exactly_one_h10_without_emitting_identity() {
        assert!(select_h10(&[]).is_err());
        let selected = select_h10(&[
            device("other", "Headset"),
            device("private", "Polar H10 private"),
        ])
        .unwrap();
        assert_eq!(selected.id, "private");
        assert!(
            select_h10(&[
                device("one", "Polar H10 one"),
                device("two", "Polar H10 two"),
            ])
            .is_err()
        );
    }

    #[test]
    fn observations_require_advancing_timestamps() {
        let mut observation = StreamObservation::default();
        observation.observe(10, 73);
        assert!(!observation.advanced());
        observation.observe(20, 73);
        assert!(observation.advanced());
        assert_eq!(observation.frames, 2);
        assert_eq!(observation.samples, 146);
    }
}
