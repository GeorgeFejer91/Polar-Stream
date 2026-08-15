#!/usr/bin/env python3
"""Bounded physical H10 acceptance for the opt-in Rusty LSL backend."""

from __future__ import annotations

import json
import pathlib
import queue
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field


ROOT = pathlib.Path(__file__).resolve().parents[1]
RUSTY_LSL_REVISION = "74f7d0ea2cce9b3d049ea24602527a5f52360554"
BASE = "polar_stream_h10_acceptance"
EXPECTED = {
    "ecg": (f"{BASE}_rawECG", "ECG", 1, 130.0, f"polar-h10-{BASE}_rawECG"),
    "acc": (
        f"{BASE}_rawACC",
        "Accelerometer",
        3,
        200.0,
        f"polar-h10-{BASE}_rawACC",
    ),
}


def descriptor(info):
    return (
        info.name(),
        info.type(),
        info.channel_count(),
        info.nominal_srate(),
        info.channel_format(),
        info.source_id(),
    )


def expected_descriptor(pylsl, role: str):
    name, stream_type, channels, rate, source_id = EXPECTED[role]
    return (name, stream_type, channels, rate, pylsl.cf_float32, source_id)


def git_read(*arguments: str) -> str:
    return subprocess.check_output(
        ["git", *arguments], cwd=ROOT, text=True, encoding="utf-8"
    ).strip()


def resolve_exact_streams(pylsl, timeout: float):
    """Use only broad enumeration, with exact client-side descriptor matching."""
    deadline = time.monotonic() + timeout
    latest = []
    while time.monotonic() < deadline:
        latest = pylsl.resolve_streams(wait_time=min(1.0, deadline - time.monotonic()))
        selected = {}
        for role in ("ecg", "acc"):
            expected = expected_descriptor(pylsl, role)
            relevant = [
                info
                for info in latest
                if info.name() == expected[0] or info.source_id() == expected[5]
            ]
            unique = {info.uid(): info for info in relevant}
            exact = [info for info in unique.values() if descriptor(info) == expected]
            if len(unique) > 1 or (unique and len(exact) != 1):
                raise RuntimeError(
                    f"{role} discovery ambiguous/mismatched: "
                    f"{sorted(descriptor(info) for info in unique.values())!r}"
                )
            if len(exact) == 1:
                selected[role] = exact[0]
        if selected.keys() == {"ecg", "acc"}:
            if selected["ecg"].uid() == selected["acc"].uid():
                raise RuntimeError("ECG and ACC shared one outlet identity")
            return selected
    raise RuntimeError(
        "exact physical outlets were not found in broad enumeration: "
        f"{sorted(descriptor(info) for info in latest)!r}"
    )


@dataclass
class InletEvidence:
    channels: int
    nominal_rate: float
    sample_count: int = 0
    first_timestamp: float | None = None
    last_timestamp: float | None = None
    reordered: int = 0
    estimated_missing: int = 0
    max_step_seconds: float = 0.0
    nonzero_by_channel: list[int] = field(default_factory=list)
    minimum_by_channel: list[float] = field(default_factory=list)
    maximum_by_channel: list[float] = field(default_factory=list)

    def __post_init__(self):
        self.nonzero_by_channel = [0] * self.channels
        self.minimum_by_channel = [float("inf")] * self.channels
        self.maximum_by_channel = [float("-inf")] * self.channels

    def observe(self, samples, timestamps):
        if len(samples) != len(timestamps):
            raise RuntimeError("official inlet returned unequal sample/timestamp counts")
        for sample, timestamp in zip(samples, timestamps, strict=True):
            if len(sample) != self.channels:
                raise RuntimeError(
                    f"official inlet sample shape {len(sample)} != {self.channels}"
                )
            if self.last_timestamp is not None:
                step = timestamp - self.last_timestamp
                self.max_step_seconds = max(self.max_step_seconds, step)
                if step <= 0:
                    self.reordered += 1
                else:
                    represented = round(step * self.nominal_rate)
                    self.estimated_missing += max(0, represented - 1)
            if self.first_timestamp is None:
                self.first_timestamp = timestamp
            self.last_timestamp = timestamp
            self.sample_count += 1
            for channel, value in enumerate(sample):
                if value != 0.0:
                    self.nonzero_by_channel[channel] += 1
                self.minimum_by_channel[channel] = min(
                    self.minimum_by_channel[channel], value
                )
                self.maximum_by_channel[channel] = max(
                    self.maximum_by_channel[channel], value
                )

    def observed_rate(self):
        if (
            self.first_timestamp is None
            or self.last_timestamp is None
            or self.last_timestamp <= self.first_timestamp
            or self.sample_count < 2
        ):
            return None
        return (self.sample_count - 1) / (self.last_timestamp - self.first_timestamp)

    def evidence(self):
        return {
            "samples": self.sample_count,
            "first_lsl_timestamp": self.first_timestamp,
            "last_lsl_timestamp": self.last_timestamp,
            "timestamps_advanced": (
                self.first_timestamp is not None
                and self.last_timestamp is not None
                and self.last_timestamp > self.first_timestamp
            ),
            "observed_rate_hz": self.observed_rate(),
            "reordered_samples": self.reordered,
            "estimated_missing_samples": self.estimated_missing,
            "max_step_seconds": self.max_step_seconds,
            "nonzero_by_channel": self.nonzero_by_channel,
            "minimum_by_channel": self.minimum_by_channel,
            "maximum_by_channel": self.maximum_by_channel,
        }


