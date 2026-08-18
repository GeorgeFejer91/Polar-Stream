#!/usr/bin/env python3
"""Contract tests for the bounded four-inlet two-H10 verifier."""

from __future__ import annotations

import queue
import sys
import threading
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from verify_two_h10_rusty_lsl import (
    EXPECTED,
    ROLES,
    collect_official_inlets,
    resolve_exact_streams,
    wait_for_source_ready,
)


class FakeInfo:
    def __init__(self, role: str, *, uid: str | None = None, channels: int | None = None):
        name, stream_type, expected_channels, rate, source_id = EXPECTED[role]
        self.role = role
        self.values = (
            name,
            stream_type,
            expected_channels if channels is None else channels,
            rate,
            1,
            source_id,
        )
        self._uid = uid or f"{role}-uid"

    def name(self):
        return self.values[0]

    def type(self):
        return self.values[1]

    def channel_count(self):
        return self.values[2]

    def nominal_srate(self):
        return self.values[3]

    def channel_format(self):
        return self.values[4]

    def source_id(self):
        return self.values[5]

    def uid(self):
        return self._uid


class FakeInlet:
    def __init__(self, info, *, max_buflen, recover):
        del max_buflen, recover
        self.info = info
        self.closed = False

    def open_stream(self, *, timeout):
        del timeout

    def pull_chunk(self, *, timeout, max_samples):
        del timeout, max_samples
        if self.info.role.endswith("-ecg"):
            samples = [[float(index + 1)] for index in range(260)]
            timestamps = [index / 130.0 for index in range(260)]
        else:
            samples = [[1.0, -2.0, 3.0] for _ in range(400)]
            timestamps = [index / 200.0 for index in range(400)]
        return samples, timestamps

    def close_stream(self):
        self.closed = True


class FakePylsl:
    cf_float32 = 1

    def __init__(self, streams=None):
        self.streams = list(streams or [FakeInfo(role) for role in ROLES])
        self.inlets = []

    def resolve_streams(self, *, wait_time):
        del wait_time
        return self.streams

    def StreamInlet(self, info, *, max_buflen, recover):
        inlet = FakeInlet(info, max_buflen=max_buflen, recover=recover)
        self.inlets.append(inlet)
        return inlet


class TwoH10OfficialInletTests(unittest.TestCase):
    def test_collects_four_exact_streams_and_waits_for_close(self):
        pylsl = FakePylsl()
        results = queue.Queue()
        progress = queue.Queue()
        close_requested = threading.Event()
        worker = threading.Thread(
            target=collect_official_inlets,
            args=(pylsl, results, close_requested, progress),
            daemon=True,
        )
        worker.start()

        status, result = results.get(timeout=1.0)
        self.assertEqual(status, "ready")
        self.assertEqual(set(result["inlets"]), set(ROLES))
        self.assertTrue(result["four_outlet_uids_distinct"])
        self.assertTrue(worker.is_alive())
        for role in ROLES:
            expected = 260 if role.endswith("-ecg") else 400
            self.assertEqual(result["inlets"][role]["samples"], expected)

        close_requested.set()
        worker.join(timeout=1.0)
        self.assertFalse(worker.is_alive())
        self.assertEqual(len(pylsl.inlets), 4)
        self.assertTrue(all(inlet.closed for inlet in pylsl.inlets))

    def test_rejects_missing_damaged_duplicate_and_shared_uid_streams(self):
        complete = [FakeInfo(role) for role in ROLES]
        with self.assertRaises(RuntimeError):
            resolve_exact_streams(FakePylsl(complete[:-1]), timeout=0.01)

        damaged = complete[:-1] + [FakeInfo(ROLES[-1], channels=2)]
        with self.assertRaises(RuntimeError):
            resolve_exact_streams(FakePylsl(damaged), timeout=0.01)

        duplicate = complete + [FakeInfo(ROLES[0], uid="duplicate-uid")]
        with self.assertRaises(RuntimeError):
            resolve_exact_streams(FakePylsl(duplicate), timeout=0.01)

        shared_uid = [FakeInfo(role, uid="shared") for role in ROLES]
        with self.assertRaises(RuntimeError):
            resolve_exact_streams(FakePylsl(shared_uid), timeout=0.01)

    def test_collection_observes_bounded_cancellation_and_closes_every_inlet(self):
        pylsl = FakePylsl()
        results = queue.Queue()
        close_requested = threading.Event()
        close_requested.set()

        collect_official_inlets(
            pylsl,
            results,
            close_requested,
            collection_timeout=0.01,
        )

        status, message = results.get_nowait()
        self.assertEqual(status, "error")
        self.assertIn("cancelled", message)
        self.assertEqual(len(pylsl.inlets), 4)
        self.assertTrue(all(inlet.closed for inlet in pylsl.inlets))

    def test_starts_official_consumers_before_source_readiness(self):
        class RunningProcess:
            @staticmethod
            def poll():
                return None

        events = queue.Queue()
        events.put("TWO_H10_LSL_INITIALIZED {}\n")
        events.put("TWO_H10_SELECTED {}\n")
        events.put("TWO_H10_SOURCE_READY {}\n")
        starts = []

        wait_for_source_ready(RunningProcess(), events, lambda: starts.append("official"))
        self.assertEqual(starts, ["official"])

    def test_rejects_source_readiness_before_official_startup(self):
        class RunningProcess:
            @staticmethod
            def poll():
                return None

        events = queue.Queue()
        events.put("TWO_H10_SELECTED {}\n")
        events.put("TWO_H10_SOURCE_READY {}\n")
        with self.assertRaisesRegex(RuntimeError, "preceded official inlet startup"):
            wait_for_source_ready(RunningProcess(), events, lambda: None)


if __name__ == "__main__":
    unittest.main()
