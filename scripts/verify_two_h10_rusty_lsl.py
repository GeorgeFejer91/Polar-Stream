#!/usr/bin/env python3
"""Bounded two-H10 acceptance through four pinned official LSL inlets."""

from __future__ import annotations

import json
import os
import pathlib
import queue
import subprocess
import sys
import threading
import time
import traceback

from verify_rusty_lsl_h10 import (
    InletEvidence,
    RUSTY_LSL_REVISION,
    descriptor,
    git_read,
    physical_source_environment,
)


ROOT = pathlib.Path(__file__).resolve().parents[1]
BASES = {
    "device-1": "polar_stream_two_h10_device_1",
    "device-2": "polar_stream_two_h10_device_2",
}
ROLES = tuple(
    f"{slot}-{signal}"
    for slot in ("device-1", "device-2")
    for signal in ("ecg", "acc")
)
EXPECTED = {
    f"{slot}-ecg": (
        f"{base}_rawECG",
        "ECG",
        1,
        130.0,
        f"polar-h10-{base}_rawECG",
    )
    for slot, base in BASES.items()
}
EXPECTED.update(
    {
        f"{slot}-acc": (
            f"{base}_rawACC",
            "Accelerometer",
            3,
            200.0,
            f"polar-h10-{base}_rawACC",
        )
        for slot, base in BASES.items()
    }
)


def expected_descriptor(pylsl, role: str):
    name, stream_type, channels, rate, source_id = EXPECTED[role]
    return (name, stream_type, channels, rate, pylsl.cf_float32, source_id)


def resolve_exact_streams(pylsl, timeout: float):
    """Broadly enumerate, then require one exact descriptor for every role."""
    deadline = time.monotonic() + timeout
    latest = []
    while time.monotonic() < deadline:
        latest = pylsl.resolve_streams(wait_time=min(1.0, deadline - time.monotonic()))
        selected = {}
        for role in ROLES:
            expected = expected_descriptor(pylsl, role)
            relevant = [
                info
                for info in latest
                if info.name() == expected[0] or info.source_id() == expected[5]
            ]
            unique = {info.uid(): info for info in relevant}
            exact = [info for info in unique.values() if descriptor(info) == expected]
            if len(unique) > 1 or (unique and len(exact) != 1):
                raise RuntimeError(f"{role} discovery was ambiguous or mismatched")
            if len(exact) == 1:
                selected[role] = exact[0]
        if selected.keys() == set(ROLES):
            uids = [selected[role].uid() for role in ROLES]
            if len(set(uids)) != len(uids):
                raise RuntimeError("two-H10 streams did not have four distinct outlet identities")
            return selected
    raise RuntimeError(
        "the four exact two-H10 outlets were not found in broad enumeration"
    )


