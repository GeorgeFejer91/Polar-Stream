#!/usr/bin/env python3
"""Evaluate a Polar Stream browser CSV against the physical-H10 acceptance limits."""

from __future__ import annotations

import argparse
import csv
from dataclasses import asdict, dataclass
import itertools
import json
import math
from pathlib import Path
import sys
from typing import TextIO


CSV_COLUMNS = [
    "host_timestamp_ms",
    "relative_time_s",
    "sensor_timestamp_ns",
    "stream",
    "sample_index",
    "x_mg",
    "y_mg",
    "z_mg",
    "value",
    "unit",
]
RAW_STREAMS = {"raw_ecg": 130.0, "raw_acc": 200.0}
RATE_LIMITS = {"raw_ecg": (129.0, 131.0), "raw_acc": (199.0, 201.0)}


@dataclass(frozen=True)
class AcceptanceLimits:
    minimum_seconds: float = 118.0
    maximum_gap_seconds: float = 1.0
    maximum_loss_percent: float = 0.1


@dataclass
class RawStats:
    nominal_rate_hz: float
    count: int = 0
    first_sensor_ns: int | None = None
    last_sensor_ns: int | None = None
    previous_sensor_ns: int | None = None
    previous_host_ms: float | None = None
    maximum_sensor_gap_seconds: float = 0.0
    maximum_host_gap_seconds: float = 0.0
    estimated_missing_samples: int = 0

    def append(self, sensor_ns: int, host_ms: float) -> bool:
        strictly_increasing = self.previous_sensor_ns is None or sensor_ns > self.previous_sensor_ns
        if self.previous_sensor_ns is not None and sensor_ns > self.previous_sensor_ns:
            delta_ns = sensor_ns - self.previous_sensor_ns
            self.maximum_sensor_gap_seconds = max(self.maximum_sensor_gap_seconds, delta_ns / 1e9)
            period_ns = 1e9 / self.nominal_rate_hz
            elapsed_periods = max(1, int(delta_ns / period_ns + 0.5))
            self.estimated_missing_samples += max(0, elapsed_periods - 1)
        if self.previous_host_ms is not None and host_ms > self.previous_host_ms:
            self.maximum_host_gap_seconds = max(
                self.maximum_host_gap_seconds, (host_ms - self.previous_host_ms) / 1000
            )
        self.first_sensor_ns = sensor_ns if self.first_sensor_ns is None else self.first_sensor_ns
        self.last_sensor_ns = sensor_ns
        self.previous_sensor_ns = sensor_ns
        self.previous_host_ms = host_ms
        self.count += 1
        return strictly_increasing

    def metrics(self) -> dict[str, float | int]:
        span_ns = 0 if self.first_sensor_ns is None or self.last_sensor_ns is None else self.last_sensor_ns - self.first_sensor_ns
        coverage_seconds = span_ns / 1e9
        effective_rate_hz = (self.count - 1) / coverage_seconds if self.count > 1 and coverage_seconds > 0 else 0.0
        possible_samples = self.count + self.estimated_missing_samples
        loss_percent = 100 * self.estimated_missing_samples / possible_samples if possible_samples else 100.0
        return {
            "sampleCount": self.count,
            "sensorCoverageSeconds": coverage_seconds,
            "effectiveRateHz": effective_rate_hz,
            "estimatedMissingSamples": self.estimated_missing_samples,
            "estimatedLossPercent": loss_percent,
            "maximumSensorGapSeconds": self.maximum_sensor_gap_seconds,
            "maximumHostGapSeconds": self.maximum_host_gap_seconds,
            "firstSensorTimestampNs": self.first_sensor_ns or 0,
            "lastSensorTimestampNs": self.last_sensor_ns or 0,
        }


def finite_number(value: str) -> float | None:
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    return number if math.isfinite(number) else None


def parse_metadata_line(line: str) -> tuple[str, str] | None:
    fields = next(csv.reader([line.removeprefix("#").lstrip()]))
    if len(fields) != 2:
        return None
    return fields[0].strip(), fields[1].strip()


