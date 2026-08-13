#!/usr/bin/env python3
"""Stage the canonical Polar Stream UI as a GitHub Pages artifact."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import tempfile


ROOT = Path(__file__).resolve().parents[1]
UI = ROOT / "apps/polar-stream/ui"
DEFAULT_OUTPUT = ROOT / "artifacts/browser-demo"
REQUIRED_ASSETS = (
    "index.html",
    "styles.css",
    "runtime-api.js",
    "preferences.js",
    "app.js",
    "metric-previews.js",
    "demo-data.js",
    "favicon.png",
)


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate_sources() -> None:
    missing = [name for name in REQUIRED_ASSETS if not (UI / name).is_file()]
    if missing:
        raise SystemExit("Browser demo is missing canonical UI assets: " + ", ".join(missing))
    html = (UI / "index.html").read_text(encoding="utf-8")
    for name in ("styles.css", "runtime-api.js", "preferences.js", "app.js", "favicon.png"):
        if name not in html:
            raise SystemExit(f"Canonical index.html no longer references {name}")
    if "http://" in html or "https://" in html:
        raise SystemExit("Canonical UI must not require remote runtime assets")


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
        shutil.copy2(source, destination)
        hashes[name] = digest(source)
        if digest(destination) != hashes[name]:
            raise SystemExit(f"Staged browser asset differs from canonical source: {name}")
    shutil.copy2(UI / "index.html", output / "404.html")
    (output / ".nojekyll").write_text("", encoding="utf-8")
    (output / "browser-demo-manifest.json").write_text(
        json.dumps({"schemaVersion": 1, "canonicalSource": "apps/polar-stream/ui", "sha256": hashes}, indent=2) + "\n",
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
        print(f"Validated {len(hashes)} canonical browser-demo assets")
        return
    hashes = stage(arguments.output)
    print(f"Staged {len(hashes)} canonical browser-demo assets in {arguments.output.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
