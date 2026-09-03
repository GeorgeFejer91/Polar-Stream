#!/usr/bin/env python3
"""Stage the canonical Polar Stream UI as a GitHub Pages artifact."""

from __future__ import annotations

import argparse
import hashlib
from html.parser import HTMLParser
import json
from pathlib import Path
import shutil
import tempfile


ROOT = Path(__file__).resolve().parents[1]
UI = ROOT / "apps/polar-stream/ui"
DOWNLOAD = ROOT / "download"
DEFAULT_OUTPUT = ROOT / "artifacts/browser-demo"
REQUIRED_ASSETS = (
    "index.html",
    "styles.css",
    "polar-web-bluetooth.js",
    "vernier-web-bluetooth.js",
    "browser-session.js",
    "audio-data-link.js",
    "metric-catalog.js",
    "preview-fixture.js",
    "formula-preview.js",
    "runtime-api.js",
    "preferences.js",
    "app.js",
    "metric-previews.js",
    "data/preview-recording.json",
    "favicon.png",
)
REQUIRED_DOWNLOAD_ASSETS = ("index.html", "styles.css", "downloads.js", "favicon.png")
TEXT_ASSET_SUFFIXES = {".cjs", ".css", ".html", ".js", ".json", ".md", ".txt"}


class RuntimeResourceParser(HTMLParser):
    RESOURCE_ATTRIBUTES = {
        "audio": ("src",),
        "embed": ("src",),
        "iframe": ("src",),
        "img": ("src", "srcset"),
        "link": ("href",),
        "object": ("data",),
        "script": ("src",),
        "source": ("src", "srcset"),
        "video": ("poster", "src"),
    }

    def __init__(self) -> None:
        super().__init__()
        self.remote_urls: list[str] = []

    def handle_starttag(
        self, tag: str, attrs: list[tuple[str, str | None]]
    ) -> None:
        resource_attributes = self.RESOURCE_ATTRIBUTES.get(tag, ())
        for name, value in attrs:
            if name not in resource_attributes or value is None:
                continue
            normalized = value.strip().lower()
            if (
                "http://" in normalized
                or "https://" in normalized
                or normalized.startswith("//")
            ):
                self.remote_urls.append(value)

    handle_startendtag = handle_starttag


def canonical_asset_bytes(path: Path) -> bytes:
    data = path.read_bytes()
    if path.suffix.lower() in TEXT_ASSET_SUFFIXES:
        return data.replace(b"\r\n", b"\n")
    return data


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def validate_sources() -> None:
    missing = [name for name in REQUIRED_ASSETS if not (UI / name).is_file()]
    if missing:
        raise SystemExit("Browser demo is missing canonical UI assets: " + ", ".join(missing))
    download_names = {
        path.relative_to(DOWNLOAD).as_posix()
        for path in DOWNLOAD.rglob("*")
        if path.is_file()
    }
    required_download_names = set(REQUIRED_DOWNLOAD_ASSETS)
    missing_download = sorted(required_download_names - download_names)
    if missing_download:
        raise SystemExit("Download page is missing required assets: " + ", ".join(missing_download))
    unexpected_download = sorted(download_names - required_download_names)
    if unexpected_download:
        raise SystemExit(
            "Download page contains unexpected assets: " + ", ".join(unexpected_download)
        )
    if any(path.is_symlink() for path in DOWNLOAD.rglob("*")):
        raise SystemExit("Download page contains a symbolic link; Pages artifacts must contain regular files only")
    html = (UI / "index.html").read_text(encoding="utf-8")
    for name in (
        "styles.css",
        "polar-web-bluetooth.js",
        "vernier-web-bluetooth.js",
        "browser-session.js",
        "audio-data-link.js",
        "metric-catalog.js",
        "metric-previews.js",
        "preview-fixture.js",
        "formula-preview.js",
        "runtime-api.js",
        "preferences.js",
        "app.js",
        "favicon.png",
    ):
        if name not in html:
            raise SystemExit(f"Canonical index.html no longer references {name}")
    resource_parser = RuntimeResourceParser()
    resource_parser.feed(html)
    if resource_parser.remote_urls:
        raise SystemExit(
            "Canonical UI must not require remote runtime assets: "
            + ", ".join(resource_parser.remote_urls)
        )


def stage(output: Path) -> dict[str, str]:
    validate_sources()
    if output.exists():
        shutil.rmtree(output)
    output.mkdir(parents=True)
    hashes: dict[str, str] = {}
    sources = sorted(path for path in UI.rglob("*") if path.is_file())
    if any(path.is_symlink() for path in sources):
        raise SystemExit("Canonical UI contains a symbolic link; Pages artifacts must contain regular files only")
    for source in sources:
        name = source.relative_to(UI).as_posix()
        destination = output / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        data = canonical_asset_bytes(source)
        destination.write_bytes(data)
        hashes[name] = digest(data)
        if digest(destination.read_bytes()) != hashes[name]:
            raise SystemExit(f"Staged browser asset differs from canonical source: {name}")
    (output / "404.html").write_bytes((output / "index.html").read_bytes())
    download_hashes: dict[str, str] = {}
    for name in REQUIRED_DOWNLOAD_ASSETS:
        source = DOWNLOAD / name
        destination = output / "download" / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        data = canonical_asset_bytes(source)
        destination.write_bytes(data)
        download_hashes[name] = digest(data)
        if digest(destination.read_bytes()) != download_hashes[name]:
            raise SystemExit(f"Staged download asset differs from canonical source: {name}")
    (output / ".nojekyll").write_text("", encoding="utf-8")
    (output / "browser-demo-manifest.json").write_text(
        json.dumps(
            {
                "schemaVersion": 2,
                "canonicalSource": "apps/polar-stream/ui",
                "sha256": hashes,
                "canonicalDownloadSource": "download",
                "downloadSha256": download_hashes,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    return hashes


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--check", action="store_true", help="stage in a temporary directory and verify parity")
    arguments = parser.parse_args()
    if arguments.check:
        with tempfile.TemporaryDirectory(prefix="polar-stream-browser-demo-") as temporary:
            hashes = stage(Path(temporary))
        print(
            f"Validated {len(hashes)} canonical browser-demo assets and "
            f"{len(REQUIRED_DOWNLOAD_ASSETS)} download assets"
        )
        return
    hashes = stage(arguments.output)
    print(
        f"Staged {len(hashes)} canonical browser-demo assets and "
        f"{len(REQUIRED_DOWNLOAD_ASSETS)} download assets in "
        f"{arguments.output.relative_to(ROOT)}"
    )


if __name__ == "__main__":
    main()
