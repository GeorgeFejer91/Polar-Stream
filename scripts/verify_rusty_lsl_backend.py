#!/usr/bin/env python3
"""Verify the opt-in Rusty backend with pinned official liblsl inlets."""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import time
from dataclasses import dataclass


ROOT = pathlib.Path(__file__).resolve().parents[1]
RUSTY_LSL_REVISION = "74f7d0ea2cce9b3d049ea24602527a5f52360554"


@dataclass(frozen=True)
class ExpectedStream:
    name: str
    stream_type: str
    channel_count: int
    nominal_rate: float
    source_id: str


EXPECTED = {
    "ecg": ExpectedStream(
        name="polar_rusty_backend_rawECG",
        stream_type="ECG",
        channel_count=1,
        nominal_rate=130.0,
        source_id="polar-h10-polar_rusty_backend_rawECG",
    ),
    "acc": ExpectedStream(
        name="polar_rusty_backend_rawACC",
        stream_type="Accelerometer",
        channel_count=3,
        nominal_rate=200.0,
        source_id="polar-h10-polar_rusty_backend_rawACC",
    ),
}


def descriptor(info) -> tuple[str, str, int, float, int, str]:
    return (
        info.name(),
        info.type(),
        info.channel_count(),
        info.nominal_srate(),
        info.channel_format(),
        info.source_id(),
    )


def expected_descriptor(pylsl, expected: ExpectedStream):
    return (
        expected.name,
        expected.stream_type,
        expected.channel_count,
        expected.nominal_rate,
        pylsl.cf_float32,
        expected.source_id,
    )


def git_read(*arguments: str) -> str:
    return subprocess.check_output(
        ["git", *arguments], cwd=ROOT, text=True, encoding="utf-8"
    ).strip()


def resolve_exact_streams(pylsl, timeout: float):
    """Enumerate broadly, then select only exact descriptors client-side."""
    deadline = time.monotonic() + timeout
    latest = []
    while time.monotonic() < deadline:
        latest = pylsl.resolve_streams(wait_time=min(1.0, deadline - time.monotonic()))
        selected = {}
        complete = True
        for role, expected in EXPECTED.items():
            relevant = [
                info
                for info in latest
                if info.name() == expected.name or info.source_id() == expected.source_id
            ]
            unique = {info.uid(): info for info in relevant}
            exact = [
                info
                for info in unique.values()
                if descriptor(info) == expected_descriptor(pylsl, expected)
            ]
            if len(unique) > 1 or (unique and len(exact) != 1):
                observed = sorted(descriptor(info) for info in unique.values())
                raise RuntimeError(
                    f"{role} discovery was ambiguous or mismatched: {observed!r}"
                )
            if len(exact) != 1:
                complete = False
                break
            selected[role] = exact[0]
        if complete:
            if selected["ecg"].uid() == selected["acc"].uid():
                raise RuntimeError("ECG and ACC resolved to the same outlet identity")
            return selected
    observed = sorted(descriptor(info) for info in latest)
    raise RuntimeError(f"exact Rusty LSL streams were not found: {observed!r}")


def main() -> int:
    try:
        import pylsl
    except ImportError as error:
        raise SystemExit(
            "pylsl is required; use the pinned pylsl 1.18.2/liblsl 1.17.7 environment"
        ) from error

    if pylsl.__version__ != "1.18.2":
        raise SystemExit(f"expected pylsl 1.18.2, found {pylsl.__version__}")
    status = git_read("status", "--porcelain", "--untracked-files=normal")
    if status:
        raise SystemExit(
            "synthetic qualification requires an exact clean Polar Stream checkout"
        )
    polar_stream_revision = git_read("rev-parse", "HEAD")
    polar_stream_tree = git_read("rev-parse", "HEAD^{tree}")

    command = [
        "cargo",
        "run",
        "-p",
        "polar-h10-output",
        "--example",
        "verify_rusty_lsl",
        "--no-default-features",
        "--features",
        "rusty-lsl-backend",
        "--quiet",
    ]
    process = subprocess.Popen(
        command,
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    output: list[str] = []
    inlets = []
    try:
        ready_deadline = time.monotonic() + 180.0
        assert process.stdout is not None
        while time.monotonic() < ready_deadline:
            line = process.stdout.readline()
            if line:
                output.append(line)
                if line.startswith("POLAR_RUSTY_LSL_READY "):
                    break
            elif process.poll() is not None:
                raise RuntimeError("Rusty backend exited before becoming ready")
        else:
            raise RuntimeError("Rusty backend did not become ready before the deadline")

        streams = resolve_exact_streams(pylsl, timeout=15.0)
        for role in ("ecg", "acc"):
            inlet = pylsl.StreamInlet(streams[role], max_buflen=10, recover=False)
            inlet.open_stream(timeout=10.0)
            inlets.append(inlet)

        samples = {}
        timestamps = {}
        for role, inlet in zip(("ecg", "acc"), inlets, strict=True):
            sample, timestamp = inlet.pull_sample(timeout=15.0)
            if sample is None or not timestamp:
                raise RuntimeError(f"{role} inlet did not return a timestamped sample")
            samples[role] = sample
            timestamps[role] = timestamp
        if samples["ecg"] != [1725.0]:
            raise RuntimeError(f"unexpected ECG sample: {samples['ecg']!r}")
        if samples["acc"] != [101.0, -202.0, 303.0]:
            raise RuntimeError(f"unexpected ACC sample: {samples['acc']!r}")

        for inlet in inlets:
            inlet.close_stream()
        inlets.clear()
        remaining, _ = process.communicate(timeout=20.0)
        output.append(remaining)
        if process.returncode != 0:
            raise RuntimeError("Rusty backend exited unsuccessfully")
        result = {
            "schema": "polar.stream.rusty_lsl_backend_official_consumer.v2",
            "polar_stream_revision": polar_stream_revision,
            "polar_stream_tree": polar_stream_tree,
            "rusty_lsl_revision": RUSTY_LSL_REVISION,
            "scope": {
                "official_consumer": f"pylsl {pylsl.__version__}",
                "liblsl_version": pylsl.library_version(),
                "outlets": 2,
                "discovery": "broad enumeration plus exact client-side descriptor match",
                "predicate_filter_conformance": "unsupported and not exercised",
                "physical_device": False,
                "browser_transport": False,
            },
            "descriptors": {
                role: descriptor(streams[role]) for role in ("ecg", "acc")
            },
            "first_samples": samples,
            "timestamps_advance_independently": timestamps["ecg"] != timestamps["acc"],
            "result": "pass",
        }
        print(json.dumps(result, sort_keys=True))
        return 0
    except Exception:
        process.terminate()
        try:
            trailing, _ = process.communicate(timeout=5.0)
            output.append(trailing)
        except subprocess.TimeoutExpired:
            process.kill()
            trailing, _ = process.communicate()
            output.append(trailing)
        print("".join(output), file=sys.stderr)
        raise
    finally:
        for inlet in inlets:
            inlet.close_stream()


if __name__ == "__main__":
    raise SystemExit(main())
