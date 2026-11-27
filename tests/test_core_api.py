"""POST /api/v1/core against the REAL promptcad-core binary.

Skipped (with a reason) when the binary cannot be found; CI builds it and exports
PROMPTCAD_CORE_BIN, in which case a missing file is a hard failure, not a skip.
Build locally: ``cargo build --release --locked --bins`` in crates/promptcad-core.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from app.core.config import Settings
from app.main import create_app
from app.models.cad import CadDocument
from app.services.core_bridge import find_core_binary
from app.services.validator import DesignValidator

ROOT = Path(__file__).resolve().parents[1]
EXAMPLES = sorted((ROOT / "examples" / "cad2").glob("*.cad2.json"))
V1_EXAMPLE = ROOT / "examples" / "generated" / "plate-four-holes" / "spec.json"
PARITY_CORPUS = ROOT / "crates" / "promptcad-core" / "tests" / "fixtures" / "v1_parity.json"


@pytest.fixture(scope="module")
def client(tmp_path_factory):
    settings = Settings(
        env="test",
        data_dir=tmp_path_factory.mktemp("core-generated"),
        planner_mode="rule",
        render_backend="source_only",
    )
    binary = find_core_binary(settings)
    env_value = os.environ.get("PROMPTCAD_CORE_BIN")
    if env_value and binary is None:
        pytest.fail(f"PROMPTCAD_CORE_BIN={env_value!r} does not point to a file")
    if binary is None:
        pytest.skip(
            "promptcad-core binary not found (cargo build --release --locked --bins in "
            "crates/promptcad-core, or set PROMPTCAD_CORE_BIN)"
        )
    with TestClient(create_app(settings)) as test_client:
        yield test_client


def load(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def rpc(client: TestClient, request: dict) -> dict:
    response = client.post("/api/v1/core", json=request)
    assert response.status_code == 200, response.text
    return response.json()


def example_document(path: Path) -> dict:
    document = load(path)
    document.pop("expected_resolved", None)  # example-only key, the core does not strip it
    return document


def test_version(client):
    body = rpc(client, {"op": "version"})
    assert body["status"] == "ok"
    assert body["result"]["op"] == "version"
    assert body["result"]["schema_versions"] == ["1.0", "2.0"]


def test_capabilities_reports_core(client):
    caps = client.get("/api/v1/capabilities").json()
    assert caps["core_available"] is True
    assert caps["core_api_version"] == 1
    assert caps["core_schema_versions"] == ["1.0", "2.0"]
    assert caps["core_crate_version"]


@pytest.mark.parametrize("path", EXAMPLES, ids=lambda p: p.name)
def test_validate_cad2_examples(client, path):
    body = rpc(client, {"op": "validate", "document": example_document(path)})
    assert body["status"] == "ok", body
    assert body["result"]["report"]["valid"] is True


def test_validate_v1_example_upgrades(client):
    body = rpc(client, {"op": "validate", "document": load(V1_EXAMPLE)})
    assert body["status"] == "ok", body
    assert body["result"]["input_version"] == "1.0"
    assert body["result"]["report"]["valid"] is True
    upgraded = rpc(client, {"op": "upgrade", "document": load(V1_EXAMPLE)})
    assert upgraded["result"]["document"]["schema_version"] == "2.0"


def test_describe(client):
    body = rpc(client, {"op": "describe", "document": example_document(EXAMPLES[0])})
    assert body["status"] == "ok"
    assert body["result"]["op"] == "describe"
    assert body["result"]["input_version"] == "2.0"
    assert body["result"]["parts"]


def _note_patch(document: dict, base_revision: int) -> dict:
    return {
        "agent": "assembler",
        "base_revision": base_revision,
        "rationale": "integration test note",
        "resolves": [],
        "ops": [{"op": "add_note", "text": "hello"}],
    }


def test_apply_patch_accepted_and_stale_revision_rejected(client):
    document = example_document(EXAMPLES[0])
    accepted = rpc(
        client,
        {
            "op": "apply_patch",
            "document": document,
            "patch": _note_patch(document, document["revision"]),
            "origin": "assembler",
        },
    )
    assert accepted["status"] == "ok"
    assert accepted["result"]["outcome"]["status"] == "accepted"

    rejected = rpc(
        client,
        {
            "op": "apply_patch",
            "document": document,
            "patch": _note_patch(document, document["revision"] + 41),
            "origin": "assembler",
        },
    )
    # A rejected patch is a domain result, not a transport or envelope error.
    assert rejected["status"] == "ok"
    assert rejected["result"]["outcome"]["status"] == "rejected"
    assert rejected["result"]["outcome"]["reason"]["code"] == "stale_revision"


def test_malformed_document_is_http_200_status_error(client):
    body = rpc(client, {"op": "validate", "document": {"schema_version": "3.0"}})
    assert body["status"] == "error"
    assert body["code"] == "unsupported_schema_version"
    assert body["path"] == "/document/schema_version"
    unknown = rpc(client, {"op": "does_not_exist"})
    assert unknown["status"] == "error"
    assert unknown["code"] == "bad_request"


def test_oversize_body_is_413_with_real_default_limit(client):
    limit = client.app.state.settings.core_max_request_bytes
    assert limit <= 8 * 1024 * 1024
    big = b'{"op":"validate","document":"' + b"a" * (limit + 1) + b'"}'
    assert client.post("/api/v1/core", content=big).status_code == 413


# ---- parity with the 1.0 Python validator (ADR 0005 differential test) -------------------

# 1.0 codes the core carries over; anything else on an upgraded document is a 2.0-only extra.
CARRIED = {
    "thin_wall", "thin_bracket", "extreme_aspect_ratio", "hole_outside_part",
    "hole_in_inner_void", "hole_intersects_inner_void", "low_edge_clearance",
    "overlapping_holes", "blind_depth_exceeds_material",
    "counterbore_depth_exceeds_material", "countersink_depth_exceeds_material",
    "tapped_hole_without_thread", "thread_annotation_only", "fillet_too_large",
    "chamfer_too_large", "assumption",
}  # fmt: skip

# Justified 2.0-only extras on upgraded 1.0 documents (see validate_parity_v1.rs EXTRAS):
# the 1.0 hole starts where the 1.0 geometry has no material, so the 2.0 cut removes nothing.
ALLOWED_EXTRAS = {
    "ring_hole_in_inner_void": {("empty_cut_no_effect", "warning")},
    "lbracket_z_blind_too_deep": {("empty_cut_no_effect", "warning")},
    "enclosure_z_blind_too_deep": {("empty_cut_no_effect", "warning")},
}

_THREAD_CASES = {"plate_thread_normalized", "plate_thread_invalid"}


def _corpus() -> list[dict]:
    cases = json.loads(PARITY_CORPUS.read_text(encoding="utf-8"))
    # The 1.0 example plus every case of tests/test_validation.py are in the corpus.
    return cases


def _python_pairs(spec: dict) -> set[tuple[str, str]]:
    report = DesignValidator().validate(CadDocument.model_validate(spec))
    return {(issue.code, issue.severity.value) for issue in report.issues}


@pytest.mark.parametrize("case", _corpus(), ids=lambda c: c["name"])
def test_parity_with_python_validator(client, case):
    name = case["name"]
    spec = case["spec"]
    if name in _THREAD_CASES:
        # 2.0 normalizes / drops the thread before validating (spec §10.1); the corpus
        # records the verdict on the normalized document, so compare against that.
        expected = {(i["code"], i["severity"]) for i in case["expected_issues"]}
    else:
        expected = _python_pairs(spec)
        assert expected == {(i["code"], i["severity"]) for i in case["expected_issues"]}

    body = rpc(client, {"op": "validate", "document": spec})
    assert body["status"] == "ok", body
    issues = body["result"]["report"]["issues"]
    carried = {(i["code"], i["severity"]) for i in issues if i["code"] in CARRIED}
    extras = {(i["code"], i["severity"]) for i in issues if i["code"] not in CARRIED}

    assert carried == expected
    assert extras == ALLOWED_EXTRAS.get(name, set())
    assert body["result"]["report"]["valid"] == (not any(sev == "error" for _, sev in expected))
