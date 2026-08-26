use std::collections::VecDeque;

use polar_h10_core::AccSample;

use crate::{
    BreathingDiagnostics, BreathingPhase, BreathingSettings, BreathingSnapshot, BreathingStateMode,
    BreathingWaveformPoint, TimedAccBatch,
};

const DEFAULT_SAMPLE_PERIOD_NS: u64 = 5_000_000;
const PRESENTATION_POINT_LIMIT: usize = 512;
const MINIMUM_CALIBRATION_SAMPLES: usize = 8;
const ADAPTIVE_POINT_INTERVAL_NS: u64 = 50_000_000;
const ADAPTIVE_UPDATE_INTERVAL_NS: u64 = 500_000_000;

#[derive(Clone, Copy)]
struct TimedVector {
    source_timestamp_ns: u64,
    value: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
enum BatchTimeline {
    Nominal {
        first_timestamp_ns: u64,
        period_ns: u64,
    },
    Interpolated {
        previous_newest_timestamp_ns: u64,
        anchor_delta_ns: u64,
        sample_count: usize,
    },
}

impl BatchTimeline {
    fn timestamp_at(self, index: usize) -> u64 {
        match self {
            Self::Nominal {
                first_timestamp_ns,
                period_ns,
            } => first_timestamp_ns.saturating_add(period_ns.saturating_mul(index as u64)),
            Self::Interpolated {
                previous_newest_timestamp_ns,
                anchor_delta_ns,
                sample_count,
            } => {
                let offset = u128::from(anchor_delta_ns).saturating_mul((index + 1) as u128)
                    / sample_count as u128;
                previous_newest_timestamp_ns.saturating_add(offset as u64)
            }
        }
    }
}

pub(crate) struct TimedBreathingState {
    settings: BreathingSettings,
    filtered: [f32; 3],
    has_filtered: bool,
    motion_filtered: [f32; 3],
    has_motion_filtered: bool,
    motion_delta_ema_g: f32,
    calibration: VecDeque<TimedVector>,
    session_origin_ns: Option<u64>,
    center: [f32; 3],
    axis: [f32; 3],
    fixed_lower: f32,
    fixed_upper: f32,
    output_lower: f32,
    output_upper: f32,
    calibration_span: f32,
    pca_dominance_01: f32,
    calibrated: bool,
    latest_projection_g: f32,
    latest_volume_01: f32,
    adaptive_projections: VecDeque<(u64, f32)>,
    last_adaptive_update_ns: Option<u64>,
    last_processed_timestamp_ns: Option<u64>,
    last_batch_newest_timestamp_ns: Option<u64>,
    active_phase: BreathingPhase,
    active_since_ns: Option<u64>,
    candidate_phase: Option<BreathingPhase>,
    candidate_since_ns: Option<u64>,
    previous_state_coordinate: f32,
    previous_state_timestamp_ns: Option<u64>,
    derivative_per_second: f32,
    presentation_points: VecDeque<BreathingWaveformPoint>,
    diagnostics: BreathingDiagnostics,
}

impl TimedBreathingState {
    pub(crate) fn new(settings: BreathingSettings, config_generation: u64) -> Self {
        let mut state = Self {
            settings,
            filtered: [0.0; 3],
            has_filtered: false,
            motion_filtered: [0.0; 3],
            has_motion_filtered: false,
            motion_delta_ema_g: 0.0,
            calibration: VecDeque::new(),
            session_origin_ns: None,
            center: [0.0; 3],
            axis: [1.0, 0.0, 0.0],
            fixed_lower: -0.02,
            fixed_upper: 0.02,
            output_lower: -0.02,
            output_upper: 0.02,
            calibration_span: 0.04,
            pca_dominance_01: 0.0,
            calibrated: false,
            latest_projection_g: 0.0,
            latest_volume_01: 0.5,
            adaptive_projections: VecDeque::new(),
            last_adaptive_update_ns: None,
            last_processed_timestamp_ns: None,
            last_batch_newest_timestamp_ns: None,
            active_phase: BreathingPhase::Pausing,
            active_since_ns: None,
            candidate_phase: None,
            candidate_since_ns: None,
            previous_state_coordinate: 0.0,
            previous_state_timestamp_ns: None,
            derivative_per_second: 0.0,
            presentation_points: VecDeque::new(),
            diagnostics: BreathingDiagnostics {
                config_generation,
                pca_axis: [1.0, 0.0, 0.0],
                latest_effective_sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
                ..BreathingDiagnostics::default()
            },
        };
        state.reset_phase();
        state
    }

    pub(crate) fn set_config_generation(&mut self, generation: u64) {
        self.diagnostics.config_generation = generation;
    }

    pub(crate) fn diagnostics(&self) -> BreathingDiagnostics {
        self.diagnostics
    }

    pub(crate) fn take_presentation_points(&mut self) -> Vec<BreathingWaveformPoint> {
        self.presentation_points.drain(..).collect()
    }

    pub(crate) fn apply_settings(&mut self, settings: BreathingSettings, generation: u64) {
        let volume_changed = self.settings.volume_mode != settings.volume_mode
            || self.settings.axes != settings.axes
            || self.settings.calibration_window_seconds != settings.calibration_window_seconds
            || self.settings.minimum_axis_range_g != settings.minimum_axis_range_g
            || self.settings.invert_direction != settings.invert_direction
            || self.settings.adaptive_bounds != settings.adaptive_bounds
            || self.settings.adaptive_window_seconds != settings.adaptive_window_seconds
            || self.settings.lower_quantile != settings.lower_quantile
            || self.settings.upper_quantile != settings.upper_quantile
            || self.settings.volume_filter_tau_seconds != settings.volume_filter_tau_seconds;
        let state_changed = self.settings.state_mode != settings.state_mode
            || self.settings.sensitivity != settings.sensitivity
            || self.settings.stale_timeout_seconds != settings.stale_timeout_seconds
            || self.settings.phase_derivative_tau_seconds != settings.phase_derivative_tau_seconds
            || self.settings.phase_enter_threshold_per_second
                != settings.phase_enter_threshold_per_second
            || self.settings.phase_hold_threshold_per_second
                != settings.phase_hold_threshold_per_second
            || self.settings.phase_confirmation_seconds != settings.phase_confirmation_seconds
            || self.settings.phase_minimum_dwell_seconds != settings.phase_minimum_dwell_seconds;

        self.settings = settings;
        self.diagnostics.config_generation = generation;
        if volume_changed {
            self.reset_estimator(false);
        } else if state_changed {
            self.reset_phase();
        }
    }

