#!/usr/bin/env python3
"""Focused tests for the LabRecorder artifact boundary."""

from __future__ import annotations

import io
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile

import prepare_lab_recorder as recorder


class PrepareLabRecorderTests(unittest.TestCase):
    def test_zip_extraction_rejects_parent_traversal(self) -> None:
        package = io.BytesIO()
        with zipfile.ZipFile(package, "w") as archive:
            archive.writestr("../outside.txt", "bad")
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaisesRegex(RuntimeError, "Unsafe archive member"):
                recorder.extract_archive("bundle.zip", package.getvalue(), Path(temporary))

    def test_tar_extraction_rejects_escaping_symlink(self) -> None:
        package = io.BytesIO()
        with tarfile.open(fileobj=package, mode="w:gz") as archive:
            member = tarfile.TarInfo("bundle/link")
            member.type = tarfile.SYMTYPE
            member.linkname = "../../outside"
            archive.addfile(member)
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaisesRegex(RuntimeError, "symlink escapes"):
                recorder.extract_archive("bundle.tar.gz", package.getvalue(), Path(temporary))

    def test_supported_targets_cover_every_release_matrix_class(self) -> None:
        self.assertEqual(recorder.PLATFORM_ARCHES["windows"], {"x86_64", "aarch64"})
        self.assertEqual(recorder.PLATFORM_ARCHES["linux"], {"x86_64", "aarch64"})
        self.assertEqual(recorder.PLATFORM_ARCHES["macos"], {"universal"})
        self.assertIn(("windows", "x86_64"), recorder.ARCHIVES)
        self.assertIn(("linux", "x86_64"), recorder.ARCHIVES)
        self.assertIn(("macos", "universal"), recorder.ARCHIVES)

    def test_packaged_profile_disables_remote_control(self) -> None:
        config = recorder.PACKAGED_CONFIG.read_text(encoding="utf-8")
        self.assertIn("RCSEnabled=0", config)
        self.assertNotIn("RCSEnabled=1", config)

    def test_packaged_qt_notice_preserves_dynamic_relinking_rights(self) -> None:
        notice = recorder.PACKAGED_QT_NOTICE.read_text(encoding="utf-8")
        self.assertRegex(notice, r"GNU Lesser\s+General Public License version 3")
        self.assertIn("replace the Qt libraries", notice)
        self.assertEqual(set(recorder.QT_LICENSES), {
            "QT-LICENSE-LGPL-3.0.txt",
            "QT-LICENSE-GPL-3.0.txt",
        })

    def test_linux_release_archive_may_have_one_nested_distribution_root(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            install = root / "release"
            executable = install / "LabRecorder-1.17.0-jammy_amd64" / "bin" / "LabRecorder"
            executable.parent.mkdir(parents=True)
            executable.write_bytes(b"fixture")
            library = executable.parent.parent / "lib" / "liblsl.so"
            library.parent.mkdir()
            library.write_bytes(b"library fixture")
            destination = root / "destination"
            destination.mkdir()

            original_bundle = recorder.bundle_linux_dependencies
            recorder.bundle_linux_dependencies = lambda _destination: None
            try:
                recorder.stage_linux_install(install, destination)
            finally:
                recorder.bundle_linux_dependencies = original_bundle

            self.assertEqual((destination / "LabRecorder").read_bytes(), b"fixture")


if __name__ == "__main__":
    unittest.main()
