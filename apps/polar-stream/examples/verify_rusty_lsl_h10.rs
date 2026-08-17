use std::{
    env,
    io::{self, BufRead, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use polar_h10_core::AccSample;
use polar_h10_input::{DeviceSummary, InputEvent, InputManager};
use polar_h10_output::{OutputConfig, OutputRouter};
use serde_json::json;

const DEVICE_ID_ENV: &str = "POLAR_STREAM_H10_DEVICE_ID";
const STREAM_BASE: &str = "polar_stream_h10_acceptance";
const MAX_CAPTURE: Duration = Duration::from_secs(120);
const CONSUMER_CLOSE_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_JOIN_TIMEOUT: Duration = Duration::from_secs(2);
const MIN_ECG_FRAMES: u64 = 8;
const MIN_ACC_FRAMES: u64 = 16;
const MIN_CONSUMER_ECG_SAMPLES: u64 = 260;
const MIN_CONSUMER_ACC_SAMPLES: u64 = 400;

#[derive(Default)]
struct SequenceStats {
    frames: u64,
    samples: u64,
    first_samples: usize,
    first_timestamp_ns: Option<u64>,
    last_timestamp_ns: Option<u64>,
    reordered_frames: u64,
    estimated_missing_samples: u64,
}

impl SequenceStats {
    fn observe(&mut self, sensor_timestamp_ns: u64, sample_count: usize, nominal_rate: f64) {
        self.frames = self.frames.saturating_add(1);
        self.samples = self
            .samples
            .saturating_add(u64::try_from(sample_count).unwrap_or(u64::MAX));
        if self.first_timestamp_ns.is_none() {
            self.first_timestamp_ns = Some(sensor_timestamp_ns);
            self.first_samples = sample_count;
        }
        if let Some(previous) = self.last_timestamp_ns {
            if sensor_timestamp_ns <= previous {
                self.reordered_frames = self.reordered_frames.saturating_add(1);
            } else {
                let elapsed_ns = sensor_timestamp_ns - previous;
                let represented = (elapsed_ns as f64 * nominal_rate / 1_000_000_000.0).round();
                let represented = represented.max(0.0) as u64;
                let received = u64::try_from(sample_count).unwrap_or(u64::MAX);
                self.estimated_missing_samples = self
                    .estimated_missing_samples
                    .saturating_add(represented.saturating_sub(received));
            }
        }
        self.last_timestamp_ns = Some(sensor_timestamp_ns);
    }

    fn observed_rate_hz(&self) -> Option<f64> {
        let elapsed = self
            .last_timestamp_ns?
            .checked_sub(self.first_timestamp_ns?)?;
        if elapsed == 0 {
            return None;
        }
        let between = self
            .samples
            .saturating_sub(u64::try_from(self.first_samples).ok()?);
        Some(between as f64 * 1_000_000_000.0 / elapsed as f64)
    }

    fn timestamp_advanced(&self) -> bool {
        self.first_timestamp_ns
            .zip(self.last_timestamp_ns)
            .is_some_and(|(first, last)| last > first)
    }

    fn evidence(&self) -> serde_json::Value {
        json!({
            "frames": self.frames,
            "samples": self.samples,
            "first_sensor_timestamp_ns": self.first_timestamp_ns,
            "last_sensor_timestamp_ns": self.last_timestamp_ns,
            "sensor_timestamp_advanced": self.timestamp_advanced(),
            "observed_rate_hz": self.observed_rate_hz(),
            "reordered_frames": self.reordered_frames,
            "estimated_missing_samples": self.estimated_missing_samples,
        })
    }
}

#[derive(Default)]
struct AxisStats {
    x_min: i16,
    x_max: i16,
    y_min: i16,
    y_max: i16,
    z_min: i16,
    z_max: i16,
    initialized: bool,
    x_nonzero: u64,
    y_nonzero: u64,
    z_nonzero: u64,
}

impl AxisStats {
    fn observe(&mut self, samples: &[AccSample]) {
        for sample in samples {
            if !self.initialized {
                self.x_min = sample.x_mg;
                self.x_max = sample.x_mg;
                self.y_min = sample.y_mg;
                self.y_max = sample.y_mg;
                self.z_min = sample.z_mg;
                self.z_max = sample.z_mg;
                self.initialized = true;
            } else {
                self.x_min = self.x_min.min(sample.x_mg);
                self.x_max = self.x_max.max(sample.x_mg);
                self.y_min = self.y_min.min(sample.y_mg);
                self.y_max = self.y_max.max(sample.y_mg);
                self.z_min = self.z_min.min(sample.z_mg);
                self.z_max = self.z_max.max(sample.z_mg);
            }
            self.x_nonzero = self.x_nonzero.saturating_add(u64::from(sample.x_mg != 0));
            self.y_nonzero = self.y_nonzero.saturating_add(u64::from(sample.y_mg != 0));
            self.z_nonzero = self.z_nonzero.saturating_add(u64::from(sample.z_mg != 0));
        }
    }

    fn each_axis_nonzero(&self) -> bool {
        self.x_nonzero > 0 && self.y_nonzero > 0 && self.z_nonzero > 0
    }

    fn evidence(&self) -> serde_json::Value {
        json!({
            "x_mg": {"min": self.x_min, "max": self.x_max, "nonzero_samples": self.x_nonzero},
            "y_mg": {"min": self.y_min, "max": self.y_max, "nonzero_samples": self.y_nonzero},
            "z_mg": {"min": self.z_min, "max": self.z_max, "nonzero_samples": self.z_nonzero},
            "each_axis_nonzero": self.each_axis_nonzero(),
            "bounded_by_i16_wire_type": true,
        })
    }
}

fn select_device(
    devices: &[DeviceSummary],
    requested_id: Option<&str>,
) -> Result<DeviceSummary, String> {
    if let Some(requested_id) = requested_id {
        let matches = devices
            .iter()
            .filter(|device| device.id == requested_id)
            .cloned()
            .collect::<Vec<_>>();
        return match matches.as_slice() {
            [selected] => Ok(selected.clone()),
            [] => Err("the exact requested H10 was not present in the scan".into()),
            _ => Err("the exact requested H10 identity was ambiguous".into()),
        };
    }
    let matches = devices
        .iter()
        .filter(|device| device.name.to_ascii_lowercase().contains("polar h10"))
        .cloned()
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [selected] => Ok(selected.clone()),
        [] => Err("no Polar H10 was present in the bounded scan".into()),
        _ => Err(format!(
            "{} Polar H10 devices were present; set {DEVICE_ID_ENV} to select one exactly",
            matches.len()
        )),
    }
}

fn flush_stdout() {
    let _ = io::stdout().flush();
}

fn validate_lsl_poll_progress(polls: u64, task_finished: bool) -> Result<(), &'static str> {
    if task_finished {
        Err("Rusty LSL blocking poll task exited before shutdown")
    } else if polls == 0 {
        Err("Rusty LSL blocking poll task made no progress before source readiness")
    } else {
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let output = Arc::new(OutputRouter::new());
    let health = output
        .configure(OutputConfig {
            stream_name: STREAM_BASE.into(),
            lsl_enabled: true,
            outputs: vec!["raw_ecg".into(), "raw_acc".into()],
            ..OutputConfig::default()
        })
        .await?;
    if !health.lsl.contains("Optional Rusty LSL backend") {
        return Err(format!("Rusty LSL did not initialize: {}", health.lsl));
    }

    let poll_stop = Arc::new(AtomicBool::new(false));
    let poll_count = Arc::new(AtomicU64::new(0));
    let poll_output = output.clone();
    let poll_task = {
        let stop = poll_stop.clone();
        let polls = poll_count.clone();
        tokio::task::spawn_blocking(move || {
            while !stop.load(Ordering::Acquire) {
                if let Some(message) = poll_output.poll_lsl() {
                    eprintln!("POLAR_H10_LSL_WARNING {message}");
                }
                polls.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(2));
            }
        })
    };

    println!("POLAR_H10_LSL_INITIALIZED {}", health.lsl);
    flush_stdout();

    let manager = Arc::new(InputManager::new());
    println!("POLAR_H10_STAGE scanning");
    flush_stdout();
    let devices = manager.scan().await?;
    let requested_id = env::var(DEVICE_ID_ENV).ok();
    let selected = select_device(&devices, requested_id.as_deref())?;
    println!(
        "POLAR_H10_SELECTED {}",
        json!({
            "device_id": selected.id,
            "device_name": selected.name,
            "polar_candidates": devices.len(),
            "selection": if requested_id.is_some() { "exact environment identity" } else { "sole exact Polar H10 name" },
        })
    );
    flush_stdout();

    println!("POLAR_H10_STAGE opening-winrt-session");
    flush_stdout();
    let mut events = manager.connect(&selected.id).await?;
    let deadline = Instant::now() + MAX_CAPTURE;
    let mut connected_name = None;
    let mut ecg = SequenceStats::default();
    let mut acc = SequenceStats::default();
    let mut axes = AxisStats::default();
    let mut consumer_ecg_samples = 0_u64;
    let mut consumer_acc_samples = 0_u64;
    let mut source_ready_announced = false;
    let mut last_lsl_health = output.health().lsl;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("physical H10 capture exceeded its two-minute bound".into());
        }
        let event = tokio::time::timeout(remaining, events.recv())
            .await
            .map_err(|_| "physical H10 capture timed out".to_owned())?
            .ok_or_else(|| "physical H10 event stream ended before acceptance".to_owned())?;
        match event {
            InputEvent::Status { phase, message } => {
                println!(
                    "POLAR_H10_STATUS {}",
                    json!({"phase": phase, "message": message})
                );
            }
            InputEvent::Connected { device_name, .. } => {
                connected_name = Some(device_name);
            }
            InputEvent::Ecg {
                sensor_timestamp_ns,
                microvolts,
            } => {
                let _ = output.publish_ecg(sensor_timestamp_ns, &microvolts);
                ecg.observe(sensor_timestamp_ns, microvolts.len(), 130.0);
                if output.health().lsl.contains("2 consumer(s)") {
                    consumer_ecg_samples = consumer_ecg_samples
                        .saturating_add(u64::try_from(microvolts.len()).unwrap_or(u64::MAX));
                }
            }
            InputEvent::Accelerometer {
                sensor_timestamp_ns,
                samples,
            } => {
                let _ = output.publish_accelerometer(sensor_timestamp_ns, &samples);
                acc.observe(sensor_timestamp_ns, samples.len(), 200.0);
                axes.observe(&samples);
                if output.health().lsl.contains("2 consumer(s)") {
                    consumer_acc_samples = consumer_acc_samples
                        .saturating_add(u64::try_from(samples.len()).unwrap_or(u64::MAX));
                }
            }
            InputEvent::HeartRate { .. } => {}
            InputEvent::Error(message) => return Err(format!("H10 input error: {message}")),
            InputEvent::Disconnected { .. } => {
                return Err("H10 disconnected before acceptance completed".into());
            }
        }
        flush_stdout();

        let lsl_health = output.health().lsl;
        if lsl_health != last_lsl_health {
            eprintln!("POLAR_H10_LSL_HEALTH {lsl_health}");
            last_lsl_health = lsl_health;
        }

        let source_ready = connected_name.is_some()
            && ecg.frames >= 2
            && acc.frames >= 2
            && ecg.timestamp_advanced()
            && acc.timestamp_advanced()
            && axes.each_axis_nonzero()
            && ecg.reordered_frames == 0
            && acc.reordered_frames == 0;
        if source_ready && !source_ready_announced {
            let polls = poll_count.load(Ordering::Relaxed);
            validate_lsl_poll_progress(polls, poll_task.is_finished())?;
            eprintln!("POLAR_H10_LSL_POLL_DIAGNOSTIC polls={polls} task_finished=false");
            println!("POLAR_H10_SOURCE_READY {}", output.health().lsl);
            flush_stdout();
            source_ready_announced = true;
        }

        let physical_ready = connected_name.is_some()
            && ecg.frames >= MIN_ECG_FRAMES
            && acc.frames >= MIN_ACC_FRAMES
            && ecg.timestamp_advanced()
            && acc.timestamp_advanced()
            && axes.each_axis_nonzero()
            && ecg.reordered_frames == 0
            && acc.reordered_frames == 0;
        let official_ready = consumer_ecg_samples >= MIN_CONSUMER_ECG_SAMPLES
            && consumer_acc_samples >= MIN_CONSUMER_ACC_SAMPLES;
        if physical_ready && official_ready {
            break;
        }
    }

    let result = json!({
        "schema": "polar.stream.h10_rusty_lsl_physical_source.v1",
        "selected_session": {
            "device_id": selected.id,
            "scan_name": selected.name,
            "connected_name": connected_name,
        },
        "descriptors": {
            "ecg": {"channels": 1, "nominal_rate_hz": 130.0, "format": "float32"},
            "acc": {"channels": 3, "nominal_rate_hz": 200.0, "format": "float32"},
        },
        "ecg": ecg.evidence(),
        "acc": acc.evidence(),
        "acc_axes": axes.evidence(),
        "samples_published_after_two_consumers": {
            "ecg": consumer_ecg_samples,
            "acc_records": consumer_acc_samples,
        },
        "lsl_health": output.health().lsl,
        "result": "source-pass",
    });
    println!("POLAR_H10_CAPTURE_COMPLETE {result}");
    flush_stdout();

    let (close_tx, close_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = io::stdin().lock().read_line(&mut line);
        let _ = close_tx.send(());
    });
    let close_deadline = Instant::now() + CONSUMER_CLOSE_TIMEOUT;
    while close_rx.try_recv().is_err() {
        if Instant::now() >= close_deadline {
            return Err("official consumers did not confirm inlet closure".into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    manager.disconnect().await?;
    poll_stop.store(true, Ordering::Release);
    tokio::time::timeout(POLL_JOIN_TIMEOUT, poll_task)
        .await
        .map_err(|_| "Rusty LSL blocking poll task did not stop within its deadline".to_owned())?
        .map_err(|error| format!("Rusty LSL blocking poll task failed: {error}"))?;
    println!("POLAR_H10_STOPPED {}", output.health().lsl);
    flush_stdout();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, name: &str) -> DeviceSummary {
        DeviceSummary {
            id: id.into(),
            name: name.into(),
            rssi: None,
        }
    }

    #[test]
    fn selection_requires_one_exact_candidate_or_explicit_identity() {
        let devices = [device("one", "Polar H10 A"), device("two", "Polar H10 B")];
        assert!(select_device(&devices, None).is_err());
        assert_eq!(select_device(&devices, Some("two")).unwrap().id, "two");
        assert!(select_device(&devices, Some("missing")).is_err());
    }

    #[test]
    fn sequence_evidence_detects_reorder_and_missing_samples() {
        let mut stats = SequenceStats::default();
        stats.observe(1_000_000_000, 10, 100.0);
        stats.observe(1_200_000_000, 10, 100.0);
        stats.observe(1_100_000_000, 10, 100.0);
        assert_eq!(stats.estimated_missing_samples, 10);
        assert_eq!(stats.reordered_frames, 1);
    }

    #[test]
    fn poll_progress_guard_rejects_zero_and_early_exit() {
        assert!(validate_lsl_poll_progress(1, false).is_ok());
        assert_eq!(
            validate_lsl_poll_progress(0, false),
            Err("Rusty LSL blocking poll task made no progress before source readiness")
        );
        assert_eq!(
            validate_lsl_poll_progress(1, true),
            Err("Rusty LSL blocking poll task exited before shutdown")
        );
    }
}