    pub(crate) fn push(
        &mut self,
        samples: &[AccSample],
        timing: TimedAccBatch,
    ) -> Option<BreathingSnapshot> {
        if samples.is_empty() {
            return None;
        }
        self.diagnostics.latest_gap = false;
        self.diagnostics.latest_clock_reset = false;
        self.diagnostics.latest_lost = false;
        self.diagnostics.clock_revision = timing.clock_revision;

        let clock_reset = timing.clock_reset;
        if clock_reset {
            self.diagnostics.clock_reset_count =
                self.diagnostics.clock_reset_count.saturating_add(1);
            self.reset_estimator(true);
            self.diagnostics.latest_clock_reset = true;
            self.diagnostics.latest_lost = true;
            self.diagnostics.lost_count = self.diagnostics.lost_count.saturating_add(1);
        }

        let period_ns = if timing.sample_period_ns == 0 {
            DEFAULT_SAMPLE_PERIOD_NS
        } else {
            timing.sample_period_ns
        };
        let sample_count = samples.len();
        let expected_anchor_delta_ns = period_ns.saturating_mul(sample_count as u64);
        let previous_batch_newest = self.last_batch_newest_timestamp_ns;
        let advancing_anchor_delta = previous_batch_newest.and_then(|previous| {
            timing
                .newest_sensor_timestamp_ns
                .checked_sub(previous)
                .filter(|delta| *delta > 0)
        });
        let nominal_first_timestamp_ns = timing
            .newest_sensor_timestamp_ns
            .saturating_sub(period_ns.saturating_mul(sample_count.saturating_sub(1) as u64));
        let genuine_forward_gap = previous_batch_newest.is_some_and(|previous| {
            nominal_first_timestamp_ns.saturating_sub(previous) > self.stale_timeout_ns()
        });
        let nominal_backfill_advances =
            previous_batch_newest.is_none_or(|previous| nominal_first_timestamp_ns > previous);
        let timeline = match (previous_batch_newest, advancing_anchor_delta) {
            (Some(previous), Some(delta))
                if !clock_reset
                    && !timing.gap_before
                    && !genuine_forward_gap
                    && nominal_backfill_advances =>
            {
                BatchTimeline::Interpolated {
                    previous_newest_timestamp_ns: previous,
                    anchor_delta_ns: delta,
                    sample_count,
                }
            }
            _ => BatchTimeline::Nominal {
                first_timestamp_ns: nominal_first_timestamp_ns,
                period_ns,
            },
        };
        self.observe_batch_timing(
            period_ns,
            sample_count,
            expected_anchor_delta_ns,
            advancing_anchor_delta,
            matches!(timeline, BatchTimeline::Interpolated { .. }),
        );
        let mut accepted_any = false;
        let mut batch_lost = clock_reset || genuine_forward_gap;

        for (index, sample) in samples.iter().enumerate() {
            let source_timestamp_ns = timeline.timestamp_at(index);
            if self
                .last_processed_timestamp_ns
                .is_some_and(|watermark| source_timestamp_ns <= watermark)
            {
                self.diagnostics.late_samples_dropped =
                    self.diagnostics.late_samples_dropped.saturating_add(1);
                continue;
            }

            if self.last_processed_timestamp_ns.is_some_and(|watermark| {
                source_timestamp_ns.saturating_sub(watermark) > self.stale_timeout_ns()
            }) {
                batch_lost = true;
            }
            if !accepted_any && timing.gap_before {
                batch_lost = true;
            }
            if batch_lost && !self.diagnostics.latest_lost {
                self.diagnostics.latest_gap = true;
                self.diagnostics.latest_lost = true;
                self.diagnostics.gap_count = self.diagnostics.gap_count.saturating_add(1);
                if !clock_reset {
                    self.diagnostics.lost_count = self.diagnostics.lost_count.saturating_add(1);
                }
                self.reset_phase();
            }

            self.process_sample(*sample, source_timestamp_ns, !batch_lost);
            self.last_processed_timestamp_ns = Some(source_timestamp_ns);
            self.diagnostics.source_timestamp_ns = source_timestamp_ns;
            self.diagnostics.accepted_samples = self.diagnostics.accepted_samples.saturating_add(1);
            accepted_any = true;
        }

        if !accepted_any {
            return None;
        }
        self.last_batch_newest_timestamp_ns = Some(timing.newest_sensor_timestamp_ns);

        let motion_score = self.motion_score();
        let ready = self.calibrated && !batch_lost && motion_score >= 0.35;
        let confidence_01 = if ready {
            self.signal_confidence(motion_score)
        } else {
            0.0
        };
        let phase = if ready {
            self.active_phase
        } else {
            BreathingPhase::BadSignal
        };
        let calibration_progress_01 = self.calibration_progress_01();
        let time_seconds = self
            .session_origin_ns
            .map(|origin| self.diagnostics.source_timestamp_ns.saturating_sub(origin) as f64 / 1e9)
            .unwrap_or(0.0);

        Some(BreathingSnapshot {
            calibrated: self.calibrated,
            ready,
            calibration_progress_01,
            confidence_01,
            volume_01: self.latest_volume_01,
            magnitude_g: self.latest_projection_g,
            phase,
            axis_range_g: self.output_upper - self.output_lower,
            time_seconds,
        })
    }

