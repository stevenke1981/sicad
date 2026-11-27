"""1.0 flows compile through Cad2Compiler on the core's canonical 2.0 document (source_only)."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from app.main import create_app
from app.services import job_service as job_service_module
from app.services.compiler2 import CompileUnsupported

ROOT = Path(__file__).resolve().parents[1]
PROMPT = "長120mm寬60mm厚10mm固定板，四角M6通孔，R5"


@pytest.fixture
def core_client(settings, core_binary):
    app = create_app(settings)
    app.state.core.override_binary(core_binary)
    with TestClient(app) as client:
        yield client


def _generate(client):
    response = client.post(
        "/api/v1/generate",
        json={"prompt": PROMPT, "planner": "rule", "formats": ["step"], "render": True},
    )
    assert response.status_code == 200, response.text
    return response.json()


def _names(body):
    return {item["filename"] for item in body["artifacts"]}


def test_v1_job_model_py_comes_from_cad2_compiler(core_client):
    body = _generate(core_client)
    assert body["status"] == "source_only"
    assert _names(body) == {
        "spec.json",
        "validation.json",
        "model.py",
        "model.scad",
        "preview.svg",
    }
    model = core_client.get(f"/api/v1/jobs/{body['job_id']}/files/model.py").text
    assert "def build()" in model
    assert "CAD_SPEC" not in model
    assert PROMPT not in model
    assert body["spec"]["base"]["kind"] == "plate"
    spec = core_client.get(f"/api/v1/jobs/{body['job_id']}/files/spec.json").json()
    assert spec["schema_version"] == "1.0"


def test_compile_unsupported_on_valid_spec_is_failed(core_client, monkeypatch):
    def refuse(self, document, resolved):
        raise CompileUnsupported([("part 1", "not supported yet")])

    monkeypatch.setattr(job_service_module.Cad2Compiler, "compile", refuse)
    body = _generate(core_client)
    assert body["status"] == "failed"
    assert body["renderer_used"] == "compile-unsupported"
    assert "part 1" in body["error"]
    names = _names(body)
    assert "model.py" not in names
    assert {"model.scad", "preview.svg", "spec.json", "validation.json"} <= names


def test_invalid_spec_is_validation_blocked_with_model_py(core_client):
    spec = json.loads(
        (ROOT / "examples" / "generated" / "plate-four-holes" / "spec.json").read_text(
            encoding="utf-8"
        )
    )
    spec["holes"][0]["diameter"] = 500
    response = core_client.post(
        "/api/v1/generate-from-spec",
        json={"spec": spec, "formats": ["step"], "render": True},
    )
    assert response.status_code == 200, response.text
    body = response.json()
    assert body["status"] == "failed"
    assert body["renderer_used"] == "validation-blocked"
    assert body["validation"]["valid"] is False
    assert "model.py" in _names(body)
