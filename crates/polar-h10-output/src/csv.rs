use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, ErrorKind, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use polar_h10_core::AccSample;
use polar_h10_metrics::MetricDefinition;

use crate::{
    SourcePalette, VERNIER_BREATHING_RECORDING_ID,
    provenance::{PolarRespirationProvenance, VernierBreathingProvenance},
};

const QUEUE_CAPACITY: usize = 128;
const FILE_COLLISION_ATTEMPTS: usize = 1_000;
const ECG_RATE_HZ: f64 = 130.0;
const ACC_RATE_HZ: f64 = 200.0;

#[derive(Debug)]
enum CsvMessage {
    VernierBreathingProvenance,
    Ecg {
        clock: CaptureClock,
        sensor_timestamp_ns: u64,
        samples: Vec<i32>,
    },
    Accelerometer {
        clock: CaptureClock,
        sensor_timestamp_ns: u64,
        samples: Vec<AccSample>,
    },
    Force {
        clock: CaptureClock,
        host_receive_timestamp_ns: u64,
        sample_period_us: u32,
        values: Vec<f32>,
    },
    HeartRate {
        clock: CaptureClock,
        beats_per_minute: u16,
        rr_intervals_ms: Vec<f32>,
    },
    Metrics {
        clock: CaptureClock,
        sensor_timestamp_ns: u64,
        values: Vec<(String, f32, String)>,
    },
    MetricSeries {
        clock: CaptureClock,
        newest_timestamp_ns: u64,
        sample_period_us: u32,
        id: String,
        unit: String,
        values: Vec<f32>,
    },
}

#[derive(Clone, Copy, Debug)]
struct CaptureClock {
    host_timestamp_ms: f64,
    relative_time_s: f64,
}

#[derive(Default)]
struct WriterStatus {
    error: Option<String>,
}

/// A bounded, fail-stop CSV writer. The sensor thread only copies one decoded
/// notification and calls `try_send`; all formatting and filesystem I/O happen
/// on the dedicated writer thread.
pub(crate) struct CsvPublisher {
    sender: SyncSender<CsvMessage>,
    path: PathBuf,
    status: Arc<Mutex<WriterStatus>>,
    started_at: Instant,
    stream_name: String,
    source_palette: Option<SourcePalette>,
    respiration_provenance: Option<PolarRespirationProvenance>,
    vernier_breathing_provenance_sent: Mutex<bool>,
}

