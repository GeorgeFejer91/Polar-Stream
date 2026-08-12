//! Native output fan-out. Sensor traffic never takes a detour through the web UI.

mod config;
mod lsl;
mod osc;

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

pub use config::{
    MetricOutputOptions, MetricProcessingOptions, MetricSpec, NormalizationMode, OutputConfig,
    OutputHealth, normalize_stream_base, output_stream_name,
};
use lsl::LslPublisher;
use osc::{OSC_TARGET, OscPublisher};
use polar_h10_core::AccSample;

#[derive(Clone, Copy, Debug)]
pub struct MetricValue<'a> {
    pub id: &'a str,
    pub value: f32,
}

pub struct OutputRouter {
    inner: Mutex<RouterInner>,
}

struct RouterInner {
    config: OutputConfig,
    osc: Option<OscPublisher>,
    lsl: LslPublisher,
    normalizers: HashMap<String, Normalizer>,
}

impl Default for OutputRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputRouter {
    pub fn new() -> Self {
        Self::with_bundled_lsl(None)
    }

    pub fn with_bundled_lsl(library_path: Option<PathBuf>) -> Self {
        Self {
            inner: Mutex::new(RouterInner {
                config: OutputConfig::default(),
                osc: None,
                lsl: LslPublisher::new(library_path),
                normalizers: HashMap::new(),
            }),
        }
    }

    pub async fn configure(&self, config: OutputConfig) -> Result<OutputHealth, String> {
        let config = config.normalized()?;
        let osc = if config.osc_enabled {
            Some(OscPublisher::connect(OSC_TARGET).await?)
        } else {
            None
        };

        let mut inner = self.inner.lock().map_err(|_| "Output router lock failed")?;
        inner.reconcile_normalizers(&config);
        inner.config = config;
        inner.osc = osc;
        inner.rebuild_lsl();
        Ok(inner.health())
    }

    pub fn config(&self) -> OutputConfig {
        self.inner
            .lock()
            .map(|inner| inner.config.clone())
            .unwrap_or_default()
    }

    /// Starts fresh whole-run and sliding normalization state for a new sensor session.
    pub fn reset_measurement(&self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.normalizers.clear();
        let config = inner.config.clone();
        inner.reconcile_normalizers(&config);
    }

    pub fn publish_ecg(&self, sensor_timestamp_ns: u64, samples: &[i32]) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if !inner.config.includes("raw_ecg") {
            return;
        }
        inner
            .lsl
            .push_scalar_series("raw_ecg", samples.iter().map(|value| *value as f32));
        if let Some(osc) = &inner.osc {
            osc.send_series(
                &inner.config.stream_name,
                "raw_ecg",
                sensor_timestamp_ns,
                samples.iter().map(|value| *value as f32),
            );
        }
    }

    pub fn publish_accelerometer(&self, sensor_timestamp_ns: u64, samples: &[AccSample]) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if inner.config.includes("raw_acc") {
            inner.lsl.push_accelerometer(samples);
            if let Some(osc) = &inner.osc {
                osc.send_accelerometer(&inner.config.stream_name, sensor_timestamp_ns, samples);
            }
        }
    }

    pub fn publish_metrics(&self, values: &[MetricValue<'_>]) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        for metric in values {
            if !inner.config.includes(metric.id) {
                continue;
            }
            let value = inner.transform(metric.id, metric.value);
            inner.lsl.push_scalar(metric.id, value);
            if let Some(osc) = &inner.osc {
                osc.send_series(
                    &inner.config.stream_name,
                    metric.id,
                    0,
                    std::iter::once(value),
                );
            }
        }
    }
}

impl RouterInner {
    fn reconcile_normalizers(&mut self, config: &OutputConfig) {
        self.normalizers.retain(|id, _| config.outputs.contains(id));
        for id in &config.outputs {
            let options = config.metric_options.get(id).copied().unwrap_or_default();
            if options.normalization == NormalizationMode::None {
                self.normalizers.remove(id);
                continue;
            }
            self.normalizers
                .entry(id.clone())
                .and_modify(|normalizer| normalizer.reconfigure(options))
                .or_insert_with(|| Normalizer::new(options));
        }
    }

    fn transform(&mut self, id: &str, value: f32) -> f32 {
        self.normalizers
            .get_mut(id)
            .map_or(value, |normalizer| normalizer.apply(value))
    }

