#!/usr/bin/env python3
"""Derive metric previews from the canonical anonymized Polar H10 recording.

NeuroKit remains an offline signal-processing dependency. The packaged app
loads the checked-in real recording and never substitutes a generated signal.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import sys
import warnings
from pathlib import Path

import neurokit2 as nk
import numpy as np


ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / "crates/polar-h10-metrics/src/catalog.rs"
OUTPUT = ROOT / "apps/polar-stream/ui/metric-previews.js"
FIXTURE = ROOT / "apps/polar-stream/ui/data/preview-recording.json"
SAMPLE_RATE = 130
DURATION_SECONDS = 60
PREVIEW_POINTS = 112
RAW_ECG_POINTS = 224
VIEWBOX_WIDTH = 240
VIEWBOX_HEIGHT = 72
# NeuroKit peak detection and FFT reductions vary slightly across otherwise
# pinned Linux CPU runners. These remain small presentation tolerances: they do
# not permit ID, channel, time-axis, loop, or gross waveform changes.
EXTREMA_RELATIVE_TOLERANCE = 0.02
RAW_ECG_EXTREMA_RELATIVE_TOLERANCE = 0.10
SVG_VERTICAL_MAX_TOLERANCE = 3.5
SVG_VERTICAL_RMS_TOLERANCE = 1.75


def catalog_ids() -> list[str]:
    text = CATALOG.read_text(encoding="utf-8")
    return re.findall(r'metric!\(\s*"([a-z0-9_]+)"', text)


def finite(values: np.ndarray | list[float], fallback: float = 0.0) -> np.ndarray:
    result = np.asarray(values, dtype=float).reshape(-1)
    good = np.isfinite(result)
    if good.all():
        return result
    if not good.any():
        return np.full(result.shape, fallback, dtype=float)
    indices = np.arange(len(result))
    return np.interp(indices, indices[good], result[good])


def resample(values: np.ndarray | list[float], count: int = PREVIEW_POINTS) -> np.ndarray:
    values = finite(values)
    if len(values) == count:
        return values
    return np.interp(np.linspace(0, len(values) - 1, count), np.arange(len(values)), values)


def close_loop(values: np.ndarray | list[float]) -> np.ndarray:
    """Blend the final 14% back to the first sample for seamless SVG repetition."""
    result = finite(values).copy()
    blend_count = max(4, round(len(result) * 0.14))
    start = len(result) - blend_count
    delta = result[-1] - result[0]
    progress = np.linspace(0, 1, blend_count)
    smooth = progress * progress * (3 - 2 * progress)
    result[start:] -= delta * smooth
    result[-1] = result[0]
    return result


def rolling_signal_feature(signal: np.ndarray, times: np.ndarray, operation) -> np.ndarray:
    half_window = int(2.5 * SAMPLE_RATE)
    output = []
    for second in times:
        center = int(second * SAMPLE_RATE)
        window = signal[max(0, center - half_window) : min(len(signal), center + half_window)]
        output.append(operation(window) if len(window) else 0.0)
    return finite(output)


def rr_windows(peaks: np.ndarray, times: np.ndarray) -> dict[str, np.ndarray]:
    intervals = np.diff(peaks) / SAMPLE_RATE * 1000
    interval_times = peaks[1:] / SAMPLE_RATE
    rows: dict[str, list[float]] = {
        "rr_interval": [], "mean_nn": [], "mean_heart_rate": [], "rmssd": [],
        "ln_rmssd": [], "sdnn": [], "pnn50": [], "sd1": [],
    }
    for second in times:
        mask = np.abs(interval_times - second) <= 20
        window = intervals[mask]
        if len(window) < 5:
            nearest = np.argsort(np.abs(interval_times - second))[: min(12, len(intervals))]
            window = intervals[np.sort(nearest)]
        differences = np.diff(window)
        mean_nn = float(np.mean(window))
        rmssd = float(np.sqrt(np.mean(np.square(differences)))) if len(differences) else 0.0
        rows["rr_interval"].append(float(np.interp(second, interval_times, intervals)))
        rows["mean_nn"].append(mean_nn)
        rows["mean_heart_rate"].append(60_000 / mean_nn)
        rows["rmssd"].append(rmssd)
        rows["ln_rmssd"].append(math.log(max(rmssd, 1e-9)))
        rows["sdnn"].append(float(np.std(window, ddof=1)) if len(window) > 1 else 0.0)
        rows["pnn50"].append(float(np.mean(np.abs(differences) > 50) * 100) if len(differences) else 0.0)
        rows["sd1"].append(rmssd / math.sqrt(2))
    return {key: finite(value) for key, value in rows.items()}


def coherence_windows(peaks: np.ndarray, times: np.ndarray) -> dict[str, np.ndarray]:
    intervals = np.diff(peaks) / SAMPLE_RATE * 1000
    interval_times = peaks[1:] / SAMPLE_RATE
    rows: dict[str, list[float]] = {
        "coherence": [], "coherence_confidence": [], "heartmath_coherence": [],
        "coherence_peak_frequency": [], "coherence_peak_power": [], "coherence_total_power": [],
    }
    for second in times:
        mask = np.abs(interval_times - second) <= 30
        local_t = interval_times[mask]
        local_rr = intervals[mask]
        if len(local_rr) < 12:
            local_t, local_rr = interval_times, intervals
        grid = np.arange(local_t[0], local_t[-1], 0.25)
        interpolated = np.interp(grid, local_t, local_rr)
        centered = (interpolated - np.mean(interpolated)) * np.hanning(len(interpolated))
        spectrum = np.abs(np.fft.rfft(centered)) ** 2 / max(len(centered), 1)
        frequency = np.fft.rfftfreq(len(centered), 0.25)
        domain = (frequency >= 0.04) & (frequency <= 0.4)
        peak_domain = (frequency >= 0.04) & (frequency <= 0.26)
        peak_index = np.flatnonzero(peak_domain)[np.argmax(spectrum[peak_domain])]
        peak_frequency = float(frequency[peak_index])
        band = np.abs(frequency - peak_frequency) <= 0.015
        total_power = float(np.sum(spectrum[domain]))
        peak_power = float(np.sum(spectrum[band]))
        remaining = max(total_power - peak_power, 1e-9)
        coherence = peak_power / max(total_power, 1e-9)
        rows["coherence"].append(coherence)
        rows["coherence_confidence"].append(min(1.0, len(local_rr) / 45))
        rows["heartmath_coherence"].append((peak_power / remaining) ** 2)
        rows["coherence_peak_frequency"].append(peak_frequency)
        rows["coherence_peak_power"].append(peak_power)
        rows["coherence_total_power"].append(total_power)
    return {key: finite(value) for key, value in rows.items()}


def binary_lz_complexity(values: np.ndarray) -> float:
    bits = "".join("1" if value >= np.median(values) else "0" for value in values)
    if len(bits) < 2:
        return 0.0
    phrases: set[str] = set()
    cursor = 0
    while cursor < len(bits):
        length = 1
        while cursor + length <= len(bits) and bits[cursor : cursor + length] in phrases:
            length += 1
        phrases.add(bits[cursor : min(cursor + length, len(bits))])
        cursor += length
    return len(phrases) / max(len(bits), 1)


def sample_entropy(values: np.ndarray, order: int = 2) -> float:
    values = finite(values)
    if len(values) <= order + 2 or np.std(values) < 1e-12:
        return 0.0
    tolerance = 0.2 * np.std(values)

    def matches(length: int) -> int:
        templates = np.array([values[index : index + length] for index in range(len(values) - length + 1)])
        total = 0
        for index in range(len(templates) - 1):
            total += int(np.sum(np.max(np.abs(templates[index + 1 :] - templates[index]), axis=1) <= tolerance))
        return total

    short = matches(order)
    long = matches(order + 1)
    return float(-math.log((long + 1) / (short + 1)))


def psd_slope(values: np.ndarray) -> float:
    values = finite(values) - np.mean(values)
    power = np.abs(np.fft.rfft(values)) ** 2
    frequency = np.fft.rfftfreq(len(values))
    valid = (frequency > 0) & (power > 1e-12)
    if np.sum(valid) < 2:
        return 0.0
    return float(np.polyfit(np.log(frequency[valid]), np.log(power[valid]), 1)[0])


def acw50(values: np.ndarray) -> float:
    values = finite(values) - np.mean(values)
    denominator = float(np.dot(values, values))
    if denominator <= 1e-12:
        return 0.0
    for lag in range(1, max(2, len(values) // 2)):
        correlation = float(np.dot(values[:-lag], values[lag:]) / denominator)
        if correlation <= 0.5:
            return float(lag)
    return float(max(1, len(values) // 2))


def multiscale_entropy(values: np.ndarray) -> float:
    scores = []
    for scale in (1, 2, 3):
        usable = len(values) // scale * scale
        coarse = values[:usable].reshape(-1, scale).mean(axis=1)
        scores.append(sample_entropy(coarse))
    return float(np.sum(scores))


def breath_features(events: np.ndarray, event_times: np.ndarray, times: np.ndarray, prefix: str) -> dict[str, np.ndarray]:
    rows = {name: [] for name in ("mean", "sd", "cv", "acw50", "psd_slope", "lzc", "sampen", "mse")}
    for second in times:
        nearest = np.argsort(np.abs(event_times - second))[: min(12, len(events))]
        window = finite(events[np.sort(nearest)])
        mean = float(np.mean(window))
        sd = float(np.std(window, ddof=1)) if len(window) > 1 else 0.0
        rows["mean"].append(mean)
        rows["sd"].append(sd)
        rows["cv"].append(sd / max(abs(mean), 1e-9))
        rows["acw50"].append(acw50(window))
        rows["psd_slope"].append(psd_slope(window))
        rows["lzc"].append(binary_lz_complexity(window))
        rows["sampen"].append(sample_entropy(window))
        rows["mse"].append(multiscale_entropy(window))
    return {f"breath_{prefix}_{key}": finite(value) for key, value in rows.items()}


def smooth_standardize(values: np.ndarray) -> np.ndarray:
    values = finite(values)
    return (values - np.mean(values)) / max(np.std(values), 1e-9)


def path_for(values: np.ndarray, minimum: float, maximum: float, *, step: bool = False) -> str:
    values = close_loop(values)
    scale = max(maximum - minimum, 1e-9)
    points = []
    previous_y = None
    for index, value in enumerate(values):
        x = index / max(len(values) - 1, 1) * VIEWBOX_WIDTH
        y = 7 + (1 - (value - minimum) / scale) * (VIEWBOX_HEIGHT - 14)
        if index == 0:
            points.append(f"M{x:.1f},{y:.1f}")
        elif step:
            points.append(f"L{x:.1f},{previous_y:.1f}L{x:.1f},{y:.1f}")
        else:
            points.append(f"L{x:.1f},{y:.1f}")
        previous_y = y
    return "".join(points)


def rounded(value: float) -> float:
    absolute = abs(value)
    if absolute >= 100:
        return round(value, 1)
    if absolute >= 1:
        return round(value, 3)
    return round(value, 5)


def make_preview(series: list[tuple[str, str, np.ndarray]], *, step: bool = False, duration: float = DURATION_SECONDS) -> dict:
    all_values = np.concatenate([finite(values) for _, _, values in series])
    minimum = float(np.min(all_values))
    maximum = float(np.max(all_values))
    padding = (maximum - minimum) * 0.08
    if padding <= 1e-9:
        padding = max(abs(maximum) * 0.08, 0.08)
    plot_minimum = minimum - padding
    plot_maximum = maximum + padding
    return {
        "durationSeconds": rounded(duration),
        "minimum": rounded(minimum),
        "maximum": rounded(maximum),
        "channels": [
            {
                "label": label,
                "color": color,
                "path": path_for(values, plot_minimum, plot_maximum, step=step),
                "values": [rounded(value) for value in close_loop(values)],
            }
            for label, color, values in series
        ],
    }


def generate() -> dict:
    warnings.filterwarnings("ignore", category=RuntimeWarning)
    fixture_bytes = FIXTURE.read_bytes()
    fixture = json.loads(fixture_bytes)
    if fixture.get("source") != "real-polar-h10-recording":
        raise RuntimeError("Preview fixture is not marked as a real Polar H10 recording")
    if fixture.get("durationMs") != DURATION_SECONDS * 1000:
        raise RuntimeError("Preview fixture duration does not match the generator")
    if fixture["ecg"]["sampleRateHz"] != SAMPLE_RATE:
        raise RuntimeError("Preview fixture ECG sample rate does not match the generator")

    ecg = np.asarray(fixture["ecg"]["microvolts"], dtype=float)
    clean_ecg = finite(nk.ecg_clean(ecg, sampling_rate=SAMPLE_RATE, method="neurokit"))
    rr_intervals: list[float] = []
    for event in fixture.get("metricEvents", []):
        rr_intervals.extend(float(value) for value in event.get("rrIntervalsMs", []))
    peak_seconds = np.cumsum([0.0, *[value / 1000 for value in rr_intervals]])
    peaks = np.asarray(np.round(peak_seconds * SAMPLE_RATE), dtype=int)
    peaks = peaks[peaks < len(ecg)]

    acc_rate = int(fixture["accelerometer"]["sampleRateHz"])
    acc = np.asarray(fixture["accelerometer"]["samples"], dtype=float)
    acc_time = np.arange(len(acc)) / acc_rate
    centered_acc = acc - np.median(acc, axis=0)
    projection = (centered_acc[:, 0] + centered_acc[:, 2]) / 2_000
    smoothing_count = max(1, round(0.75 * acc_rate))
    rsp_clean = np.convolve(projection, np.ones(smoothing_count) / smoothing_count, mode="same")
    times = np.linspace(0, DURATION_SECONDS, PREVIEW_POINTS, endpoint=False)
    sample_times = np.arange(len(ecg)) / SAMPLE_RATE

    values: dict[str, np.ndarray] = {}
    beat_start = int((peaks[min(8, len(peaks) - 2)] + peaks[min(9, len(peaks) - 1)]) / 2)
    beat_end = min(len(ecg), beat_start + SAMPLE_RATE * 6)
    raw_ecg = resample(ecg[beat_start:beat_end], RAW_ECG_POINTS)
    values["raw_ecg"] = raw_ecg

    rsp_preview = np.interp(times, acc_time, rsp_clean)
    rsp_preview = (rsp_preview - np.min(rsp_preview)) / max(np.ptp(rsp_preview), 1e-9)
    rsp_centered = rsp_preview - 0.5
    acc_x = np.interp(times, acc_time, acc[:, 0])
    acc_y = np.interp(times, acc_time, acc[:, 1])
    acc_z = np.interp(times, acc_time, acc[:, 2])
    values["raw_acc"] = np.vstack([acc_x, acc_y, acc_z])
    values["acc_magnitude"] = np.sqrt(acc_x**2 + acc_y**2 + acc_z**2) / 1_000

    values["ecg_mean"] = rolling_signal_feature(clean_ecg, times, np.mean)
    values["ecg_rms"] = rolling_signal_feature(clean_ecg, times, lambda window: np.sqrt(np.mean(window**2)))
    values["ecg_peak_to_peak"] = rolling_signal_feature(clean_ecg, times, np.ptp)
    values["ecg_sd"] = rolling_signal_feature(clean_ecg, times, np.std)

    heart_events = fixture.get("metricEvents", [])
    heart_times = np.asarray([event["offsetMs"] / 1000 for event in heart_events])
    heart_rate = np.asarray([event["heartRateBpm"] for event in heart_events], dtype=float)
    values["heart_rate"] = np.interp(times, heart_times, heart_rate)
    values.update(rr_windows(peaks, times))
    values.update(coherence_windows(peaks, times))

    values["acc_breathing_magnitude"] = np.interp(times, acc_time, rsp_clean)
    values["breathing_volume"] = rsp_preview
    derivative = np.gradient(rsp_preview)
    threshold = np.quantile(np.abs(derivative), 0.22)
    phase = np.where(derivative > threshold, 1, np.where(derivative < -threshold, -1, 0)).astype(float)
    values["breathing_phase"] = phase
    values["breathing_calibration"] = np.clip(times / 12, 0, 1)
    rolling_range = np.asarray([
        np.ptp(rsp_clean[max(0, round((second - 20) * acc_rate)):round(second * acc_rate) + 1])
        for second in times
    ])
    values["breathing_axis_range"] = rolling_range

    peak_candidates = np.asarray(nk.signal_findpeaks(rsp_clean, relative_height_min=0.05)["Peaks"], dtype=int)
    minimum_peak_distance = max(1, round(acc_rate * 2.0))
    retained_peaks: list[int] = []
    for candidate in peak_candidates:
        if not retained_peaks or int(candidate) - retained_peaks[-1] >= minimum_peak_distance:
            retained_peaks.append(int(candidate))
    if len(retained_peaks) < 3:
        retained_peaks = [0, len(rsp_clean) // 2, len(rsp_clean) - 1]
    rsp_peaks = np.asarray(retained_peaks, dtype=int)
    breath_times = rsp_peaks[1:] / acc_rate
    breath_intervals = np.diff(rsp_peaks) / acc_rate
    breath_amplitudes = np.abs(rsp_clean[rsp_peaks[1:]] - rsp_clean[rsp_peaks[:-1]])
    values["breathing_rate"] = np.interp(times, breath_times, 60 / np.maximum(breath_intervals, 1e-6))
    event_count = np.searchsorted(breath_times, times, side="right")
    values["breathing_dynamics_confidence"] = np.clip(event_count / 10, 0, 1)
    values.update(breath_features(breath_intervals, breath_times, times, "interval"))
    values.update(breath_features(breath_amplitudes, breath_times, times, "amplitude"))

    z_hr = smooth_standardize(values["heart_rate"])
    z_rr = smooth_standardize(values["rr_interval"])
    z_rmssd = smooth_standardize(values["rmssd"])
    sigmoid = lambda vector: 1 / (1 + np.exp(-vector))
    normal_cdf = np.vectorize(lambda value: (1 + math.erf(value / math.sqrt(2))) / 2)
    values["excitement_score"] = np.clip(1 - (normal_cdf(z_rr) + normal_cdf(z_rmssd)) / 2, 0, 1)
    values["excitometer"] = sigmoid(0.65 * z_hr - 0.35 * z_rmssd)

    palette = {
        "ecg": "#c95151", "hr": "#d05a55", "hrv": "#168259", "coherence": "#6c62a8",
        "breathing": "#3b78aa", "dynamics": "#a66d19", "excitation": "#b94b40",
    }
    channel_series: dict[str, list[tuple[str, str, np.ndarray]]] = {}
    for metric_id, metric_values in values.items():
        if metric_id == "raw_acc":
            channel_series[metric_id] = [
                ("X", "#3b78aa", metric_values[0]),
                ("Y", "#168259", metric_values[1]),
                ("Z", "#a66d19", metric_values[2]),
            ]
            continue
        if metric_id.startswith("ecg_") or metric_id == "raw_ecg":
            color = palette["ecg"]
        elif metric_id in {"heart_rate", "rr_interval", "mean_nn", "mean_heart_rate"}:
            color = palette["hr"]
        elif metric_id in {"rmssd", "ln_rmssd", "sdnn", "pnn50", "sd1"}:
            color = palette["hrv"]
        elif metric_id.startswith("coherence") or metric_id == "heartmath_coherence":
            color = palette["coherence"]
        elif metric_id.startswith("breath_"):
            color = palette["dynamics"]
        elif metric_id.startswith("breathing_") or metric_id == "acc_magnitude":
            color = palette["breathing"]
        else:
            color = palette["excitation"]
        channel_series[metric_id] = [("Value", color, metric_values)]

    ids = catalog_ids()
    missing = [metric_id for metric_id in ids if metric_id not in channel_series]
    extra = [metric_id for metric_id in channel_series if metric_id not in ids]
    if missing or extra:
        raise RuntimeError(f"Preview/catalog mismatch; missing={missing}, extra={extra}")

    previews = {}
    for metric_id in ids:
        previews[metric_id] = make_preview(
            channel_series[metric_id],
            step=metric_id == "breathing_phase",
            duration=(beat_end - beat_start) / SAMPLE_RATE if metric_id == "raw_ecg" else DURATION_SECONDS,
        )
    return {
        "schemaVersion": 1,
        "source": {
            "library": "Recorded Polar H10",
            "version": "60-second anonymized fixture",
            "model": "real-polar-h10-recording",
            "fixtureSha256": hashlib.sha256(fixture_bytes).hexdigest(),
            "samplingRateHz": SAMPLE_RATE,
            "methodProvenance": f"NeuroKit2 {nk.__version__} ECG cleaning; Polar Stream formulas",
            "note": "Derived from the canonical anonymized recording; illustrative, not algorithm validation.",
        },
        "viewBox": [0, 0, VIEWBOX_WIDTH, VIEWBOX_HEIGHT],
        "metrics": previews,
    }


def render_javascript(payload: dict) -> str:
    serialized = json.dumps(payload, ensure_ascii=False, separators=(",", ":"))
    return (
        "// Generated by scripts/generate_metric_previews.py; do not edit by hand.\n"
        f"window.PolarMetricPreviews = Object.freeze({serialized});\n"
    )


def parse_javascript_payload(rendered: str) -> dict:
    prefix = "window.PolarMetricPreviews = Object.freeze("
    start = rendered.find(prefix)
    if start < 0 or not rendered.rstrip().endswith(");"):
        raise ValueError("generated preview asset does not contain the expected assignment")
    serialized = rendered[start + len(prefix) : rendered.rfind(");")]
    return json.loads(serialized)


def path_coordinates(path: str) -> np.ndarray:
    pairs = re.findall(r"[ML](-?\d+(?:\.\d+)?),(-?\d+(?:\.\d+)?)", path)
    if not pairs:
        raise ValueError("SVG path contains no M/L coordinate pairs")
    return np.asarray([(float(x), float(y)) for x, y in pairs])


def check_payload(existing: dict, generated: dict) -> list[str]:
    """Compare deterministic structure while tolerating platform floating-point drift."""
    errors: list[str] = []
    for key in ("schemaVersion", "source", "viewBox"):
        if existing.get(key) != generated.get(key):
            errors.append(f"top-level {key} differs")

    expected_ids = catalog_ids()
    existing_metrics = existing.get("metrics", {})
    generated_metrics = generated["metrics"]
    if list(existing_metrics) != expected_ids:
        errors.append("checked-in metric IDs/order differ from the Rust catalog")

    for metric_id in expected_ids:
        current = existing_metrics.get(metric_id)
        expected = generated_metrics.get(metric_id)
        if not isinstance(current, dict) or expected is None:
            errors.append(f"{metric_id}: preview is missing")
            continue
        for key in ("durationSeconds", "minimum", "maximum"):
            actual_value = current.get(key)
            expected_value = expected[key]
            if not isinstance(actual_value, (int, float)) or not math.isfinite(actual_value):
                errors.append(f"{metric_id}: {key} is not finite")
                continue
            relative_tolerance = (
                RAW_ECG_EXTREMA_RELATIVE_TOLERANCE
                if metric_id == "raw_ecg"
                else EXTREMA_RELATIVE_TOLERANCE
            )
            tolerance = max(0.05, abs(expected_value) * relative_tolerance)
            delta = abs(actual_value - expected_value)
            if delta > tolerance:
                errors.append(
                    f"{metric_id}: {key} drifted by {delta:.6g} "
                    f"(allowed {tolerance:.6g}; checked-in {actual_value:.6g}; generated {expected_value:.6g})"
                )

        current_channels = current.get("channels", [])
        expected_channels = expected["channels"]
        if len(current_channels) != len(expected_channels):
            errors.append(f"{metric_id}: channel count differs")
            continue
        for index, (actual_channel, expected_channel) in enumerate(zip(current_channels, expected_channels, strict=True)):
            for key in ("label", "color"):
                if actual_channel.get(key) != expected_channel.get(key):
                    errors.append(f"{metric_id}: channel {index} {key} differs")
            try:
                actual_points = path_coordinates(actual_channel.get("path", ""))
                expected_points = path_coordinates(expected_channel["path"])
            except ValueError as error:
                errors.append(f"{metric_id}: channel {index} {error}")
                continue
            if actual_points.shape != expected_points.shape:
                errors.append(f"{metric_id}: channel {index} SVG point count differs")
                continue
            if np.max(np.abs(actual_points[:, 0] - expected_points[:, 0])) > 0.05:
                errors.append(f"{metric_id}: channel {index} SVG time axis differs")
            vertical_delta = np.abs(actual_points[:, 1] - expected_points[:, 1])
            maximum_delta = float(np.max(vertical_delta))
            rms_delta = float(np.sqrt(np.mean(vertical_delta**2)))
            if (
                maximum_delta > SVG_VERTICAL_MAX_TOLERANCE
                or rms_delta > SVG_VERTICAL_RMS_TOLERANCE
            ):
                errors.append(
                    f"{metric_id}: channel {index} SVG shape drifted "
                    f"(max {maximum_delta:.3f}/{SVG_VERTICAL_MAX_TOLERANCE:.3f} px; "
                    f"RMS {rms_delta:.3f}/{SVG_VERTICAL_RMS_TOLERANCE:.3f} px)"
                )
            if abs(actual_points[0, 1] - actual_points[-1, 1]) > 0.11:
                errors.append(f"{metric_id}: channel {index} SVG loop is not closed")
            actual_values = np.asarray(actual_channel.get("values", []), dtype=float)
            expected_values = np.asarray(expected_channel["values"], dtype=float)
            if actual_values.shape != expected_values.shape:
                errors.append(f"{metric_id}: channel {index} numeric preview length differs")
            elif not np.all(np.isfinite(actual_values)):
                errors.append(f"{metric_id}: channel {index} numeric preview is not finite")
            elif not np.allclose(actual_values, expected_values, rtol=0.02, atol=0.001):
                errors.append(f"{metric_id}: channel {index} numeric recorded preview drifted")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail when the checked-in asset is stale")
    arguments = parser.parse_args()
    payload = generate()
    rendered = render_javascript(payload)
    if arguments.check:
        existing = OUTPUT.read_text(encoding="utf-8") if OUTPUT.exists() else ""
        try:
            existing_payload = parse_javascript_payload(existing)
        except (ValueError, json.JSONDecodeError) as error:
            print(f"{OUTPUT.relative_to(ROOT)} is invalid: {error}", file=sys.stderr)
            return 1
        errors = check_payload(existing_payload, payload)
        if errors:
            print(f"{OUTPUT.relative_to(ROOT)} is stale or invalid:", file=sys.stderr)
            for error in errors:
                print(f"- {error}", file=sys.stderr)
            return 1
        print(f"Validated {len(catalog_ids())} recorded Polar H10 metric previews with cross-platform numeric tolerances")
        return 0
    OUTPUT.write_text(rendered, encoding="utf-8")
    print(f"Generated {len(catalog_ids())} metric previews in {OUTPUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
