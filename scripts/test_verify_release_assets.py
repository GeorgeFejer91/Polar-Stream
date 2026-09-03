#!/usr/bin/env python3
"""Focused tests for exact release asset and checksum verification."""

from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest

import verify_release_assets as verifier


VERSION = "1.2.3-rc.1"


def stage_candidate(directory: Path) -> Path:
    names = verifier.expected_package_names(VERSION)
    for index, name in enumerate(names, 1):
        (directory / name).write_bytes(f"package-{index}\n".encode())
    checksums = directory / verifier.CHECKSUM_NAME
    checksums.write_text(
        "".join(
            f"{verifier.file_sha256(directory / name)}  {name}\n"
            for name in names
        ),
        encoding="utf-8",
    )
    return checksums


def remote_payload(directory: Path) -> list[dict[str, object]]:
    return [
        {
            "name": path.name,
            "size": path.stat().st_size,
            "digest": f"sha256:{verifier.file_sha256(path)}",
        }
        for path in sorted(directory.iterdir())
        if path.is_file()
    ]


class VerifyReleaseAssetsTests(unittest.TestCase):
    def test_accepts_exact_local_packages_and_checksums(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            checksums = stage_candidate(directory)

            verifier.verify_release_assets(directory, f"v{VERSION}", checksums)

    def test_rejects_an_unexpected_or_missing_package(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            checksums = stage_candidate(directory)
            (directory / verifier.expected_package_names(VERSION)[0]).unlink()
            (directory / "Polar-Stream_wrong_linux_x86_64.deb").write_bytes(b"extra")

            with self.assertRaisesRegex(verifier.VerificationError, "asset set mismatch"):
                verifier.verify_release_assets(directory, VERSION, checksums)

    def test_rejects_content_changed_after_checksum_generation(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            checksums = stage_candidate(directory)
            (directory / verifier.expected_package_names(VERSION)[0]).write_bytes(b"tampered")

            with self.assertRaisesRegex(verifier.VerificationError, "checksum mismatch"):
                verifier.verify_release_assets(directory, VERSION, checksums)

    def test_matches_uploaded_sizes_and_digests_to_the_local_candidate(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            checksums = stage_candidate(directory)
            payload_path = directory.parent / f"{directory.name}-assets.json"
            payload_path.write_text(
                json.dumps({"assets": remote_payload(directory)}), encoding="utf-8"
            )
            self.addCleanup(payload_path.unlink, missing_ok=True)

            verifier.verify_release_assets(
                payload_path,
                VERSION,
                checksums,
                reference_directory=directory,
            )

    def test_rejects_changed_uploaded_assets(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            checksums = stage_candidate(directory)
            payload = remote_payload(directory)
            payload[0] = {**payload[0], "digest": "sha256:" + "0" * 64}
            payload_path = directory.parent / f"{directory.name}-assets.json"
            payload_path.write_text(json.dumps(payload), encoding="utf-8")
            self.addCleanup(payload_path.unlink, missing_ok=True)

            with self.assertRaisesRegex(verifier.VerificationError, "size/digest mismatch"):
                verifier.verify_release_assets(
                    payload_path,
                    VERSION,
                    checksums,
                    reference_directory=directory,
                )

    def test_rejects_duplicate_uploaded_assets(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            checksums = stage_candidate(directory)
            payload = remote_payload(directory)
            payload.append(payload[1])
            payload_path = directory.parent / f"{directory.name}-assets.json"
            payload_path.write_text(json.dumps(payload), encoding="utf-8")
            self.addCleanup(payload_path.unlink, missing_ok=True)

            with self.assertRaisesRegex(verifier.VerificationError, "duplicate"):
                verifier.verify_release_assets(
                    payload_path,
                    VERSION,
                    checksums,
                    reference_directory=directory,
                )

    def test_rejects_non_semver_versions(self) -> None:
        for version in ("release/latest", "01.2.3", "1.2.3-rc.", "1.2.3+build..1"):
            with self.subTest(version=version):
                with self.assertRaisesRegex(
                    verifier.VerificationError, "invalid release version"
                ):
                    verifier.expected_package_names(version)


if __name__ == "__main__":
    unittest.main()
