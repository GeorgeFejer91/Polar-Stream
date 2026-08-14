#!/usr/bin/env python3
"""Contract tests for exact client-side Rusty LSL stream selection."""

from __future__ import annotations

import unittest

from verify_rusty_lsl_backend import EXPECTED, resolve_exact_streams


class FakeInfo:
    def __init__(
        self,
        *,
        role: str,
        uid: str,
        channels: int | None = None,
        source_id: str | None = None,
    ):
        expected = EXPECTED[role]
        self.values = (
            expected.name,
            expected.stream_type,
            channels if channels is not None else expected.channel_count,
            expected.nominal_rate,
            1,
            source_id if source_id is not None else expected.source_id,
        )
        self.identity = uid

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
        return self.identity


class FakePylsl:
    cf_float32 = 1

    def __init__(self, streams):
        self.streams = streams

    def resolve_streams(self, *, wait_time):
        del wait_time
        return self.streams


class ExactStreamSelectionTests(unittest.TestCase):
    def test_accepts_one_exact_ecg_and_acc_with_distinct_uids(self):
        pylsl = FakePylsl(
            [FakeInfo(role="ecg", uid="ecg"), FakeInfo(role="acc", uid="acc")]
        )
        result = resolve_exact_streams(pylsl, timeout=0.01)
        self.assertEqual(set(result), {"ecg", "acc"})

    def test_rejects_wrong_shape_instead_of_opening_it(self):
        pylsl = FakePylsl(
            [
                FakeInfo(role="ecg", uid="ecg"),
                FakeInfo(role="acc", uid="acc", channels=1),
            ]
        )
        with self.assertRaisesRegex(RuntimeError, "mismatched"):
            resolve_exact_streams(pylsl, timeout=0.01)

    def test_rejects_multiple_exact_candidates(self):
        pylsl = FakePylsl(
            [
                FakeInfo(role="ecg", uid="ecg-one"),
                FakeInfo(role="ecg", uid="ecg-two"),
                FakeInfo(role="acc", uid="acc"),
            ]
        )
        with self.assertRaisesRegex(RuntimeError, "ambiguous"):
            resolve_exact_streams(pylsl, timeout=0.01)

    def test_rejects_source_id_drift(self):
        pylsl = FakePylsl(
            [
                FakeInfo(role="ecg", uid="ecg", source_id="wrong"),
                FakeInfo(role="acc", uid="acc"),
            ]
        )
        with self.assertRaisesRegex(RuntimeError, "mismatched"):
            resolve_exact_streams(pylsl, timeout=0.01)


if __name__ == "__main__":
    unittest.main()
