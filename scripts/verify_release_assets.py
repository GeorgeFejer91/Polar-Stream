#!/usr/bin/env python3
"""Refuse to publish a release unless every supported PC package exists."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re


REQUIRED = {
    "Windows x64 installer": r"(?:windows|win32).*(?:x86_64|x64|amd64).*\.exe$",
    "Windows ARM64 installer": r"(?:windows|win32).*(?:aarch64|arm64).*\.exe$",
    "Windows x64 MSI": r"(?:windows|win32).*(?:x86_64|x64|amd64).*\.msi$",
    "Windows ARM64 MSI": r"(?:windows|win32).*(?:aarch64|arm64).*\.msi$",
    "macOS universal DMG": r"(?:macos|darwin).*universal.*\.dmg$",
    "Linux x64 AppImage": r"linux.*(?:x86_64|x64|amd64).*\.appimage$",
    "Linux ARM64 AppImage": r"linux.*(?:aarch64|arm64).*\.appimage$",
    "Linux x64 DEB": r"linux.*(?:x86_64|x64|amd64).*\.deb$",
    "Linux ARM64 DEB": r"linux.*(?:aarch64|arm64).*\.deb$",
}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("asset_json", type=Path)
    args = parser.parse_args()
    payload = json.loads(args.asset_json.read_text(encoding="utf-8"))
    names = [asset["name"].lower() for asset in payload]
    missing = [label for label, pattern in REQUIRED.items() if not any(re.search(pattern, name) for name in names)]
    if missing:
        raise SystemExit("Release remains a draft; missing: " + ", ".join(missing) + f". Assets: {names}")
    print(f"Verified {len(REQUIRED)} required package classes across {len(names)} release assets.")


if __name__ == "__main__":
    main()
