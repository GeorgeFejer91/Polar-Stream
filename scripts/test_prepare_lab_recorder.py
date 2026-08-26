#!/usr/bin/env python3
"""Focused tests for the LabRecorder artifact boundary."""

from __future__ import annotations

import io
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock
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
        self.assertNotIn(("linux", "x86_64"), recorder.ARCHIVES)
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

    def test_linux_install_may_have_one_nested_distribution_root(self) -> None:
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

    def test_ubuntu_qtpaths_location_is_used_when_it_is_not_on_path(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            def fake_run(command: list[str], **_kwargs: object) -> object:
                if command[0] != "/usr/lib/qt6/bin/qtpaths6":
                    raise FileNotFoundError(command[0])
                return type("Result", (), {"stdout": temporary + "\n"})()

            with mock.patch.object(recorder.subprocess, "run", side_effect=fake_run):
                self.assertEqual(recorder.qt_plugin_root(), Path(temporary))

    def test_linux_dependency_walk_ignores_a_library_reporting_itself(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            destination = root / "bundle"
            library = destination / "lib" / "libfixture.so"
            executable = destination / "LabRecorder"
            plugin_root = root / "plugins"
            library.parent.mkdir(parents=True)
            plugin_root.mkdir()
            library.write_bytes(b"library")
            executable.write_bytes(b"executable")

            with (
                mock.patch.object(recorder, "qt_plugin_root", return_value=plugin_root),
                mock.patch.object(
                    recorder,
                    "ldd_dependencies",
                    side_effect=lambda binary, _library_path: {binary},
                ),
            ):
                recorder.bundle_linux_dependencies(destination)

    def test_linux_dependency_walk_accepts_its_already_staged_copy(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            destination = root / "bundle"
            seed_library = destination / "lib" / "libfixture.so"
            staged_dependency = destination / "lib" / "libstdc++.so.6"
            system_dependency = root / "system" / "libstdc++.so.6"
            executable = destination / "LabRecorder"
            plugin_root = root / "plugins"
            seed_library.parent.mkdir(parents=True)
            system_dependency.parent.mkdir()
            plugin_root.mkdir()
            seed_library.write_bytes(b"seed")
            system_dependency.write_bytes(b"runtime")
            executable.write_bytes(b"executable")

            def dependencies(binary: Path, _library_path: Path) -> set[Path]:
                if binary.name == "libfixture.so":
                    return {system_dependency}
                return {staged_dependency}

            with (
                mock.patch.object(recorder, "qt_plugin_root", return_value=plugin_root),
                mock.patch.object(recorder, "ldd_dependencies", side_effect=dependencies),
            ):
                recorder.bundle_linux_dependencies(destination)

            self.assertEqual(staged_dependency.read_bytes(), b"runtime")

    def test_linux_dependency_walk_searches_the_staged_library_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "LabRecorder"
            library_path = root / "lib"
            binary.write_bytes(b"fixture")
            library_path.mkdir()
            completed = type("Result", (), {"stdout": ""})()

            with mock.patch.object(recorder.subprocess, "run", return_value=completed) as run:
                recorder.ldd_dependencies(binary, library_path)

            environment = run.call_args.kwargs["env"]
            self.assertEqual(
                environment["LD_LIBRARY_PATH"].split(os.pathsep)[0],
                str(library_path),
            )


if __name__ == "__main__":
    unittest.main()
