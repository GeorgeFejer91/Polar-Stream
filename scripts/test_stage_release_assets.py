#!/usr/bin/env python3
"""Focused tests for deterministic release asset staging."""

from __future__ import annotations

from pathlib import Path
import tempfile
import unittest

import stage_release_assets as staging


class StageReleaseAssetsTests(unittest.TestCase):
    def test_reads_the_workspace_package_version_without_tomllib(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            manifest = Path(temporary) / "Cargo.toml"
            manifest.write_text(
                """
[workspace]
members = []

[workspace.package]
version = "0.5.0" # release version

[package]
version = "9.9.9"
""".strip(),
                encoding="utf-8",
            )

            self.assertEqual(staging.cargo_workspace_version(manifest), "0.5.0")

    def test_missing_workspace_package_version_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            manifest = Path(temporary) / "Cargo.toml"
            manifest.write_text('[package]\nversion = "0.5.0"\n', encoding="utf-8")

            with self.assertRaisesRegex(SystemExit, "workspace version was not found"):
                staging.cargo_workspace_version(manifest)


if __name__ == "__main__":
    unittest.main()
