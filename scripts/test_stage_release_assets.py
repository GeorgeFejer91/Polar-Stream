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
        app_names = ("polar-stream", "polar-stream-mini", "vernier-stream-mini")
        for app_name in app_names:
            (root / f"apps/{app_name}").mkdir(parents=True)
        (root / "Cargo.toml").write_text(
            f'[workspace.package]\nversion = "{version}"\n', encoding="utf-8"
        )
        local_packages = (
            "polar-stream",
            "polar-stream-mini",
            "vernier-stream-mini",
            "stream-mini-runtime",
        )
        (root / "Cargo.lock").write_text(
            "version = 3\n\n"
            + "\n".join(
                f'[[package]]\nname = "{name}"\nversion = "{version}"\n'
                for name in local_packages
            ),
            encoding="utf-8",
        )
        for app_name in app_names:
            (root / f"apps/{app_name}/tauri.conf.json").write_text(
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

    def test_stages_each_standalone_product_for_windows(self) -> None:
        version = "1.2.3"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle_root = root / "bundle"
            output = root / "release-assets"
            for product in staging.PRODUCTS:
                (bundle_root / "nsis").mkdir(parents=True, exist_ok=True)
                (bundle_root / "msi").mkdir(parents=True, exist_ok=True)
                (
                    bundle_root
                    / "nsis"
                    / f"{product.display_name}_{version}_x64-setup.exe"
                ).write_bytes(product.asset_prefix.encode())
                (
                    bundle_root
                    / "msi"
                    / f"{product.display_name}_{version}_x64_en-US.msi"
                ).write_bytes(product.asset_prefix.encode())

            staging.stage_release_assets(
                bundle_root,
                output,
                "windows",
                "x86_64",
                version,
            )

            self.assertEqual(
                sorted(path.name for path in output.iterdir()),
                sorted(
                    name
                    for product in staging.PRODUCTS
                    for name in (
                        f"{product.asset_prefix}_{version}_windows_x86_64.msi",
                        f"{product.asset_prefix}_{version}_windows_x86_64_setup.exe",
                    )
                ),
            )


if __name__ == "__main__":
    unittest.main()
