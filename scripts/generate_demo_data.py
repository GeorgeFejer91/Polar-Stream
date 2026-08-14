#!/usr/bin/env python3
"""Generate the deterministic offline NeuroKit input fixture.

The generated JavaScript is replayed by the shared frontend runtime adapter in
both Tauri and GitHub Pages. Python, NumPy, and NeuroKit remain development-only
dependencies and are not required by either runtime.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
import warnings
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "apps/polar-stream/ui/demo-data.js"
DURATION_SECONDS = 30
ECG_RATE = 130
ACC_RATE = 200
METRIC_RATE = 20
SEED = 314_159
METRIC_IDS = (
    "heart_rate",
    "rr_interval",
    "rmssd",
    "ln_rmssd",
    "acc_breathing_magnitude",
    "breathing_phase",
    "breathing_rate",
)


def load_generation_dependencies() -> None:
    global nk, np
    try:
        import neurokit2 as nk_module
        import numpy as np_module
    except ImportError as error:
        raise SystemExit(
            "Generating demo-data.js requires the development dependencies in requirements-previews.txt"
        ) from error
    nk = nk_module
    np = np_module


def finite(values: np.ndarray, fallback: float = 0.0) -> np.ndarray:
    result = np.asarray(values, dtype=float).reshape(-1)
    good = np.isfinite(result)
    if good.all():
        return result
    if not good.any():
        return np.full(result.shape, fallback, dtype=float)
    indices = np.arange(len(result))
    return np.interp(indices, indices[good], result[good])


def at_rate(values: np.ndarray, source_rate: int, target_rate: int) -> np.ndarray:
    values = finite(values)
    target_count = DURATION_SECONDS * target_rate
    source_time = np.arange(len(values)) / source_rate
    target_time = np.arange(target_count) / target_rate
    return np.interp(target_time, source_time, values)


def rolling_rmssd(peaks: np.ndarray) -> np.ndarray:
    intervals = np.diff(peaks) / ECG_RATE * 1_000
    interval_times = peaks[1:] / ECG_RATE
    target_times = np.arange(DURATION_SECONDS * METRIC_RATE) / METRIC_RATE
    result: list[float] = []
    for second in target_times:
        mask = np.abs(interval_times - second) <= 12
        window = intervals[mask]
        if len(window) < 4:
            nearest = np.argsort(np.abs(interval_times - second))[: min(8, len(intervals))]
            window = intervals[np.sort(nearest)]
        differences = np.diff(window)
        value = math.sqrt(float(np.mean(np.square(differences)))) if len(differences) else 0.0
        result.append(value)
    return finite(np.asarray(result))


def rounded(values: np.ndarray, digits: int) -> list[float | int]:
    if digits == 0:
        return np.rint(finite(values)).astype(int).tolist()
    return np.round(finite(values), digits).tolist()


def generate() -> dict:
    load_generation_dependencies()
    warnings.filterwarnings("ignore", category=RuntimeWarning)
    ecg = np.asarray(nk.ecg_simulate(
        duration=DURATION_SECONDS,
        sampling_rate=ECG_RATE,
        heart_rate=69,
        heart_rate_std=5,
        noise=0.012,
        method="ecgsyn",
        random_state=SEED,
    ))
    ecg_signals, ecg_info = nk.ecg_process(ecg, sampling_rate=ECG_RATE)
    peaks = np.asarray(ecg_info["ECG_R_Peaks"], dtype=int)

    respiration = np.asarray(nk.rsp_simulate(
        duration=DURATION_SECONDS,
        sampling_rate=ACC_RATE,
        respiratory_rate=11,
        noise=0.008,
        random_state=SEED + 1,
    ))
    rsp_signals, _ = nk.rsp_process(respiration, sampling_rate=ACC_RATE)
    rsp_clean = finite(rsp_signals["RSP_Clean"].to_numpy())
    rsp_clean = (rsp_clean - np.mean(rsp_clean)) / max(np.std(rsp_clean), 1e-9)
    time = np.arange(len(rsp_clean)) / ACC_RATE

    # These axes emulate a quiet chest-mounted sensor with gravity on Z. They
    # are illustrative motion channels derived from NeuroKit respiration, not a
    # model validated against a Polar H10 or reference respiratory belt.
    acc_x = 38 * rsp_clean + 5 * np.sin(2 * np.pi * time / 1.9)
    acc_y = -21 * rsp_clean + 3 * np.sin(2 * np.pi * time / 2.7 + 0.6)
    acc_z = 995 + 17 * rsp_clean + 2 * np.sin(2 * np.pi * time / 1.35)
    projection = at_rate(rsp_clean * 0.031, ACC_RATE, METRIC_RATE)
    derivative = np.gradient(projection)
    threshold = float(np.quantile(np.abs(derivative), 0.25))
    phase = np.where(derivative > threshold, 1, np.where(derivative < -threshold, -1, 0))

    heart_rate = at_rate(
        finite(ecg_signals["ECG_Rate"].to_numpy(), fallback=69),
        ECG_RATE,
        METRIC_RATE,
    )
    rsp_rate = at_rate(
        finite(rsp_signals["RSP_Rate"].to_numpy(), fallback=11),
        ACC_RATE,
        METRIC_RATE,
    )
    rmssd = rolling_rmssd(peaks)

    return {
        "schemaVersion": 1,
        "source": {
            "library": "NeuroKit2",
            "version": nk.__version__,
            "models": ["ECGSYN", "RSP"],
            "seed": SEED,
            "note": "Synthetic offline interface fixture; not recorded data, device validation, or algorithm validation.",
        },
        "durationSeconds": DURATION_SECONDS,
        "ecg": {
            "samplingRateHz": ECG_RATE,
            "microvolts": rounded(ecg * 1_000, 0),
        },
        "accelerometer": {
            "samplingRateHz": ACC_RATE,
            "milligravity": [
                rounded(acc_x, 0),
                rounded(acc_y, 0),
                rounded(acc_z, 0),
            ],
        },
        "metrics": {
            "samplingRateHz": METRIC_RATE,
            "values": {
                "heart_rate": rounded(heart_rate, 3),
                "rr_interval": rounded(60_000 / np.maximum(heart_rate, 1), 3),
                "rmssd": rounded(rmssd, 3),
                "ln_rmssd": rounded(np.log(np.maximum(rmssd, 1e-9)), 4),
                "acc_breathing_magnitude": rounded(projection, 5),
                "breathing_phase": rounded(phase, 0),
                "breathing_rate": rounded(rsp_rate, 3),
            },
        },
    }


def render(payload: dict) -> str:
    serialized = json.dumps(payload, ensure_ascii=False, separators=(",", ":"))
    return (
        "// Generated by scripts/generate_demo_data.py; do not edit by hand.\n"
        f"window.PolarDemoData = Object.freeze({serialized});\n"
    )


def parse(rendered: str) -> dict:
    prefix = "window.PolarDemoData = Object.freeze("
    start = rendered.find(prefix)
    if start < 0 or not rendered.rstrip().endswith(");"):
        raise ValueError("generated demo asset does not contain the expected assignment")
    return json.loads(rendered[start + len(prefix): rendered.rfind(");")])


def validate_stored_fixture(existing: dict) -> list[str]:
    errors: list[str] = []

    def validate_series(name: str, values: object, count: int) -> list[float]:
        if not isinstance(values, list) or len(values) != count:
            errors.append(f"{name} sample count differs")
            return []
        if any(isinstance(value, bool) or not isinstance(value, (int, float)) for value in values):
            errors.append(f"{name} contains a non-numeric value")
            return []
        numbers = [float(value) for value in values]
        if not all(math.isfinite(value) for value in numbers):
            errors.append(f"{name} contains a non-finite value")
            return []
        return numbers

    source_value = existing.get("source", {})
    source = source_value if isinstance(source_value, dict) else {}
    if existing.get("schemaVersion") != 1:
        errors.append("schemaVersion is not 1")
    if existing.get("durationSeconds") != DURATION_SECONDS:
        errors.append(f"durationSeconds is not {DURATION_SECONDS}")
    if not isinstance(source_value, dict) or source.get("library") != "NeuroKit2":
        errors.append("source does not identify NeuroKit2")
    if source.get("models") != ["ECGSYN", "RSP"] or source.get("seed") != SEED:
        errors.append("source models or seed differ")
    if "Synthetic offline interface fixture" not in str(source.get("note", "")):
        errors.append("source note does not identify a synthetic fixture")

    ecg = existing.get("ecg", {})
    if not isinstance(ecg, dict) or ecg.get("samplingRateHz") != ECG_RATE:
        errors.append(f"ECG sampling rate is not {ECG_RATE} Hz")
    ecg_values = validate_series(
        "ECG", ecg.get("microvolts", []) if isinstance(ecg, dict) else [], DURATION_SECONDS * ECG_RATE
    )
    if ecg_values and (max(abs(value) for value in ecg_values) > 10_000 or max(ecg_values) == min(ecg_values)):
        errors.append("ECG values are outside fixture bounds or constant")

    accelerometer = existing.get("accelerometer", {})
    if not isinstance(accelerometer, dict) or accelerometer.get("samplingRateHz") != ACC_RATE:
        errors.append(f"accelerometer sampling rate is not {ACC_RATE} Hz")
    axes = accelerometer.get("milligravity", []) if isinstance(accelerometer, dict) else []
    if not isinstance(axes, list) or len(axes) != 3:
        errors.append("accelerometer does not contain three axes")
    else:
        for label, axis in zip(("X", "Y", "Z"), axes, strict=True):
            values = validate_series(f"accelerometer {label}", axis, DURATION_SECONDS * ACC_RATE)
            if values and (max(abs(value) for value in values) > 4_000 or max(values) == min(values)):
                errors.append(f"accelerometer {label} values are outside fixture bounds or constant")

    metrics = existing.get("metrics", {})
    if not isinstance(metrics, dict) or metrics.get("samplingRateHz") != METRIC_RATE:
        errors.append(f"metric sampling rate is not {METRIC_RATE} Hz")
    metric_values = metrics.get("values", {}) if isinstance(metrics, dict) else {}
    if not isinstance(metric_values, dict) or tuple(metric_values) != METRIC_IDS:
        errors.append("metric IDs/order differ")
    else:
        checked_metrics = {
            name: validate_series(name, metric_values[name], DURATION_SECONDS * METRIC_RATE)
            for name in METRIC_IDS
        }
        if checked_metrics["heart_rate"] and not all(20 <= value <= 240 for value in checked_metrics["heart_rate"]):
            errors.append("heart_rate leaves plausible fixture bounds")
        if checked_metrics["rr_interval"] and not all(250 <= value <= 3_000 for value in checked_metrics["rr_interval"]):
            errors.append("rr_interval leaves plausible fixture bounds")
        if checked_metrics["breathing_phase"] and not set(checked_metrics["breathing_phase"]) <= {-1.0, 0.0, 1.0}:
            errors.append("breathing_phase contains a value outside -1, 0, and 1")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="validate the checked-in fixture without regenerating it")
    arguments = parser.parse_args()
    if arguments.check:
        try:
            existing = parse(OUTPUT.read_text(encoding="utf-8"))
        except (FileNotFoundError, ValueError, json.JSONDecodeError) as error:
            print(f"{OUTPUT.relative_to(ROOT)} is invalid: {error}", file=sys.stderr)
            return 1
        errors = validate_stored_fixture(existing)
        if errors:
            print(f"{OUTPUT.relative_to(ROOT)} is stale or invalid:", file=sys.stderr)
            for error in errors:
                print(f"- {error}", file=sys.stderr)
            return 1
        print(f"Validated {DURATION_SECONDS}s NeuroKit offline demo fixture")
        return 0
    payload = generate()
    rendered = render(payload)
    OUTPUT.write_text(rendered, encoding="utf-8")
    print(f"Generated NeuroKit offline demo fixture in {OUTPUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
