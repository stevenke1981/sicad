"""POST /generate-from-document and the CLI route for CadDocument 2.0 (source_only, real core)."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from app.cli import main_args
from app.main import create_app
from app.services import job_service as job_service_module
from app.services.compiler2 import CompileUnsupported

CAD2 = Path(__file__).resolve().parents[1] / "examples" / "cad2"
URL = "/api/v1/generate-from-document"


def load(name: str) -> dict:
    document = json.loads((CAD2 / f"{name}.cad2.json").read_text(encoding="utf-8"))
    document.pop("expected_resolved")
    return document


@pytest.fixture
def plate() -> dict:
    return load("plate-four-holes")


@pytest.fixture
def core_client(settings, core_binary):
    app = create_app(settings)
    app.state.core.override_binary(core_binary)
    with TestClient(app) as client:
        yield client


def post(client, document, **extra):
    return client.post(
        URL, json={"document": document, "formats": ["step", "stl", "py"], "render": True, **extra}
    )


@pytest.mark.parametrize("name", ["plate-four-holes", "enclosure-with-lid"])
def test_examples_generate_source_only(core_client, name):
    document = load(name)
    response = post(core_client, document)
    assert response.status_code == 200, response.text
    body = response.json()
    assert body["status"] == "source_only"
    assert body["renderer_used"] == "source_only"
    assert body["spec_version"] == "2.0"
    assert body["spec"] is None
    assert body["document"]["schema_version"] == "2.0"
    assert body["planner_used"] == "manual-dsl"
    assert body["prompt"] == document["source_prompt"]
    names = {item["filename"] for item in body["artifacts"]}
    assert {"spec.json", "validation.json", "resolved.json", "model.py"} <= names
    assert not names & {"model.scad", "preview.svg"}
    assert any("OpenSCAD" in warning for warning in body["warnings"])

    job_id = body["job_id"]
    assert core_client.get(f"/api/v1/jobs/{job_id}").json()["document"]["name"] == document["name"]
    listing = core_client.get("/api/v1/jobs").json()
    assert [item["name"] for item in listing if item["job_id"] == job_id] == [document["name"]]
    model = core_client.get(f"/api/v1/jobs/{job_id}/files/model.py")
    assert model.status_code == 200 and "def build" in model.text


def test_render_false_writes_source_only(core_client, plate):
    body = post(core_client, plate, render=False).json()
    assert body["renderer_used"] == "not-run"
    assert any("render=false" in warning for warning in body["warnings"])


def test_unknown_field_is_422_with_core_code(core_client, plate):
    plate["bogus"] = 1
    response = post(core_client, plate)
    assert response.status_code == 422
    assert "invalid_document" in response.json()["detail"]
    assert core_client.get("/api/v1/jobs").json() == []


def test_unsupported_schema_version_is_422(core_client, plate):
    plate["schema_version"] = "9"
    response = post(core_client, plate)
    assert response.status_code == 422
    assert "2.0" in response.json()["detail"]


def test_1_0_document_is_422(core_client):
    spec = json.loads(
        (Path(__file__).resolve().parents[1] / "examples" / "generated" / "plate-four-holes"
         / "spec.json").read_text(encoding="utf-8")
    )
    response = post(core_client, spec)
    assert response.status_code == 422
    assert "2.0" in response.json()["detail"]


def test_design_error_is_failed_validation_blocked(core_client, plate):
    for component in plate["components"]:
        if component["id"] == "e_h1":
            component["shape"]["diameter"] = 500
    body = post(core_client, plate).json()
    assert body["status"] == "failed"
    assert body["renderer_used"] == "validation-blocked"
    assert body["error"]
    assert body["validation"]["valid"] is False
    names = {item["filename"] for item in body["artifacts"]}
    assert "model.py" not in names
    assert {"spec.json", "validation.json", "resolved.json"} <= names


def test_compile_unsupported_is_failed(core_client, plate, monkeypatch):
    def refuse(self, document, resolved):
        raise CompileUnsupported([("v_ext", "extrude is not supported yet")])

    monkeypatch.setattr(job_service_module.Cad2Compiler, "compile", refuse)
    body = post(core_client, plate).json()
    assert body["status"] == "failed"
    assert body["renderer_used"] == "compile-unsupported"
    assert "v_ext" in body["error"] and "extrude" in body["error"]
    names = {item["filename"] for item in body["artifacts"]}
    assert "model.py" not in names and "resolved.json" in names


def test_core_unavailable_is_503(settings, plate, tmp_path):
    missing = settings.model_copy(update={"core_bin": tmp_path / "does-not-exist"})
    with TestClient(create_app(missing)) as client:
        response = post(client, plate)
    assert response.status_code == 503


def test_old_1_0_manifest_still_lists(core_client):
    core_client.post(
        "/api/v1/generate",
        json={"prompt": "長100mm寬50mm厚8mm固定板，兩個5mm孔", "planner": "rule",
              "formats": ["py"], "render": False},
    )
    assert len(core_client.get("/api/v1/jobs").json()) == 1


# ---------------------------------------------------------------------------------- CLI


def run_cli(monkeypatch, tmp_path, core_binary, argv):
    monkeypatch.setenv("PROMPTCAD_DATA_DIR", str(tmp_path / "data"))
    monkeypatch.setenv("PROMPTCAD_RENDER_BACKEND", "source_only")
    monkeypatch.setenv("PROMPTCAD_CORE_BIN", str(core_binary))
    return main_args(argv)


def test_cli_render_routes_2_0_and_strips_example_key(
    monkeypatch, tmp_path, core_binary, capsys
):
    code = run_cli(
        monkeypatch,
        tmp_path,
        core_binary,
        ["render", str(CAD2 / "plate-four-holes.cad2.json"), "--formats", "py", "step"],
    )
    assert code == 0
    manifest = json.loads(capsys.readouterr().out)
    assert manifest["spec_version"] == "2.0"
    assert manifest["status"] == "source_only"


def test_cli_validate_routes_2_0(monkeypatch, tmp_path, core_binary, capsys):
    code = run_cli(
        monkeypatch, tmp_path, core_binary, ["validate", str(CAD2 / "plate-four-holes.cad2.json")]
    )
    assert code == 0
    assert json.loads(capsys.readouterr().out)["valid"] is True


def test_cli_rejected_document_exits_2(monkeypatch, tmp_path, core_binary, capsys):
    bad = load("plate-four-holes")
    bad["bogus"] = 1
    path = tmp_path / "bad.cad2.json"
    path.write_text(json.dumps(bad), encoding="utf-8")
    assert run_cli(monkeypatch, tmp_path, core_binary, ["render", str(path)]) == 2
    assert "invalid_document" in capsys.readouterr().err