impl CsvPublisher {
    pub(crate) fn start(
        directory: &Path,
        stream_name: &str,
        source_palette: Option<&SourcePalette>,
        respiration_provenance: Option<&PolarRespirationProvenance>,
    ) -> Result<Self, String> {
        fs::create_dir_all(directory)
            .map_err(|error| format!("Could not create the CSV recording directory: {error}"))?;
        let started_at_ms = unix_timestamp_ms();
        let (path, file) = create_recording_file(directory, stream_name, started_at_ms)
            .map_err(|error| format!("Could not create the CSV recording: {error}"))?;
        let mut writer = BufWriter::new(file);
        write_header(
            &mut writer,
            stream_name,
            started_at_ms,
            source_palette,
            respiration_provenance,
        )
        .and_then(|()| writer.flush())
        .map_err(|error| format!("Could not initialize the CSV recording: {error}"))?;

        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let status = Arc::new(Mutex::new(WriterStatus::default()));
        let writer_status = Arc::clone(&status);
        thread::Builder::new()
            .name("polar-csv-writer".into())
            .spawn(move || run_writer(receiver, writer, writer_status))
            .map_err(|error| format!("Could not start the CSV writer: {error}"))?;

        Ok(Self {
            sender,
            path,
            status,
            started_at: Instant::now(),
            stream_name: stream_name.to_owned(),
            source_palette: source_palette.cloned(),
            respiration_provenance: respiration_provenance.copied(),
            vernier_breathing_provenance_sent: Mutex::new(false),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.status
            .lock()
            .ok()
            .and_then(|status| status.error.clone())
    }

    pub(crate) fn publish_ecg(
        &self,
        sensor_timestamp_ns: u64,
        samples: &[i32],
    ) -> Result<(), String> {
        self.send(CsvMessage::Ecg {
            clock: self.clock(),
            sensor_timestamp_ns,
            samples: samples.to_vec(),
        })
    }

    pub(crate) fn publish_accelerometer(
        &self,
        sensor_timestamp_ns: u64,
        samples: &[AccSample],
    ) -> Result<(), String> {
        self.send(CsvMessage::Accelerometer {
            clock: self.clock(),
            sensor_timestamp_ns,
            samples: samples.to_vec(),
        })
    }

    pub(crate) fn publish_heart_rate(
        &self,
        beats_per_minute: u16,
        rr_intervals_ms: &[f32],
    ) -> Result<(), String> {
        self.send(CsvMessage::HeartRate {
            clock: self.clock(),
            beats_per_minute,
            rr_intervals_ms: rr_intervals_ms.to_vec(),
        })
    }

    pub(crate) fn publish_force(
        &self,
        host_receive_timestamp_ns: u64,
        values: &[f32],
        sample_period_us: u32,
    ) -> Result<(), String> {
        self.send(CsvMessage::Force {
            clock: self.clock(),
            host_receive_timestamp_ns,
            sample_period_us,
            values: values.to_vec(),
        })
    }

    pub(crate) fn publish_metrics_at(
        &self,
        sensor_timestamp_ns: u64,
        values: &[(&str, f32)],
    ) -> Result<(), String> {
        self.send(CsvMessage::Metrics {
            clock: self.clock(),
            sensor_timestamp_ns,
            values: values
                .iter()
                .map(|(id, value)| {
                    (
                        (*id).to_owned(),
                        *value,
                        MetricDefinition::for_id(id)
                            .map_or("", |metric| metric.unit)
                            .to_owned(),
                    )
                })
                .collect(),
        })
    }

    pub(crate) fn records_header_configuration(
        &self,
        stream_name: &str,
        source_palette: Option<&SourcePalette>,
        respiration_provenance: Option<&PolarRespirationProvenance>,
    ) -> bool {
        self.stream_name == stream_name
            && self.source_palette.as_ref() == source_palette
            && self.respiration_provenance.as_ref() == respiration_provenance
    }

    pub(crate) fn publish_vernier_breathing_provenance(&self) -> Result<(), String> {
        let mut sent = self
            .vernier_breathing_provenance_sent
            .lock()
            .map_err(|_| "CSV provenance lock failed".to_owned())?;
        if *sent {
            return Ok(());
        }
        self.send(CsvMessage::VernierBreathingProvenance)?;
        *sent = true;
        Ok(())
    }

    pub(crate) fn publish_metric_series_at(
        &self,
        newest_timestamp_ns: u64,
        sample_period_us: u32,
        id: &str,
        unit: &str,
        values: &[f32],
    ) -> Result<(), String> {
        if values.is_empty() {
            return Ok(());
        }
        if id == VERNIER_BREATHING_RECORDING_ID {
            self.publish_vernier_breathing_provenance()?;
        }
        self.send(CsvMessage::MetricSeries {
            clock: self.clock(),
            newest_timestamp_ns,
            sample_period_us,
            id: id.to_owned(),
            unit: unit.to_owned(),
            values: values.to_vec(),
        })
    }

    pub(crate) fn publish_custom_metrics(
        &self,
        values: &[(String, f32, String)],
    ) -> Result<(), String> {
        if values.is_empty() {
            return Ok(());
        }
        self.send(CsvMessage::Metrics {
            clock: self.clock(),
            sensor_timestamp_ns: 0,
            values: values.to_vec(),
        })
    }

    fn clock(&self) -> CaptureClock {
        CaptureClock {
            host_timestamp_ms: unix_timestamp_ms(),
            relative_time_s: self.started_at.elapsed().as_secs_f64(),
        }
    }

    fn send(&self, message: CsvMessage) -> Result<(), String> {
        if let Some(error) = self.error() {
            return Err(error);
        }
        match self.sender.try_send(message) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                let error = format!(
                    "CSV recording stopped because its bounded {QUEUE_CAPACITY}-batch writer queue filled."
                );
                self.set_error(error.clone());
                Err(error)
            }
            Err(TrySendError::Disconnected(_)) => {
                let error = "CSV recording stopped because its writer exited.".to_owned();
                self.set_error(error.clone());
                Err(error)
            }
        }
    }