def open_rows(handle: TextIO) -> tuple[dict[str, str], csv.DictReader[str], int]:
    metadata: dict[str, str] = {}
    line_number = 0
    for line in handle:
        line_number += 1
        if line.startswith("#"):
            item = parse_metadata_line(line)
            if item:
                metadata[item[0]] = item[1]
            continue
        reader = csv.DictReader(itertools.chain([line], handle))
        if reader.fieldnames != CSV_COLUMNS:
            raise ValueError(f"CSV columns do not match schema 2: {reader.fieldnames}")
        return metadata, reader, line_number
    raise ValueError("CSV has no column header")


def analyze_recording(path: Path, limits: AcceptanceLimits = AcceptanceLimits()) -> dict[str, object]:
    errors: list[str] = []
    seen_errors: set[str] = set()

    def reject(message: str) -> None:
        if message not in seen_errors and len(errors) < 100:
            errors.append(message)
            seen_errors.add(message)

    raw = {name: RawStats(rate) for name, rate in RAW_STREAMS.items()}
    metric_counts = {"heart_rate": 0, "rr_interval": 0}
    metric_ranges: dict[str, list[float | None]] = {
        "heart_rate": [None, None],
        "rr_interval": [None, None],
    }
    total_rows = 0

    with path.open("r", encoding="utf-8-sig", newline="") as handle:
        metadata, reader, header_line = open_rows(handle)
        required_metadata = {
            "schema_version": "2",
            "input_kind": "web-bluetooth",
        }
        for name, expected in required_metadata.items():
            if metadata.get(name) != expected:
                reject(f"metadata {name!r} must be {expected!r}, found {metadata.get(name)!r}")
        if "polar h10" not in metadata.get("source", "").lower():
            reject("metadata source does not identify a Polar H10")
        if metadata.get("stop_reason") not in {"user", "export"}:
            reject(f"recorder stop reason is not a normal export: {metadata.get('stop_reason')!r}")

        for row_number, row in enumerate(reader, start=header_line + 1):
            total_rows += 1
            if None in row or any(value is None for value in row.values()):
                reject(f"row {row_number}: malformed column count")
                continue
            stream = row["stream"]
            host_ms = finite_number(row["host_timestamp_ms"])
            relative_s = finite_number(row["relative_time_s"])
            if host_ms is None or relative_s is None:
                reject(f"row {row_number}: host or relative time is not finite")
                continue

            if stream in RAW_STREAMS:
                try:
                    sensor_ns = int(row["sensor_timestamp_ns"])
                except ValueError:
                    reject(f"row {row_number}: {stream} sensor timestamp is missing or invalid")
                    continue
                if sensor_ns <= 0:
                    reject(f"row {row_number}: {stream} sensor timestamp is not positive")
                if not raw[stream].append(sensor_ns, host_ms):
                    reject(f"row {row_number}: {stream} sensor timestamps are not strictly increasing")
                if stream == "raw_ecg":
                    if finite_number(row["value"]) is None:
                        reject(f"row {row_number}: raw ECG value is not finite")
                    if row["unit"] != "uV":
                        reject(f"row {row_number}: raw ECG unit is not uV")
                else:
                    if any(finite_number(row[axis]) is None for axis in ("x_mg", "y_mg", "z_mg")):
                        reject(f"row {row_number}: raw ACC does not contain three finite axes")
                    if row["unit"] != "mg":
                        reject(f"row {row_number}: raw ACC unit is not mg")
                continue

            if stream in metric_counts:
                value = finite_number(row["value"])
                if value is None:
                    reject(f"row {row_number}: {stream} is not finite")
                    continue
                plausible = 20 <= value <= 240 if stream == "heart_rate" else 250 <= value <= 3000
                if not plausible:
                    reject(f"row {row_number}: {stream} value {value:g} is implausible")
                metric_counts[stream] += 1
                current_min, current_max = metric_ranges[stream]
                metric_ranges[stream] = [
                    value if current_min is None else min(current_min, value),
                    value if current_max is None else max(current_max, value),
                ]

    stream_metrics: dict[str, dict[str, float | int]] = {}
    for stream, stats in raw.items():
        metrics = stats.metrics()
        stream_metrics[stream] = metrics
        if metrics["sampleCount"] < 2:
            reject(f"{stream} is absent or has fewer than two samples")
            continue
        if metrics["sensorCoverageSeconds"] < limits.minimum_seconds:
            reject(
                f"{stream} sensor coverage {metrics['sensorCoverageSeconds']:.3f}s "
                f"is below {limits.minimum_seconds:.3f}s"
            )
        low_rate, high_rate = RATE_LIMITS[stream]
        if not low_rate <= metrics["effectiveRateHz"] <= high_rate:
            reject(
                f"{stream} effective rate {metrics['effectiveRateHz']:.3f}Hz "
                f"is outside {low_rate:.0f}-{high_rate:.0f}Hz"
            )
        if metrics["estimatedLossPercent"] >= limits.maximum_loss_percent:
            reject(
                f"{stream} estimated loss {metrics['estimatedLossPercent']:.4f}% "
                f"is not below {limits.maximum_loss_percent:.4f}%"
            )
        if metrics["maximumSensorGapSeconds"] > limits.maximum_gap_seconds:
            reject(
                f"{stream} maximum sensor gap {metrics['maximumSensorGapSeconds']:.6f}s "
                f"exceeds {limits.maximum_gap_seconds:.3f}s"
            )
        if metrics["maximumHostGapSeconds"] > limits.maximum_gap_seconds:
            reject(
                f"{stream} maximum host gap {metrics['maximumHostGapSeconds']:.6f}s "
                f"exceeds {limits.maximum_gap_seconds:.3f}s"
            )

    for metric, count in metric_counts.items():
        if count == 0:
            reject(f"CSV contains no finite, plausible {metric} values")

    return {
        "schemaVersion": 1,
        "scope": "offline-csv-only",
        "csvAcceptancePassed": not errors,
        "recording": path.name,
        "limits": asdict(limits),
        "metadata": metadata,
        "totalRows": total_rows,
        "streams": stream_metrics,
        "metrics": {
            name: {"count": metric_counts[name], "minimum": values[0], "maximum": values[1]}
            for name, values in metric_ranges.items()
        },
        "errors": errors,
        "manualEvidenceStillRequired": [
            "ordinary Brave tab and public GitHub Pages URL visible",
            "physical H10 selected in Android chooser",
            "no malformed-frame error or unexpected disconnect",
            "reconnect produces data within five seconds without duplicate subscriptions",
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("recording", type=Path)
    parser.add_argument("--minimum-seconds", type=float, default=118.0)
    parser.add_argument("--maximum-gap-seconds", type=float, default=1.0)
    parser.add_argument("--maximum-loss-percent", type=float, default=0.1)
    parser.add_argument("--json-output", type=Path)
    arguments = parser.parse_args()
    limits = AcceptanceLimits(
        minimum_seconds=arguments.minimum_seconds,
        maximum_gap_seconds=arguments.maximum_gap_seconds,
        maximum_loss_percent=arguments.maximum_loss_percent,
    )
    try:
        report = analyze_recording(arguments.recording, limits)
    except Exception as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 2

    if arguments.json_output:
        arguments.json_output.parent.mkdir(parents=True, exist_ok=True)
        arguments.json_output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    for name, metrics in report["streams"].items():
        print(
            f"{name}: {metrics['sampleCount']} samples, "
            f"{metrics['sensorCoverageSeconds']:.3f}s, {metrics['effectiveRateHz']:.3f}Hz, "
            f"loss {metrics['estimatedLossPercent']:.4f}%, "
            f"max gap {metrics['maximumSensorGapSeconds']:.6f}s"
        )
    if report["csvAcceptancePassed"]:
        print("PASS: offline CSV criteria passed; screen and reconnect evidence are still required")
        return 0
    print("FAIL: offline CSV criteria did not pass", file=sys.stderr)
    for error in report["errors"]:
        print(f"- {error}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
