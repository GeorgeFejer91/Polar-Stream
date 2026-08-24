#!/usr/bin/env python3
"""Verify simultaneous Polar and Vernier streams through official pylsl inlets."""

from __future__ import annotations

import pathlib
import subprocess
import sys
import time


ROOT = pathlib.Path(__file__).resolve().parents[1]
LIBLSL = ROOT / "apps" / "polar-stream" / "resources" / "lsl.dll"
BASE = "polar_mixed_acceptance"


def descriptor(info):
    return (
        info.name(),
        info.type(),
        info.channel_count(),
        info.nominal_srate(),
        info.channel_format(),
        info.source_id(),
    )


def expected_descriptors(pylsl):
    polar = f"{BASE}_source-1"
    vernier = f"{BASE}_source-2"
    return {
        "ecg": (
            f"{polar}_rawECG",
            "ECG",
            1,
            130.0,
            pylsl.cf_float32,
            f"polar-h10-{polar}_rawECG",
        ),
        "acc": (
            f"{polar}_rawACC",
            "Accelerometer",
            3,
            200.0,
            pylsl.cf_float32,
            f"polar-h10-{polar}_rawACC",
        ),
        "vernier_raw": (
            f"{vernier}_rawVernier",
            "VernierRaw",
            9,
            0.0,
            pylsl.cf_double64,
            f"polar-stream-vernier-raw-{vernier}_rawVernier",
        ),
        "vernier_breathing": (
            f"{vernier}_vernierBreathing",
            "Respiration",
            1,
            0.0,
            pylsl.cf_float32,
            f"polar-stream-vernier-breathing-{vernier}_vernierBreathing",
        ),
    }


def resolve_exact(pylsl, timeout: float):
    expected = expected_descriptors(pylsl)
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
        "the four exact mixed-device outlets were not found: "
        f"{sorted(descriptor(info) for info in latest)!r}"
    )


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
            "verify_mixed_lsl",
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
                if line.startswith("POLAR_MIXED_LSL_READY "):
                    break
            elif process.poll() is not None:
                raise RuntimeError("mixed-device producer exited before becoming ready")
        else:
            raise RuntimeError("mixed-device producer did not become ready")

        streams = resolve_exact(pylsl, 15.0)
        rows = {role: [] for role in streams}
        timestamps = {role: [] for role in streams}
        for role, stream in streams.items():
            inlet = pylsl.StreamInlet(stream, max_buflen=10, recover=False)
            inlet.open_stream(timeout=10.0)
            inlets.append(inlet)

        minimum_rows = {
            "ecg": 260,
            "acc": 400,
            "vernier_raw": 25,
            "vernier_breathing": 25,
        }
        deadline = time.monotonic() + 15.0
        while time.monotonic() < deadline:
            for role, inlet in zip(streams, inlets):
                chunk, chunk_timestamps = inlet.pull_chunk(timeout=0.02, max_samples=512)
                rows[role].extend(chunk)
                timestamps[role].extend(chunk_timestamps)
            if all(len(rows[role]) >= count for role, count in minimum_rows.items()):
                break

        for role, count in minimum_rows.items():
            if len(rows[role]) < count:
                raise RuntimeError(f"{role} stopped early: {len(rows[role])} rows")
            if any(
                right <= left
                for left, right in zip(timestamps[role], timestamps[role][1:])
            ):
                raise RuntimeError(f"{role} timestamps did not increase")
        overlap = min(values[-1] for values in timestamps.values()) - max(
            values[0] for values in timestamps.values()
        )
        if overlap < 1.5:
            raise RuntimeError(f"mixed-device LSL clocks did not overlap: {overlap:.3f}s")
        if not all(0.0 <= row[0] <= 1.0 for row in rows["vernier_breathing"]):
            raise RuntimeError("Vernier breathing left its 0-1 contract")

        remaining, _ = process.communicate(timeout=15.0)
        output.append(remaining)
        if process.returncode != 0:
            raise RuntimeError("mixed-device producer exited unsuccessfully")
        counts = ", ".join(f"{role}={len(values)}" for role, values in rows.items())
        print(
            "Mixed Polar/Vernier LSL verification passed: "
            f"{counts}, overlapping LSL time={overlap:.3f}s"
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
