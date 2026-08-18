use std::{
    io::{self, BufRead, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use polar_h10_core::AccSample;
use polar_h10_input::{DeviceSummary, InputEvent, InputSessionPool};
use polar_h10_output::RustyLslTwoSessionOutput;
use serde_json::json;
use tokio::sync::{mpsc::Receiver, watch};

const MAX_CAPTURE: Duration = Duration::from_secs(120);
const CONSUMER_CLOSE_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_JOIN_TIMEOUT: Duration = Duration::from_secs(2);
const MIN_ECG_FRAMES: u64 = 8;
const MIN_ACC_FRAMES: u64 = 16;
const MIN_CONSUMER_ECG_SAMPLES: u64 = 260;
const MIN_CONSUMER_ACC_SAMPLES: u64 = 400;
const SLOT_1: &str = "device-1";
const SLOT_2: &str = "device-2";
const BASE_1: &str = "polar_stream_two_h10_device_1";
const BASE_2: &str = "polar_stream_two_h10_device_2";

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
                let represented = ((sensor_timestamp_ns - previous) as f64 * nominal_rate
                    / 1_000_000_000.0)
                    .round()
                    .max(0.0) as u64;
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
    x_nonzero: u64,
    y_nonzero: u64,
    z_nonzero: u64,
}

impl AxisStats {
    fn observe(&mut self, samples: &[AccSample]) {
        for sample in samples {
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
            "nonzero_samples": {
                "x": self.x_nonzero,
                "y": self.y_nonzero,
                "z": self.z_nonzero,
            },
            "each_axis_nonzero": self.each_axis_nonzero(),
        })
    }
}

struct SlotRuntime {
    slot: &'static str,
    events: Receiver<InputEvent>,
    output: Arc<RustyLslTwoSessionOutput>,
    connected: bool,
    ecg: SequenceStats,
    acc: SequenceStats,
    axes: AxisStats,
    consumer_ecg_samples: u64,
    consumer_acc_samples: u64,
}

impl SlotRuntime {
    fn new(
        slot: &'static str,
        events: Receiver<InputEvent>,
        output: Arc<RustyLslTwoSessionOutput>,
    ) -> Self {
        Self {
            slot,
            events,
            output,
            connected: false,
            ecg: SequenceStats::default(),
            acc: SequenceStats::default(),
            axes: AxisStats::default(),
            consumer_ecg_samples: 0,
            consumer_acc_samples: 0,
        }
    }

    fn observe(&mut self, event: InputEvent) -> Result<(), String> {
        match event {
            InputEvent::Status { phase, .. } => {
                println!(
                    "TWO_H10_STATUS {}",
                    json!({"slot": self.slot, "phase": phase})
                );
            }
            InputEvent::Connected { .. } => self.connected = true,
            InputEvent::Ecg {
                sensor_timestamp_ns,
                microvolts,
            } => {
                let _ = self
                    .output
                    .publish_ecg(self.slot, sensor_timestamp_ns, &microvolts);
                self.ecg
                    .observe(sensor_timestamp_ns, microvolts.len(), 130.0);
                if self.output.connected_consumers(self.slot) == Some(2) {
                    self.consumer_ecg_samples = self
                        .consumer_ecg_samples
                        .saturating_add(u64::try_from(microvolts.len()).unwrap_or(u64::MAX));
                }
            }
            InputEvent::Accelerometer {
                sensor_timestamp_ns,
                samples,
            } => {
                let _ = self
                    .output
                    .publish_accelerometer(self.slot, sensor_timestamp_ns, &samples);
                self.acc.observe(sensor_timestamp_ns, samples.len(), 200.0);
                self.axes.observe(&samples);
                if self.output.connected_consumers(self.slot) == Some(2) {
                    self.consumer_acc_samples = self
                        .consumer_acc_samples
                        .saturating_add(u64::try_from(samples.len()).unwrap_or(u64::MAX));
                }
            }
            InputEvent::HeartRate { .. } => {}
            InputEvent::Error(_) => return Err(format!("{} input reported an error", self.slot)),
            InputEvent::Disconnected { .. } => {
                return Err(format!("{} disconnected before acceptance", self.slot));
            }
        }
        flush_stdout();
        Ok(())
    }

    fn source_ready(&self) -> bool {
        self.connected
            && self.ecg.frames >= 2
            && self.acc.frames >= 2
            && self.ecg.timestamp_advanced()
            && self.acc.timestamp_advanced()
            && self.axes.each_axis_nonzero()
            && self.ecg.reordered_frames == 0
            && self.acc.reordered_frames == 0
    }

