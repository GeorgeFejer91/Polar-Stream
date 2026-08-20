//! Offline, identifier-free comparison of native Polar H10 and GDX-RB CSVs.
//!
//! This tool replays raw H10 ACC batches through the current Rust
//! `BreathingProcessor`, aligns that output with raw GDX-RB Force (N) on host
//! time, and emits descriptive agreement evidence. It never treats one run as
//! physiological acceptance.

use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use polar_h10_core::AccSample;
use polar_h10_metrics::{
    BreathingProcessor, BreathingSettings, RespirationReferenceSettings, TimedReferenceSample,
    TimedRespirationSample, analyze_respiration_reference,
};
use serde::Serialize;
use serde_json::json;

const MAX_CSV_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RAW_ROWS: usize = 1_000_000;
const CSV_HEADER: &str = "host_timestamp_ms,relative_time_s,sensor_timestamp_ns,stream,sample_index,x_mg,y_mg,z_mg,value,unit";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecordingRole {
    H10,
    Gdx,
}

#[derive(Debug)]
struct AccBatch {
    host_receive_time_seconds: f64,
    newest_sensor_timestamp_ns: u64,
    analysis_time_seconds: f64,
    samples: Vec<AccSample>,
}

#[derive(Debug, Default)]
struct ParsedRecording {
    schema_version: Option<String>,
    acc_batches: Vec<AccBatch>,
    force: Vec<TimedReferenceSample>,
    raw_acc_samples: usize,
    raw_force_samples: usize,
    first_acc_sensor_timestamp_ns: Option<u64>,
    last_acc_sensor_timestamp_ns: Option<u64>,
    first_acc_host_time_seconds: Option<f64>,
    last_acc_host_time_seconds: Option<f64>,
    maximum_acc_host_gap_seconds: f64,
    maximum_force_host_gap_seconds: f64,
}

