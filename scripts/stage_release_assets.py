#!/usr/bin/env python3
"""Copy Tauri bundles into a deterministic, verifiable release package set."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
from pathlib import Path
import re
import shutil


EXTENSIONS = {
    "linux": (".appimage", ".deb"),
    "windows": (".exe", ".msi"),
    "macos": (".dmg",),
}
TAURI_APPS = (
    Path("apps/polar-stream/tauri.conf.json"),
    Path("apps/polar-stream-mini/tauri.conf.json"),
    Path("apps/vernier-stream-mini/tauri.conf.json"),
)


@dataclass(frozen=True)
class ReleaseProduct:
    display_name: str
    asset_prefix: str


PRODUCTS = (
    ReleaseProduct("Polar Stream", "Polar-Stream"),
    ReleaseProduct("Polar Stream Mini", "Polar-Stream-Mini"),
    ReleaseProduct("Vernier Stream Mini", "Vernier-Stream-Mini"),
)

VERSION_PATTERN = re.compile(
    r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)


def cargo_workspace_version(path: Path) -> str:
    section = ""
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.split("#", 1)[0].strip()
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1].strip()
            continue
        if section == "workspace.package":
            match = re.fullmatch(r'version\s*=\s*"([^"]+)"', line)
            if match:
                return match.group(1)
    raise SystemExit(f"Cargo workspace version was not found in {path}")


def cargo_lockfile_versions(path: Path) -> dict[str, str]:
    packages: dict[str, str] = {}
    for block in path.read_text(encoding="utf-8").split("[[package]]")[1:]:
        name = re.search(r'^name\s*=\s*"([^"]+)"', block, re.MULTILINE)
        version = re.search(r'^version\s*=\s*"([^"]+)"', block, re.MULTILINE)
        source = re.search(r"^source\s*=", block, re.MULTILINE)
        if name is None or version is None or source is not None:
            continue
        if name.group(1) in packages:
            raise SystemExit(f"Cargo lockfile repeats local package {name.group(1)} in {path}")
        packages[name.group(1)] = version.group(1)
    if not packages:
        raise SystemExit(f"Cargo lockfile has no local workspace packages in {path}")
    return packages


def product_version_pattern(product: ReleaseProduct, version: str) -> re.Pattern[str]:
    product_tokens = [
        re.escape(token) for token in re.split(r"\s+", product.display_name.strip())
    ]
    product_prefix = r"[\s_.-]+".join(product_tokens)
    return re.compile(
        rf"^{product_prefix}[\s_.-]+v?{re.escape(version)}(?:[\s_.-]|$)",
        re.IGNORECASE,
    )


def find_one(bundle_root: Path, extension: str, product: ReleaseProduct, version: str) -> Path:
    product_pattern = product_version_pattern(product, version)
    candidates = sorted(
        path
        for path in bundle_root.rglob("*")
        if path.is_file()
        and path.name.lower().endswith(extension.lower())
        and product_pattern.search(path.name)
    )
    if len(candidates) != 1:
        raise SystemExit(
            f"Expected exactly one {product.display_name} {extension} below "
            f"{bundle_root}, found {len(candidates)}: "
            + ", ".join(str(path) for path in candidates)
        )
    return candidates[0]


def npm_lockfile_versions(path: Path) -> tuple[str, str]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    try:
        return payload["version"], payload["packages"][""]["version"]
    except (KeyError, TypeError) as error:
        raise SystemExit(f"npm lockfile versions were not found in {path}") from error


def release_version(raw_version: str) -> str:
    if not raw_version.startswith("v"):
        raise SystemExit(f"Release tag must start with v: {raw_version}")
    version = raw_version[1:]
    if VERSION_PATTERN.fullmatch(version) is None:
        raise SystemExit(f"Release tag is not a supported semantic version: {raw_version}")
    return version


def verify_repository_versions(root: Path, raw_version: str) -> str:
    version = release_version(raw_version)
    lockfile_version, lockfile_root_version = npm_lockfile_versions(
        root / "package-lock.json"
    )
    configured_versions = {
        "Cargo workspace": cargo_workspace_version(root / "Cargo.toml"),
        "npm tooling": json.loads(
            (root / "package.json").read_text(encoding="utf-8")
        )["version"],
        "npm lockfile": lockfile_version,
        "npm lockfile root": lockfile_root_version,
    }
    configured_versions.update(
        {
            f"Tauri application {path.parent.name}": json.loads(
                (root / path).read_text(encoding="utf-8")
            )["version"]
            for path in TAURI_APPS
        }
    )
    configured_versions.update(
        {
            f"Cargo lockfile package {name}": configured
            for name, configured in cargo_lockfile_versions(root / "Cargo.lock").items()
        }
    )
    mismatches = {
        source: configured
        for source, configured in configured_versions.items()
        if configured != version
    }
    if mismatches:
        details = ", ".join(
            f"{source}={configured}" for source, configured in mismatches.items()
        )
        raise SystemExit(
            f"Release tag {raw_version} does not match repository versions: {details}"
        )
    return version


def stage_release_assets(
    bundle_root: Path,
    output: Path,
    platform: str,
    arch: str,
    version: str,
) -> None:
    output.mkdir(parents=True, exist_ok=True)
    for product in PRODUCTS:
        for extension in EXTENSIONS[platform]:
            source = find_one(bundle_root, extension, product, version)
            setup = "_setup" if platform == "windows" and extension == ".exe" else ""
            destination = output / (
                f"{product.asset_prefix}_{version}_{platform}_{arch}"
                f"{setup}{source.suffix}"
            )
            shutil.copy2(source, destination)
            print(f"Staged {destination.name} from {source}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bundle-root", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--platform", choices=sorted(EXTENSIONS))
    parser.add_argument("--arch")
    parser.add_argument("--version", required=True)
    parser.add_argument(
        "--check-version-only",
        action="store_true",
        help="fail unless the tag exactly matches every repository version surface",
    )
    args = parser.parse_args()

    version = verify_repository_versions(Path.cwd(), args.version)
    if args.check_version_only:
        print(f"Verified exact repository version {args.version}.")
        return

    missing_arguments = [
        name
        for name, value in (
            ("--bundle-root", args.bundle_root),
            ("--output", args.output),
            ("--platform", args.platform),
            ("--arch", args.arch),
        )
        if value is None
    ]
    if missing_arguments:
        parser.error(
            "the following arguments are required unless --check-version-only is used: "
            + ", ".join(missing_arguments)
        )

    stage_release_assets(
        args.bundle_root,
        args.output,
        args.platform,
        args.arch,
        version,
    )


if __name__ == "__main__":
    main()