    fn accepted(&self) -> bool {
        self.source_ready()
            && self.ecg.frames >= MIN_ECG_FRAMES
            && self.acc.frames >= MIN_ACC_FRAMES
            && self
                .ecg
                .observed_rate_hz()
                .is_some_and(|rate| (120.0..=140.0).contains(&rate))
            && self
                .acc
                .observed_rate_hz()
                .is_some_and(|rate| (180.0..=220.0).contains(&rate))
            && self.consumer_ecg_samples >= MIN_CONSUMER_ECG_SAMPLES
            && self.consumer_acc_samples >= MIN_CONSUMER_ACC_SAMPLES
    }

    fn evidence(&self) -> serde_json::Value {
        json!({
            "slot": self.slot,
            "result": "source-pass",
            "ecg": self.ecg.evidence(),
            "acc": self.acc.evidence(),
            "acc_axes": self.axes.evidence(),
            "samples_published_after_two_consumers": {
                "ecg": self.consumer_ecg_samples,
                "acc_records": self.consumer_acc_samples,
            },
            "lsl_health": self.output.health(),
        })
    }
}

fn is_exact_h10_name(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    normalized == "polar h10" || normalized.starts_with("polar h10 ")
}

fn select_two_devices(devices: &[DeviceSummary]) -> Result<[DeviceSummary; 2], String> {
    let exact = devices
        .iter()
        .filter(|device| is_exact_h10_name(&device.name))
        .cloned()
        .collect::<Vec<_>>();
    match exact.as_slice() {
        [first, second] if first.id != second.id => Ok([first.clone(), second.clone()]),
        [_, _] => Err("the two discovered H10 candidates shared one identity".into()),
        _ => Err(format!(
            "bounded discovery requires exactly two distinct H10s; observed {}",
            exact.len()
        )),
    }
}

fn flush_stdout() {
    let _ = io::stdout().flush();
}

fn validate_poll_progress(polls: u64, task_finished: bool) -> Result<(), &'static str> {
    if task_finished {
        Err("Rusty LSL blocking poll task exited before shutdown")
    } else if polls == 0 {
        Err("Rusty LSL blocking poll task made no progress before source readiness")
    } else {
        Ok(())
    }
}

fn configure_output() -> Result<Arc<RustyLslTwoSessionOutput>, String> {
    let output = Arc::new(RustyLslTwoSessionOutput::new([
        (SLOT_1, BASE_1),
        (SLOT_2, BASE_2),
    ])?);
    if !output.health().contains("Optional Rusty LSL backend") {
        return Err(format!("Rusty LSL did not initialize: {}", output.health()));
    }
    Ok(output)
}

async fn capture(
    pool: &InputSessionPool,
    output: Arc<RustyLslTwoSessionOutput>,
    poll_count: &AtomicU64,
    poll_finished: impl Fn() -> bool,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    println!("TWO_H10_STAGE scanning");
    flush_stdout();
    let selected = select_two_devices(&pool.scan().await?)?;
    println!(
        "TWO_H10_SELECTED {}",
        json!({"slots": [SLOT_1, SLOT_2], "exact_h10_candidates": 2})
    );
    flush_stdout();

    println!("TWO_H10_STAGE opening-device-1");
    flush_stdout();
    let events_1 = pool.connect(SLOT_1, &selected[0].id).await?;
    let mut device_1 = SlotRuntime::new(SLOT_1, events_1, output.clone());

    println!("TWO_H10_STAGE opening-device-2");
    flush_stdout();
    let second_connection = pool.connect(SLOT_2, &selected[1].id);
    tokio::pin!(second_connection);
    let events_2 = loop {
        tokio::select! {
            result = &mut second_connection => break result?,
            event = device_1.events.recv() => {
                let event = event.ok_or_else(|| "device-1 event stream ended during device-2 setup".to_string())?;
                device_1.observe(event)?;
            }
            () = wait_for_shutdown(shutdown) => {
                return Err("two-H10 capture was cancelled during device-2 setup".into());
            }
        }
    };
    let mut device_2 = SlotRuntime::new(SLOT_2, events_2, output);

    let deadline = Instant::now() + MAX_CAPTURE;
    let mut source_ready_announced = false;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("two-H10 capture exceeded its two-minute bound".into());
        }
        tokio::select! {
            event = device_1.events.recv() => {
                device_1.observe(event.ok_or_else(|| "device-1 event stream ended".to_string())?)?;
            }
            event = device_2.events.recv() => {
                device_2.observe(event.ok_or_else(|| "device-2 event stream ended".to_string())?)?;
            }
            _ = tokio::time::sleep(remaining) => {
                return Err("two-H10 capture timed out".into());
            }
            () = wait_for_shutdown(shutdown) => {
                return Err("two-H10 capture was cancelled".into());
            }
        }

        if device_1.source_ready() && device_2.source_ready() && !source_ready_announced {
            let polls = poll_count.load(Ordering::Relaxed);
            validate_poll_progress(polls, poll_finished())?;
            println!(
                "TWO_H10_SOURCE_READY {}",
                json!({"slots": [SLOT_1, SLOT_2], "sessions": pool.active_session_count().await})
            );
            flush_stdout();
            source_ready_announced = true;
        }
        if device_1.accepted() && device_2.accepted() {
            break;
        }
    }

    println!(
        "TWO_H10_CAPTURE_COMPLETE {}",
        json!({
            "schema": "polar.stream.two_h10_rusty_lsl_physical_source.v1",
            "result": "source-pass",
            "devices": [device_1.evidence(), device_2.evidence()],
            "descriptors_per_device": {
                "ecg": {"channels": 1, "nominal_rate_hz": 130.0, "format": "float32"},
                "acc": {"channels": 3, "nominal_rate_hz": 200.0, "format": "float32"},
            },
        })
    );
    flush_stdout();

    tokio::time::timeout(CONSUMER_CLOSE_TIMEOUT, wait_for_shutdown(shutdown))
        .await
        .map_err(|_| "official consumers did not confirm four-inlet closure".to_string())?;
    Ok(())
}