def main() -> int:
    try:
        import pylsl
    except ImportError as error:
        raise SystemExit(
            "pylsl is required; use the pinned pylsl 1.18.2/liblsl 1.17.7 environment"
        ) from error
    if pylsl.__version__ != "1.18.2" or pylsl.library_version() != 117:
        raise SystemExit(
            f"expected pylsl 1.18.2/liblsl 117, found "
            f"{pylsl.__version__}/{pylsl.library_version()}"
        )
    status = git_read("status", "--porcelain", "--untracked-files=normal")
    if status:
        raise SystemExit(
            "physical qualification requires an exact clean Polar Stream checkout"
        )
    polar_stream_revision = git_read("rev-parse", "HEAD")
    polar_stream_tree = git_read("rev-parse", "HEAD^{tree}")

    command = [
        "cargo",
        "run",
        "-p",
        "polar-stream",
        "--example",
        "verify_rusty_lsl_h10",
        "--no-default-features",
        "--features",
        "rusty-lsl-backend",
        "--quiet",
    ]
    process = subprocess.Popen(
        command,
        cwd=ROOT,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        bufsize=1,
    )
    lines: list[str] = []
    events: queue.Queue[str] = queue.Queue()

    def read_output():
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line)
            events.put(line)

    reader = threading.Thread(target=read_output, daemon=True)
    reader.start()
    inlets = {}
    try:
        ready_deadline = time.monotonic() + 180.0
        while time.monotonic() < ready_deadline:
            try:
                line = events.get(timeout=0.25)
            except queue.Empty:
                if process.poll() is not None:
                    raise RuntimeError("physical source exited before LSL readiness")
                continue
            if line.startswith("POLAR_H10_LSL_READY "):
                break
        else:
            raise RuntimeError("physical source did not reach LSL readiness")

        streams = resolve_exact_streams(pylsl, timeout=15.0)
        for role in ("ecg", "acc"):
            inlet = pylsl.StreamInlet(streams[role], max_buflen=30, recover=False)
            inlet.open_stream(timeout=10.0)
            inlets[role] = inlet

        evidence = {
            "ecg": InletEvidence(channels=1, nominal_rate=130.0),
            "acc": InletEvidence(channels=3, nominal_rate=200.0),
        }
        source_result = None
        collection_deadline = time.monotonic() + 120.0
        while time.monotonic() < collection_deadline:
            for role in ("ecg", "acc"):
                samples, timestamps = inlets[role].pull_chunk(
                    timeout=0.05, max_samples=1024
                )
                evidence[role].observe(samples, timestamps)
            while True:
                try:
                    line = events.get_nowait()
                except queue.Empty:
                    break
                if line.startswith("POLAR_H10_CAPTURE_COMPLETE "):
                    source_result = json.loads(
                        line.removeprefix("POLAR_H10_CAPTURE_COMPLETE ")
                    )
            if source_result is not None:
                break
            if process.poll() is not None:
                raise RuntimeError("physical source exited before capture completion")
        if source_result is None:
            raise RuntimeError("physical source did not complete within two minutes")

        ecg = evidence["ecg"]
        acc = evidence["acc"]
        if ecg.sample_count < 260 or acc.sample_count < 400:
            raise RuntimeError(
                f"insufficient official samples: ECG={ecg.sample_count}, ACC={acc.sample_count}"
            )
        if ecg.reordered or acc.reordered:
            raise RuntimeError(
                f"official inlet reorder: ECG={ecg.reordered}, ACC={acc.reordered}"
            )
        if not all(count > 0 for count in acc.nonzero_by_channel):
            raise RuntimeError(
                f"ACC axes were not independently nonzero: {acc.nonzero_by_channel!r}"
            )
        if any(
            abs(value) > 32768.0
            for value in acc.minimum_by_channel + acc.maximum_by_channel
        ):
            raise RuntimeError("ACC value exceeded the bounded i16 device domain")
        if source_result["result"] != "source-pass":
            raise RuntimeError(f"physical source result was not a pass: {source_result!r}")

        for inlet in inlets.values():
            inlet.close_stream()
        inlets.clear()
        assert process.stdin is not None
        process.stdin.write("\n")
        process.stdin.flush()
        process.stdin.close()
        return_code = process.wait(timeout=30.0)
        reader.join(timeout=5.0)
        if return_code != 0:
            raise RuntimeError("physical source exited unsuccessfully during cleanup")
        stopped = any(line.startswith("POLAR_H10_STOPPED ") for line in lines)
        if not stopped:
            raise RuntimeError("physical source did not report clean stop")

        result = {
            "schema": "polar.stream.h10_rusty_lsl_official_acceptance.v1",
            "polar_stream_revision": polar_stream_revision,
            "polar_stream_tree": polar_stream_tree,
            "rusty_lsl_revision": RUSTY_LSL_REVISION,
            "official_consumer": {
                "pylsl": pylsl.__version__,
                "liblsl": pylsl.library_version(),
                "discovery": "broad enumeration plus exact client-side descriptor match",
                "predicate_filter_conformance": "unsupported and not exercised",
            },
            "descriptors": {
                role: descriptor(streams[role]) for role in ("ecg", "acc")
            },
            "outlet_uids_distinct": streams["ecg"].uid() != streams["acc"].uid(),
            "inlets": {role: evidence[role].evidence() for role in ("ecg", "acc")},
            "source": source_result,
            "cross_stream_misidentification": False,
            "cleanup": {
                "inlets_closed_before_source_stop": True,
                "source_reported_clean_stop": stopped,
                "process_exit_code": return_code,
            },
            "evidence_classification": (
                "private ignored aggregate; contains a physical device identity and no recording"
            ),
            "result": "pass",
        }
        print(json.dumps(result, sort_keys=True))
        return 0
    except Exception:
        for inlet in inlets.values():
            inlet.close_stream()
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10.0)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5.0)
        reader.join(timeout=2.0)
        print("".join(lines), file=sys.stderr)
        raise


if __name__ == "__main__":
    raise SystemExit(main())
