#!/usr/bin/env python3
"""Run bounded native GDX-RB qualification and retain identifier-free evidence."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
MARKERS = {
    "POLAR_GDX_VERIFY_SELECTED": "selection",
    "POLAR_GDX_VERIFY_STATUS": "status_updates",
    "POLAR_GDX_VERIFY_COMPLETE": "completion",
    "POLAR_GDX_VERIFY_FAILED": "failure",
}


def parse_markers(output: str) -> dict[str, Any]:
    parsed: dict[str, Any] = {"status_updates": []}
    for line in output.splitlines():
        for marker, field in MARKERS.items():
            prefix = f"{marker} "
            if not line.startswith(prefix):
                continue
            value = json.loads(line[len(prefix) :])
            if field == "status_updates":
                parsed[field].append(value)
            else:
                parsed[field] = value
            break
    return parsed


def build_evidence(
    parsed: dict[str, Any], returncode: int, generated_at: str
) -> dict[str, Any]:
    passed = returncode == 0 and "completion" in parsed
    evidence: dict[str, Any] = {
        "schema": "polar.stream.gdx_rb_native_runner.v2",
        "result": "pass" if passed else "fail",
        "generated_at_utc": generated_at,
        "command": "cargo run -p polar-stream --example verify_gdx_rb",
        "returncode": returncode,
        "identity_retained": False,
        "selection": parsed.get("selection"),
        "status_updates": parsed.get("status_updates", []),
    }
    if passed:
        evidence["completion"] = parsed["completion"]
    else:
        evidence["failure"] = parsed.get(
            "failure",
            {
                "schema": "polar.stream.gdx_rb_native_physical.v2",
                "result": "fail",
                "code": "RUNNER_OR_BUILD_FAILED",
            },
        )
    return evidence


def cargo_executable() -> str:
    discovered = shutil.which("cargo")
    if discovered:
        return discovered
    fallback = (
        Path.home()
        / ".rustup"
        / "toolchains"
        / "1.94.1-x86_64-pc-windows-msvc"
        / "bin"
        / "cargo.exe"
    )
    if fallback.is_file():
        return str(fallback)
    raise RuntimeError("cargo executable was not found")


def main() -> int:
    generated = datetime.now(timezone.utc)
    output_dir = ROOT / "artifacts" / "physical-gdx" / generated.strftime(
        "%Y%m%dT%H%M%SZ"
    )
    output_dir.mkdir(parents=True, exist_ok=False)

    command = [
        cargo_executable(),
        "run",
        "-p",
        "polar-stream",
        "--example",
        "verify_gdx_rb",
    ]
    environment = os.environ.copy()
    toolchain_bin = str(Path(command[0]).parent)
    environment["PATH"] = toolchain_bin + os.pathsep + environment.get("PATH", "")

    try:
        completed = subprocess.run(
            command,
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            timeout=180,
            check=False,
        )
        combined = "\n".join((completed.stdout, completed.stderr))
        parsed = parse_markers(combined)
        returncode = completed.returncode
        for line in completed.stdout.splitlines():
            if any(line.startswith(f"{marker} ") for marker in MARKERS):
                print(line)
        if completed.stderr:
            print(completed.stderr, file=sys.stderr, end="")
    except subprocess.TimeoutExpired as error:
        partial = "\n".join(
            value.decode(errors="replace") if isinstance(value, bytes) else value or ""
            for value in (error.stdout, error.stderr)
        )
        parsed = parse_markers(partial)
        parsed["failure"] = {
            "schema": "polar.stream.gdx_rb_native_physical.v2",
            "result": "fail",
            "code": "RUNNER_TIMEOUT",
        }
        returncode = 124

    evidence = build_evidence(parsed, returncode, generated.isoformat())
    evidence_path = output_dir / "native-gdx-verification.json"
    evidence_path.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
    print(f"Identifier-free evidence: {evidence_path}")
    return 0 if evidence["result"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
