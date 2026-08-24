//! Bounded, identifier-free physical qualification for one Vernier GDX-RB.
//!
//! This executable exercises the same `vernier-gdx-input` pool as the shipped
//! application. It requires exact device/channel metadata, sustained Force (N)
//! samples, health telemetry, explicit cleanup, and a second connection.

use std::{
    collections::BTreeSet,
    env,
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::json;
use tokio::sync::mpsc;
use vernier_gdx_core::{SensorInfo, SensorSamples};
use vernier_gdx_input::{
    DEFAULT_PERIOD_US, DeviceSummary, InputEvent, InputSessionPool, SessionConfig,
};

const PRIMARY_MINIMUM_SAMPLES: u64 = 70;
const RECONNECT_MINIMUM_SAMPLES: u64 = 20;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(60);
const DISCONNECT_EVENT_TIMEOUT: Duration = Duration::from_secs(4);
const TARGET_NAME_ENV: &str = "POLAR_GDX_TARGET_NAME";

#[derive(Clone, Debug, Serialize)]
struct ConnectionEvidence {
    model_code: String,
    sensor_number: u8,
    sensor_name: String,
    sensor_unit: String,
    sample_period_us: u32,
    main_firmware_version: String,
    battery_percent: u8,
    sensors: Vec<SensorEvidence>,
}

#[derive(Clone, Debug, Serialize)]
struct SensorEvidence {
    number: u8,
    sensor_id: u32,
    name: String,
    unit: String,
    numeric_type: String,
    sampling_mode: String,
}

impl From<&SensorInfo> for SensorEvidence {
    fn from(sensor: &SensorInfo) -> Self {
        Self {
            number: sensor.number,
            sensor_id: sensor.sensor_id,
            name: sensor.description.clone(),
            unit: sensor.unit.clone(),
            numeric_type: format!("{:?}", sensor.numeric_type),
            sampling_mode: format!("{:?}", sensor.sampling_mode),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
struct HealthEvidence {
    notifications: u64,
    samples: u64,
    sample_rows: u64,
    malformed_frames: u64,
    dropped_batches: u64,
    device_drop_reports: u64,
    queue_high_water: usize,
    decode_latency_p50_ns: u64,
    decode_latency_p95_ns: u64,
    decode_latency_p99_ns: u64,
    max_decode_latency_ns: u64,
}

#[derive(Debug, Default)]
struct StreamObservation {
    batches: u64,
    samples: u64,
    scalar_values: u64,
    observed_sensor_numbers: BTreeSet<u8>,
    first_host_receive_timestamp_ns: Option<u64>,
    last_host_receive_timestamp_ns: Option<u64>,
    first_reconstructed_sample_timestamp_ns: Option<u64>,
    sequence_next: Option<u64>,
    sequence_missing: u64,
    sequence_reordered: u64,
    dropped_before: u64,
    device_drop_reports_before: u64,
    nonfinite_values: u64,
    period_us: Option<u32>,
    period_changes: u64,
    force_min_n: Option<f64>,
    force_max_n: Option<f64>,
    previous_batch_samples: usize,
    host_gap_errors_ns: Vec<u64>,
    max_host_gap_ns: u64,
}

impl StreamObservation {
    fn observe(
        &mut self,
        sensors: &[SensorSamples],
        host_receive_timestamp_ns: u64,
        sample_period_us: u32,
        sequence: u64,
        dropped_before: u64,
        device_drop_reports_before: u64,
    ) -> Result<(), String> {
        let row_count = sensors
            .iter()
            .map(|sensor| sensor.values.len())
            .max()
            .unwrap_or(0);
        if row_count == 0 {
            return Err("physical verifier received an empty measurement frame".into());
        }

        for sensor in sensors {
            self.observed_sensor_numbers.insert(sensor.sensor_number);
            self.scalar_values = self
                .scalar_values
                .saturating_add(sensor.values.len() as u64);
        }

        if let Some(previous_period) = self.period_us {
            if previous_period != sample_period_us {
                self.period_changes = self.period_changes.saturating_add(1);
            }
        } else {
            self.period_us = Some(sample_period_us);
        }

        if let Some(expected) = self.sequence_next {
            if sequence < expected {
                self.sequence_reordered = self
                    .sequence_reordered
                    .saturating_add(expected.saturating_sub(sequence));
            } else {
                self.sequence_missing = self
                    .sequence_missing
                    .saturating_add(sequence.saturating_sub(expected));
            }
        }
        self.sequence_next = Some(sequence.saturating_add(row_count as u64));
        self.dropped_before = self.dropped_before.saturating_add(dropped_before);
        self.device_drop_reports_before = self
            .device_drop_reports_before
            .saturating_add(device_drop_reports_before);

        self.batches = self.batches.saturating_add(1);
        let Some(force) = sensors.iter().find(|sensor| sensor.sensor_number == 1) else {
            return Ok(());
        };
        if force.values.is_empty() {
            return Err("physical verifier received an empty Force batch".into());
        }

        if let Some(previous) = self.last_host_receive_timestamp_ns {
            if host_receive_timestamp_ns <= previous {
                return Err("Go Direct host receipt timestamps did not advance".into());
            }
            let observed_gap = host_receive_timestamp_ns.saturating_sub(previous);
            let expected_gap = (self.previous_batch_samples as u64)
                .saturating_mul(u64::from(sample_period_us))
                .saturating_mul(1_000);
            self.max_host_gap_ns = self.max_host_gap_ns.max(observed_gap);
            self.host_gap_errors_ns
                .push(observed_gap.abs_diff(expected_gap));
        }

        self.samples = self.samples.saturating_add(force.values.len() as u64);
        self.first_host_receive_timestamp_ns
            .get_or_insert(host_receive_timestamp_ns);
        self.last_host_receive_timestamp_ns = Some(host_receive_timestamp_ns);
        self.previous_batch_samples = force.values.len();
        let batch_backfill_ns = (force.values.len().saturating_sub(1) as u64)
            .saturating_mul(u64::from(sample_period_us))
            .saturating_mul(1_000);
        self.first_reconstructed_sample_timestamp_ns
            .get_or_insert(host_receive_timestamp_ns.saturating_sub(batch_backfill_ns));

        for value in &force.values {
            if !value.is_finite() {
                self.nonfinite_values = self.nonfinite_values.saturating_add(1);
                continue;
            }
            self.force_min_n = Some(self.force_min_n.map_or(*value, |old| old.min(*value)));
            self.force_max_n = Some(self.force_max_n.map_or(*value, |old| old.max(*value)));
        }
        Ok(())
    }

    fn observed_rate_hz(&self) -> Option<f64> {
        let first = self.first_reconstructed_sample_timestamp_ns?;
        let last = self.last_host_receive_timestamp_ns?;
        if self.samples < 2 || last <= first {
            return None;
        }
        Some((self.samples - 1) as f64 / ((last - first) as f64 / 1_000_000_000.0))
    }

    fn force_span_n(&self) -> Option<f64> {
        self.force_min_n
            .zip(self.force_max_n)
            .map(|(minimum, maximum)| maximum - minimum)
    }

    fn evidence(&self) -> serde_json::Value {
        let mut gap_errors = self.host_gap_errors_ns.clone();
        gap_errors.sort_unstable();
        json!({
            "batches": self.batches,
            "samples": self.samples,
            "scalar_values": self.scalar_values,
            "observed_sensor_numbers": self.observed_sensor_numbers,
            "host_receipt_timestamps_advanced": self.first_host_receive_timestamp_ns
                .zip(self.last_host_receive_timestamp_ns)
                .is_some_and(|(first, last)| last > first),
            "observed_rate_hz": self.observed_rate_hz(),
            "sequence_missing_samples": self.sequence_missing,
            "sequence_reordered_samples": self.sequence_reordered,
            "dropped_before_samples": self.dropped_before,
            "device_drop_reports_before": self.device_drop_reports_before,
            "nonfinite_values": self.nonfinite_values,
            "period_changes": self.period_changes,
            "force_min_n": self.force_min_n,
            "force_max_n": self.force_max_n,
            "force_span_n": self.force_span_n(),
            "force_changed": self.force_span_n().is_some_and(|span| span > 0.0),
            "host_gap_error_p95_ms": percentile(&gap_errors, 95) as f64 / 1_000_000.0,
            "max_host_receipt_gap_ms": self.max_host_gap_ns as f64 / 1_000_000.0,
        })
    }
}

#[derive(Debug, Serialize)]
struct SessionEvidence {
    connection: ConnectionEvidence,
    stream: serde_json::Value,
    health: Option<HealthEvidence>,
    startup_ms: f64,
    cleanup_ms: f64,
    disconnect_event_observed: bool,
}

fn percentile(sorted: &[u64], percent: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = (sorted.len() - 1).saturating_mul(percent).div_ceil(100);
    sorted[index.min(sorted.len() - 1)]
}

fn select_device(
    devices: &[DeviceSummary],
    exact_target: Option<&str>,
) -> Result<DeviceSummary, String> {
    if let Some(target) = exact_target.filter(|target| !target.is_empty()) {
        let matches = devices
            .iter()
            .filter(|device| device.name == target)
            .cloned()
            .collect::<Vec<_>>();
        return match matches.as_slice() {
            [selected] => Ok(selected.clone()),
            [] => Err("targeted Go Direct device was not present in the bounded scan".into()),
            _ => Err("targeted Go Direct device was ambiguous in the bounded scan".into()),
        };
    }

    let candidates = devices
        .iter()
        .filter(|device| device.respiration_belt_candidate)
        .cloned()
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [selected] => Ok(selected.clone()),
        [] if devices.len() == 1 => Ok(devices[0].clone()),
        [] => Err("physical verifier found no unambiguous GDX-RB candidate".into()),
        _ => Err(format!(
            "physical verifier found {} GDX-RB candidates; set {TARGET_NAME_ENV} for an exact private selection",
            candidates.len()
        )),
    }
}

async fn capture(
    events: &mut mpsc::Receiver<InputEvent>,
    minimum_samples: u64,
    require_health: bool,
) -> Result<
    (
        ConnectionEvidence,
        StreamObservation,
        Option<HealthEvidence>,
        f64,
    ),
    String,
> {
    let started = Instant::now();
    let deadline = tokio::time::Instant::now() + CAPTURE_TIMEOUT;
    let mut connection = None;
    let mut stream = StreamObservation::default();
    let mut health = None;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("physical verifier capture deadline elapsed".into());
        }
        let event = tokio::time::timeout(remaining, events.recv())
            .await
            .map_err(|_| "physical verifier capture timed out".to_string())?
            .ok_or_else(|| "physical verifier event stream closed".to_string())?;
        match event {
            InputEvent::Status { phase, .. } => {
                println!("POLAR_GDX_VERIFY_STATUS {}", json!({"phase": phase}));
            }
            InputEvent::Connected {
                model_code,
                sensor_number,
                sensor_name,
                sensor_unit,
                sample_period_us,
                main_firmware_version,
                battery_percent,
                sensors,
                ..
            } => {
                let metadata = ConnectionEvidence {
                    model_code,
                    sensor_number,
                    sensor_name,
                    sensor_unit,
                    sample_period_us,
                    main_firmware_version,
                    battery_percent,
                    sensors: sensors.iter().map(SensorEvidence::from).collect(),
                };
                validate_connection(&metadata)?;
                connection = Some(metadata);
            }
            InputEvent::Samples {
                sensors,
                host_receive_timestamp_ns,
                sample_period_us,
                sequence,
                dropped_before,
                device_drop_reports_before,
                ..
            } => stream.observe(
                &sensors,
                host_receive_timestamp_ns,
                sample_period_us,
                sequence,
                dropped_before,
                device_drop_reports_before,
            )?,
            InputEvent::StreamHealth {
                notifications,
                samples,
                sample_rows,
                malformed_frames,
                dropped_batches,
                device_drop_reports,
                queue_high_water,
                decode_latency_p50_ns,
                decode_latency_p95_ns,
                decode_latency_p99_ns,
                max_decode_latency_ns,
            } => {
                health = Some(HealthEvidence {
                    notifications,
                    samples,
                    sample_rows,
                    malformed_frames,
                    dropped_batches,
                    device_drop_reports,
                    queue_high_water,
                    decode_latency_p50_ns,
                    decode_latency_p95_ns,
                    decode_latency_p99_ns,
                    max_decode_latency_ns,
                });
            }
            InputEvent::Error(_) => {
                return Err("Go Direct input reported a protocol or transport error".into());
            }
            InputEvent::Disconnected { .. } => {
                return Err("Go Direct disconnected before physical qualification".into());
            }
        }

        if connection.is_some()
            && stream.samples >= minimum_samples
            && (!require_health || health.is_some())
        {
            validate_stream(&stream, health.as_ref(), require_health)?;
            let connection =
                connection.ok_or_else(|| "GDX-RB connection metadata disappeared".to_string())?;
            return Ok((
                connection,
                stream,
                health,
                started.elapsed().as_secs_f64() * 1_000.0,
            ));
        }
    }
}

fn validate_connection(connection: &ConnectionEvidence) -> Result<(), String> {
    if connection.model_code != "GDX-RB"
        || connection.sensor_number != 1
        || !connection.sensor_name.eq_ignore_ascii_case("force")
        || !connection.sensor_unit.eq_ignore_ascii_case("n")
    {
        return Err("connected Go Direct metadata was not exact GDX-RB channel-1 Force (N)".into());
    }
    if connection.main_firmware_version.is_empty() {
        return Err("GDX-RB main firmware version was empty".into());
    }
    if connection.sensors.is_empty()
        || connection
            .sensors
            .windows(2)
            .any(|pair| pair[0].number >= pair[1].number)
        || !connection.sensors.iter().any(|sensor| {
            sensor.number == 1
                && sensor.name.eq_ignore_ascii_case("force")
                && sensor.unit.eq_ignore_ascii_case("n")
        })
    {
        return Err("GDX-RB did not expose a stable, ordered all-channel schema".into());
    }
    if !(1_000..=60_000_000).contains(&connection.sample_period_us) {
        return Err("GDX-RB selected an invalid sample period".into());
    }
    Ok(())
}

fn validate_stream(
    stream: &StreamObservation,
    health: Option<&HealthEvidence>,
    require_health: bool,
) -> Result<(), String> {
    if stream.batches < 2
        || stream.sequence_missing != 0
        || stream.sequence_reordered != 0
        || stream.dropped_before != 0
        || stream.device_drop_reports_before != 0
        || stream.nonfinite_values != 0
        || stream.period_changes != 0
    {
        return Err("GDX-RB stream integrity counters did not pass".into());
    }
    let period_us = stream
        .period_us
        .ok_or_else(|| "GDX-RB stream did not establish a period".to_string())?;
    let nominal_rate = 1_000_000.0 / f64::from(period_us);
    let observed_rate = stream
        .observed_rate_hz()
        .ok_or_else(|| "GDX-RB observed rate could not be calculated".to_string())?;
    if !(nominal_rate * 0.70..=nominal_rate * 1.30).contains(&observed_rate) {
        return Err(format!(
            "GDX-RB observed rate {observed_rate:.2} Hz was outside the bounded nominal band"
        ));
    }
    if stream.max_host_gap_ns > 2_000_000_000 {
        return Err("GDX-RB host receipt gap exceeded two seconds".into());
    }
    if require_health {
        let health = health.ok_or_else(|| "GDX-RB emitted no health snapshot".to_string())?;
        if health.malformed_frames != 0
            || health.dropped_batches != 0
            || health.device_drop_reports != 0
            || health.queue_high_water > 32
            || health.decode_latency_p99_ns > u64::from(period_us) * 1_000
        {
            return Err("GDX-RB health thresholds did not pass".into());
        }
    }
    Ok(())
}

async fn await_disconnected(events: &mut mpsc::Receiver<InputEvent>) -> Result<(), String> {
    tokio::time::timeout(DISCONNECT_EVENT_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            if matches!(event, InputEvent::Disconnected { .. }) {
                return Ok(());
            }
        }
        Err("Go Direct event stream closed before the disconnect event".to_string())
    })
    .await
    .map_err(|_| "Go Direct disconnect event timed out".to_string())?
}

