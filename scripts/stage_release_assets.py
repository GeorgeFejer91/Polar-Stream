#!/usr/bin/env python3
"""Copy Tauri bundles into a deterministic, verifiable release package set."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import tomllib


EXTENSIONS = {
    "linux": (".appimage", ".deb"),
    "windows": (".exe", ".msi"),
    "macos": (".dmg",),
}


def find_one(bundle_root: Path, extension: str) -> Path:
    candidates = sorted(
        path for path in bundle_root.rglob("*")
        if path.is_file() and path.name.lower().endswith(extension)
    )
    if len(candidates) != 1:
        raise SystemExit(
            f"Expected exactly one {extension} below {bundle_root}, found {len(candidates)}: "
            + ", ".join(str(path) for path in candidates)
        )
    return candidates[0]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bundle-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--platform", choices=sorted(EXTENSIONS), required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()

    version = args.version.removeprefix("v")
    configured_versions = {
        "Cargo workspace": tomllib.loads(
            Path("Cargo.toml").read_text(encoding="utf-8")
        )["workspace"]["package"]["version"],
        "Tauri application": json.loads(
            Path("apps/polar-stream/tauri.conf.json").read_text(encoding="utf-8")
        )["version"],
        "npm tooling": json.loads(
            Path("package.json").read_text(encoding="utf-8")
        )["version"],
    }
    mismatches = {
        source: configured
        for source, configured in configured_versions.items()
        if configured != version
    }
    if mismatches:
        details = ", ".join(
            f"{source}={configured}" for source, configured in mismatches.items()
        )
        raise SystemExit(f"Release tag {args.version} does not match repository versions: {details}")

    args.output.mkdir(parents=True, exist_ok=True)
    for extension in EXTENSIONS[args.platform]:
        source = find_one(args.bundle_root, extension)
        setup = "_setup" if args.platform == "windows" and extension == ".exe" else ""
        destination = args.output / (
            f"Polar-Stream_{version}_{args.platform}_{args.arch}{setup}{source.suffix}"
        )
        shutil.copy2(source, destination)
        print(f"Staged {destination.name} from {source}")


if __name__ == "__main__":
    main()
