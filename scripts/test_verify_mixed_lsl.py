#!/usr/bin/env python3
"""Contract tests for the official-pylsl mixed-device verifier."""

from __future__ import annotations

import pathlib
import sys
import unittest


SCRIPTS = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))

import verify_mixed_lsl as verifier


class FakePylsl:
    cf_float32 = 1
    cf_double64 = 2


class MixedLslVerifierTests(unittest.TestCase):
    def test_polar_breathing_descriptor_matches_irregular_notification_output(self) -> None:
        descriptor = verifier.expected_descriptors(FakePylsl)["polar_breathing"]
        self.assertEqual(descriptor[0], "polar_mixed_acceptance_source-1_breathingVolume")
        self.assertEqual(descriptor[1:5], ("Breathing", 1, 0.0, FakePylsl.cf_float32))

    def test_exact_mixed_contract_contains_five_source_scoped_streams(self) -> None:
        descriptors = verifier.expected_descriptors(FakePylsl)
        self.assertEqual(
            set(descriptors),
            {"ecg", "acc", "polar_breathing", "vernier_raw", "vernier_breathing"},
        )
        names = [descriptor[0] for descriptor in descriptors.values()]
        self.assertEqual(len(names), len(set(names)))
        self.assertTrue(all("_source-" in name for name in names))


if __name__ == "__main__":
    unittest.main()
