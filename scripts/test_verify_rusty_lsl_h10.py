#!/usr/bin/env python3
"""Contract tests for the bounded physical-verifier inlet worker."""

from __future__ import annotations

import queue
import threading
import unittest

from verify_rusty_lsl_h10 import (
    EXPECTED,
    PMD_PROBE_SYNC_OWNER_PROFILE,
    SCAN_DIAGNOSTICS_ENV,
    SESSION_DIAGNOSTICS_ENV,
    SESSION_PROFILE_ENV,
    collect_official_inlets,
    physical_source_environment,
)


class FakeInfo:
    def __init__(self, role: str):
        name, stream_type, channels, rate, source_id = EXPECTED[role]
        self.role = role
        self.values = (name, stream_type, channels, rate, 1, source_id)

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
        return f"{self.role}-uid"


class FakeInlet:
    def __init__(self, info, *, max_buflen, recover):
        del max_buflen, recover
        self.info = info
        self.closed = False

    def open_stream(self, *, timeout):
        del timeout

    def pull_chunk(self, *, timeout, max_samples):
        del timeout, max_samples
        if self.info.role == "ecg":
            count = 260
            samples = [[float(index + 1)] for index in range(count)]
            timestamps = [index / 130.0 for index in range(count)]
        else:
            count = 400
            samples = [[1.0, -2.0, 3.0] for _ in range(count)]
            timestamps = [index / 200.0 for index in range(count)]
        return samples, timestamps

    def close_stream(self):
        self.closed = True


class FakePylsl:
    cf_float32 = 1

    def __init__(self):
        self.streams = [FakeInfo("ecg"), FakeInfo("acc")]
        self.inlets = []

    def resolve_streams(self, *, wait_time):
        del wait_time
        return self.streams

    def StreamInlet(self, info, *, max_buflen, recover):
        inlet = FakeInlet(info, max_buflen=max_buflen, recover=recover)
        self.inlets.append(inlet)
        return inlet


class OfficialInletWorkerTests(unittest.TestCase):
    def test_physical_source_enables_identifier_free_stage_diagnostics(self):
        environment = physical_source_environment()
        self.assertEqual(environment[SCAN_DIAGNOSTICS_ENV], "1")
        self.assertEqual(environment[SESSION_DIAGNOSTICS_ENV], "1")
        self.assertEqual(
            environment[SESSION_PROFILE_ENV], PMD_PROBE_SYNC_OWNER_PROFILE
        )

    def test_collects_exact_shapes_and_waits_for_explicit_close(self):
        pylsl = FakePylsl()
        results = queue.Queue()
        close_requested = threading.Event()
        worker = threading.Thread(
            target=collect_official_inlets,
            args=(pylsl, results, close_requested),
            daemon=True,
        )
        worker.start()

        status, result = results.get(timeout=1.0)
        self.assertEqual(status, "ready")
        self.assertEqual(result["inlets"]["ecg"]["samples"], 260)
        self.assertEqual(result["inlets"]["acc"]["samples"], 400)
        self.assertTrue(result["outlet_uids_distinct"])
        self.assertTrue(worker.is_alive())

        close_requested.set()
        worker.join(timeout=1.0)
        self.assertFalse(worker.is_alive())
        self.assertTrue(all(inlet.closed for inlet in pylsl.inlets))


if __name__ == "__main__":
    unittest.main()