    fn observe_batch_timing(
        &mut self,
        nominal_period_ns: u64,
        sample_count: usize,
        expected_anchor_delta_ns: u64,
        advancing_anchor_delta: Option<u64>,
        interpolated: bool,
    ) {
        let Some(anchor_delta_ns) = advancing_anchor_delta else {
            self.diagnostics.latest_effective_sample_period_ns = nominal_period_ns;
            self.diagnostics.latest_anchor_residual_ns = 0;
            return;
        };
        let count = sample_count.max(1) as u64;
        self.diagnostics.latest_effective_sample_period_ns =
            anchor_delta_ns.saturating_add(count / 2) / count;
        self.diagnostics.latest_anchor_residual_ns =
            signed_difference(anchor_delta_ns, expected_anchor_delta_ns);
        self.diagnostics.maximum_absolute_anchor_residual_ns = self
            .diagnostics
            .maximum_absolute_anchor_residual_ns
            .max(anchor_delta_ns.abs_diff(expected_anchor_delta_ns));
        if interpolated {
            self.diagnostics.interpolated_batch_count =
                self.diagnostics.interpolated_batch_count.saturating_add(1);
        }
    }

    fn process_sample(&mut self, sample: AccSample, source_timestamp_ns: u64, update_phase: bool) {
        let raw = [
            f32::from(sample.x_mg) / 1_000.0,
            f32::from(sample.y_mg) / 1_000.0,
            f32::from(sample.z_mg) / 1_000.0,
        ];
        self.session_origin_ns.get_or_insert(source_timestamp_ns);
        let dt_seconds = self
            .last_processed_timestamp_ns
            .map(|previous| source_timestamp_ns.saturating_sub(previous) as f32 / 1e9)
            .unwrap_or(0.0);
        let alpha = ema_alpha(dt_seconds, self.settings.volume_filter_tau_seconds);

        if !self.has_motion_filtered {
            self.motion_filtered = raw;
            self.has_motion_filtered = true;
        } else {
            let previous = self.motion_filtered;
            for (filtered, input) in self.motion_filtered.iter_mut().zip(raw) {
                *filtered += (input - *filtered) * alpha;
            }
            let delta = subtract(self.motion_filtered, previous);
            let magnitude = dot(delta, delta).sqrt();
            let motion_alpha = ema_alpha(dt_seconds, 0.50);
            self.motion_delta_ema_g += (magnitude - self.motion_delta_ema_g) * motion_alpha;
        }

        let mut selected = raw;
        for (index, enabled) in self.settings.axes.into_iter().enumerate() {
            if !enabled {
                selected[index] = 0.0;
            }
        }
        if !self.has_filtered {
            self.filtered = selected;
            self.has_filtered = true;
        } else {
            for (filtered, input) in self.filtered.iter_mut().zip(selected) {
                *filtered += (input - *filtered) * alpha;
            }
        }

        if !self.calibrated {
            self.calibration.push_back(TimedVector {
                source_timestamp_ns,
                value: self.filtered,
            });
            self.trim_calibration(source_timestamp_ns);
            if self.calibration_ready() {
                self.try_calibrate(source_timestamp_ns);
            }
        }
        if !self.calibrated {
            return;
        }

        let projection = dot(subtract(self.filtered, self.center), self.axis);
        self.latest_projection_g = projection;
        self.update_adaptive_bounds(source_timestamp_ns, projection);
        self.latest_volume_01 = inverse_lerp(self.output_lower, self.output_upper, projection);
        if update_phase {
            self.update_phase(source_timestamp_ns, projection);
        }
        self.presentation_points.push_back(BreathingWaveformPoint {
            source_timestamp_ns,
            volume_01: self.latest_volume_01,
        });
        while self.presentation_points.len() > PRESENTATION_POINT_LIMIT {
            self.presentation_points.pop_front();
        }
    }

    fn trim_calibration(&mut self, now_ns: u64) {
        let retain_ns = seconds_to_ns(self.settings.calibration_window_seconds)
            .saturating_add(DEFAULT_SAMPLE_PERIOD_NS.saturating_mul(2));
        while self
            .calibration
            .front()
            .is_some_and(|sample| now_ns.saturating_sub(sample.source_timestamp_ns) > retain_ns)
        {
            self.calibration.pop_front();
        }
    }

    fn calibration_ready(&self) -> bool {
        self.calibration.len() >= MINIMUM_CALIBRATION_SAMPLES
            && self.calibration.front().is_some_and(|first| {
                self.calibration.back().is_some_and(|last| {
                    last.source_timestamp_ns
                        .saturating_sub(first.source_timestamp_ns)
                        >= seconds_to_ns(self.settings.calibration_window_seconds)
                })
            })
    }

    fn calibration_progress_01(&self) -> f32 {
        if self.calibrated {
            return 1.0;
        }
        let elapsed_ns = self
            .calibration
            .front()
            .zip(self.calibration.back())
            .map(|(first, last)| {
                last.source_timestamp_ns
                    .saturating_sub(first.source_timestamp_ns)
            })
            .unwrap_or(0);
        (elapsed_ns as f64 / seconds_to_ns(self.settings.calibration_window_seconds) as f64)
            .clamp(0.0, 1.0) as f32
    }

