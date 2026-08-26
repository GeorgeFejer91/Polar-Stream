//! Shared monotonic timing primitives for native acquisition and publication.
//!
//! The epoch is intentionally process-local. Values are comparable across all
//! Polar Stream crates in one run, but they are not wall-clock timestamps and
//! must not be persisted as UTC time.

use std::{collections::VecDeque, sync::OnceLock, time::Instant};

const MAX_OBSERVATIONS: usize = 128;
const MIN_TRACKING_OBSERVATIONS: usize = 8;
const MIN_TRACKING_SPAN_NS: u64 = 2_000_000_000;
const MAX_CLOCK_DRIFT: f64 = 500.0 / 1_000_000.0;
const REGRESSION_RESET_NS: u64 = 1_000_000_000;

/// Nanoseconds since the first timing observation in this process.
pub fn monotonic_now_ns() -> u64 {
    static PROCESS_EPOCH: OnceLock<Instant> = OnceLock::new();
    PROCESS_EPOCH
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockQuality {
    Warmup,
    Tracking,
}

impl ClockQuality {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Warmup => "warmup",
            Self::Tracking => "tracking",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockMapping {
    pub mapped_time_ns: u64,
    pub revision: u64,
    pub quality: ClockQuality,
    pub uncertainty_ns: u64,
    pub reset: bool,
}

#[derive(Clone, Copy, Debug)]
struct Observation {
    source_delta_seconds: f64,
    target_delta_seconds: f64,
}

/// Maps a monotonic device clock onto a process/common target clock.
///
/// The fit uses a bounded observation window, a drift-constrained affine
/// slope, and a low-delay offset quantile. Arrival jitter therefore increases
/// uncertainty instead of being smoothed into the device timeline. A large
/// source-clock regression starts a new segment.
#[derive(Debug, Default)]
pub struct SourceClockMapper {
    source_origin_ns: Option<u64>,
    target_origin_ns: u64,
    maximum_source_ns: u64,
    observations: VecDeque<Observation>,
    scale: f64,
    offset_seconds: f64,
    revision: u64,
    uncertainty_ns: u64,
}

impl SourceClockMapper {
    pub fn observe_and_map(&mut self, source_time_ns: u64, target_receive_ns: u64) -> ClockMapping {
        if source_time_ns == 0 {
            return ClockMapping {
                mapped_time_ns: target_receive_ns,
                revision: self.revision,
                quality: ClockQuality::Warmup,
                uncertainty_ns: self.uncertainty_ns,
                reset: false,
            };
        }

        let reset = self.maximum_source_ns > 0
            && source_time_ns.saturating_add(REGRESSION_RESET_NS) < self.maximum_source_ns;
        if self.source_origin_ns.is_none() || reset {
            self.reset(source_time_ns, target_receive_ns);
        } else if source_time_ns > self.maximum_source_ns {
            self.push_observation(source_time_ns, target_receive_ns);
        }

        let mapped_time_ns = self.map(source_time_ns);
        ClockMapping {
            mapped_time_ns,
            revision: self.revision,
            quality: if self.is_tracking() {
                ClockQuality::Tracking
            } else {
                ClockQuality::Warmup
            },
            uncertainty_ns: self.uncertainty_ns,
            reset,
        }
    }

    pub fn map(&self, source_time_ns: u64) -> u64 {
        let Some(origin) = self.source_origin_ns else {
            return 0;
        };
        let delta_seconds = signed_delta_seconds(source_time_ns, origin);
        let mapped_delta = self.scale_or_one() * delta_seconds + self.offset_seconds;
        add_signed_seconds(self.target_origin_ns, mapped_delta)
    }

    fn reset(&mut self, source_time_ns: u64, target_receive_ns: u64) {
        self.source_origin_ns = Some(source_time_ns);
        self.target_origin_ns = target_receive_ns;
        self.maximum_source_ns = source_time_ns;
        self.observations.clear();
        self.observations.push_back(Observation {
            source_delta_seconds: 0.0,
            target_delta_seconds: 0.0,
        });
        self.scale = 1.0;
        self.offset_seconds = 0.0;
        self.uncertainty_ns = 0;
        self.revision = self.revision.saturating_add(1);
    }

    fn push_observation(&mut self, source_time_ns: u64, target_receive_ns: u64) {
        let Some(origin) = self.source_origin_ns else {
            return;
        };
        self.maximum_source_ns = source_time_ns;
        if self.observations.len() == MAX_OBSERVATIONS {
            self.observations.pop_front();
        }
        self.observations.push_back(Observation {
            source_delta_seconds: signed_delta_seconds(source_time_ns, origin),
            target_delta_seconds: signed_delta_seconds(target_receive_ns, self.target_origin_ns),
        });
        self.refit();
    }

    fn refit(&mut self) {
        let Some(origin) = self.source_origin_ns else {
            return;
        };
        if self.observations.len() < MIN_TRACKING_OBSERVATIONS
            || self.maximum_source_ns.saturating_sub(origin) < MIN_TRACKING_SPAN_NS
        {
            return;
        }
        let count = self.observations.len() as f64;
        let source_mean = self
            .observations
            .iter()
            .map(|observation| observation.source_delta_seconds)
            .sum::<f64>()
            / count;
        let target_mean = self
            .observations
            .iter()
            .map(|observation| observation.target_delta_seconds)
            .sum::<f64>()
            / count;
        let mut covariance = 0.0;
        let mut variance = 0.0;
        for observation in &self.observations {
            let source = observation.source_delta_seconds - source_mean;
            covariance += source * (observation.target_delta_seconds - target_mean);
            variance += source * source;
        }
        if variance > f64::EPSILON {
            self.scale =
                (covariance / variance).clamp(1.0 - MAX_CLOCK_DRIFT, 1.0 + MAX_CLOCK_DRIFT);
        }

        let mut offsets = self
            .observations
            .iter()
            .map(|observation| {
                observation.target_delta_seconds
                    - self.scale_or_one() * observation.source_delta_seconds
            })
            .collect::<Vec<_>>();
        offsets.sort_by(f64::total_cmp);
        self.offset_seconds = percentile(&offsets, 5);
        let spread_seconds = (percentile(&offsets, 95) - self.offset_seconds).max(0.0);
        self.uncertainty_ns = seconds_to_ns(spread_seconds);
        self.revision = self.revision.saturating_add(1);
    }

    fn is_tracking(&self) -> bool {
        let Some(origin) = self.source_origin_ns else {
            return false;
        };
        self.observations.len() >= MIN_TRACKING_OBSERVATIONS
            && self.maximum_source_ns.saturating_sub(origin) >= MIN_TRACKING_SPAN_NS
    }

    fn scale_or_one(&self) -> f64 {
        if self.scale == 0.0 { 1.0 } else { self.scale }
    }
}

fn signed_delta_seconds(value: u64, origin: u64) -> f64 {
    if value >= origin {
        value.saturating_sub(origin) as f64 / 1_000_000_000.0
    } else {
        -(origin.saturating_sub(value) as f64 / 1_000_000_000.0)
    }
}

fn add_signed_seconds(origin_ns: u64, delta_seconds: f64) -> u64 {
    if !delta_seconds.is_finite() {
        return origin_ns;
    }
    if delta_seconds >= 0.0 {
        origin_ns.saturating_add(seconds_to_ns(delta_seconds))
    } else {
        origin_ns.saturating_sub(seconds_to_ns(-delta_seconds))
    }
}

fn seconds_to_ns(seconds: f64) -> u64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        0
    } else {
        (seconds * 1_000_000_000.0)
            .round()
            .clamp(0.0, u64::MAX as f64) as u64
    }
}