def collect_official_inlets(
    pylsl, results, close_requested, progress=None, collection_timeout=120.0
):
    inlets = {}

    def report(stage: str):
        if progress is not None:
            progress.put(stage)

    try:
        report("resolving-four-exact-outlets")
        streams = resolve_exact_streams(pylsl, timeout=20.0)
        report("resolved-four-exact-outlets")
        for role in ROLES:
            inlet = pylsl.StreamInlet(streams[role], max_buflen=30, recover=False)
            inlet.open_stream(timeout=10.0)
            inlets[role] = inlet
            report(f"opened-{role}-inlet")

        evidence = {
            role: InletEvidence(
                channels=EXPECTED[role][2], nominal_rate=EXPECTED[role][3]
            )
            for role in ROLES
        }
        collection_deadline = time.monotonic() + collection_timeout
        while any(
            item.sample_count < (260 if role.endswith("-ecg") else 400)
            for role, item in evidence.items()
        ):
            if close_requested.is_set():
                raise RuntimeError("four-inlet collection was cancelled")
            if time.monotonic() >= collection_deadline:
                raise RuntimeError("four-inlet sample thresholds timed out")
            for role in ROLES:
                samples, timestamps = inlets[role].pull_chunk(
                    timeout=0.05, max_samples=1024
                )
                evidence[role].observe(samples, timestamps)

        for role, item in evidence.items():
            if item.reordered:
                raise RuntimeError(f"{role} official inlet reordered samples")
            if role.endswith("-acc"):
                if not all(count > 0 for count in item.nonzero_by_channel):
                    raise RuntimeError(f"{role} did not advance every ACC axis")
                if any(
                    abs(value) > 32768.0
                    for value in item.minimum_by_channel + item.maximum_by_channel
                ):
                    raise RuntimeError(f"{role} exceeded the bounded i16 device domain")
            observed_rate = item.observed_rate()
            lower, upper = (120.0, 140.0) if role.endswith("-ecg") else (180.0, 220.0)
            if observed_rate is None or not lower <= observed_rate <= upper:
                raise RuntimeError(f"{role} observed rate was outside its acceptance band")

        results.put(
            (
                "ready",
                {
                    "descriptors": {role: descriptor(streams[role]) for role in ROLES},
                    "four_outlet_uids_distinct": len(
                        {streams[role].uid() for role in ROLES}
                    )
                    == 4,
                    "inlets": {role: evidence[role].evidence() for role in ROLES},
                },
            )
        )
        report("four-sample-thresholds-passed")
        close_requested.wait(timeout=150.0)
    except Exception as error:
        report("worker-error")
        results.put(("error", f"{error}\n{traceback.format_exc()}"))
    finally:
        report("closing-four-inlets")
        for inlet in inlets.values():
            inlet.close_stream()
        report("four-inlets-closed")


