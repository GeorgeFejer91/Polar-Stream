#!/usr/bin/env python3
"""Download a pinned liblsl runtime for a native release package."""

from __future__ import annotations

import argparse
import hashlib
import io
import os
from pathlib import Path
import tarfile
import urllib.request
import zipfile


VERSION = "1.17.7"
BASE_URL = f"https://github.com/sccn/liblsl/releases/download/v{VERSION}"
ARCHIVES = {
    ("linux", "x86_64"): (
        f"liblsl-{VERSION}-jammy_amd64.tar.gz",
        "a73dfca12aaffe65d22599032fa6ba683e9a54e2f8e4212cfdb05d5854b86bec",
        f"liblsl-{VERSION}-jammy_amd64/lib/liblsl.so.1.17.7",
    ),
    ("linux", "aarch64"): (
        f"liblsl-{VERSION}-jammy_arm64.tar.gz",
        "73512bd00c6c4ad88f9a4269ca3f2e42520bdb69ce52014e19690baae6496acb",
        f"liblsl-{VERSION}-jammy_arm64/lib/liblsl.so.1.17.7",
    ),
    ("windows", "x86_64"): (
        f"liblsl-{VERSION}-Win_amd64.zip",
        "1285c4846f705108d417f5b7e57727f7e864941692d936fa18f8e7ab9b7112e1",
        f"liblsl-{VERSION}-Win_amd64/bin/lsl.dll",
    ),
    ("windows", "aarch64"): (
        f"liblsl-{VERSION}-Win_arm64.zip",
        "cb581610684e53a8d384f3018a94d75898d5cc8d480fe29349eabad86bc3da29",
        f"liblsl-{VERSION}-Win_arm64/bin/lsl.dll",
    ),
    ("macos", "universal"): (
        "lsl.xcframework.1.17.zip",
        "7886450aa2abe8545417f2c7eb11a9e1b224c9e33d716cfccb13a01706d5b593",
        "lsl.xcframework/macos-arm64_x86_64/lsl.framework/Versions/A/lsl",
    ),
}


def read_member(archive_name: str, archive: bytes, member: str) -> bytes:
    if archive_name.endswith(".tar.gz"):
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as package:
            extracted = package.extractfile(member)
            if extracted is None:
                raise RuntimeError(f"Missing {member} in {archive_name}")
            return extracted.read()
    with zipfile.ZipFile(io.BytesIO(archive)) as package:
        return package.read(member)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", required=True, choices=["linux", "windows", "macos"])
    parser.add_argument("--arch", required=True, choices=["x86_64", "aarch64", "universal"])
    parser.add_argument("--output", default="apps/polar-stream/resources")
    args = parser.parse_args()

    archive_name, expected_hash, member = ARCHIVES[(args.platform, args.arch)]
    with urllib.request.urlopen(f"{BASE_URL}/{archive_name}") as response:
        archive = response.read()
    actual_hash = hashlib.sha256(archive).hexdigest()
    if actual_hash != expected_hash:
        raise RuntimeError(f"Checksum mismatch for {archive_name}: {actual_hash}")

    output_dir = Path(args.output)
    output_dir.mkdir(parents=True, exist_ok=True)
    destination_name = {
        "linux": "liblsl.so",
        "windows": "lsl.dll",
        "macos": "liblsl.dylib",
    }[args.platform]
    destination = output_dir / destination_name
    destination.write_bytes(read_member(archive_name, archive, member))
    if args.platform != "windows":
        os.chmod(destination, 0o755)
    print(f"Prepared {destination} from liblsl v{VERSION} ({actual_hash[:12]})")


if __name__ == "__main__":
    main()