    fn set_error(&self, error: String) {
        if let Ok(mut status) = self.status.lock() {
            status.error.get_or_insert(error);
        }
    }
}

fn create_recording_file(
    directory: &Path,
    stream_name: &str,
    started_at_ms: f64,
) -> std::io::Result<(PathBuf, File)> {
    let base = format!("{stream_name}_{}", started_at_ms.round() as u128);
    for collision in 0..FILE_COLLISION_ATTEMPTS {
        let filename = if collision == 0 {
            format!("{base}.csv")
        } else {
            format!("{base}_{collision}.csv")
        };
        let path = directory.join(filename);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        ErrorKind::AlreadyExists,
        "too many CSV recordings share the same timestamp",
    ))
}

fn run_writer(
    receiver: Receiver<CsvMessage>,
    mut writer: BufWriter<File>,
    status: Arc<Mutex<WriterStatus>>,
) {
    loop {
        match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(message) => {
                if let Err(error) = write_message(&mut writer, message) {
                    record_writer_error(
                        &status,
                        format!("CSV recording stopped after a write failed: {error}"),
                    );
                    return;
                }
                // Drain a short burst before flushing. This amortizes wakeups
                // while keeping already accepted data durable in the OS cache.
                loop {
                    match receiver.try_recv() {
                        Ok(message) => {
                            if let Err(error) = write_message(&mut writer, message) {
                                record_writer_error(
                                    &status,
                                    format!("CSV recording stopped after a write failed: {error}"),
                                );
                                return;
                            }
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            let _ = writer.flush();
                            return;
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = writer.flush();
                return;
            }
        }
        if let Err(error) = writer.flush() {
            record_writer_error(
                &status,
                format!("CSV recording stopped after a flush failed: {error}"),
            );
            return;
        }
    }
}

fn record_writer_error(status: &Mutex<WriterStatus>, error: String) {
    if let Ok(mut status) = status.lock() {
        status.error.get_or_insert(error);
    }
}

fn write_header(
    writer: &mut impl Write,
    stream_name: &str,
    started_at_ms: f64,
    source_palette: Option<&SourcePalette>,
    respiration_provenance: Option<&PolarRespirationProvenance>,
) -> std::io::Result<()> {
    writeln!(writer, "# Polar Stream native recording")?;
    writeln!(writer, "# schema_version,3")?;
    writeln!(writer, "# stream_name,{}", csv_cell(stream_name))?;
    if let Some(palette) = source_palette {
        writeln!(writer, "# source_palette_id,{}", csv_cell(&palette.id))?;
        writeln!(
            writer,
            "# source_palette_light_primary,{}",
            palette.light.primary
        )?;
        writeln!(
            writer,
            "# source_palette_light_secondary,{}",
            palette.light.secondary
        )?;
        writeln!(
            writer,
            "# source_palette_dark_primary,{}",
            palette.dark.primary
        )?;
        writeln!(
            writer,
            "# source_palette_dark_secondary,{}",
            palette.dark.secondary
        )?;
    }
    if let Some(provenance) = respiration_provenance {
        for field in provenance.fields() {
            writeln!(
                writer,
                "# polar_respiration_{},{}",
                field.name,
                csv_cell(&field.value)
            )?;
        }
    }
    writeln!(writer, "# started_at_unix_ms,{started_at_ms:.3}")?;
    writeln!(
        writer,
        "# scope,All received raw ECG, ACC, and Go Direct force; HR/RR; and every derived metric produced by the active processors."
    )?;
    writeln!(
        writer,
        "host_timestamp_ms,relative_time_s,sensor_timestamp_ns,stream,sample_index,x_mg,y_mg,z_mg,value,unit"
    )
}

fn write_message(writer: &mut impl Write, message: CsvMessage) -> std::io::Result<()> {
    match message {
        CsvMessage::VernierBreathingProvenance => {
            for field in VernierBreathingProvenance.fields() {
                writeln!(
                    writer,
                    "# vernier_breathing_{},{}",
                    field.name,
                    csv_cell(&field.value)
                )?;
            }
        }
        CsvMessage::Ecg {
            clock,
            sensor_timestamp_ns,
            samples,
        } => {
            let count = samples.len();
            for (index, value) in samples.into_iter().enumerate() {
                let offset_s = sample_offset_s(index, count, ECG_RATE_HZ);
                writeln!(
                    writer,
                    "{:.3},{:.6},{},raw_ecg,{index},,,,{value},uV",
                    clock.host_timestamp_ms - offset_s * 1_000.0,
                    (clock.relative_time_s - offset_s).max(0.0),
                    sensor_timestamp(sensor_timestamp_ns, index, count, ECG_RATE_HZ),
                )?;
            }
        }
        CsvMessage::Accelerometer {
            clock,
            sensor_timestamp_ns,
            samples,
        } => {
            let count = samples.len();
            for (index, sample) in samples.into_iter().enumerate() {
                let offset_s = sample_offset_s(index, count, ACC_RATE_HZ);
                writeln!(
                    writer,
                    "{:.3},{:.6},{},raw_acc,{index},{},{},{},,mg",
                    clock.host_timestamp_ms - offset_s * 1_000.0,
                    (clock.relative_time_s - offset_s).max(0.0),
                    sensor_timestamp(sensor_timestamp_ns, index, count, ACC_RATE_HZ),
                    sample.x_mg,
                    sample.y_mg,
                    sample.z_mg,
                )?;
            }
        }
        CsvMessage::Force {
            clock,
            host_receive_timestamp_ns,
            sample_period_us,
            values,
        } => {
            let count = values.len();
            let rate_hz = 1_000_000.0 / f64::from(sample_period_us.max(1));
            for (index, value) in values.into_iter().enumerate() {
                let offset_s = sample_offset_s(index, count, rate_hz);
                writeln!(
                    writer,
                    "{:.3},{:.6},{},raw_force,{index},,,,{value},N",
                    clock.host_timestamp_ms - offset_s * 1_000.0,
                    (clock.relative_time_s - offset_s).max(0.0),
                    sensor_timestamp(host_receive_timestamp_ns, index, count, rate_hz),
                )?;
            }
        }
        CsvMessage::HeartRate {
            clock,
            beats_per_minute,
            rr_intervals_ms,
        } => {
            write_scalar(
                writer,
                clock,
                "heart_rate",
                0,
                f32::from(beats_per_minute),
                "bpm",
            )?;
            for (index, interval) in rr_intervals_ms.into_iter().enumerate() {
                write_scalar(writer, clock, "rr_interval", index, interval, "ms")?;
            }
        }
        CsvMessage::Metrics {
            clock,
            sensor_timestamp_ns,
            values,
        } => {
            for (index, (id, value, unit)) in values.into_iter().enumerate() {
                write_scalar_at(writer, clock, sensor_timestamp_ns, &id, index, value, &unit)?;
            }
        }
        CsvMessage::MetricSeries {
            clock,
            newest_timestamp_ns,
            sample_period_us,
            id,
            unit,
            values,
        } => {
            let count = values.len();
            let rate_hz = 1_000_000.0 / f64::from(sample_period_us.max(1));
            for (index, value) in values.into_iter().enumerate() {
                let offset_s = sample_offset_s(index, count, rate_hz);
                writeln!(
                    writer,
                    "{:.3},{:.6},{},{},{index},,,,{value},{}",
                    clock.host_timestamp_ms - offset_s * 1_000.0,
                    (clock.relative_time_s - offset_s).max(0.0),
                    sensor_timestamp(newest_timestamp_ns, index, count, rate_hz),
                    csv_cell(&id),
                    csv_cell(&unit),
                )?;
            }
        }
    }
    Ok(())
}

fn write_scalar(
    writer: &mut impl Write,
    clock: CaptureClock,
    id: &str,
    index: usize,
    value: f32,
    unit: &str,
) -> std::io::Result<()> {
    write_scalar_at(writer, clock, 0, id, index, value, unit)
}

fn write_scalar_at(
    writer: &mut impl Write,
    clock: CaptureClock,
    sensor_timestamp_ns: u64,
    id: &str,
    index: usize,
    value: f32,
    unit: &str,
) -> std::io::Result<()> {
    let sensor_timestamp = if sensor_timestamp_ns == 0 {
        String::new()
    } else {
        sensor_timestamp_ns.to_string()
    };
    writeln!(
        writer,
        "{:.3},{:.6},{},{},{index},,,,{value},{}",
        clock.host_timestamp_ms,
        clock.relative_time_s,
        sensor_timestamp,
        csv_cell(id),
        csv_cell(unit),
    )
}

fn sample_offset_s(index: usize, count: usize, rate_hz: f64) -> f64 {
    count.saturating_sub(index + 1) as f64 / rate_hz
}

fn sensor_timestamp(timestamp_ns: u64, index: usize, count: usize, rate_hz: f64) -> String {
    if timestamp_ns == 0 {
        return String::new();
    }
    let offset_ns = (sample_offset_s(index, count, rate_hz) * 1_000_000_000.0).round() as u64;
    timestamp_ns.saturating_sub(offset_ns).to_string()
}

fn unix_timestamp_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1_000.0
}

fn csv_cell(value: &str) -> String {
    if value.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_stream_and_start_millisecond_create_distinct_files() {
        let directory = std::env::temp_dir().join(format!(
            "polar-stream-csv-collision-test-{}-{}",
            std::process::id(),
            unix_timestamp_ms().round() as u128
        ));
        fs::create_dir_all(&directory).unwrap();
        let (first_path, mut first) =
            create_recording_file(&directory, "Shared_Stream", 1_000.0).unwrap();
        let (second_path, mut second) =
            create_recording_file(&directory, "Shared_Stream", 1_000.0).unwrap();
        assert_ne!(first_path, second_path);
        assert_eq!(
            first_path.file_name().and_then(|name| name.to_str()),
            Some("Shared_Stream_1000.csv")
        );
        assert_eq!(
            second_path.file_name().and_then(|name| name.to_str()),
            Some("Shared_Stream_1000_1.csv")
        );
        first.write_all(b"polar").unwrap();
        second.write_all(b"vernier").unwrap();
        drop((first, second));
        assert_eq!(fs::read_to_string(first_path).unwrap(), "polar");
        assert_eq!(fs::read_to_string(second_path).unwrap(), "vernier");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn writes_raw_and_scalar_rows_without_blocking_the_publisher() {
        let directory = std::env::temp_dir().join(format!(
            "polar-stream-csv-test-{}-{}",
            std::process::id(),
            unix_timestamp_ms().round() as u128
        ));
        let palette = crate::source_palette("ocean").unwrap();
        let provenance = PolarRespirationProvenance::new(Default::default());
        let publisher =
            CsvPublisher::start(&directory, "Test_Stream", Some(&palette), Some(&provenance))
                .unwrap();
        let path = publisher.path().to_owned();
        publisher.publish_ecg(1_000_000_000, &[1, -2]).unwrap();
        publisher
            .publish_accelerometer(
                2_000_000_000,
                &[AccSample {
                    x_mg: 3,
                    y_mg: -4,
                    z_mg: 5,
                }],
            )
            .unwrap();
        publisher.publish_heart_rate(61, &[983.5]).unwrap();
        publisher.publish_vernier_breathing_provenance().unwrap();
        publisher
            .publish_metrics_at(3_000_000_000, &[("breathing_volume", 0.75)])
            .unwrap();
        publisher
            .publish_metric_series_at(
                4_000_000_000,
                100_000,
                "vernier_breathing",
                "0–1",
                &[0.25, 0.75],
            )
            .unwrap();
        drop(publisher);

        let mut contents = String::new();
        for _ in 0..40 {
            contents = fs::read_to_string(&path).unwrap_or_default();
            if contents.contains(",rr_interval,") {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(contents.contains(",raw_ecg,0,,,,1,uV"));
        assert!(contents.contains(",raw_acc,0,3,-4,5,,mg"));
        assert!(contents.contains(",heart_rate,0,,,,61,bpm"));
        assert!(contents.contains(",rr_interval,0,,,,983.5,ms"));
        assert!(contents.contains(",3000000000,breathing_volume,0,,,,0.75,0–1"));
        assert!(contents.contains(",3900000000,vernier_breathing,0,,,,0.25,0–1"));
        assert!(contents.contains(",4000000000,vernier_breathing,1,,,,0.75,0–1"));
        assert!(contents.contains("# schema_version,3"));
        assert!(contents.contains("# source_palette_id,ocean"));
        assert!(contents.contains("# source_palette_light_primary,#1368AA"));
        assert!(contents.contains("# source_palette_light_secondary,#1368AA"));
        assert!(contents.contains("# source_palette_dark_primary,#67B7F7"));
        assert!(contents.contains("# source_palette_dark_secondary,#67B7F7"));
        assert!(contents.contains("# polar_respiration_algorithm,polar-stream-acc-respiration"));
        assert!(contents.contains("# polar_respiration_volume_mode,timed-pca-v1"));
        assert!(contents.contains("# polar_respiration_state_mode,hysteresis-v1"));
        assert!(contents.contains("# polar_respiration_axes,\"x,z\""));
        assert!(contents.contains(&format!(
            "# polar_respiration_application_version,{}",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(
            contents
                .contains("# vernier_breathing_algorithm,polar-stream-vernier-force-respiration")
        );
        assert!(
            contents.contains("# vernier_breathing_settings_schema,vernier-breathing-settings-v1")
        );
        assert!(contents.contains("# vernier_breathing_window_seconds,30"));
        assert!(contents.contains("# vernier_breathing_lower_quantile,0.05"));
        assert!(contents.contains("# vernier_breathing_upper_quantile,0.95"));
        assert!(contents.contains("# vernier_breathing_inhale_direction,increasing-force"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn vernier_only_recording_header_has_versioned_processing_provenance() {
        let directory = std::env::temp_dir().join(format!(
            "polar-stream-vernier-csv-test-{}-{}",
            std::process::id(),
            unix_timestamp_ms().round() as u128
        ));
        let publisher = CsvPublisher::start(&directory, "Vernier_Stream", None, None).unwrap();
        let path = publisher.path().to_owned();
        publisher
            .publish_metric_series_at(100_000_000, 100_000, "vernier_breathing", "0–1", &[0.5])
            .unwrap();
        drop(publisher);
        let mut contents = String::new();
        for _ in 0..40 {
            contents = fs::read_to_string(&path).unwrap_or_default();
            if contents.contains(",vernier_breathing,") {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!contents.contains("# polar_respiration_"));
        assert!(contents.contains(&format!(
            "# vernier_breathing_application_version,{}",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(contents.contains("# vernier_breathing_robust_bounds_minimum_samples,20"));
        assert!(contents.contains("# vernier_breathing_bounds_update_samples,5"));
        assert!(contents.contains("# vernier_breathing_nonfinite_policy,hold-last-output"));
        fs::remove_dir_all(directory).unwrap();
    }
}