async fn qualify_session(
    pool: &InputSessionPool,
    device: &DeviceSummary,
    slot: &str,
    minimum_samples: u64,
    require_health: bool,
) -> Result<SessionEvidence, String> {
    let mut events = pool
        .connect(
            slot,
            &device.id,
            SessionConfig {
                period_us: DEFAULT_PERIOD_US,
            },
        )
        .await?;
    let capture_result = capture(&mut events, minimum_samples, require_health).await;
    let cleanup_started = Instant::now();
    let cleanup_result = pool.disconnect(slot).await;
    let disconnect_event_result = await_disconnected(&mut events).await;
    let cleanup_ms = cleanup_started.elapsed().as_secs_f64() * 1_000.0;

    match (capture_result, cleanup_result, disconnect_event_result) {
        (Ok((connection, stream, health, startup_ms)), Ok(()), Ok(())) => Ok(SessionEvidence {
            connection,
            stream: stream.evidence(),
            health,
            startup_ms,
            cleanup_ms,
            disconnect_event_observed: true,
        }),
        (Err(error), Ok(()), Ok(())) => Err(error),
        (result, cleanup, disconnected) => Err(format!(
            "{}; cleanup={}; disconnect_event={}",
            result.err().unwrap_or_else(|| "capture passed".into()),
            cleanup.err().unwrap_or_else(|| "ok".into()),
            disconnected.err().unwrap_or_else(|| "ok".into()),
        )),
    }
}

