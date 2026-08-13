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

import neurokit2 as nk
import numpy as np


ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "apps/polar-stream/ui/demo-data.js"
DURATION_SECONDS = 30
ECG_RATE = 130
ACC_RATE = 200
METRIC_RATE = 20
SEED = 314_159


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


def validate(existing: dict, expected: dict) -> list[str]:
    errors: list[str] = []
    for key in ("schemaVersion", "source", "durationSeconds"):
        if existing.get(key) != expected.get(key):
            errors.append(f"top-level {key} differs")
    shapes = {
        "ecg.microvolts": DURATION_SECONDS * ECG_RATE,
        "accelerometer.milligravity": 3,
        "metrics.values": 7,
    }
    if len(existing.get("ecg", {}).get("microvolts", [])) != shapes["ecg.microvolts"]:
        errors.append("ECG sample count differs")
    axes = existing.get("accelerometer", {}).get("milligravity", [])
    if len(axes) != shapes["accelerometer.milligravity"] or any(len(axis) != DURATION_SECONDS * ACC_RATE for axis in axes):
        errors.append("accelerometer shape differs")
    metric_values = existing.get("metrics", {}).get("values", {})
    if list(metric_values) != list(expected["metrics"]["values"]):
        errors.append("metric IDs/order differ")
    elif any(len(values) != DURATION_SECONDS * METRIC_RATE for values in metric_values.values()):
        errors.append("metric sample count differs")

    for path, actual, wanted, tolerance in (
        ("ecg", existing.get("ecg", {}).get("microvolts", []), expected["ecg"]["microvolts"], 3.0),
        ("acc-x", axes[0] if len(axes) == 3 else [], expected["accelerometer"]["milligravity"][0], 2.0),
        ("breathing", metric_values.get("acc_breathing_magnitude", []), expected["metrics"]["values"]["acc_breathing_magnitude"], 0.002),
    ):
        if len(actual) != len(wanted):
            continue
        delta = np.abs(np.asarray(actual, dtype=float) - np.asarray(wanted, dtype=float))
        if not np.isfinite(delta).all() or float(np.max(delta)) > tolerance:
            errors.append(f"{path} fixture drifted beyond tolerance")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail when the checked-in fixture is stale")
    arguments = parser.parse_args()
    payload = generate()
    rendered = render(payload)
    if arguments.check:
        try:
            existing = parse(OUTPUT.read_text(encoding="utf-8"))
        except (FileNotFoundError, ValueError, json.JSONDecodeError) as error:
            print(f"{OUTPUT.relative_to(ROOT)} is invalid: {error}", file=sys.stderr)
            return 1
        errors = validate(existing, payload)
        if errors:
            print(f"{OUTPUT.relative_to(ROOT)} is stale or invalid:", file=sys.stderr)
            for error in errors:
                print(f"- {error}", file=sys.stderr)
            return 1
        print(f"Validated {DURATION_SECONDS}s NeuroKit offline demo fixture")
        return 0
    OUTPUT.write_text(rendered, encoding="utf-8")
    print(f"Generated NeuroKit offline demo fixture in {OUTPUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
