#!/usr/bin/env python3
"""Tests for strict browser-recording acceptance analysis."""

from __future__ import annotations

import csv
import io
from pathlib import Path
import tempfile
import unittest

from analyze_browser_recording import AcceptanceLimits, CSV_COLUMNS, analyze_recording


class BrowserRecordingAnalysisTests(unittest.TestCase):
    limits = AcceptanceLimits(minimum_seconds=2.0, maximum_gap_seconds=1.0, maximum_loss_percent=0.1)

    def write_recording(
        self,
        directory: Path,
        *,
        duration: float = 2.1,
        input_kind: str = "web-bluetooth",
        stop_reason: str = "user",
        zero_timestamps: bool = False,
        omit_ecg_index: int | None = None,
        invalid_acc: bool = False,
    ) -> Path:
        output = io.StringIO(newline="")
        output.write("# Polar Stream browser recording\n")
        output.write("# schema_version,2\n")
        output.write("# source,Polar H10 12345678\n")
        output.write(f"# input_kind,{input_kind}\n")
        output.write(f"# stop_reason,{stop_reason}\n")
        writer = csv.DictWriter(output, fieldnames=CSV_COLUMNS, lineterminator="\n")
        writer.writeheader()
        base_sensor = 40_000_000_000
        base_host = 1_800_000_000_000.0
        for stream, rate in (("raw_ecg", 130), ("raw_acc", 200)):
            count = int(duration * rate) + 1
            for index in range(count):
                if stream == "raw_ecg" and index == omit_ecg_index:
                    continue
                sensor = 0 if zero_timestamps else base_sensor + round(index * 1e9 / rate)
                row = {name: "" for name in CSV_COLUMNS}
                row.update(
                    host_timestamp_ms=f"{base_host + index * 1000 / rate:.3f}",
                    relative_time_s=f"{index / rate:.6f}",
                    sensor_timestamp_ns=str(sensor),
                    stream=stream,
                    sample_index=str(index % 10),
                )
                if stream == "raw_ecg":
                    row.update(value=str(100 + index % 7), unit="uV")
                else:
                    row.update(x_mg="nan" if invalid_acc and index == 1 else "1", y_mg="2", z_mg="999", unit="mg")
                writer.writerow(row)
        for stream, value, unit in (("heart_rate", "72", "bpm"), ("rr_interval", "833", "ms")):
            row = {name: "" for name in CSV_COLUMNS}
            row.update(
                host_timestamp_ms=f"{base_host:.3f}",
                relative_time_s="0.0",
                stream=stream,
                sample_index="0",
                value=value,
                unit=unit,
            )
            writer.writerow(row)
        path = directory / "recording.csv"
        path.write_text(output.getvalue(), encoding="utf-8")
        return path

    def analyze(self, **changes: object) -> dict[str, object]:
        with tempfile.TemporaryDirectory() as temporary:
            path = self.write_recording(Path(temporary), **changes)
            return analyze_recording(path, self.limits)

    def test_accepts_complete_physical_recording(self) -> None:
        report = self.analyze()
        self.assertTrue(report["csvAcceptancePassed"], report["errors"])

    def test_default_limits_accept_full_two_minute_fixture(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = self.write_recording(Path(temporary), duration=118.1)
            report = analyze_recording(path)
        self.assertTrue(report["csvAcceptancePassed"], report["errors"])

    def test_rejects_mock_provenance_and_zero_sensor_time(self) -> None:
        report = self.analyze(input_kind="mock", zero_timestamps=True)
        self.assertFalse(report["csvAcceptancePassed"])
        self.assertTrue(any("input_kind" in error for error in report["errors"]))
        self.assertTrue(any("not positive" in error for error in report["errors"]))

    def test_rejects_sample_loss(self) -> None:
        report = self.analyze(omit_ecg_index=100)
        self.assertFalse(report["csvAcceptancePassed"])
        self.assertTrue(any("estimated loss" in error for error in report["errors"]))

    def test_rejects_non_finite_acc_axis(self) -> None:
        report = self.analyze(invalid_acc=True)
        self.assertFalse(report["csvAcceptancePassed"])
        self.assertTrue(any("three finite axes" in error for error in report["errors"]))

    def test_rejects_capacity_stop(self) -> None:
        report = self.analyze(stop_reason="capacity")
        self.assertFalse(report["csvAcceptancePassed"])
        self.assertTrue(any("stop reason" in error for error in report["errors"]))


if __name__ == "__main__":
    unittest.main()
