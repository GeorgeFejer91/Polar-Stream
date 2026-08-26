#!/usr/bin/env python3
"""Prepare a pinned, self-contained LabRecorder resource bundle.

Official release archives are used where upstream publishes a matching native
artifact. ARM64 Windows/Linux jobs build the same immutable upstream source and
liblsl revisions, then pass the install tree through the same staging checks.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import struct
import subprocess
import tarfile
import tempfile
import urllib.request
import zipfile


ROOT = Path(__file__).resolve().parents[1]
RESOURCE_OUTPUT = ROOT / "apps" / "polar-stream" / "resources" / "lab-recorder"
PACKAGED_CONFIG = ROOT / "packaging" / "lab-recorder" / "PolarStream-LabRecorder.cfg"
PACKAGED_QT_NOTICE = ROOT / "packaging" / "lab-recorder" / "QT-NOTICE.txt"

UPSTREAM_PROJECT_VERSION = "1.17.0"
UPSTREAM_RELEASE = "v1.17.1"
UPSTREAM_COMMIT = "8419550553e4336dd46378a9a871b3065a70b895"
UPSTREAM_SOURCE_SHA256 = "e4c23a91b00e3da8f6ba3030f76453f58ad2d6cd09f0a379f5f718919139611f"
UPSTREAM_SOURCE_URL = (
    "https://codeload.github.com/labstreaminglayer/App-LabRecorder/tar.gz/"
    f"{UPSTREAM_COMMIT}"
)
LIBLSL_SOURCE_COMMIT = "03316f61137485450e7a43aea972c8e55b0c796a"
RELEASE_BASE_URL = (
    "https://github.com/labstreaminglayer/App-LabRecorder/releases/download/"
    f"{UPSTREAM_RELEASE}"
)

QT_LICENSES = {
    "QT-LICENSE-LGPL-3.0.txt": (
        "https://raw.githubusercontent.com/qt/qtbase/v6.8.3/LICENSES/LGPL-3.0-only.txt",
        "da7eabb7bafdf7d3ae5e9f223aa5bdc1eece45ac569dc21b3b037520b4464768",
    ),
    "QT-LICENSE-GPL-3.0.txt": (
        "https://raw.githubusercontent.com/qt/qtbase/v6.8.3/LICENSES/GPL-3.0-only.txt",
        "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903",
    ),
}

ARCHIVES = {
    ("windows", "x86_64"): (
        "LabRecorder-1.17.0-Win_amd64.zip",
        "01bde1d9af07d29de1a8363c967cc1eeaf524915f2db76552484f7becdb161ed",
    ),
    ("linux", "x86_64"): (
        "LabRecorder-1.17.0-jammy_amd64.tar.gz",
        "83a9d3f1e77b3406dc38ce9fbdc5876360105279d268de73c17e5846a4cd6389",
    ),
    ("macos", "universal"): (
        "LabRecorder-1.17.0-macOS_universal-signed.tar.gz",
        "9843d491e52768caf783f30e8ec22313f805c168f6cc0bcbdb24acc437319403",
    ),
}

PLATFORM_ARCHES = {
    "windows": {"x86_64", "aarch64"},
    "linux": {"x86_64", "aarch64"},
    "macos": {"universal"},
}

LINUX_GLIBC_LIBRARIES = {
    "ld-linux-aarch64.so.1",
    "ld-linux-x86-64.so.2",
    "libanl.so.1",
    "libc.so.6",
    "libdl.so.2",
    "libm.so.6",
    "libpthread.so.0",
    "libresolv.so.2",
    "librt.so.1",
    "libutil.so.1",
}


def sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def download(url: str, expected_hash: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": "Polar-Stream-packager"})
    with urllib.request.urlopen(request) as response:
        payload = response.read()
    actual_hash = sha256(payload)
    if actual_hash != expected_hash:
        raise RuntimeError(f"Checksum mismatch for {url}: {actual_hash}")
    return payload


def safe_archive_path(root: Path, member_name: str) -> Path:
    member = PurePosixPath(member_name.replace("\\", "/"))
    if member.is_absolute() or ".." in member.parts:
        raise RuntimeError(f"Unsafe archive member: {member_name}")
    destination = root.joinpath(*member.parts)
    resolved_root = root.resolve()
    resolved_destination = destination.resolve(strict=False)
    if os.path.commonpath((resolved_root, resolved_destination)) != str(resolved_root):
        raise RuntimeError(f"Archive member escapes its destination: {member_name}")
    return destination


def extract_archive(name: str, payload: bytes, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    if name.endswith(".zip"):
        with zipfile.ZipFile(io.BytesIO(payload)) as package:
            for member in package.infolist():
                safe_archive_path(destination, member.filename)
            package.extractall(destination)
        return

    with tarfile.open(fileobj=io.BytesIO(payload), mode="r:gz") as package:
        members = package.getmembers()
        for member in members:
            member_path = safe_archive_path(destination, member.name)
            if member.issym():
                link_target = member_path.parent / member.linkname
                resolved_target = link_target.resolve(strict=False)
                if os.path.commonpath((destination.resolve(), resolved_target)) != str(
                    destination.resolve()
                ):
                    raise RuntimeError(f"Archive symlink escapes its destination: {member.name}")
            elif member.islnk():
                safe_archive_path(destination, member.linkname)
        package.extractall(destination, members=members)


def one_directory(root: Path) -> Path:
    directories = sorted(path for path in root.iterdir() if path.is_dir())
    files = [path for path in root.iterdir() if path.is_file()]
    if len(directories) != 1 or files:
        raise RuntimeError(f"Expected one archive root below {root}")
    return directories[0]


def copy_tree_contents(source: Path, destination: Path, excluded: set[str] | None = None) -> None:
    excluded = excluded or set()
    for path in source.iterdir():
        if path.name in excluded:
            continue
        target = destination / path.name
        if path.is_dir():
            shutil.copytree(path, target, symlinks=True)
        else:
            shutil.copy2(path, target, follow_symlinks=False)


def stage_windows_install(install_root: Path, destination: Path) -> None:
    source = install_root
    if not (source / "LabRecorder.exe").is_file():
        candidates = sorted(install_root.rglob("LabRecorder.exe"))
        if len(candidates) != 1:
            raise RuntimeError("Windows LabRecorder install does not contain one LabRecorder.exe")
        source = candidates[0].parent
    copy_tree_contents(source, destination, {"LabRecorderCLI.exe", "xdfwriter.lib"})


def qt_plugin_root() -> Path:
    commands = (["qtpaths6"], ["qtpaths"], ["/usr/lib/qt6/bin/qtpaths6"])
    for command in commands:
        try:
            result = subprocess.run(
                [*command, "--query", "QT_INSTALL_PLUGINS"],
                check=True,
                capture_output=True,
                text=True,
            )
        except (FileNotFoundError, subprocess.CalledProcessError):
            continue
        candidate = Path(result.stdout.strip())
        if candidate.is_dir():
            return candidate
    raise RuntimeError("Qt 6 plugin directory was not found; install qt6-base-dev first")


def ldd_dependencies(binary: Path) -> set[Path]:
    result = subprocess.run(
        ["ldd", str(binary)], check=True, capture_output=True, text=True
    )
    dependencies: set[Path] = set()
    for line in result.stdout.splitlines():
        match = re.search(r"=>\s+(/[^\s]+)\s+\(", line)
        if not match:
            match = re.match(r"\s*(/[^\s]+)\s+\(", line)
        if match:
            dependencies.add(Path(match.group(1)))
        elif "=> not found" in line:
            raise RuntimeError(f"Unresolved Linux dependency for {binary}: {line.strip()}")
    return dependencies


def bundle_linux_dependencies(destination: Path) -> None:
    plugins_source = qt_plugin_root()
    plugins_destination = destination / "plugins"
    plugin_families = (
        "iconengines",
        "imageformats",
        "networkinformation",
        "platforms",
        "platformthemes",
        "styles",
        "tls",
        "xcbglintegrations",
    )
    for family in plugin_families:
        source = plugins_source / family
        if source.is_dir():
            shutil.copytree(source, plugins_destination / family, symlinks=True)

    library_destination = destination / "lib"
    library_destination.mkdir(exist_ok=True)
    queue = [destination / "LabRecorder"]
    queue.extend(path for path in plugins_destination.rglob("*.so") if path.is_file())
    queue.extend(path for path in library_destination.iterdir() if path.is_file())
    visited: set[Path] = set()
    copied_by_name: dict[str, Path] = {}
    while queue:
        binary = queue.pop()
        resolved = binary.resolve()
        if resolved in visited:
            continue
        visited.add(resolved)
        for dependency in ldd_dependencies(binary):
            if dependency.name in LINUX_GLIBC_LIBRARIES:
                continue
            existing = copied_by_name.get(dependency.name)
            if existing and existing.resolve() != dependency.resolve():
                raise RuntimeError(
                    f"Conflicting Linux dependencies named {dependency.name}: "
                    f"{existing} and {dependency}"
                )
            target = library_destination / dependency.name
            if not target.exists():
                shutil.copy2(dependency, target, follow_symlinks=True)
                copied_by_name[dependency.name] = dependency
                queue.append(target)


def stage_linux_install(install_root: Path, destination: Path) -> None:
    executable_candidates = sorted(
        path
        for path in install_root.rglob("LabRecorder")
        if path.is_file() and (path.parent == install_root or path.parent.name == "bin")
    )
    if len(executable_candidates) != 1:
        raise RuntimeError(
            "Linux LabRecorder install must contain exactly one LabRecorder executable; "
            f"found {len(executable_candidates)}"
        )
    executable = executable_candidates[0]
    shutil.copy2(executable, destination / "LabRecorder")
    os.chmod(destination / "LabRecorder", 0o755)

    library_destination = destination / "lib"
    library_destination.mkdir()
    libraries = sorted(install_root.rglob("liblsl.so*"))
    regular_libraries = [path for path in libraries if path.is_file()]
    if not regular_libraries:
        raise RuntimeError("Linux LabRecorder install does not contain liblsl")
    for library in regular_libraries:
        target = library_destination / library.name
        if not target.exists():
            shutil.copy2(library, target, follow_symlinks=True)

    for name in ("LabRecorder.cfg", "LICENSE", "README.md"):
        candidates = sorted(install_root.rglob(name))
        if candidates:
            shutil.copy2(candidates[0], destination / name)
    bundle_linux_dependencies(destination)


def stage_macos_install(install_root: Path, destination: Path) -> None:
    candidates = sorted(install_root.rglob("LabRecorder.app"))
    if len(candidates) != 1:
        raise RuntimeError("macOS LabRecorder archive does not contain one LabRecorder.app")
    shutil.copytree(candidates[0], destination / "LabRecorder.app", symlinks=True)
    for apple_double in (destination / "LabRecorder.app").rglob("._*"):
        apple_double.unlink()


def source_build(platform: str, arch: str, work_root: Path) -> Path:
    payload = download(UPSTREAM_SOURCE_URL, UPSTREAM_SOURCE_SHA256)
    source_archive = work_root / "source"
    extract_archive("source.tar.gz", payload, source_archive)
    source = one_directory(source_archive)
    build = work_root / "build"
    install = work_root / "install"
    configure = [
        "cmake",
        "-S",
        str(source),
        "-B",
        str(build),
        "-DCMAKE_BUILD_TYPE=Release",
        f"-DCMAKE_INSTALL_PREFIX={install}",
        "-DLSL_FETCH_IF_MISSING=ON",
        f"-DLSL_FETCH_REF={LIBLSL_SOURCE_COMMIT}",
    ]
    if platform == "windows":
        qt_root = os.environ.get("POLAR_STREAM_QT_ROOT") or os.environ.get("QT_ROOT_DIR")
        if not qt_root and os.environ.get("Qt6_DIR"):
            qt_root = str(Path(os.environ["Qt6_DIR"]).resolve().parents[2])
        if not qt_root:
            raise RuntimeError(
                "POLAR_STREAM_QT_ROOT, QT_ROOT_DIR, or Qt6_DIR is required for the Windows ARM64 build"
            )
        configure.extend(["-A", "ARM64", f"-DCMAKE_PREFIX_PATH={qt_root}"])
    subprocess.run(configure, check=True)
    subprocess.run(
        ["cmake", "--build", str(build), "--config", "Release", "--parallel", "2"],
        check=True,
    )
    subprocess.run(
        ["cmake", "--install", str(build), "--config", "Release"], check=True
    )
    return install


def pe_machine(path: Path) -> int:
    payload = path.read_bytes()
    if payload[:2] != b"MZ" or len(payload) < 0x40:
        raise RuntimeError(f"{path} is not a PE executable")
    pe_offset = struct.unpack_from("<I", payload, 0x3C)[0]
    if payload[pe_offset : pe_offset + 4] != b"PE\0\0":
        raise RuntimeError(f"{path} has no PE header")
    return struct.unpack_from("<H", payload, pe_offset + 4)[0]


def elf_machine(path: Path) -> int:
    payload = path.read_bytes()[:20]
    if payload[:4] != b"\x7fELF":
        raise RuntimeError(f"{path} is not an ELF executable")
    byte_order = "<" if payload[5] == 1 else ">"
    return struct.unpack_from(f"{byte_order}H", payload, 18)[0]


def verify_bundle(destination: Path, platform: str, arch: str) -> None:
    manifest_path = destination / "manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    expected_manifest = {
        "schema": "polar.stream.lab_recorder_bundle.v1",
        "version": UPSTREAM_PROJECT_VERSION,
        "upstreamRelease": UPSTREAM_RELEASE,
        "upstreamCommit": UPSTREAM_COMMIT,
        "platform": platform,
        "arch": arch,
        "remoteControlEnabled": False,
    }
    for key, expected in expected_manifest.items():
        if manifest.get(key) != expected:
            raise RuntimeError(f"LabRecorder manifest {key} is {manifest.get(key)!r}, expected {expected!r}")
    required = [
        destination / "PolarStream-LabRecorder.cfg",
        destination / "QT-NOTICE.txt",
        destination / "QT-LICENSE-LGPL-3.0.txt",
        destination / "QT-LICENSE-GPL-3.0.txt",
    ]
    if platform == "windows":
        required.extend(
            destination / name
            for name in (
                "LabRecorder.exe",
                "LICENSE",
                "lsl.dll",
                "Qt6Core.dll",
                "Qt6Widgets.dll",
                "platforms/qwindows.dll",
            )
        )
        expected_machine = {"x86_64": 0x8664, "aarch64": 0xAA64}[arch]
        if pe_machine(destination / "LabRecorder.exe") != expected_machine:
            raise RuntimeError("LabRecorder.exe architecture does not match the requested bundle")
    elif platform == "linux":
        required.extend(
            destination / name
            for name in (
                "LabRecorder",
                "LICENSE",
                "plugins/platforms/libqxcb.so",
            )
        )
        if not list((destination / "lib").glob("liblsl.so*")):
            raise RuntimeError("Linux LabRecorder bundle is missing liblsl")
        if not list((destination / "lib").glob("libQt6Core.so*")):
            raise RuntimeError("Linux LabRecorder bundle is missing Qt6Core")
        if not list((destination / "lib").glob("libQt6Widgets.so*")):
            raise RuntimeError("Linux LabRecorder bundle is missing Qt6Widgets")
        expected_machine = {"x86_64": 62, "aarch64": 183}[arch]
        if elf_machine(destination / "LabRecorder") != expected_machine:
            raise RuntimeError("LabRecorder ELF architecture does not match the requested bundle")
    else:
        required.extend(
            destination / name
            for name in (
                "LabRecorder.app/Contents/MacOS/LabRecorder",
                "LabRecorder.app/Contents/MacOS/LICENSE",
                "LabRecorder.app/Contents/Frameworks/lsl.framework/lsl",
                "LabRecorder.app/Contents/PlugIns/platforms/libqcocoa.dylib",
            )
        )
        magic = (destination / "LabRecorder.app/Contents/MacOS/LabRecorder").read_bytes()[:4]
        if magic not in (b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf"):
            raise RuntimeError("macOS LabRecorder executable is not a universal Mach-O")
    missing = [str(path) for path in required if not path.exists()]
    if missing:
        raise RuntimeError("LabRecorder bundle is incomplete: " + ", ".join(missing))
    if (destination / PACKAGED_CONFIG.name).read_bytes() != PACKAGED_CONFIG.read_bytes():
        raise RuntimeError("LabRecorder packaged config does not match the reviewed profile")
    if (destination / PACKAGED_QT_NOTICE.name).read_bytes() != PACKAGED_QT_NOTICE.read_bytes():
        raise RuntimeError("LabRecorder Qt notice does not match the reviewed notice")
    for filename, (_, expected_hash) in QT_LICENSES.items():
        actual_hash = sha256((destination / filename).read_bytes())
        if actual_hash != expected_hash:
            raise RuntimeError(f"LabRecorder {filename} checksum mismatch: {actual_hash}")


def prepare(platform: str, arch: str, output: Path) -> None:
    if arch not in PLATFORM_ARCHES[platform]:
        raise RuntimeError(f"Unsupported LabRecorder target: {platform}/{arch}")
    output = output.resolve()
    if output.name != "lab-recorder":
        raise RuntimeError("The LabRecorder output directory must be named 'lab-recorder'")
    output.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=".lab-recorder-", dir=output.parent))
    try:
        archive = ARCHIVES.get((platform, arch))
        source = "source build"
        archive_hash = None
        with tempfile.TemporaryDirectory(prefix="polar-stream-lab-recorder-") as temporary:
            work_root = Path(temporary)
            if archive:
                archive_name, archive_hash = archive
                payload = download(f"{RELEASE_BASE_URL}/{archive_name}", archive_hash)
                extracted = work_root / "release"
                extract_archive(archive_name, payload, extracted)
                install_root = extracted
                source = "official release archive"
            else:
                install_root = source_build(platform, arch, work_root)

            if platform == "windows":
                stage_windows_install(install_root, staging)
            elif platform == "linux":
                stage_linux_install(install_root, staging)
            else:
                stage_macos_install(install_root, staging)

        shutil.copy2(PACKAGED_CONFIG, staging / PACKAGED_CONFIG.name)
        shutil.copy2(PACKAGED_QT_NOTICE, staging / PACKAGED_QT_NOTICE.name)
        for filename, (url, expected_hash) in QT_LICENSES.items():
            (staging / filename).write_bytes(download(url, expected_hash))
        manifest = {
            "schema": "polar.stream.lab_recorder_bundle.v1",
            "version": UPSTREAM_PROJECT_VERSION,
            "upstreamRelease": UPSTREAM_RELEASE,
            "upstreamCommit": UPSTREAM_COMMIT,
            "liblslSourceCommit": LIBLSL_SOURCE_COMMIT,
            "platform": platform,
            "arch": arch,
            "source": source,
            "archiveSha256": archive_hash,
            "remoteControlEnabled": False,
        }
        (staging / "manifest.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        verify_bundle(staging, platform, arch)
        if output.exists():
            shutil.rmtree(output)
        staging.rename(output)
        print(
            f"Prepared LabRecorder {UPSTREAM_PROJECT_VERSION} for {platform}/{arch} "
            f"from {source} at {output}"
        )
    finally:
        if staging.exists():
            shutil.rmtree(staging)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", required=True, choices=sorted(PLATFORM_ARCHES))
    parser.add_argument("--arch", required=True)
    parser.add_argument("--output", type=Path, default=RESOURCE_OUTPUT)
    parser.add_argument("--verify-only", action="store_true")
    arguments = parser.parse_args()
    if arguments.verify_only:
        verify_bundle(arguments.output.resolve(), arguments.platform, arguments.arch)
        print(f"Verified bundled LabRecorder at {arguments.output.resolve()}")
    else:
        prepare(arguments.platform, arguments.arch, arguments.output)


if __name__ == "__main__":
    main()