fn percentile(sorted: &[f64], percent: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[(sorted.len() - 1) * percent.min(100) / 100]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_monotonic_clock_advances() {
        let first = monotonic_now_ns();
        let second = monotonic_now_ns();
        assert!(second >= first);
    }

    #[test]
    fn affine_mapper_tracks_drift_without_absorbing_delay_spikes() {
        let mut mapper = SourceClockMapper::default();
        let source_origin = 10_000_000_000_u64;
        let target_origin = 2_000_000_000_u64;
        let mut mapping = mapper.observe_and_map(source_origin, target_origin + 4_000_000);
        for index in 1..=40_u64 {
            let source = source_origin + index * 100_000_000;
            let drifted = target_origin + index * 100_010_000;
            let jitter = if index % 9 == 0 {
                12_000_000
            } else {
                4_000_000
            };
            mapping = mapper.observe_and_map(source, drifted + jitter);
        }
        assert_eq!(mapping.quality, ClockQuality::Tracking);
        assert!(mapping.uncertainty_ns >= 7_000_000);
        let expected = target_origin + 40 * 100_010_000 + 4_000_000;
        assert!(mapping.mapped_time_ns.abs_diff(expected) < 2_000_000);
    }

    #[test]
    fn source_clock_regression_starts_a_new_segment() {
        let mut mapper = SourceClockMapper::default();
        mapper.observe_and_map(5_000_000_000, 1_000_000_000);
        mapper.observe_and_map(7_000_000_000, 3_000_000_000);
        let mapping = mapper.observe_and_map(1_000_000_000, 4_000_000_000);
        assert!(mapping.reset);
        assert_eq!(mapping.mapped_time_ns, 4_000_000_000);
    }

    #[test]
    fn slightly_out_of_order_cross_stream_observation_uses_existing_map() {
        let mut mapper = SourceClockMapper::default();
        mapper.observe_and_map(10_000_000_000, 2_000_000_000);
        mapper.observe_and_map(10_500_000_000, 2_500_000_000);
        let mapping = mapper.observe_and_map(10_450_000_000, 2_510_000_000);
        assert!(!mapping.reset);
        assert_eq!(mapping.mapped_time_ns, 2_450_000_000);
    }
}