fn git_revision() -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|revision| revision.trim().to_string())
        .filter(|revision| !revision.is_empty())
        .unwrap_or_else(|| "unavailable".into())
}

fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

async fn verify() -> Result<serde_json::Value, String> {
    let pool = InputSessionPool::new(1);
    let devices = pool.scan().await?;
    let advertised_candidate_count = devices
        .iter()
        .filter(|device| device.respiration_belt_candidate)
        .count();
    let selected = select_device(&devices, env::var(TARGET_NAME_ENV).ok().as_deref())?;
    println!(
        "POLAR_GDX_VERIFY_SELECTED {}",
        json!({
            "discovered_go_direct_count": devices.len(),
            "advertised_gdx_rb_candidate_count": advertised_candidate_count,
            "adapter_backend": selected.adapter_info,
        })
    );

    let primary = qualify_session(
        &pool,
        &selected,
        "gdx-primary",
        PRIMARY_MINIMUM_SAMPLES,
        true,
    )
    .await?;
    let reconnect = qualify_session(
        &pool,
        &selected,
        "gdx-reconnect",
        RECONNECT_MINIMUM_SAMPLES,
        false,
    )
    .await?;
    pool.disconnect_all().await?;

    Ok(json!({
        "schema": "polar.stream.gdx_rb_native_physical.v2",
        "result": "pass",
        "generated_at_unix_ms": unix_time_ms(),
        "source_revision": git_revision(),
        "package_version": env!("CARGO_PKG_VERSION"),
        "host": {
            "os": env::consts::OS,
            "architecture": env::consts::ARCH,
            "adapter_backend": selected.adapter_info,
        },
        "selection": {
            "discovered_go_direct_count": devices.len(),
            "advertised_gdx_rb_candidate_count": advertised_candidate_count,
            "private_exact_target_used": env::var_os(TARGET_NAME_ENV).is_some(),
        },
        "requested_period_us": DEFAULT_PERIOD_US,
        "primary": primary,
        "reconnect": reconnect,
        "identity_retained": false,
        "limitations": [
            "Go Direct does not provide an absolute device sample clock on this path.",
            "Decode latency excludes controller, radio, operating-system, and output-transport delay.",
            "Force variation is reported but is not required when the belt is stationary."
        ],
    }))
}