def wait_for_source_ready(process, events, start_official_inlets):
    selected = False
    official_started = False
    deadline = time.monotonic() + 35.0
    while time.monotonic() < deadline:
        try:
            line = events.get(timeout=0.25)
        except queue.Empty:
            if process.poll() is not None:
                raise RuntimeError("two-H10 source exited before readiness")
            continue
        if line.startswith("TWO_H10_LSL_INITIALIZED ") and not official_started:
            start_official_inlets()
            official_started = True
        if line.startswith("TWO_H10_SELECTED "):
            selected = True
            deadline = time.monotonic() + 100.0
        if line.startswith("TWO_H10_SOURCE_READY "):
            if not official_started:
                raise RuntimeError("sensor readiness preceded official inlet startup")
            return
    if not selected:
        raise RuntimeError("bounded discovery did not select exactly two H10s")
    raise RuntimeError("both physical sources did not reach readiness after selection")


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
    if git_read("status", "--porcelain", "--untracked-files=normal"):
        raise SystemExit("physical qualification requires an exact clean checkout")

    revision = git_read("rev-parse", "HEAD")
    tree = git_read("rev-parse", "HEAD^{tree}")
    subprocess.run(
        [
            "cargo",
            "build",
            "-p",
            "polar-stream",
            "--example",
            "verify_two_h10_rusty_lsl",
            "--no-default-features",
            "--features",
            "rusty-lsl-backend",
        ],
        cwd=ROOT,
        check=True,
        timeout=240.0,
    )
    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--no-deps"],
            cwd=ROOT,
            text=True,
            encoding="utf-8",
        )
    )
    executable = (
        pathlib.Path(metadata["target_directory"])
        / "debug"
        / "examples"
        / (
            "verify_two_h10_rusty_lsl.exe"
            if os.name == "nt"
            else "verify_two_h10_rusty_lsl"
        )
    )
    process = subprocess.Popen(
        [str(executable)],
        cwd=ROOT,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        bufsize=1,
        env=physical_source_environment(),
    )
    lines = []
    events = queue.Queue()

    def read_output():
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line)
            events.put(line)

    reader = threading.Thread(target=read_output, daemon=True)
    reader.start()
    official_results = queue.Queue()
    official_progress = queue.Queue()
    close_official = threading.Event()
    official_thread = threading.Thread(
        target=collect_official_inlets,
        args=(pylsl, official_results, close_official, official_progress),
        daemon=True,
    )
    try:
        wait_for_source_ready(process, events, official_thread.start)
        source_result = None
        official_result = None
        deadline = time.monotonic() + 120.0
        while time.monotonic() < deadline:
            while True:
                try:
                    stage = official_progress.get_nowait()
                except queue.Empty:
                    break
                print(f"TWO_H10_OFFICIAL_STAGE {stage}", file=sys.stderr)
            while True:
                try:
                    line = events.get_nowait()
                except queue.Empty:
                    break
                if line.startswith("TWO_H10_CAPTURE_COMPLETE "):
                    source_result = json.loads(
                        line.removeprefix("TWO_H10_CAPTURE_COMPLETE ")
                    )
            try:
                status, payload = official_results.get_nowait()
            except queue.Empty:
                pass
            else:
                if status == "error":
                    raise RuntimeError(f"official inlet worker failed: {payload}")
                official_result = payload
            if source_result is not None and official_result is not None:
                break
            if process.poll() is not None:
                raise RuntimeError("two-H10 source exited before capture completion")
            time.sleep(0.01)
        if source_result is None or official_result is None:
            raise RuntimeError("source and four official inlets did not complete in time")
        if source_result["result"] != "source-pass":
            raise RuntimeError("two-H10 physical source did not pass")

        close_official.set()
        official_thread.join(timeout=10.0)
        if official_thread.is_alive():
            raise RuntimeError("four official inlets did not close within ten seconds")
        assert process.stdin is not None
        process.stdin.write("\n")
        process.stdin.flush()
        process.stdin.close()
        return_code = process.wait(timeout=45.0)
        reader.join(timeout=5.0)
        stopped = any(line.startswith("TWO_H10_STOPPED ") for line in lines)
        if return_code != 0 or not stopped:
            raise RuntimeError("two-H10 source did not stop cleanly")

        print(
            json.dumps(
                {
                    "schema": "polar.stream.two_h10_rusty_lsl_official_acceptance.v1",
                    "polar_stream_revision": revision,
                    "polar_stream_tree": tree,
                    "rusty_lsl_revision": RUSTY_LSL_REVISION,
                    "official_consumer": {
                        "pylsl": pylsl.__version__,
                        "liblsl": pylsl.library_version(),
                        "discovery": "broad enumeration plus exact client-side descriptor match",
                        "predicate_filter_conformance": "unsupported and not exercised",
                    },
                    "descriptors": official_result["descriptors"],
                    "four_outlet_uids_distinct": official_result[
                        "four_outlet_uids_distinct"
                    ],
                    "inlets": official_result["inlets"],
                    "source": source_result,
                    "cross_device_or_stream_misidentification": False,
                    "cleanup": {
                        "inlets_closed_before_source_stop": True,
                        "source_reported_clean_stop": stopped,
                        "process_exit_code": return_code,
                    },
                    "evidence_classification": (
                        "private ignored identifier-free aggregate; no recording"
                    ),
                    "result": "pass",
                },
                sort_keys=True,
            )
        )
        return 0
    except Exception:
        close_official.set()
        if process.poll() is None:
            if process.stdin is not None and not process.stdin.closed:
                try:
                    process.stdin.write("\n")
                    process.stdin.flush()
                    process.stdin.close()
                except (BrokenPipeError, OSError):
                    pass
            try:
                process.wait(timeout=60.0)
            except subprocess.TimeoutExpired:
                process.terminate()
                try:
                    process.wait(timeout=10.0)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5.0)
        if official_thread.ident is not None:
            official_thread.join(timeout=2.0)
        reader.join(timeout=2.0)
        print("".join(lines), file=sys.stderr)
        raise


if __name__ == "__main__":
    raise SystemExit(main())
