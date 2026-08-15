use std::{
    fs::{self, File},
    io::{BufWriter, Write},
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

const QUEUE_CAPACITY: usize = 128;
const ECG_RATE_HZ: f64 = 130.0;
const ACC_RATE_HZ: f64 = 200.0;

#[derive(Debug)]
enum CsvMessage {
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
    HeartRate {
        clock: CaptureClock,
        beats_per_minute: u16,
        rr_intervals_ms: Vec<f32>,
    },
    Metrics {
        clock: CaptureClock,
        values: Vec<(String, f32, String)>,
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
}

impl CsvPublisher {
    pub(crate) fn start(directory: &Path, stream_name: &str) -> Result<Self, String> {
        fs::create_dir_all(directory)
            .map_err(|error| format!("Could not create the CSV recording directory: {error}"))?;
        let started_at_ms = unix_timestamp_ms();
        let filename = format!("{stream_name}_{}.csv", started_at_ms.round() as u128);
        let path = directory.join(filename);
        let file = File::create(&path)
            .map_err(|error| format!("Could not create the CSV recording: {error}"))?;
        let mut writer = BufWriter::new(file);
        write_header(&mut writer, stream_name, started_at_ms)
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

    pub(crate) fn publish_metrics(&self, values: &[(&str, f32)]) -> Result<(), String> {
        self.send(CsvMessage::Metrics {
            clock: self.clock(),
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

    pub(crate) fn publish_custom_metrics(
        &self,
        values: &[(String, f32, String)],
    ) -> Result<(), String> {
        if values.is_empty() {
            return Ok(());
        }
        self.send(CsvMessage::Metrics {
            clock: self.clock(),
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
) -> std::io::Result<()> {
    writeln!(writer, "# Polar Stream native recording")?;
    writeln!(writer, "# schema_version,2")?;
    writeln!(writer, "# stream_name,{}", csv_cell(stream_name))?;
    writeln!(writer, "# started_at_unix_ms,{started_at_ms:.3}")?;
    writeln!(
        writer,
        "# scope,All received raw ECG and ACC; HR/RR; and every derived metric produced by the active processors."
    )?;
    writeln!(
        writer,
        "host_timestamp_ms,relative_time_s,sensor_timestamp_ns,stream,sample_index,x_mg,y_mg,z_mg,value,unit"
    )
}

fn write_message(writer: &mut impl Write, message: CsvMessage) -> std::io::Result<()> {
    match message {
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
        CsvMessage::Metrics { clock, values } => {
            for (index, (id, value, unit)) in values.into_iter().enumerate() {
                write_scalar(writer, clock, &id, index, value, &unit)?;
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
    writeln!(
        writer,
        "{:.3},{:.6},,{},{index},,,,{value},{}",
        clock.host_timestamp_ms,
        clock.relative_time_s,
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
    fn writes_raw_and_scalar_rows_without_blocking_the_publisher() {
        let directory = std::env::temp_dir().join(format!(
            "polar-stream-csv-test-{}-{}",
            std::process::id(),
            unix_timestamp_ms().round() as u128
        ));
        let publisher = CsvPublisher::start(&directory, "Test_Stream").unwrap();
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
        fs::remove_dir_all(directory).unwrap();
    }
}
