#!/usr/bin/env python3
"""Verify the exact, checksummed Polar Stream release asset set."""

from __future__ import annotations

import argparse
from collections import Counter
from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
import re


VERSION_PATTERN = re.compile(
    r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)
CHECKSUM_LINE = re.compile(r"([0-9a-fA-F]{64}) [ *]([^/\\]+)")
PACKAGE_TEMPLATES = (
    "Polar-Stream_{version}_linux_aarch64.AppImage",
    "Polar-Stream_{version}_linux_aarch64.deb",
    "Polar-Stream_{version}_linux_x86_64.AppImage",
    "Polar-Stream_{version}_linux_x86_64.deb",
    "Polar-Stream_{version}_macos_universal.dmg",
    "Polar-Stream_{version}_windows_aarch64.msi",
    "Polar-Stream_{version}_windows_aarch64_setup.exe",
    "Polar-Stream_{version}_windows_x86_64.msi",
    "Polar-Stream_{version}_windows_x86_64_setup.exe",
)
CHECKSUM_NAME = "SHA256SUMS.txt"


class VerificationError(RuntimeError):
    """The candidate asset set is unsafe to publish."""


@dataclass(frozen=True)
class Asset:
    name: str
    size: int
    digest: str | None


def normalize_version(raw_version: str) -> str:
    version = raw_version.removeprefix("v")
    if VERSION_PATTERN.fullmatch(version) is None:
        raise VerificationError(f"invalid release version: {raw_version!r}")
    return version


def expected_package_names(raw_version: str) -> tuple[str, ...]:
    version = normalize_version(raw_version)
    return tuple(template.format(version=version) for template in PACKAGE_TEMPLATES)


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_directory(directory: Path) -> list[Asset]:
    return [
        Asset(path.name, path.stat().st_size, f"sha256:{file_sha256(path)}")
        for path in sorted(directory.iterdir())
        if path.is_file()
    ]


def load_json(path: Path) -> list[Asset]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(payload, dict):
        payload = payload.get("assets")
    if not isinstance(payload, list):
        raise VerificationError("release asset JSON must be a list or contain an assets list")

    assets = []
    for index, item in enumerate(payload):
        if not isinstance(item, dict):
            raise VerificationError(f"release asset JSON entry {index} is not an object")
        name = item.get("name")
        size = item.get("size")
        digest = item.get("digest")
        if not isinstance(name, str) or not name:
            raise VerificationError(f"release asset JSON entry {index} has no valid name")
        if not isinstance(size, int):
            raise VerificationError(f"release asset {name!r} has no integer size")
        if digest is not None and not isinstance(digest, str):
            raise VerificationError(f"release asset {name!r} has an invalid digest")
        assets.append(Asset(name, size, digest))
    return assets


def indexed_assets(assets: list[Asset], expected_names: set[str]) -> dict[str, Asset]:
    names = [asset.name for asset in assets]
    duplicates = sorted(name for name, count in Counter(names).items() if count > 1)
    if duplicates:
        raise VerificationError("duplicate release assets: " + ", ".join(duplicates))

    actual_names = set(names)
    missing = sorted(expected_names - actual_names)
    unexpected = sorted(actual_names - expected_names)
    if missing or unexpected:
        details = []
        if missing:
            details.append("missing: " + ", ".join(missing))
        if unexpected:
            details.append("unexpected: " + ", ".join(unexpected))
        raise VerificationError("release asset set mismatch; " + "; ".join(details))

    empty = sorted(asset.name for asset in assets if asset.size <= 0)
    if empty:
        raise VerificationError("empty release assets: " + ", ".join(empty))
    return {asset.name: asset for asset in assets}


def parse_checksums(path: Path, package_names: set[str]) -> dict[str, str]:
    if path.name != CHECKSUM_NAME or not path.is_file():
        raise VerificationError(f"checksum manifest must be an existing {CHECKSUM_NAME}")
    entries: dict[str, str] = {}
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        match = CHECKSUM_LINE.fullmatch(line)
        if match is None:
            raise VerificationError(f"invalid checksum line {line_number}: {line!r}")
        digest, name = match.groups()
        if name in entries:
            raise VerificationError(f"duplicate checksum entry: {name}")
        entries[name] = digest.lower()

    actual_names = set(entries)
    missing = sorted(package_names - actual_names)
    unexpected = sorted(actual_names - package_names)
    if missing or unexpected:
        details = []
        if missing:
            details.append("missing: " + ", ".join(missing))
        if unexpected:
            details.append("unexpected: " + ", ".join(unexpected))
        raise VerificationError("checksum manifest mismatch; " + "; ".join(details))
    return entries


def verify_local_hashes(
    assets: dict[str, Asset], checksums: dict[str, str], package_names: set[str]
) -> None:
    mismatches = sorted(
        name
        for name in package_names
        if assets[name].digest != f"sha256:{checksums[name]}"
    )
    if mismatches:
        raise VerificationError("local checksum mismatch: " + ", ".join(mismatches))


def verify_remote_assets(
    remote: dict[str, Asset], reference_directory: Path, expected_names: set[str]
) -> None:
    reference = indexed_assets(load_directory(reference_directory), expected_names)
    mismatches = []
    for name in sorted(expected_names):
        remote_asset = remote[name]
        local_asset = reference[name]
        if remote_asset.size != local_asset.size or remote_asset.digest != local_asset.digest:
            mismatches.append(name)
    if mismatches:
        raise VerificationError(
            "uploaded asset size/digest mismatch: " + ", ".join(mismatches)
        )


def verify_release_assets(
    source: Path,
    raw_version: str,
    checksum_path: Path,
    reference_directory: Path | None = None,
) -> None:
    package_names = set(expected_package_names(raw_version))
    checksums = parse_checksums(checksum_path, package_names)
    expected_names = package_names | {CHECKSUM_NAME}

    if source.is_dir():
        assets = indexed_assets(load_directory(source), expected_names)
        verify_local_hashes(assets, checksums, package_names)
        checksum_asset = assets[CHECKSUM_NAME]
        if checksum_asset.digest != f"sha256:{file_sha256(checksum_path)}":
            raise VerificationError("the staged checksum manifest is not the verified manifest")
    else:
        if reference_directory is None:
            raise VerificationError("remote JSON verification requires --reference-directory")
        assets = indexed_assets(load_json(source), expected_names)
        verify_remote_assets(assets, reference_directory, expected_names)
        for name in package_names:
            if assets[name].digest != f"sha256:{checksums[name]}":
                raise VerificationError(f"uploaded checksum mismatch: {name}")

    print(
        f"Verified {len(package_names)} exact packages and {CHECKSUM_NAME} "
        f"for v{normalize_version(raw_version)}."
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--checksums", type=Path, required=True)
    parser.add_argument("--reference-directory", type=Path)
    args = parser.parse_args()
    try:
        verify_release_assets(
            args.source,
            args.version,
            args.checksums,
            args.reference_directory,
        )
    except (OSError, ValueError, VerificationError) as error:
        raise SystemExit(f"Release remains a draft: {error}") from error


if __name__ == "__main__":
    main()