async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            return;
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let output = configure_output()?;
    let poll_stop = Arc::new(AtomicBool::new(false));
    let poll_count = Arc::new(AtomicU64::new(0));
    let poll_task = {
        let stop = poll_stop.clone();
        let polls = poll_count.clone();
        let output = output.clone();
        tokio::task::spawn_blocking(move || {
            while !stop.load(Ordering::Acquire) {
                if let Some(message) = output.poll_lsl() {
                    eprintln!("TWO_H10_LSL_WARNING {message}");
                }
                polls.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(2));
            }
        })
    };
    println!(
        "TWO_H10_LSL_INITIALIZED {}",
        json!({"slots": [SLOT_1, SLOT_2], "outlets": 4})
    );
    flush_stdout();

    let (shutdown_tx, mut shutdown) = watch::channel(false);
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = io::stdin().lock().read_line(&mut line);
        shutdown_tx.send_replace(true);
    });

    let pool = InputSessionPool::two_h10s();
    let result = capture(
        &pool,
        output,
        &poll_count,
        || poll_task.is_finished(),
        &mut shutdown,
    )
    .await;
    let disconnect_result = pool.disconnect_all().await;
    poll_stop.store(true, Ordering::Release);
    let poll_result = tokio::time::timeout(POLL_JOIN_TIMEOUT, poll_task)
        .await
        .map_err(|_| "Rusty LSL blocking poll task did not stop within its deadline".to_owned())?
        .map_err(|error| format!("Rusty LSL blocking poll task failed: {error}"));

    println!(
        "TWO_H10_STOPPED {}",
        json!({
            "sessions": pool.active_session_count().await,
            "result": if disconnect_result.is_ok() && poll_result.is_ok() {
                "cleanup-pass"
            } else {
                "cleanup-failed"
            }
        })
    );
    flush_stdout();
    result.and(disconnect_result).and(poll_result)
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
    fn selection_requires_exactly_two_distinct_h10s() {
        let devices = [device("one", "Polar H10 A"), device("two", "Polar H10 B")];
        assert!(select_two_devices(&devices).is_ok());
        assert!(select_two_devices(&devices[..1]).is_err());
        assert!(select_two_devices(&[devices[0].clone(), devices[0].clone()]).is_err());
        assert!(select_two_devices(&[devices[0].clone(), device("x", "Polar H100")]).is_err());
    }

    #[test]
    fn slot_bases_are_distinct_and_identifier_free() {
        assert_ne!(BASE_1, BASE_2);
        assert!(!BASE_1.contains(':'));
        assert!(!BASE_2.contains(':'));
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
        assert!(validate_poll_progress(1, false).is_ok());
        assert!(validate_poll_progress(0, false).is_err());
        assert!(validate_poll_progress(1, true).is_err());
    }

    #[tokio::test]
    async fn shutdown_waiter_handles_preexisting_and_later_stop() {
        let (first_tx, mut first_rx) = watch::channel(false);
        first_tx.send_replace(true);
        tokio::time::timeout(Duration::from_millis(10), wait_for_shutdown(&mut first_rx))
            .await
            .unwrap();

        let (second_tx, mut second_rx) = watch::channel(false);
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            second_tx.send_replace(true);
        });
        tokio::time::timeout(
            Duration::from_millis(100),
            wait_for_shutdown(&mut second_rx),
        )
        .await
        .unwrap();
    }
}