    fn rebuild_lsl(&mut self) {
        self.lsl.clear();
        if !self.config.lsl_enabled {
            return;
        }
        for id in &self.config.outputs {
            if let Some(spec) = MetricSpec::for_id(id) {
                self.lsl.add_outlet(&self.config.stream_name, spec);
            }
        }
    }

    fn health(&self) -> OutputHealth {
        OutputHealth {
            stream_name: self.config.stream_name.clone(),
            lsl: if !self.config.lsl_enabled {
                "Off".into()
            } else {
                self.lsl.status().into()
            },
            osc: if !self.config.osc_enabled {
                "Off".into()
            } else if self.osc.is_some() {
                format!("Sending to {OSC_TARGET}")
            } else {
                "Unavailable".into()
            },
        }
    }
}

struct Normalizer {
    options: MetricOutputOptions,
    session_min: f32,
    session_max: f32,
    sequence: u64,
    window: VecDeque<WindowPoint>,
    window_min: VecDeque<(u64, f32)>,
    window_max: VecDeque<(u64, f32)>,
}

struct WindowPoint {
    sequence: u64,
    time: Instant,
}

impl Normalizer {
    fn new(options: MetricOutputOptions) -> Self {
        Self {
            options,
            session_min: f32::INFINITY,
            session_max: f32::NEG_INFINITY,
            sequence: 0,
            window: VecDeque::new(),
            window_min: VecDeque::new(),
            window_max: VecDeque::new(),
        }
    }

    fn reconfigure(&mut self, options: MetricOutputOptions) {
        if self.options != options {
            *self = Self::new(options);
        }
    }

    fn apply(&mut self, value: f32) -> f32 {
        if !value.is_finite() {
            return value;
        }
        match self.options.normalization {
            NormalizationMode::None => value,
            NormalizationMode::Session => {
                self.session_min = self.session_min.min(value);
                self.session_max = self.session_max.max(value);
                min_max(value, self.session_min, self.session_max)
            }
            NormalizationMode::SlidingWindow => {
                let now = Instant::now();
                self.sequence = self.sequence.wrapping_add(1);
                let sequence = self.sequence;
                self.window.push_back(WindowPoint {
                    sequence,
                    time: now,
                });
                while self
                    .window_min
                    .back()
                    .is_some_and(|(_, candidate)| *candidate >= value)
                {
                    self.window_min.pop_back();
                }
                self.window_min.push_back((sequence, value));
                while self
                    .window_max
                    .back()
                    .is_some_and(|(_, candidate)| *candidate <= value)
                {
                    self.window_max.pop_back();
                }
                self.window_max.push_back((sequence, value));
                let span = Duration::from_secs(u64::from(self.options.window_seconds));
                while self
                    .window
                    .front()
                    .is_some_and(|point| now.duration_since(point.time) > span)
                {
                    let Some(expired) = self.window.pop_front().map(|point| point.sequence) else {
                        break;
                    };
                    if self
                        .window_min
                        .front()
                        .is_some_and(|(candidate, _)| *candidate == expired)
                    {
                        self.window_min.pop_front();
                    }
                    if self
                        .window_max
                        .front()
                        .is_some_and(|(candidate, _)| *candidate == expired)
                    {
                        self.window_max.pop_front();
                    }
                }
                let minimum = self.window_min.front().map_or(value, |(_, value)| *value);
                let maximum = self.window_max.front().map_or(value, |(_, value)| *value);
                min_max(value, minimum, maximum)
            }
        }
    }
}

fn min_max(value: f32, minimum: f32, maximum: f32) -> f32 {
    if (maximum - minimum).abs() < f32::EPSILON {
        0.5
    } else {
        ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod normalization_tests {
    use super::*;

    #[test]
    fn session_normalization_tracks_measurement_extrema() {
        let mut normalizer = Normalizer::new(MetricOutputOptions {
            normalization: NormalizationMode::Session,
            window_seconds: 60,
            ..MetricOutputOptions::default()
        });
        assert_eq!(normalizer.apply(10.0), 0.5);
        assert_eq!(normalizer.apply(20.0), 1.0);
        assert_eq!(normalizer.apply(15.0), 0.5);
        assert_eq!(normalizer.apply(5.0), 0.0);
    }
}
