#!/usr/bin/env python3
"""Focused tests for deterministic release asset staging."""

from __future__ import annotations

from pathlib import Path
import json
import tempfile
import unittest

import stage_release_assets as staging


class StageReleaseAssetsTests(unittest.TestCase):
    @staticmethod
    def write_version_surfaces(root: Path, version: str) -> None:
        (root / "apps/polar-stream").mkdir(parents=True)
        (root / "Cargo.toml").write_text(
            f'[workspace.package]\nversion = "{version}"\n', encoding="utf-8"
        )
        (root / "Cargo.lock").write_text(
            f'version = 3\n\n[[package]]\nname = "polar-stream"\nversion = "{version}"\n',
            encoding="utf-8",
        )
        (root / "apps/polar-stream/tauri.conf.json").write_text(
            json.dumps({"version": version}), encoding="utf-8"
        )
        (root / "package.json").write_text(
            json.dumps({"version": version}), encoding="utf-8"
        )
        (root / "package-lock.json").write_text(
            json.dumps({"version": version, "packages": {"": {"version": version}}}),
            encoding="utf-8",
        )

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

    def test_reads_only_local_cargo_lockfile_versions(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            lockfile = Path(temporary) / "Cargo.lock"
            lockfile.write_text(
                '''version = 3

[[package]]
name = "polar-stream"
version = "0.6.0"

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
''',
                encoding="utf-8",
            )

            self.assertEqual(
                staging.cargo_lockfile_versions(lockfile),
                {"polar-stream": "0.6.0"},
            )

    def test_stale_local_cargo_lockfile_version_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.write_version_surfaces(root, "0.6.0")
            (root / "Cargo.lock").write_text(
                'version = 3\n\n[[package]]\nname = "polar-stream"\nversion = "0.5.0"\n',
                encoding="utf-8",
            )

            with self.assertRaisesRegex(SystemExit, "Cargo lockfile package polar-stream=0.5.0"):
                staging.verify_repository_versions(root, "v0.6.0")

    def test_reads_both_npm_lockfile_version_surfaces(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            lockfile = Path(temporary) / "package-lock.json"
            lockfile.write_text(
                '{"version":"0.6.0","packages":{"":{"version":"0.6.0"}}}',
                encoding="utf-8",
            )

            self.assertEqual(
                staging.npm_lockfile_versions(lockfile),
                ("0.6.0", "0.6.0"),
            )

    def test_missing_npm_lockfile_root_version_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            lockfile = Path(temporary) / "package-lock.json"
            lockfile.write_text('{"version":"0.6.0","packages":{}}', encoding="utf-8")

            with self.assertRaisesRegex(SystemExit, "lockfile versions were not found"):
                staging.npm_lockfile_versions(lockfile)

    def test_exact_prerelease_tag_matches_every_version_surface(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.write_version_surfaces(root, "0.6.0-rc.1")

            self.assertEqual(
                staging.verify_repository_versions(root, "v0.6.0-rc.1"),
                "0.6.0-rc.1",
            )

    def test_prerelease_tag_rejects_stable_repository_versions(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.write_version_surfaces(root, "0.6.0")

            with self.assertRaisesRegex(SystemExit, "does not match repository versions"):
                staging.verify_repository_versions(root, "v0.6.0-rc.1")

    def test_release_tag_requires_v_prefix_and_semver(self) -> None:
        for tag in ("0.6.0", "v01.2.3", "release-0.6.0"):
            with self.subTest(tag=tag):
                with self.assertRaises(SystemExit):
                    staging.release_version(tag)


if __name__ == "__main__":
    unittest.main()