    fn try_calibrate(&mut self, source_timestamp_ns: u64) {
        let count = self.calibration.len() as f32;
        let mut center = [0.0; 3];
        for sample in &self.calibration {
            for (value, sample_value) in center.iter_mut().zip(sample.value) {
                *value += sample_value / count;
            }
        }
        let mut covariance = [[0.0_f32; 3]; 3];
        for sample in &self.calibration {
            let delta = subtract(sample.value, center);
            for row in 0..3 {
                for column in 0..3 {
                    covariance[row][column] += delta[row] * delta[column] / count;
                }
            }
        }
        let trace = covariance[0][0] + covariance[1][1] + covariance[2][2];
        if !trace.is_finite() || trace <= 1e-10 {
            return;
        }
        let dominant_dimension = (0..3)
            .max_by(|left, right| covariance[*left][*left].total_cmp(&covariance[*right][*right]))
            .unwrap_or(0);
        let mut axis = [0.0; 3];
        axis[dominant_dimension] = 1.0;
        for _ in 0..32 {
            let next = [
                dot(covariance[0], axis),
                dot(covariance[1], axis),
                dot(covariance[2], axis),
            ];
            let magnitude = dot(next, next).sqrt();
            if !magnitude.is_finite() || magnitude <= 1e-10 {
                return;
            }
            axis = next.map(|value| value / magnitude);
        }
        let sign_index = (0..3)
            .max_by(|left, right| axis[*left].abs().total_cmp(&axis[*right].abs()))
            .unwrap_or(0);
        if axis[sign_index] < 0.0 {
            axis = axis.map(|value| -value);
        }
        if self.settings.invert_direction {
            axis = axis.map(|value| -value);
        }

        let eigenvalue = dot(
            axis,
            [
                dot(covariance[0], axis),
                dot(covariance[1], axis),
                dot(covariance[2], axis),
            ],
        );
        let dominance = (eigenvalue / trace).clamp(0.0, 1.0);
        if dominance < 0.05 {
            return;
        }
        let mut projections = self
            .calibration
            .iter()
            .map(|sample| dot(subtract(sample.value, center), axis))
            .collect::<Vec<_>>();
        projections.sort_by(f32::total_cmp);
        let lower = quantile(&projections, self.settings.lower_quantile);
        let upper = quantile(&projections, self.settings.upper_quantile);
        let span = upper - lower;
        if !span.is_finite() || span < self.settings.minimum_axis_range_g {
            return;
        }

        self.center = center;
        self.axis = axis;
        self.fixed_lower = lower;
        self.fixed_upper = upper;
        self.output_lower = lower;
        self.output_upper = upper;
        self.calibration_span = span;
        self.pca_dominance_01 = dominance;
        self.calibrated = true;
        self.latest_projection_g = dot(subtract(self.filtered, center), axis);
        self.latest_volume_01 = inverse_lerp(lower, upper, self.latest_projection_g);
        self.adaptive_projections.clear();
        self.last_adaptive_update_ns = None;
        self.reset_phase_at(
            source_timestamp_ns,
            (self.latest_projection_g - lower) / span,
        );
        self.diagnostics.calibration_span_g = span;
        self.diagnostics.pca_dominance_01 = dominance;
        self.diagnostics.pca_axis = axis;
    }

    fn update_adaptive_bounds(&mut self, timestamp_ns: u64, projection: f32) {
        if self.adaptive_projections.back().is_none_or(|(last, _)| {
            timestamp_ns.saturating_sub(*last) >= ADAPTIVE_POINT_INTERVAL_NS
        }) {
            self.adaptive_projections
                .push_back((timestamp_ns, projection));
        }
        let cutoff_ns =
            timestamp_ns.saturating_sub(seconds_to_ns(self.settings.adaptive_window_seconds));
        while self
            .adaptive_projections
            .front()
            .is_some_and(|(time, _)| *time < cutoff_ns)
        {
            self.adaptive_projections.pop_front();
        }
        if !self.settings.adaptive_bounds || self.adaptive_projections.len() < 80 {
            return;
        }
        let elapsed_ns = self
            .last_adaptive_update_ns
            .map(|last| timestamp_ns.saturating_sub(last))
            .unwrap_or(ADAPTIVE_UPDATE_INTERVAL_NS);
        if elapsed_ns < ADAPTIVE_UPDATE_INTERVAL_NS {
            return;
        }
        self.last_adaptive_update_ns = Some(timestamp_ns);
        let mut values = self
            .adaptive_projections
            .iter()
            .map(|(_, value)| *value)
            .collect::<Vec<_>>();
        values.sort_by(f32::total_cmp);
        let lower = quantile(&values, self.settings.lower_quantile);
        let upper = quantile(&values, self.settings.upper_quantile);
        let span = upper - lower;
        if span < self.settings.minimum_axis_range_g
            || span < self.calibration_span * 0.50
            || span > self.calibration_span * 2.0
        {
            return;
        }
        let dt_seconds = elapsed_ns as f32 / 1e9;
        let alpha = 1.0 - (-0.50 * dt_seconds).exp();
        self.output_lower += (lower - self.output_lower) * alpha;
        self.output_upper += (upper - self.output_upper) * alpha;
    }

    fn update_phase(&mut self, timestamp_ns: u64, projection: f32) {
        let coordinate = (projection - self.fixed_lower) / self.calibration_span;
        let Some(previous_timestamp_ns) = self.previous_state_timestamp_ns else {
            self.reset_phase_at(timestamp_ns, coordinate);
            return;
        };
        let dt_seconds = timestamp_ns.saturating_sub(previous_timestamp_ns) as f32 / 1e9;
        if dt_seconds <= 0.0 || dt_seconds > self.settings.stale_timeout_seconds {
            self.reset_phase_at(timestamp_ns, coordinate);
            return;
        }
        let raw_derivative = (coordinate - self.previous_state_coordinate) / dt_seconds;
        let alpha = ema_alpha(dt_seconds, self.settings.phase_derivative_tau_seconds);
        self.derivative_per_second += (raw_derivative - self.derivative_per_second) * alpha;
        self.previous_state_coordinate = coordinate;
        self.previous_state_timestamp_ns = Some(timestamp_ns);
        self.diagnostics.phase_derivative_per_second = self.derivative_per_second;

        if self.settings.state_mode == BreathingStateMode::LegacyV0 {
            let threshold = (0.0005 + (1.0 - self.settings.sensitivity).powi(2) * 0.015_625) / 0.05;
            let next = if self.derivative_per_second > threshold {
                BreathingPhase::Inhaling
            } else if self.derivative_per_second < -threshold {
                BreathingPhase::Exhaling
            } else {
                BreathingPhase::Pausing
            };
            self.activate(next, timestamp_ns);
            return;
        }

        let requested = if self.derivative_per_second
            >= self.settings.phase_enter_threshold_per_second
        {
            BreathingPhase::Inhaling
        } else if self.derivative_per_second <= -self.settings.phase_enter_threshold_per_second {
            BreathingPhase::Exhaling
        } else if self.derivative_per_second.abs() <= self.settings.phase_hold_threshold_per_second
        {
            BreathingPhase::Pausing
        } else {
            self.active_phase
        };
        if requested == self.active_phase {
            self.candidate_phase = None;
            self.candidate_since_ns = None;
            return;
        }
        if self.candidate_phase != Some(requested) {
            self.candidate_phase = Some(requested);
            self.candidate_since_ns = Some(timestamp_ns);
            return;
        }
        let confirmed = self.candidate_since_ns.is_some_and(|start| {
            timestamp_ns.saturating_sub(start)
                >= seconds_to_ns(self.settings.phase_confirmation_seconds)
        });
        let dwell_complete = self.active_since_ns.is_none_or(|start| {
            timestamp_ns.saturating_sub(start)
                >= seconds_to_ns(self.settings.phase_minimum_dwell_seconds)
        });
        if confirmed && dwell_complete {
            self.activate(requested, timestamp_ns);
        }
    }