#[derive(Debug)]
struct PendingAccBatch {
    next_index: usize,
    host_receive_time_seconds: f64,
    newest_sensor_timestamp_ns: u64,
    samples: Vec<AccSample>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamEvidence {
    raw_samples: usize,
    notification_batches: Option<usize>,
    rate_time_basis: &'static str,
    coverage_seconds: f64,
    observed_rate_hz: f64,
    host_receive_coverage_seconds: f64,
    sensor_coverage_seconds: Option<f64>,
    maximum_host_gap_seconds: f64,
    sensor_timestamp_advanced: Option<bool>,
}

#[derive(Debug)]
struct Arguments {
    h10_path: PathBuf,
    gdx_path: PathBuf,
    output_path: Option<PathBuf>,
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut values = env::args_os().skip(1);
    let h10_path = values.next().map(PathBuf::from).ok_or_else(usage)?;
    let gdx_path = values.next().map(PathBuf::from).ok_or_else(usage)?;
    let mut output_path = None;
    while let Some(argument) = values.next() {
        if argument == "--output" {
            output_path = Some(values.next().map(PathBuf::from).ok_or_else(usage)?);
        } else {
            return Err(usage());
        }
    }
    if h10_path == gdx_path {
        return Err("H10 and GDX-RB inputs must be distinct native CSV files".into());
    }
    Ok(Arguments {
        h10_path,
        gdx_path,
        output_path,
    })
}

fn usage() -> String {
    "usage: cargo run -p polar-stream --example analyze_respiration_reference -- <h10-native.csv> <gdx-native.csv> [--output <new-evidence.json>]".into()
}

fn read_recording(path: &Path, role: RecordingRole) -> Result<ParsedRecording, String> {
    let metadata = fs::metadata(path).map_err(|_| "native CSV could not be opened".to_string())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_CSV_BYTES {
        return Err("native CSV size was empty or exceeded the 256 MiB bound".into());
    }
    let contents = fs::read_to_string(path)
        .map_err(|_| "native CSV was not valid readable UTF-8".to_string())?;
    parse_native_csv(&contents, role)
}

fn parse_native_csv(contents: &str, role: RecordingRole) -> Result<ParsedRecording, String> {
    let mut parsed = ParsedRecording::default();
    let mut header_seen = false;
    let mut pending_acc = None;
    let mut previous_acc_host = None;
    let mut previous_force_host = None;
    let mut previous_acc_sensor = None;

    for (line_index, line) in contents.lines().enumerate() {
        let line_number = line_index + 1;
        if line.starts_with('#') {
            if let Some(value) = line.strip_prefix("# schema_version,") {
                parsed.schema_version = Some(value.trim().to_string());
            }
            continue;
        }
        if !header_seen {
            if line.trim_end_matches('\r') != CSV_HEADER {
                return Err("native CSV header did not match schema 2".into());
            }
            header_seen = true;
            continue;
        }
        if line.is_empty() {
            continue;
        }
        if !line.contains(",raw_acc,") && !line.contains(",raw_force,") {
            continue;
        }
        let fields = line.trim_end_matches('\r').split(',').collect::<Vec<_>>();
        if fields.len() != 10 {
            return Err(format!(
                "native CSV row {line_number} did not contain ten columns"
            ));
        }
        let stream = fields[3];
        if stream != "raw_acc" {
            finish_acc_batch(&mut parsed, &mut pending_acc)?;
        }
        match stream {
            "raw_acc" => {
                if parsed.raw_acc_samples >= MAX_RAW_ROWS {
                    return Err("native CSV exceeded the bounded raw row count".into());
                }
                let host_time_seconds = parse_finite(fields[0], "ACC host timestamp")? / 1_000.0;
                let sensor_timestamp_ns = fields[2]
                    .parse::<u64>()
                    .map_err(|_| "ACC sensor timestamp was invalid".to_string())?;
                if sensor_timestamp_ns == 0 {
                    return Err("ACC sensor timestamp was not positive".into());
                }
                observe_host_gap(
                    &mut previous_acc_host,
                    host_time_seconds,
                    &mut parsed.maximum_acc_host_gap_seconds,
                );
                if previous_acc_sensor.is_some_and(|previous| sensor_timestamp_ns <= previous) {
                    return Err("ACC sensor timestamps were not strictly increasing".into());
                }
                previous_acc_sensor = Some(sensor_timestamp_ns);
                parsed
                    .first_acc_sensor_timestamp_ns
                    .get_or_insert(sensor_timestamp_ns);
                parsed.last_acc_sensor_timestamp_ns = Some(sensor_timestamp_ns);
                parsed
                    .first_acc_host_time_seconds
                    .get_or_insert(host_time_seconds);
                parsed.last_acc_host_time_seconds = Some(host_time_seconds);

                let index = fields[4]
                    .parse::<usize>()
                    .map_err(|_| "ACC sample index was invalid".to_string())?;
                let sample = AccSample {
                    x_mg: fields[5]
                        .parse::<i16>()
                        .map_err(|_| "ACC X value was outside its i16 mg domain".to_string())?,
                    y_mg: fields[6]
                        .parse::<i16>()
                        .map_err(|_| "ACC Y value was outside its i16 mg domain".to_string())?,
                    z_mg: fields[7]
                        .parse::<i16>()
                        .map_err(|_| "ACC Z value was outside its i16 mg domain".to_string())?,
                };
                if fields[9] != "mg" {
                    return Err("ACC unit was not mg".into());
                }
                if index == 0 {
                    finish_acc_batch(&mut parsed, &mut pending_acc)?;
                    pending_acc = Some(PendingAccBatch {
                        next_index: 1,
                        host_receive_time_seconds: host_time_seconds,
                        newest_sensor_timestamp_ns: sensor_timestamp_ns,
                        samples: vec![sample],
                    });
                } else {
                    let batch = pending_acc.as_mut().ok_or_else(|| {
                        "ACC batch did not start at sample index zero".to_string()
                    })?;
                    if index != batch.next_index {
                        return Err("ACC sample indices were not contiguous within a batch".into());
                    }
                    batch.next_index += 1;
                    batch.host_receive_time_seconds = host_time_seconds;
                    batch.newest_sensor_timestamp_ns = sensor_timestamp_ns;
                    batch.samples.push(sample);
                }
                parsed.raw_acc_samples += 1;
            }
            "raw_force" => {
                if parsed.raw_force_samples >= MAX_RAW_ROWS {
                    return Err("native CSV exceeded the bounded raw row count".into());
                }
                let host_time_seconds = parse_finite(fields[0], "force host timestamp")? / 1_000.0;
                enforce_increasing(
                    &mut previous_force_host,
                    host_time_seconds,
                    "force host timestamps",
                    &mut parsed.maximum_force_host_gap_seconds,
                )?;
                let force_n = parse_finite(fields[8], "force value")? as f32;
                if fields[9] != "N" {
                    return Err("GDX-RB force unit was not N".into());
                }
                parsed.force.push(TimedReferenceSample {
                    host_time_seconds,
                    force_n,
                });
                parsed.raw_force_samples += 1;
            }
            _ => {}
        }
    }
    finish_acc_batch(&mut parsed, &mut pending_acc)?;

    if !header_seen || parsed.schema_version.as_deref() != Some("2") {
        return Err("native CSV did not declare schema version 2".into());
    }
    match role {
        RecordingRole::H10 if parsed.raw_acc_samples == 0 || parsed.raw_force_samples != 0 => {
            return Err("H10 CSV did not contain isolated raw ACC data".into());
        }
        RecordingRole::Gdx if parsed.raw_force_samples == 0 || parsed.raw_acc_samples != 0 => {
            return Err("GDX-RB CSV did not contain isolated raw Force (N) data".into());
        }
        _ => {}
    }
    if role == RecordingRole::H10 {
        apply_h10_clock_mapping(&mut parsed.acc_batches)?;
    }
    Ok(parsed)
}

fn finish_acc_batch(
    parsed: &mut ParsedRecording,
    pending: &mut Option<PendingAccBatch>,
) -> Result<(), String> {
    let Some(batch) = pending.take() else {
        return Ok(());
    };
    if batch.samples.is_empty() || batch.samples.len() > 512 {
        return Err("ACC notification batch was empty or exceeded 512 samples".into());
    }
    parsed.acc_batches.push(AccBatch {
        host_receive_time_seconds: batch.host_receive_time_seconds,
        newest_sensor_timestamp_ns: batch.newest_sensor_timestamp_ns,
        analysis_time_seconds: 0.0,
        samples: batch.samples,
    });
    Ok(())
}

fn apply_h10_clock_mapping(batches: &mut [AccBatch]) -> Result<(), String> {
    if batches.is_empty() {
        return Ok(());
    }
    let mut offsets = batches
        .iter()
        .map(|batch| {
            batch.host_receive_time_seconds
                - batch.newest_sensor_timestamp_ns as f64 / 1_000_000_000.0
        })
        .collect::<Vec<_>>();
    if offsets.iter().any(|offset| !offset.is_finite()) {
        return Err("H10 sensor-to-host clock offset was not finite".into());
    }
    offsets.sort_by(f64::total_cmp);
    let position = (offsets.len() - 1) as f64 * 0.05;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let fraction = position - lower as f64;
    let anchor = offsets[lower] + (offsets[upper] - offsets[lower]) * fraction;
    let mut previous = None;
    for batch in batches {
        batch.analysis_time_seconds =
            batch.newest_sensor_timestamp_ns as f64 / 1_000_000_000.0 + anchor;
        if previous.is_some_and(|value| batch.analysis_time_seconds <= value) {
            return Err("mapped H10 analysis times were not strictly increasing".into());
        }
        previous = Some(batch.analysis_time_seconds);
    }
    Ok(())
}

fn parse_finite(value: &str, label: &str) -> Result<f64, String> {
    let value = value
        .parse::<f64>()
        .map_err(|_| format!("{label} was invalid"))?;
    if !value.is_finite() {
        return Err(format!("{label} was not finite"));
    }
    Ok(value)
}

fn enforce_increasing(
    previous: &mut Option<f64>,
    value: f64,
    label: &str,
    maximum_gap: &mut f64,
) -> Result<(), String> {
    if let Some(previous_value) = *previous {
        if value <= previous_value {
            return Err(format!("{label} were not strictly increasing"));
        }
        *maximum_gap = maximum_gap.max(value - previous_value);
    }
    *previous = Some(value);
    Ok(())
}

fn observe_host_gap(previous: &mut Option<f64>, value: f64, maximum_gap: &mut f64) {
    if let Some(previous_value) = *previous
        && value > previous_value
    {
        *maximum_gap = maximum_gap.max(value - previous_value);
    }
    *previous = Some(value);
}

fn derive_respiration(batches: &[AccBatch]) -> Result<Vec<TimedRespirationSample>, String> {
    let mut processor = BreathingProcessor::default();
    let mut output = Vec::with_capacity(batches.len());
    let mut previous_host_time = None;
    for batch in batches {
        if previous_host_time.is_some_and(|previous| batch.analysis_time_seconds <= previous) {
            return Err("ACC notification host times were not strictly increasing".into());
        }
        previous_host_time = Some(batch.analysis_time_seconds);
        let snapshot = processor.push(&batch.samples).ok_or_else(|| {
            "current breathing processor rejected a non-empty ACC batch".to_string()
        })?;
        if snapshot.calibrated {
            output.push(TimedRespirationSample {
                host_time_seconds: batch.analysis_time_seconds,
                waveform_01: snapshot.volume_01,
                signed_projection_g: snapshot.magnitude_g,
                ready: snapshot.ready,
                confidence_01: snapshot.confidence_01,
            });
        }
    }
    if output.len() < 2 {
        return Err("ACC recording never completed breathing calibration".into());
    }
    Ok(output)
}

fn stream_evidence(recording: &ParsedRecording, role: RecordingRole) -> StreamEvidence {
    match role {
        RecordingRole::H10 => {
            let first_host = recording.first_acc_host_time_seconds.unwrap_or_default();
            let last_host = recording.last_acc_host_time_seconds.unwrap_or(first_host);
            let host_coverage = (last_host - first_host).max(0.0);
            let sensor_coverage = recording
                .first_acc_sensor_timestamp_ns
                .zip(recording.last_acc_sensor_timestamp_ns)
                .map_or(0.0, |(first, last)| {
                    last.saturating_sub(first) as f64 / 1_000_000_000.0
                });
            StreamEvidence {
                raw_samples: recording.raw_acc_samples,
                notification_batches: Some(recording.acc_batches.len()),
                rate_time_basis: "pmd-sensor-timestamp",
                coverage_seconds: sensor_coverage,
                observed_rate_hz: if sensor_coverage > 0.0 {
                    (recording.raw_acc_samples.saturating_sub(1)) as f64 / sensor_coverage
                } else {
                    0.0
                },
                host_receive_coverage_seconds: host_coverage,
                sensor_coverage_seconds: Some(sensor_coverage),
                maximum_host_gap_seconds: recording.maximum_acc_host_gap_seconds,
                sensor_timestamp_advanced: Some(
                    recording
                        .first_acc_sensor_timestamp_ns
                        .zip(recording.last_acc_sensor_timestamp_ns)
                        .is_some_and(|(first, last)| last > first),
                ),
            }
        }
        RecordingRole::Gdx => {
            let first = recording
                .force
                .first()
                .map_or(0.0, |sample| sample.host_time_seconds);
            let last = recording
                .force
                .last()
                .map_or(first, |sample| sample.host_time_seconds);
            let coverage = (last - first).max(0.0);
            StreamEvidence {
                raw_samples: recording.raw_force_samples,
                notification_batches: None,
                rate_time_basis: "host-receive-timestamp",
                coverage_seconds: coverage,
                observed_rate_hz: if coverage > 0.0 {
                    (recording.raw_force_samples.saturating_sub(1)) as f64 / coverage
                } else {
                    0.0
                },
                host_receive_coverage_seconds: coverage,
                sensor_coverage_seconds: None,
                maximum_host_gap_seconds: recording.maximum_force_host_gap_seconds,
                sensor_timestamp_advanced: None,
            }
        }
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

fn run(arguments: &Arguments) -> Result<serde_json::Value, String> {
    let h10 = read_recording(&arguments.h10_path, RecordingRole::H10)?;
    let gdx = read_recording(&arguments.gdx_path, RecordingRole::Gdx)?;
    let respiration = derive_respiration(&h10.acc_batches)?;
    let analysis = analyze_respiration_reference(
        &respiration,
        &gdx.force,
        RespirationReferenceSettings::default(),
    )
    .map_err(|error| error.to_string())?;
    let result = if analysis.recording_quality_passed {
        "recording-quality-pass-descriptive-analysis"
    } else {
        "recording-quality-fail-descriptive-analysis"
    };

    Ok(json!({
        "schema": "polar.stream.h10_gdx_respiration_qualification.v1",
        "result": result,
        "sourceRevision": git_revision(),
        "packageVersion": env!("CARGO_PKG_VERSION"),
        "host": {"os": env::consts::OS, "architecture": env::consts::ARCH},
        "identityRetained": false,
        "inputFileNamesRetained": false,
        "clockContract": {
            "alignmentClock": "host time with H10 PMD spacing mapped by the fifth-percentile host-minus-sensor offset",
            "h10SensorClockMappedToHost": true,
            "h10ClockAnchorQuantile": 0.05,
            "gdxAbsoluteDeviceClockAvailable": false,
            "gdxSamplesBackfilledFromHostReceiptAtNegotiatedPeriod": true
        },
        "processor": {
            "implementation": "polar_h10_metrics::BreathingProcessor",
            "settings": BreathingSettings::default(),
            "recomputedFromRawAcc": true
        },
        "h10": stream_evidence(&h10, RecordingRole::H10),
        "gdxRb": stream_evidence(&gdx, RecordingRole::Gdx),
        "analysis": analysis,
        "limitations": [
            "This is host-time alignment, not hardware clock synchronization.",
            "The bounded lag includes device, radio, operating-system, queue, and filter delay and cannot separate those components.",
            "A mounting-specific polarity recommendation is not a universal orientation setting.",
            "One descriptive recording cannot establish physiological or clinical validity."
        ]
    }))
}

fn failure_code(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("usage") || lower.contains("distinct") {
        "INVALID_ARGUMENTS"
    } else if lower.contains("csv") || lower.contains("unit") || lower.contains("sample index") {
        "CSV_CONTRACT_FAILED"
    } else if lower.contains("calibration") {
        "BREATHING_CALIBRATION_FAILED"
    } else if lower.contains("overlap") || lower.contains("paired") {
        "SYNCHRONIZED_OVERLAP_FAILED"
    } else if lower.contains("timestamp") || lower.contains("time") {
        "TIMING_CONTRACT_FAILED"
    } else {
        "RESPIRATION_ANALYSIS_FAILED"
    }
}

fn write_new(path: &Path, evidence: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|_| "evidence output directory could not be created".to_string())?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "evidence output must be a new writable file".to_string())?;
    let bytes = serde_json::to_vec_pretty(evidence)
        .map_err(|_| "evidence JSON serialization failed".to_string())?;
    file.write_all(&bytes)
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|_| "evidence JSON could not be written".to_string())
}

