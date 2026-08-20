#!/usr/bin/env python3
"""Verify the exact Polar Stream assets currently served by GitHub Pages."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
from html.parser import HTMLParser
import hashlib
import ipaddress
import json
from pathlib import Path, PurePosixPath
import re
import sys
from urllib.parse import urljoin, urlparse
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[1]
CANONICAL_UI = ROOT / "apps/polar-stream/ui"
DEFAULT_BASE_URL = "https://georgefejer91.github.io/Polar-Stream/"
DEFAULT_OUTPUT_ROOT = ROOT / "artifacts/real-world-pages"
TEXT_ASSET_SUFFIXES = {".cjs", ".css", ".html", ".js", ".json", ".md", ".txt"}
NETWORK_PRIMITIVES = {
    "fetch": re.compile(r"\bfetch\s*\("),
    "WebSocket": re.compile(r"\bWebSocket\s*\("),
    "EventSource": re.compile(r"\bEventSource\s*\("),
    "XMLHttpRequest": re.compile(r"\bXMLHttpRequest\s*\("),
    "WebTransport": re.compile(r"\bWebTransport\s*\("),
    "sendBeacon": re.compile(r"\bsendBeacon\s*\("),
}
LOCAL_PREVIEW_FETCH = re.compile(
    r"\bfetch\s*\(\s*([\"'])data/preview-recording\.json\1\s*,\s*\{\s*cache\s*:\s*([\"'])no-cache\2\s*\}\s*\)"
)
URL_LITERAL = re.compile(r"\b(?:https?|wss?)://[^\s\"'`<>]+", re.IGNORECASE)


class ResourceParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.resources: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        values = dict(attrs)
        if tag == "script" and values.get("src"):
            self.resources.append(values["src"] or "")
        elif tag == "link" and values.get("href"):
            self.resources.append(values["href"] or "")
        elif tag in {"img", "audio", "video", "source"} and values.get("src"):
            self.resources.append(values["src"] or "")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical_asset_bytes(path: Path) -> bytes:
    data = path.read_bytes()
    if path.suffix.lower() in TEXT_ASSET_SUFFIXES:
        return data.replace(b"\r\n", b"\n")
    return data


def normalized_base_url(value: str) -> str:
    return value if value.endswith("/") else f"{value}/"


def assert_public_pages_url(base_url: str) -> None:
    parsed = urlparse(base_url)
    if parsed.scheme != "https":
        raise ValueError("Acceptance provenance must use HTTPS")
    if parsed.hostname != "georgefejer91.github.io" or parsed.path.rstrip("/") != "/Polar-Stream":
        raise ValueError(f"Acceptance provenance must target {DEFAULT_BASE_URL}")
    if parsed.params or parsed.query or parsed.fragment:
        raise ValueError("The Pages base URL must not contain parameters, a query, or a fragment")


def fetch(url: str) -> tuple[bytes, dict[str, object]]:
    request = Request(
        url,
        headers={
            "Accept": "*/*",
            "Cache-Control": "no-cache",
            "User-Agent": "Polar-Stream-Pages-Acceptance/1",
        },
    )
    with urlopen(request, timeout=30) as response:
        data = response.read()
        headers = {key.lower(): value for key, value in response.headers.items()}
        record: dict[str, object] = {
            "requestedUrl": url,
            "finalUrl": response.geturl(),
            "status": response.status,
            "bytes": len(data),
            "sha256": sha256(data),
            "lastModified": headers.get("last-modified"),
            "headers": headers,
        }
        return data, record


def safe_asset_name(name: str) -> bool:
    path = PurePosixPath(name)
    return bool(name) and not path.is_absolute() and ".." not in path.parts and "\\" not in name


def is_private_url_literal(value: str) -> bool:
    hostname = (urlparse(value).hostname or "").lower().rstrip(".")
    if hostname in {"localhost", "localhost.localdomain"} or hostname.endswith(".local"):
        return True
    try:
        address = ipaddress.ip_address(hostname)
    except ValueError:
        return False
    return not address.is_global


def verify(base_url: str) -> dict[str, object]:
    base_url = normalized_base_url(base_url)
    assert_public_pages_url(base_url)
    checks: list[str] = []
    responses: dict[str, dict[str, object]] = {}

    manifest_url = urljoin(base_url, "browser-demo-manifest.json")
    manifest_bytes, manifest_response = fetch(manifest_url)
    responses["browser-demo-manifest.json"] = manifest_response
    manifest = json.loads(manifest_bytes)
    if manifest.get("schemaVersion") != 1:
        raise ValueError("Live manifest schemaVersion is not 1")
    if manifest.get("canonicalSource") != "apps/polar-stream/ui":
        raise ValueError("Live manifest does not identify the canonical UI source")
    declared_hashes = manifest.get("sha256")
    if not isinstance(declared_hashes, dict) or not declared_hashes:
        raise ValueError("Live manifest has no asset hash map")
    if any(not isinstance(name, str) or not safe_asset_name(name) for name in declared_hashes):
        raise ValueError("Live manifest contains an unsafe asset path")
    checks.append(f"manifest declares {len(declared_hashes)} safe assets")

    local_assets = {
        path.relative_to(CANONICAL_UI).as_posix(): sha256(canonical_asset_bytes(path))
        for path in CANONICAL_UI.rglob("*")
        if path.is_file()
    }
    if declared_hashes != local_assets:
        missing_live = sorted(set(local_assets) - set(declared_hashes))
        unexpected_live = sorted(set(declared_hashes) - set(local_assets))
        changed = sorted(
            name for name in set(local_assets) & set(declared_hashes)
            if local_assets[name] != declared_hashes[name]
        )
        raise ValueError(
            "Live manifest differs from this checkout "
            f"(missing={missing_live}, unexpected={unexpected_live}, changed={changed})"
        )
    checks.append("live manifest hashes match the canonical UI in this checkout")

    asset_bytes: dict[str, bytes] = {}
    for name, expected_hash in sorted(declared_hashes.items()):
        data, response = fetch(urljoin(base_url, name))
        responses[name] = response
        if response["status"] != 200:
            raise ValueError(f"Live asset did not return HTTP 200: {name}")
        if response["sha256"] != expected_hash:
            raise ValueError(f"Live asset hash differs from its manifest entry: {name}")
        if urlparse(str(response["finalUrl"])).scheme != "https":
            raise ValueError(f"Live asset left HTTPS: {name}")
        asset_bytes[name] = data
    checks.append("every live asset returned HTTP 200 over HTTPS and matched SHA-256")

    index = asset_bytes.get("index.html", b"").decode("utf-8")
    parser = ResourceParser()
    parser.feed(index)
    for resource in parser.resources:
        resolved = urlparse(urljoin(base_url, resource))
        base = urlparse(base_url)
        if resolved.scheme != "https" or resolved.netloc != base.netloc:
            raise ValueError(f"index.html references a non-Pages runtime resource: {resource}")
        relative = resolved.path.removeprefix(base.path)
        if relative not in declared_hashes:
            raise ValueError(f"index.html references an asset absent from the manifest: {resource}")
    checks.append("index.html runtime resources are same-origin manifest assets")

    javascript = "\n".join(
        data.decode("utf-8") for name, data in asset_bytes.items() if name.endswith(".js")
    )
    # Hardware-free preview playback reads one manifest-hashed, same-origin
    # fixture. Remove only that exact expression before rejecting every other
    # browser network primitive; a URL variable or any different fetch remains
    # forbidden and therefore cannot smuggle in a relay/acquisition backend.
    acquisition_javascript = LOCAL_PREVIEW_FETCH.sub("loadLocalPreviewFixture()", javascript)
    forbidden = [
        name for name, pattern in NETWORK_PRIMITIVES.items()
        if pattern.search(acquisition_javascript)
    ]
    if forbidden:
        raise ValueError("Deployed JavaScript contains a remote acquisition primitive: " + ", ".join(forbidden))
    private_literals = sorted({value for value in URL_LITERAL.findall(javascript) if is_private_url_literal(value)})
    if private_literals:
        raise ValueError("Deployed JavaScript contains a loopback/private URL: " + ", ".join(private_literals))
    checks.append(
        "deployed JavaScript has no remote HTTP, WebSocket, SSE, or private-network acquisition path"
    )

    required_contracts = {
        "direct Web Bluetooth chooser": "navigator.bluetooth.requestDevice",
        "Web Bluetooth source provenance": 'transport: "web-bluetooth"',
        "browser CSV provenance": "# input_kind",
        "native-only output suppression": (
            "LSL and OSC outputs are available only in the installed Polar Stream app."
        ),
    }
    missing_contracts = [label for label, marker in required_contracts.items() if marker not in javascript]
    if missing_contracts:
        raise ValueError("Live assets are missing browser acceptance contracts: " + ", ".join(missing_contracts))
    checks.extend(required_contracts)

    server = str(manifest_response["headers"].get("server", ""))
    if "github.com" not in server.lower():
        raise ValueError(f"Manifest response was not identified as GitHub Pages (server={server!r})")
    if "strict-transport-security" not in manifest_response["headers"]:
        raise ValueError("Manifest response omitted Strict-Transport-Security")
    checks.append("GitHub Pages server and HSTS response headers are present")

    return {
        "schemaVersion": 1,
        "result": "pass",
        "retrievedAtUtc": datetime.now(timezone.utc).isoformat(),
        "baseUrl": base_url,
        "manifest": manifest,
        "checks": checks,
        "responses": responses,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default=DEFAULT_BASE_URL, help="exact public Pages URL")
    parser.add_argument("--output", type=Path, help="JSON evidence path")
    arguments = parser.parse_args()
    try:
        report = verify(arguments.url)
    except Exception as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1

    retrieved = datetime.fromisoformat(str(report["retrievedAtUtc"]))
    output = arguments.output or DEFAULT_OUTPUT_ROOT / retrieved.strftime("%Y%m%dT%H%M%SZ") / "provenance.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    manifest_response = report["responses"]["browser-demo-manifest.json"]
    print(f"PASS: verified {len(report['manifest']['sha256'])} live GitHub Pages assets")
    print(f"Manifest last modified: {manifest_response['lastModified']}")
    print(f"Evidence: {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