    fn activate(&mut self, phase: BreathingPhase, timestamp_ns: u64) {
        if phase != self.active_phase {
            self.active_phase = phase;
            self.active_since_ns = Some(timestamp_ns);
            self.diagnostics.state_transition_count =
                self.diagnostics.state_transition_count.saturating_add(1);
        }
        self.candidate_phase = None;
        self.candidate_since_ns = None;
    }

    fn reset_phase(&mut self) {
        self.active_phase = BreathingPhase::Pausing;
        self.active_since_ns = None;
        self.candidate_phase = None;
        self.candidate_since_ns = None;
        self.previous_state_timestamp_ns = None;
        self.previous_state_coordinate = 0.0;
        self.derivative_per_second = 0.0;
        self.diagnostics.phase_derivative_per_second = 0.0;
    }

    fn reset_phase_at(&mut self, timestamp_ns: u64, coordinate: f32) {
        self.reset_phase();
        self.active_since_ns = Some(timestamp_ns);
        self.previous_state_timestamp_ns = Some(timestamp_ns);
        self.previous_state_coordinate = coordinate;
    }

    fn reset_estimator(&mut self, reset_clock: bool) {
        self.filtered = [0.0; 3];
        self.has_filtered = false;
        self.motion_filtered = [0.0; 3];
        self.has_motion_filtered = false;
        self.motion_delta_ema_g = 0.0;
        self.calibration.clear();
        self.session_origin_ns = None;
        self.center = [0.0; 3];
        self.axis = [1.0, 0.0, 0.0];
        self.fixed_lower = -0.02;
        self.fixed_upper = 0.02;
        self.output_lower = -0.02;
        self.output_upper = 0.02;
        self.calibration_span = 0.04;
        self.pca_dominance_01 = 0.0;
        self.calibrated = false;
        self.latest_projection_g = 0.0;
        self.latest_volume_01 = 0.5;
        self.adaptive_projections.clear();
        self.last_adaptive_update_ns = None;
        self.presentation_points.clear();
        if reset_clock {
            self.last_processed_timestamp_ns = None;
            self.last_batch_newest_timestamp_ns = None;
        }
        self.reset_phase();
        self.diagnostics.calibration_span_g = 0.0;
        self.diagnostics.pca_dominance_01 = 0.0;
        self.diagnostics.pca_axis = self.axis;
    }

    fn stale_timeout_ns(&self) -> u64 {
        seconds_to_ns(self.settings.stale_timeout_seconds)
    }

    fn motion_score(&self) -> f32 {
        let threshold = (self.settings.minimum_axis_range_g * 0.10).max(0.001);
        let ratio = self.motion_delta_ema_g / threshold;
        (1.0 / (1.0 + ratio * ratio)).clamp(0.0, 1.0)
    }

