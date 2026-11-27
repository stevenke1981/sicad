"""Generated ValidationReport (scripts/gen_pydantic.py) against the REAL promptcad-core.

Core-dependent tests skip when the binary is missing; with PROMPTCAD_CORE_BIN set to a
missing file they fail instead (same rule as tests/test_core_api.py).
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
from enum import StrEnum
from pathlib import Path

import pytest
from pydantic import ValidationError

from app.core.config import Settings
from app.models.generated.validation_report import (
    Layer,
    Role,
    Severity,
    ValidationIssue,
    ValidationReport,
)
from app.services.core_bridge import find_core_binary

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "gen_pydantic.py"
EXAMPLES = sorted((ROOT / "examples" / "cad2").glob("*.cad2.json"))
V1_EXAMPLE = ROOT / "examples" / "generated" / "plate-four-holes" / "spec.json"
PARITY_CORPUS = ROOT / "crates" / "promptcad-core" / "tests" / "fixtures" / "v1_parity.json"


def _load_script():
    spec = importlib.util.spec_from_file_location("gen_pydantic", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules["gen_pydantic"] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="module")
def core_binary() -> Path:
    binary = find_core_binary(Settings(env="test"))
    env_value = os.environ.get("PROMPTCAD_CORE_BIN")
    if env_value and binary is None:
        pytest.fail(f"PROMPTCAD_CORE_BIN={env_value!r} does not point to a file")
    if binary is None:
        pytest.skip(
            "promptcad-core binary not found (cargo build --release --locked --bins in "
            "crates/promptcad-core, or set PROMPTCAD_CORE_BIN)"
        )
    return binary


def core_report(binary: Path, document: dict) -> dict:
    request = json.dumps({"op": "validate", "document": document})
    proc = subprocess.run(
        [str(binary), "rpc"],
        input=request,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=60,
        check=True,
    )
    response = json.loads(proc.stdout.strip().splitlines()[-1])
    assert response["status"] == "ok", response
    return response["result"]["report"]


def _load(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def _roundtrip(report: dict) -> None:
    parsed = ValidationReport.model_validate(report)
    assert parsed.model_dump(mode="json") == report


def test_enums_are_real_str_enums():
    for enum in (Severity, Role, Layer):
        assert issubclass(enum, StrEnum)
    assert Severity.error.value == "error"
    assert {s.value for s in Severity} == {"error", "warning", "info"}


@pytest.mark.parametrize("path", EXAMPLES, ids=lambda p: p.name)
def test_core_report_for_cad2_examples_roundtrips(core_binary, path):
    document = _load(path)
    document.pop("expected_resolved", None)
    _roundtrip(core_report(core_binary, document))


def test_core_report_for_v1_spec_roundtrips(core_binary):
    _roundtrip(core_report(core_binary, _load(V1_EXAMPLE)))


def test_core_report_with_issues_roundtrips(core_binary):
    seen = 0
    for case in _load(PARITY_CORPUS):
        report = core_report(core_binary, case["spec"])
        if report["issues"]:
            _roundtrip(report)
            parsed = ValidationReport.model_validate(report)
            assert all(isinstance(i.severity, Severity) for i in parsed.issues)
            seen += 1
        if seen >= 8:
            break
    assert seen, "corpus produced no issues to round-trip"


def test_legacy_four_field_issue_parses():
    # Old manifest.json issues carry only severity/code/message/feature_index.
    issue = ValidationIssue.model_validate(
        {"severity": "warning", "code": "hole_near_edge", "message": "m", "feature_index": 2}
    )
    assert issue.severity is Severity.warning
    assert issue.target is None
    assert issue.related == []
    assert issue.owner is None
    assert issue.layer is Layer.analytic
    report = ValidationReport.model_validate(
        {
            "valid": True,
            "review_required": True,
            "issues": [{"severity": "info", "code": "x", "message": "m", "feature_index": None}],
        }
    )
    assert report.issues[0].severity is Severity.info


def test_extra_fields_forbidden():
    with pytest.raises(ValidationError):
        ValidationReport.model_validate({"valid": True, "review_required": False, "bogus": 1})
    with pytest.raises(ValidationError):
        ValidationIssue.model_validate(
            {"severity": "error", "code": "c", "message": "m", "bogus": 1}
        )


def test_generator_is_deterministic_and_up_to_date():
    module = _load_script()
    first = module.outputs()
    assert first == module.outputs()
    for path, data in first.items():
        assert path.read_bytes() == data, f"{path} is stale; run scripts/gen_pydantic.py"
        assert b"\r" not in data


def test_check_fails_on_hand_edit():
    target = ROOT / "app" / "models" / "generated" / "validation_report.py"
    original = target.read_bytes()
    try:
        target.write_bytes(original + b"# hand edit\n")
        proc = subprocess.run(
            [sys.executable, str(SCRIPT), "--check"], capture_output=True, text=True, check=False
        )
        assert proc.returncode == 1
        assert "out of date" in proc.stderr
    finally:
        target.write_bytes(original)
    ok = subprocess.run([sys.executable, str(SCRIPT), "--check"], capture_output=True, check=False)
    assert ok.returncode == 0


@pytest.mark.parametrize("path", EXAMPLES, ids=lambda p: p.name)
def test_cad_document_2_roundtrips_examples(path):
    from app.models.generated.cad_document_2 import CadDocument2

    document = _load(path)
    document.pop("expected_resolved", None)  # example-only key
    parsed = CadDocument2.model_validate(document)
    assert parsed.model_dump(mode="json", exclude_unset=True) == document
    document["components"][0]["shape"]["kind"] = "bogus"
    with pytest.raises(ValidationError):
        CadDocument2.model_validate(document)