fn main() {
    let result = parse_arguments().and_then(|arguments| {
        let evidence = run(&arguments)?;
        if let Some(path) = &arguments.output_path {
            write_new(path, &evidence)?;
        }
        println!("POLAR_RESPIRATION_REFERENCE_COMPLETE {evidence}");
        Ok(())
    });
    if let Err(error) = result {
        let code = failure_code(&error);
        println!(
            "POLAR_RESPIRATION_REFERENCE_FAILED {}",
            json!({"schema": "polar.stream.h10_gdx_respiration_qualification.v1", "result": "fail", "code": code})
        );
        eprintln!("Respiration reference analysis failed ({code}).");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "# Polar Stream native recording\n# schema_version,2\nhost_timestamp_ms,relative_time_s,sensor_timestamp_ns,stream,sample_index,x_mg,y_mg,z_mg,value,unit\n";

    #[test]
    fn parses_h10_batches_and_preserves_notification_boundaries() {
        let csv = format!(
            "{HEADER}1000.000,0.000000,1000000000,raw_acc,0,1,2,3,,mg\n1005.000,0.005000,1005000000,raw_acc,1,4,5,6,,mg\n1005.100,0.005100,1005000000,breathing_volume,0,,,,0.5,0–1\n1010.000,0.010000,1010000000,raw_acc,0,7,8,9,,mg\n1015.000,0.015000,1015000000,raw_acc,1,10,11,12,,mg\n"
        );
        let parsed = parse_native_csv(&csv, RecordingRole::H10).unwrap();
        assert_eq!(parsed.raw_acc_samples, 4);
        assert_eq!(parsed.acc_batches.len(), 2);
        assert_eq!(parsed.acc_batches[0].samples.len(), 2);
        assert_eq!(parsed.acc_batches[1].samples[1].z_mg, 12);
    }

    #[test]
    fn parses_only_isolated_force_for_gdx_role() {
        let csv = format!(
            "{HEADER}1000.000,0.000000,0,raw_force,0,,,,1.25,N\n1100.000,0.100000,100000000,raw_force,0,,,,1.50,N\n"
        );
        let parsed = parse_native_csv(&csv, RecordingRole::Gdx).unwrap();
        assert_eq!(parsed.force.len(), 2);
        assert_eq!(parsed.force[1].force_n, 1.5);
        assert!(parse_native_csv(&csv, RecordingRole::H10).is_err());
    }

    #[test]
    fn rejects_reordered_rows_and_damaged_batches() {
        let reordered = format!(
            "{HEADER}1000.000,0.000000,1000000000,raw_acc,0,1,2,3,,mg\n999.000,0.001000,999000000,raw_acc,1,4,5,6,,mg\n"
        );
        assert!(parse_native_csv(&reordered, RecordingRole::H10).is_err());
        let damaged = format!("{HEADER}1000.000,0.000000,1000000000,raw_acc,1,1,2,3,,mg\n");
        assert!(parse_native_csv(&damaged, RecordingRole::H10).is_err());
    }

    #[test]
    fn failure_codes_do_not_include_private_paths_or_identifiers() {
        assert_eq!(
            failure_code("native CSV header did not match schema 2"),
            "CSV_CONTRACT_FAILED"
        );
        assert_eq!(
            failure_code("signals had insufficient overlap"),
            "SYNCHRONIZED_OVERLAP_FAILED"
        );
    }
}