    fn signal_confidence(&self, motion_score: f32) -> f32 {
        let range_score =
            (self.calibration_span / (self.settings.minimum_axis_range_g * 2.0)).clamp(0.0, 1.0);
        (range_score * motion_score * self.pca_dominance_01).clamp(0.0, 1.0)
    }
}

fn seconds_to_ns(seconds: f32) -> u64 {
    (f64::from(seconds) * 1e9)
        .round()
        .clamp(0.0, u64::MAX as f64) as u64
}

fn signed_difference(actual: u64, expected: u64) -> i64 {
    if actual >= expected {
        i64::try_from(actual - expected).unwrap_or(i64::MAX)
    } else {
        -i64::try_from(expected - actual).unwrap_or(i64::MAX)
    }
}

fn ema_alpha(dt_seconds: f32, tau_seconds: f32) -> f32 {
    if dt_seconds <= 0.0 {
        1.0
    } else {
        (dt_seconds / (tau_seconds + dt_seconds)).clamp(0.0, 1.0)
    }
}

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn inverse_lerp(lower: f32, upper: f32, value: f32) -> f32 {
    if (upper - lower).abs() <= f32::EPSILON {
        0.5
    } else {
        ((value - lower) / (upper - lower)).clamp(0.0, 1.0)
    }
}

fn quantile(sorted: &[f32], fraction: f32) -> f32 {
    let position = (sorted.len() - 1) as f32 * fraction;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    sorted[lower] + (sorted[upper] - sorted[lower]) * (position - lower as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BreathingProcessor, BreathingVolumeMode};

    fn sample(index: usize) -> AccSample {
        let seconds = index as f32 / 200.0;
        AccSample {
            x_mg: (3.0 * (seconds * 0.7).sin()) as i16,
            y_mg: 0,
            z_mg: 1_000 + (35.0 * (seconds * std::f32::consts::TAU * 0.20).sin()) as i16,
        }
    }

    fn test_settings() -> BreathingSettings {
        BreathingSettings {
            volume_mode: BreathingVolumeMode::TimedPcaV1,
            state_mode: BreathingStateMode::HysteresisV1,
            calibration_window_seconds: 1.0,
            minimum_axis_range_g: 0.005,
            adaptive_bounds: false,
            phase_confirmation_seconds: 0.10,
            phase_minimum_dwell_seconds: 0.10,
            ..BreathingSettings::default()
        }
    }

    fn replay(
        chunk_sizes: &[usize],
        adaptive_bounds: bool,
    ) -> (
        BreathingSnapshot,
        BreathingDiagnostics,
        Vec<BreathingWaveformPoint>,
    ) {
        let mut processor = BreathingProcessor::new(BreathingSettings {
            adaptive_bounds,
            ..test_settings()
        });
        let samples = (0..1_400).map(sample).collect::<Vec<_>>();
        let mut offset = 0;
        let mut chunk_index = 0;
        let mut snapshot = None;
        while offset < samples.len() {
            let size = chunk_sizes[chunk_index % chunk_sizes.len()].min(samples.len() - offset);
            let newest_index = offset + size - 1;
            snapshot = processor.push_timed(
                &samples[offset..offset + size],
                TimedAccBatch {
                    newest_sensor_timestamp_ns: 1_000_000_000
                        + newest_index as u64 * DEFAULT_SAMPLE_PERIOD_NS,
                    sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
                    clock_revision: 1,
                    clock_reset: false,
                    gap_before: false,
                },
            );
            offset += size;
            chunk_index += 1;
        }
        let diagnostics = processor.diagnostics();
        let points = processor.take_presentation_points();
        (snapshot.unwrap(), diagnostics, points)
    }

    #[test]
    fn source_time_results_are_notification_size_invariant() {
        let one = replay(&[1], false);
        let ten = replay(&[10], false);
        let thirty_seven = replay(&[37], false);
        let variable = replay(&[3, 37, 1, 19, 64, 7], false);
        for candidate in [ten, thirty_seven, variable] {
            assert!((one.0.volume_01 - candidate.0.volume_01).abs() < 1e-6);
            assert!((one.0.magnitude_g - candidate.0.magnitude_g).abs() < 1e-6);
            assert_eq!(one.0.phase, candidate.0.phase);
            assert_eq!(one.1.accepted_samples, candidate.1.accepted_samples);
            assert_eq!(one.2, candidate.2);
            assert!(
                (one.1.phase_derivative_per_second - candidate.1.phase_derivative_per_second).abs()
                    < 1e-6
            );
        }
    }

    #[test]
    fn duplicate_and_backward_batches_are_dropped_without_lost_or_transition() {
        let mut processor = BreathingProcessor::new(test_settings());
        let samples = (0..300).map(sample).collect::<Vec<_>>();
        let timing = TimedAccBatch {
            newest_sensor_timestamp_ns: 1_000_000_000 + 299 * DEFAULT_SAMPLE_PERIOD_NS,
            sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
            clock_revision: 1,
            clock_reset: false,
            gap_before: false,
        };
        processor.push_timed(&samples, timing).unwrap();
        let before = processor.diagnostics();
        assert!(processor.push_timed(&samples[250..], timing).is_none());
        let after = processor.diagnostics();
        assert!(after.late_samples_dropped > before.late_samples_dropped);
        assert_eq!(after.lost_count, before.lost_count);
        assert_eq!(after.state_transition_count, before.state_transition_count);
    }

    #[test]
    fn real_forward_gap_is_lost_but_the_next_batch_recovers() {
        let mut processor = BreathingProcessor::new(test_settings());
        let calibration = (0..300).map(sample).collect::<Vec<_>>();
        processor
            .push_timed(
                &calibration,
                TimedAccBatch {
                    newest_sensor_timestamp_ns: 1_000_000_000 + 299 * DEFAULT_SAMPLE_PERIOD_NS,
                    sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
                    clock_revision: 1,
                    clock_reset: false,
                    gap_before: false,
                },
            )
            .unwrap();
        let lost = processor
            .push_timed(
                &[sample(300)],
                TimedAccBatch {
                    newest_sensor_timestamp_ns: 3_500_000_000,
                    sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
                    clock_revision: 1,
                    clock_reset: false,
                    gap_before: false,
                },
            )
            .unwrap();
        assert!(!lost.ready);
        assert_eq!(lost.phase, BreathingPhase::BadSignal);
        assert!(processor.diagnostics().latest_lost);

        let recovered = processor
            .push_timed(
                &[sample(301)],
                TimedAccBatch {
                    newest_sensor_timestamp_ns: 3_505_000_000,
                    sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
                    clock_revision: 1,
                    clock_reset: false,
                    gap_before: false,
                },
            )
            .unwrap();
        assert!(recovered.ready);
        assert_eq!(recovered.phase, BreathingPhase::Pausing);
    }

    #[test]
    fn clock_reset_restarts_calibration_and_time_state() {
        let mut processor = BreathingProcessor::new(test_settings());
        let calibration = (0..300).map(sample).collect::<Vec<_>>();
        let calibrated = processor
            .push_timed(
                &calibration,
                TimedAccBatch {
                    newest_sensor_timestamp_ns: 1_000_000_000 + 299 * DEFAULT_SAMPLE_PERIOD_NS,
                    sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
                    clock_revision: 1,
                    clock_reset: false,
                    gap_before: false,
                },
            )
            .unwrap();
        assert!(calibrated.calibrated);
        let reset = processor
            .push_timed(
                &[sample(0)],
                TimedAccBatch {
                    newest_sensor_timestamp_ns: 10_000_000,
                    sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
                    clock_revision: 2,
                    clock_reset: true,
                    gap_before: true,
                },
            )
            .unwrap();
        assert!(!reset.calibrated);
        assert!(!reset.ready);
        assert_eq!(processor.diagnostics().clock_reset_count, 1);
    }

    #[test]
    fn adaptive_output_bounds_do_not_change_canonical_phase_derivative() {
        let fixed = replay(&[37], false);
        let adaptive = replay(&[37], true);
        assert_eq!(fixed.0.phase, adaptive.0.phase);
        assert!(
            (fixed.1.phase_derivative_per_second - adaptive.1.phase_derivative_per_second).abs()
                < 1e-6
        );
    }

    #[test]
    fn calibration_initializes_phase_in_the_same_fixed_coordinate() {
        let mut state = TimedBreathingState::new(test_settings(), 0);
        for index in 0..240 {
            let value = [
                0.0,
                0.0,
                1.0 + 0.035 * (index as f32 / 200.0 * std::f32::consts::TAU).sin(),
            ];
            state.calibration.push_back(TimedVector {
                source_timestamp_ns: index as u64 * DEFAULT_SAMPLE_PERIOD_NS,
                value,
            });
            state.filtered = value;
        }
        let calibrated_at = 239 * DEFAULT_SAMPLE_PERIOD_NS;
        state.try_calibrate(calibrated_at);
        assert!(state.calibrated);
        let unchanged_projection = state.latest_projection_g;
        state.update_phase(
            calibrated_at + DEFAULT_SAMPLE_PERIOD_NS,
            unchanged_projection,
        );
        assert!(state.derivative_per_second.abs() < 1e-6);
        assert_eq!(state.active_phase, BreathingPhase::Pausing);
    }

    #[test]
    fn hysteresis_confirmation_and_minimum_dwell_gate_transitions() {
        let settings = BreathingSettings {
            phase_derivative_tau_seconds: 0.001,
            phase_enter_threshold_per_second: 0.20,
            phase_hold_threshold_per_second: 0.10,
            phase_confirmation_seconds: 0.10,
            phase_minimum_dwell_seconds: 0.20,
            ..test_settings()
        };
        let mut state = TimedBreathingState::new(settings, 0);
        state.calibrated = true;
        state.fixed_lower = 0.0;
        state.fixed_upper = 1.0;
        state.calibration_span = 1.0;
        state.reset_phase_at(0, 0.0);

        for step in 1_u64..=10 {
            let timestamp_ns = step * DEFAULT_SAMPLE_PERIOD_NS;
            state.update_phase(timestamp_ns, step as f32 * 0.005);
        }
        assert_eq!(state.active_phase, BreathingPhase::Pausing);

        for step in 11_u64..=50 {
            let timestamp_ns = step * DEFAULT_SAMPLE_PERIOD_NS;
            state.update_phase(timestamp_ns, step as f32 * 0.005);
        }
        assert_eq!(state.active_phase, BreathingPhase::Inhaling);
        let inhale_since = state.active_since_ns.unwrap();

        let peak = 0.25;
        for step in 51_u64..=76 {
            let timestamp_ns = step * DEFAULT_SAMPLE_PERIOD_NS;
            let projection = peak - (step - 50) as f32 * 0.005;
            state.update_phase(timestamp_ns, projection);
        }
        assert!(76 * DEFAULT_SAMPLE_PERIOD_NS - inhale_since < seconds_to_ns(0.20));
        assert_eq!(state.active_phase, BreathingPhase::Inhaling);

        for step in 77_u64..=90 {
            let timestamp_ns = step * DEFAULT_SAMPLE_PERIOD_NS;
            let projection = peak - (step - 50) as f32 * 0.005;
            state.update_phase(timestamp_ns, projection);
        }
        assert_eq!(state.active_phase, BreathingPhase::Exhaling);
        assert_eq!(state.diagnostics.state_transition_count, 2);
    }

    #[test]
    fn settings_resets_are_granular_and_generation_bearing() {
        let initial = test_settings();
        let mut processor = BreathingProcessor::new(initial);
        let calibration = (0..300).map(sample).collect::<Vec<_>>();
        let calibrated = processor
            .push_timed(
                &calibration,
                TimedAccBatch {
                    newest_sensor_timestamp_ns: 1_000_000_000 + 299 * DEFAULT_SAMPLE_PERIOD_NS,
                    sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
                    clock_revision: 1,
                    clock_reset: false,
                    gap_before: false,
                },
            )
            .unwrap();
        assert!(calibrated.calibrated);
        let accepted_before = processor.diagnostics().accepted_samples;

        processor.apply_settings(BreathingSettings {
            phase_enter_threshold_per_second: 0.10,
            ..initial
        });
        let state_reset = processor.diagnostics();
        assert_eq!(state_reset.config_generation, 1);
        assert_eq!(state_reset.accepted_samples, accepted_before);
        assert_eq!(state_reset.phase_derivative_per_second, 0.0);
        let after_state_change = processor
            .push_timed(
                &[sample(300)],
                TimedAccBatch {
                    newest_sensor_timestamp_ns: 1_000_000_000 + 300 * DEFAULT_SAMPLE_PERIOD_NS,
                    sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
                    clock_revision: 1,
                    clock_reset: false,
                    gap_before: false,
                },
            )
            .unwrap();
        assert!(after_state_change.calibrated);
        assert_eq!(after_state_change.phase, BreathingPhase::Pausing);

        processor.apply_settings(BreathingSettings {
            volume_filter_tau_seconds: 0.25,
            phase_enter_threshold_per_second: 0.10,
            ..initial
        });
        let volume_reset = processor.diagnostics();
        assert_eq!(volume_reset.config_generation, 2);
        assert_eq!(volume_reset.accepted_samples, accepted_before + 1);
        assert_eq!(volume_reset.calibration_span_g, 0.0);
        let after_volume_change = processor
            .push_timed(
                &[sample(301)],
                TimedAccBatch {
                    newest_sensor_timestamp_ns: 1_000_000_000 + 301 * DEFAULT_SAMPLE_PERIOD_NS,
                    sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
                    clock_revision: 1,
                    clock_reset: false,
                    gap_before: false,
                },
            )
            .unwrap();
        assert!(!after_volume_change.calibrated);
    }

    #[test]
    fn measured_h10_frame_cadence_is_interpolated_without_boundary_damage() {
        let frame_deltas_ns = [
            177_828_376_u64,
            177_828_386,
            177_889_406,
            177_858_930,
            177_858_904,
            177_828_376,
            177_828_386,
            177_858_914,
            177_797_848,
        ];
        let mut processor = BreathingProcessor::new(test_settings());
        let mut newest_timestamp_ns = 1_000_000_000_u64;
        let mut source_index = 0;
        let mut presentation = Vec::new();
        for frame in 0..10 {
            if frame > 0 {
                newest_timestamp_ns =
                    newest_timestamp_ns.saturating_add(frame_deltas_ns[frame - 1]);
            }
            let samples = (source_index..source_index + 36)
                .map(sample)
                .collect::<Vec<_>>();
            source_index += 36;
            processor
                .push_timed(
                    &samples,
                    TimedAccBatch {
                        newest_sensor_timestamp_ns: newest_timestamp_ns,
                        sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
                        clock_revision: 1,
                        clock_reset: false,
                        gap_before: false,
                    },
                )
                .unwrap();
            presentation.extend(processor.take_presentation_points());
        }

        let diagnostics = processor.diagnostics();
        assert_eq!(diagnostics.accepted_samples, 360);
        assert_eq!(diagnostics.late_samples_dropped, 0);
        assert_eq!(diagnostics.gap_count, 0);
        assert_eq!(diagnostics.interpolated_batch_count, 9);
        assert_eq!(diagnostics.latest_effective_sample_period_ns, 4_938_829);
        assert_eq!(diagnostics.latest_anchor_residual_ns, -2_202_152);
        assert_eq!(diagnostics.maximum_absolute_anchor_residual_ns, 2_202_152);
        assert!(presentation.len() > 100);
        assert!(presentation.windows(2).all(|pair| {
            let spacing = pair[1]
                .source_timestamp_ns
                .saturating_sub(pair[0].source_timestamp_ns);
            (4_930_000..=4_950_000).contains(&spacing)
        }));
        assert_eq!(
            presentation.last().unwrap().source_timestamp_ns,
            newest_timestamp_ns
        );
    }

    #[test]
    fn genuine_gap_uses_nominal_batch_spacing_and_stays_lost() {
        let mut processor = BreathingProcessor::new(test_settings());
        let calibration = (0..300).map(sample).collect::<Vec<_>>();
        let calibration_newest = 1_000_000_000 + 299 * DEFAULT_SAMPLE_PERIOD_NS;
        processor
            .push_timed(
                &calibration,
                TimedAccBatch {
                    newest_sensor_timestamp_ns: calibration_newest,
                    sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
                    clock_revision: 1,
                    clock_reset: false,
                    gap_before: false,
                },
            )
            .unwrap();
        processor.take_presentation_points();

        let gap_newest = calibration_newest + 3 * DEFAULT_SAMPLE_PERIOD_NS + 600_000_000;
        let lost = processor
            .push_timed(
                &[sample(300), sample(301), sample(302)],
                TimedAccBatch {
                    newest_sensor_timestamp_ns: gap_newest,
                    sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
                    clock_revision: 1,
                    clock_reset: false,
                    gap_before: false,
                },
            )
            .unwrap();
        let points = processor.take_presentation_points();
        assert!(!lost.ready);
        assert_eq!(lost.phase, BreathingPhase::BadSignal);
        assert_eq!(points.len(), 3);
        assert_eq!(
            points[0].source_timestamp_ns,
            gap_newest - 2 * DEFAULT_SAMPLE_PERIOD_NS
        );
        assert!(points.windows(2).all(|pair| {
            pair[1].source_timestamp_ns - pair[0].source_timestamp_ns == DEFAULT_SAMPLE_PERIOD_NS
        }));
        let diagnostics = processor.diagnostics();
        assert_eq!(diagnostics.gap_count, 1);
        assert!(diagnostics.latest_lost);
        assert_eq!(diagnostics.interpolated_batch_count, 0);
    }

    #[test]
    fn interpolation_rounding_is_monotonic_and_hits_the_exact_anchor() {
        let delta_ns = 177_828_376_u64;
        let previous = u64::MAX - delta_ns - 100;
        let timeline = BatchTimeline::Interpolated {
            previous_newest_timestamp_ns: previous,
            anchor_delta_ns: delta_ns,
            sample_count: 36,
        };
        let timestamps = (0..36)
            .map(|index| timeline.timestamp_at(index))
            .collect::<Vec<_>>();
        assert!(timestamps.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(timestamps.last().copied(), Some(previous + delta_ns));
        let mut spacings = timestamps
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .collect::<Vec<_>>();
        spacings.push(timestamps[0] - previous);
        assert!(
            spacings
                .iter()
                .all(|spacing| { matches!(*spacing, 4_939_677 | 4_939_678) })
        );
    }

    #[test]
    fn interpolation_avoids_a_frame_boundary_derivative_spike() {
        let first_newest = 1_000_000_000_u64;
        let first = BatchTimeline::Nominal {
            first_timestamp_ns: first_newest - 35 * DEFAULT_SAMPLE_PERIOD_NS,
            period_ns: DEFAULT_SAMPLE_PERIOD_NS,
        };
        let second = BatchTimeline::Interpolated {
            previous_newest_timestamp_ns: first_newest,
            anchor_delta_ns: 177_828_376,
            sample_count: 36,
        };
        let first_last = first.timestamp_at(35);
        let second_first = second.timestamp_at(0);
        let second_next = second.timestamp_at(1);
        let boundary_rate = 1e9 / (second_first - first_last) as f64;
        let interior_rate = 1e9 / (second_next - second_first) as f64;
        assert!((boundary_rate - interior_rate).abs() < 0.001);
        assert!((202.0..203.0).contains(&boundary_rate));
    }

    #[test]
    fn presentation_points_are_bounded_and_source_ordered() {
        let mut processor = BreathingProcessor::new(test_settings());
        let samples = (0..1_400).map(sample).collect::<Vec<_>>();
        processor
            .push_timed(
                &samples,
                TimedAccBatch {
                    newest_sensor_timestamp_ns: 1_000_000_000 + 1_399 * DEFAULT_SAMPLE_PERIOD_NS,
                    sample_period_ns: DEFAULT_SAMPLE_PERIOD_NS,
                    clock_revision: 1,
                    clock_reset: false,
                    gap_before: false,
                },
            )
            .unwrap();
        let points = processor.take_presentation_points();
        assert_eq!(points.len(), PRESENTATION_POINT_LIMIT);
        assert!(
            points
                .windows(2)
                .all(|pair| { pair[0].source_timestamp_ns < pair[1].source_timestamp_ns })
        );
        assert!(processor.take_presentation_points().is_empty());
    }
}
