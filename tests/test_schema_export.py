from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "export_schema.py"
COMMITTED = ROOT / "schemas" / "promptcad.schema.json"

EXPECTED_MODELS = {
    "CadDocument",
    "ValidationReport",
    "PlanRequest",
    "GenerateRequest",
    "GenerateFromSpecRequest",
    "PlanResponse",
    "Artifact",
    "JobManifest",
    "JobListItem",
    "CapabilityResponse",
}


def _run(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=120,
    )


def test_export_is_deterministic(tmp_path):
    first = tmp_path / "a.json"
    second = tmp_path / "b.json"
    assert _run("--output", str(first)).returncode == 0
    assert _run("--output", str(second)).returncode == 0
    data = first.read_bytes()
    assert data == second.read_bytes()
    assert data.endswith(b"\n") and not data.endswith(b"\n\n")
    assert b"\r\n" not in data


def test_schema_contents(tmp_path):
    out = tmp_path / "schema.json"
    assert _run("--output", str(out)).returncode == 0
    schema = json.loads(out.read_text(encoding="utf-8"))
    defs = schema["$defs"]
    assert set(defs) >= EXPECTED_MODELS
    assert defs["CadDocument"]["additionalProperties"] is False
    refs = [line for line in out.read_text(encoding="utf-8").splitlines() if '"$ref"' in line]
    assert refs and all('"#/$defs/' in line for line in refs)
    # Keys are sorted at every level.
    assert list(schema) == sorted(schema)
    assert list(defs) == sorted(defs)


def test_check_detects_drift(tmp_path):
    out = tmp_path / "schema.json"
    assert _run("--output", str(out)).returncode == 0
    assert _run("--check", "--output", str(out)).returncode == 0

    tampered = json.loads(out.read_text(encoding="utf-8"))
    tampered["$defs"]["CadDocument"]["additionalProperties"] = True
    out.write_text(json.dumps(tampered, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    result = _run("--check", "--output", str(out))
    assert result.returncode == 1
    assert "out of date" in result.stderr

    missing = tmp_path / "missing.json"
    assert _run("--check", "--output", str(missing)).returncode == 1


def test_committed_schema_is_current():
    result = _run("--check", "--output", str(COMMITTED))
    assert result.returncode == 0, (
        "schemas/promptcad.schema.json is stale; run `python scripts/export_schema.py`"
    )
