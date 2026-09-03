#!/usr/bin/env python3
"""Focused tests for the hash-bound GitHub Pages staging contract."""

from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import build_browser_demo as builder


class BuildBrowserDemoTests(unittest.TestCase):
    @staticmethod
    def write_download_surface(root: Path, names: tuple[str, ...]) -> None:
        root.mkdir(parents=True)
        for name in names:
            (root / name).write_text(name, encoding="utf-8")

    def test_manifest_hashes_ui_and_download_surfaces(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "pages"
            ui_hashes = builder.stage(output)
            manifest = json.loads(
                (output / "browser-demo-manifest.json").read_text(encoding="utf-8")
            )

            self.assertEqual(manifest["schemaVersion"], 2)
            self.assertEqual(manifest["canonicalSource"], "apps/polar-stream/ui")
            self.assertEqual(manifest["sha256"], ui_hashes)
            self.assertEqual(manifest["canonicalDownloadSource"], "download")
            self.assertEqual(
                set(manifest["downloadSha256"]), set(builder.REQUIRED_DOWNLOAD_ASSETS)
            )

            for name, expected in manifest["downloadSha256"].items():
                canonical = builder.canonical_asset_bytes(builder.DOWNLOAD / name)
                staged = (output / "download" / name).read_bytes()
                self.assertEqual(builder.digest(canonical), expected)
                self.assertEqual(builder.digest(staged), expected)

    def test_missing_download_asset_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            download = Path(temporary) / "download"
            self.write_download_surface(download, builder.REQUIRED_DOWNLOAD_ASSETS[:-1])
            with mock.patch.object(builder, "DOWNLOAD", download):
                with self.assertRaisesRegex(SystemExit, "missing required assets"):
                    builder.validate_sources()

    def test_unexpected_download_asset_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            download = Path(temporary) / "download"
            self.write_download_surface(
                download, builder.REQUIRED_DOWNLOAD_ASSETS + ("unreviewed.txt",)
            )
            with mock.patch.object(builder, "DOWNLOAD", download):
                with self.assertRaisesRegex(SystemExit, "unexpected assets"):
                    builder.validate_sources()

    def test_runtime_resource_parser_allows_navigation_but_rejects_remote_code(self) -> None:
        parser = builder.RuntimeResourceParser()
        parser.feed(
            '<a href="https://example.test/releases">Releases</a>'
            '<script src="https://example.test/runtime.js"></script>'
        )
        self.assertEqual(parser.remote_urls, ["https://example.test/runtime.js"])


if __name__ == "__main__":
    unittest.main()