fn failure_code(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("no unambiguous") || lower.contains("not present") {
        "NO_GDX_RB_CANDIDATE"
    } else if lower.contains("ambiguous") || lower.contains("candidates") {
        "AMBIGUOUS_GDX_RB_CANDIDATE"
    } else if lower.contains("cleanup") || lower.contains("disconnect event") {
        "CLEANUP_FAILED"
    } else if lower.contains("timed out") || lower.contains("deadline") {
        "CAPTURE_TIMEOUT"
    } else if lower.contains("metadata") || lower.contains("channel-1") {
        "IDENTITY_CONTRACT_FAILED"
    } else if lower.contains("stream") || lower.contains("rate") || lower.contains("health") {
        "STREAM_CONTRACT_FAILED"
    } else {
        "GDX_VERIFICATION_FAILED"
    }
}

#[tokio::main]
async fn main() {
    match verify().await {
        Ok(evidence) => println!("POLAR_GDX_VERIFY_COMPLETE {evidence}"),
        Err(error) => {
            let code = failure_code(&error);
            println!(
                "POLAR_GDX_VERIFY_FAILED {}",
                json!({"schema": "polar.stream.gdx_rb_native_physical.v2", "result": "fail", "code": code})
            );
            eprintln!("GDX-RB physical verification failed ({code}).");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, candidate: bool) -> DeviceSummary {
        DeviceSummary {
            id: format!("private-{name}"),
            name: name.into(),
            rssi: None,
            model_code: if candidate { "GDX-RB" } else { "GDX-UNKNOWN" },
            model_name: if candidate {
                "Go Direct Respiration Belt"
            } else {
                "Go Direct device"
            },
            respiration_belt_candidate: candidate,
            adapter_info: "test-adapter".into(),
        }
    }

    #[test]
    fn selection_is_exact_and_identity_is_not_needed_in_evidence() {
        let devices = [
            device("GDX-RB private", true),
            device("GDX-X private", false),
        ];
        assert_eq!(select_device(&devices, None).unwrap().model_code, "GDX-RB");
        assert!(select_device(&devices, Some("missing")).is_err());
        assert!(select_device(&[device("one", true), device("two", true)], None).is_err());
    }

    #[test]
    fn observation_accepts_contiguous_periodic_force() {
        let mut stream = StreamObservation::default();
        stream
            .observe(
                &[SensorSamples {
                    sensor_number: 1,
                    values: vec![1.0],
                }],
                100_000_000,
                100_000,
                0,
                0,
                0,
            )
            .unwrap();
        stream
            .observe(
                &[SensorSamples {
                    sensor_number: 1,
                    values: vec![1.2],
                }],
                200_000_000,
                100_000,
                1,
                0,
                0,
            )
            .unwrap();
        assert_eq!(stream.sequence_missing, 0);
        assert_eq!(stream.observed_rate_hz(), Some(10.0));
        assert!(
            stream
                .force_span_n()
                .is_some_and(|span| (span - 0.2).abs() < f64::EPSILON * 2.0)
        );
    }

    #[test]
    fn observation_surfaces_loss_reorder_and_nonfinite_values() {
        let mut stream = StreamObservation::default();
        stream
            .observe(
                &[SensorSamples {
                    sensor_number: 1,
                    values: vec![1.0],
                }],
                100_000_000,
                100_000,
                0,
                0,
                0,
            )
            .unwrap();
        stream
            .observe(
                &[SensorSamples {
                    sensor_number: 1,
                    values: vec![f64::NAN],
                }],
                300_000_000,
                100_000,
                2,
                1,
                0,
            )
            .unwrap();
        assert_eq!(stream.sequence_missing, 1);
        assert_eq!(stream.dropped_before, 1);
        assert_eq!(stream.nonfinite_values, 1);
    }

    #[test]
    fn failure_markers_are_stable_and_nonidentifying() {
        assert_eq!(
            failure_code("physical verifier found no unambiguous GDX-RB candidate"),
            "NO_GDX_RB_CANDIDATE"
        );
        assert_eq!(
            failure_code("Go Direct session cleanup timed out"),
            "CLEANUP_FAILED"
        );
    }
}
