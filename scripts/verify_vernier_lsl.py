#!/usr/bin/env python3
"""Verify Vernier raw/derived outlets through pinned official pylsl inlets."""

from __future__ import annotations

import pathlib
import subprocess
import sys
import time


ROOT = pathlib.Path(__file__).resolve().parents[1]
LIBLSL = ROOT / "apps" / "polar-stream" / "resources" / "lsl.dll"
BASE = "polar_vernier_acceptance"


def descriptor(info):
    return (
        info.name(),
        info.type(),
        info.channel_count(),
        info.nominal_srate(),
        info.channel_format(),
        info.source_id(),
    )


def resolve_exact(pylsl, timeout: float):
    expected = {
        "raw": (
            f"{BASE}_rawVernier",
            "VernierRaw",
            11,
            0.0,
            pylsl.cf_double64,
            f"polar-stream-vernier-raw-{BASE}_rawVernier",
        ),
        "breathing": (
            f"{BASE}_vernierBreathing",
            "Respiration",
            1,
            0.0,
            pylsl.cf_float32,
            f"polar-stream-vernier-breathing-{BASE}_vernierBreathing",
        ),
    }
    deadline = time.monotonic() + timeout
    latest = []
    while time.monotonic() < deadline:
        latest = pylsl.resolve_streams(wait_time=min(1.0, deadline - time.monotonic()))
        resolved = {}
        for role, exact in expected.items():
            candidates = {
                info.uid(): info
                for info in latest
                if info.name() == exact[0] or info.source_id() == exact[5]
            }
            matches = [info for info in candidates.values() if descriptor(info) == exact]
            if len(candidates) > 1 or (candidates and len(matches) != 1):
                raise RuntimeError(
                    f"{role} outlet was ambiguous or mismatched: "
                    f"{sorted(descriptor(info) for info in candidates.values())!r}"
                )
            if len(matches) == 1:
                resolved[role] = matches[0]
        if resolved.keys() == expected.keys():
            return resolved
    raise RuntimeError(
        "exact Vernier outlets were not found: "
        f"{sorted(descriptor(info) for info in latest)!r}"
    )


def channel_metadata(info):
    channels = info.desc().child("channels").child("channel")
    result = []
    while not channels.empty():
        result.append(
            (
                channels.child_value("label"),
                channels.child_value("unit"),
                channels.child_value("type"),
            )
        )
        channels = channels.next_sibling("channel")
    return result


def main() -> int:
    try:
        import pylsl
    except ImportError as error:
        raise SystemExit(
            "pylsl is required; use pinned pylsl 1.18.2/liblsl 1.17.7"
        ) from error
    if pylsl.__version__ != "1.18.2" or pylsl.library_version() != 117:
        raise SystemExit(
            f"expected pylsl 1.18.2/liblsl 117, found "
            f"{pylsl.__version__}/{pylsl.library_version()}"
        )

    process = subprocess.Popen(
        [
            "cargo",
            "run",
            "-p",
            "polar-h10-output",
            "--example",
            "verify_vernier_lsl",
            "--quiet",
            "--",
            str(LIBLSL),
        ],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    output = []
    inlets = []
    try:
        assert process.stdout is not None
        deadline = time.monotonic() + 120.0
        while time.monotonic() < deadline:
            line = process.stdout.readline()
            if line:
                output.append(line)
                if line.startswith("POLAR_VERNIER_LSL_READY "):
                    break
            elif process.poll() is not None:
                raise RuntimeError("Vernier producer exited before becoming ready")
        else:
            raise RuntimeError("Vernier producer did not become ready")

        streams = resolve_exact(pylsl, 15.0)
        raw = pylsl.StreamInlet(streams["raw"], max_buflen=10, recover=False)
        breathing = pylsl.StreamInlet(
            streams["breathing"], max_buflen=10, recover=False
        )
        inlets.extend([raw, breathing])
        raw.open_stream(timeout=10.0)
        breathing.open_stream(timeout=10.0)
        raw_info = raw.info(timeout=10.0)
        breathing_info = breathing.info(timeout=10.0)

        raw_rows = []
        raw_timestamps = []
        breathing_rows = []
        breathing_timestamps = []
        deadline = time.monotonic() + 15.0
        while time.monotonic() < deadline:
            rows, timestamps = raw.pull_chunk(timeout=0.05, max_samples=256)
            raw_rows.extend(rows)
            raw_timestamps.extend(timestamps)
            rows, timestamps = breathing.pull_chunk(timeout=0.05, max_samples=256)
            breathing_rows.extend(rows)
            breathing_timestamps.extend(timestamps)
            integer_seen = any(
                row[2] == 2_000_000_001.0 and row[10] == 1.0 for row in raw_rows
            )
            if len(raw_rows) >= 20 and len(breathing_rows) >= 20 and integer_seen:
                break

        expected_raw_metadata = [
            ("Force", "N", "RawMeasurement"),
            ("Respiration Rate", "breaths/min", "RawMeasurement"),
            ("Steps", "count", "RawMeasurement"),
            ("Step Rate", "steps/min", "RawMeasurement"),
        ]
        observed_raw_metadata = channel_metadata(raw_info)
        if observed_raw_metadata[:4] != expected_raw_metadata:
            raise RuntimeError(f"unexpected raw metadata: {observed_raw_metadata!r}")
        if channel_metadata(breathing_info) != [
            ("Vernier breathing waveform", "0-1", "DerivedRespiration")
        ]:
            raise RuntimeError("unexpected derived breathing metadata")
        if raw_info.desc().child_value("stream_role") != "raw_measurement_recording":
            raise RuntimeError("raw outlet is not explicitly identified as raw")
        if (
            breathing_info.desc().child_value("stream_role")
            != "derived_breathing_waveform"
        ):
            raise RuntimeError("breathing outlet is not explicitly identified as derived")
        if not any(row[0] == 12.25 and row[1] == 18.0 for row in raw_rows):
            raise RuntimeError("exact initial Float32-derived device values were not received")
        if not any(
            row[2] == 2_000_000_001.0
            and row[0] != row[0]
            and row[10] == 1.0
            for row in raw_rows
        ):
            raise RuntimeError("exact sparse Int32 device value was not received")
        if not breathing_rows or not all(0.0 <= row[0] <= 1.0 for row in breathing_rows):
            raise RuntimeError("derived breathing values were not continuously bounded")
        if any(right <= left for left, right in zip(raw_timestamps, raw_timestamps[1:])):
            raise RuntimeError("raw timestamps did not increase")
        if any(
            right <= left
            for left, right in zip(breathing_timestamps, breathing_timestamps[1:])
        ):
            raise RuntimeError("breathing timestamps did not increase")

        remaining, _ = process.communicate(timeout=15.0)
        output.append(remaining)
        if process.returncode != 0:
            raise RuntimeError("Vernier producer exited unsuccessfully")
        print(
            "Vernier LSL consumer verification passed: "
            f"{len(raw_rows)} raw rows, {len(breathing_rows)} derived rows"
        )
        return 0
    except Exception:
        process.terminate()
        try:
            remaining, _ = process.communicate(timeout=5.0)
            output.append(remaining)
        except subprocess.TimeoutExpired:
            process.kill()
            remaining, _ = process.communicate()
            output.append(remaining)
        print("".join(output), file=sys.stderr)
        raise
    finally:
        for inlet in inlets:
            inlet.close_stream()


if __name__ == "__main__":
    raise SystemExit(main())
