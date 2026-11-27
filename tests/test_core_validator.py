"""1.0 validation routed through the Rust core (CoreValidator, JobService, REST, CLI)."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from app.models.api import JobManifest
from app.models.cad import CadDocument
from app.services.core_bridge import CoreBusy, CoreFailed, CoreTimeout, CoreUnavailable

ROOT = Path(__file__).resolve().parents[1]
V1_EXAMPLE = ROOT / "examples" / "generated" / "plate-four-holes" / "spec.json"
PROMPT = "長120mm寬60mm厚10mm固定板，四角M6通孔，R5"


def _spec() -> dict:
    return json.loads(V1_EXAMPLE.read_text(encoding="utf-8"))


def _overlapping_spec() -> dict:
    spec = _spec()
    spec["holes"] = [
        {"kind": "hole", "x": 0, "y": 0, "diameter": 10},
        {"kind": "hole", "x": 4, "y": 0, "diameter": 10},
    ]
    return spec


# ---- real core -------------------------------------------------------------------------


def test_core_report_carries_target_layer_and_feature_index(core_validate):
    report = core_validate(_overlapping_spec())
    assert not report.valid
    issue = next(i for i in report.issues if i.code == "overlapping_holes")
    assert issue.severity.value == "error"
    assert issue.feature_index == 0
    assert issue.target
    assert issue.layer.value == "analytic"


def test_validate_endpoint_uses_core(client, core_binary):
    response = client.post("/api/v1/validate", json=_spec())
    assert response.status_code == 200, response.text
    issues = response.json()["issues"]
    assert issues and all(i["target"] for i in issues if i["code"] != "assumption")
    assert all("layer" in i and "related" in i for i in issues)


def test_generate_writes_target_and_layer(client, core_binary, settings):
    response = client.post(
        "/api/v1/generate",
        json={"prompt": PROMPT, "planner": "rule", "formats": ["json"], "render": False},
    )
    assert response.status_code == 200, response.text
    body = response.json()
    job_dir = settings.data_dir / body["job_id"]
    for name in ("validation.json", "manifest.json"):
        data = json.loads((job_dir / name).read_text(encoding="utf-8"))
        issues = data["issues"] if name == "validation.json" else data["validation"]["issues"]
        assert issues, name
        assert any(i["target"] for i in issues), name
        assert all(i["layer"] == "analytic" for i in issues), name
    assert not any("後備" in w for w in body["warnings"])


# ---- error mapping (stubbed bridge, no binary needed) -----------------------------------

ENDPOINTS = [
    ("/api/v1/plan", lambda: {"prompt": PROMPT, "planner": "rule"}),
    (
        "/api/v1/generate",
        lambda: {"prompt": PROMPT, "planner": "rule", "formats": ["json"], "render": False},
    ),
    (
        "/api/v1/generate-from-spec",
        lambda: {"spec": _spec(), "formats": ["json"], "render": False},
    ),
    ("/api/v1/validate", _spec),
]


def _stub_call_raw(client, monkeypatch, exc=None, response=None):
    async def call_raw(request, **kwargs):
        if exc is not None:
            raise exc
        return json.dumps(response).encode("utf-8")

    monkeypatch.setattr(client.app.state.core, "call_raw", call_raw)


@pytest.mark.parametrize("path,body", ENDPOINTS, ids=[p for p, _ in ENDPOINTS])
@pytest.mark.parametrize(
    "exc,status",
    [(CoreBusy("busy", 2), 503), (CoreTimeout("slow"), 504), (CoreFailed("crash"), 502)],
    ids=["busy", "timeout", "failed"],
)
def test_core_failures_map_to_http(client, monkeypatch, path, body, exc, status):
    _stub_call_raw(client, monkeypatch, exc=exc)
    response = client.post(path, json=body())
    assert response.status_code == status, response.text
    if status == 503:
        assert response.headers["Retry-After"] == "2"


@pytest.mark.parametrize("path,body", ENDPOINTS, ids=[p for p, _ in ENDPOINTS])
def test_core_status_error_is_502_with_code_and_path(client, monkeypatch, path, body):
    _stub_call_raw(
        client,
        monkeypatch,
        response={
            "status": "error",
            "code": "invalid_document",
            "path": "/document/base",
            "message": "nope",
        },
    )
    response = client.post(path, json=body())
    assert response.status_code == 502, response.text
    detail = response.json()["detail"]
    assert "invalid_document" in detail and "/document/base" in detail


# ---- missing core: no fallback, a clear 503 ----------------------------------------------


def test_missing_binary_is_503_with_make_core_hint(client, monkeypatch):
    monkeypatch.delenv("PROMPTCAD_CORE_BIN", raising=False)
    client.app.state.core.override_binary(None)
    validated = client.post("/api/v1/validate", json=_spec())
    assert validated.status_code == 503, validated.text
    detail = validated.json()["detail"]
    assert "make core" in detail and "PROMPTCAD_CORE_BIN" in detail
    generated = client.post(
        "/api/v1/generate",
        json={"prompt": PROMPT, "planner": "rule", "formats": ["json"], "render": False},
    )
    assert generated.status_code == 503, generated.text
    assert client.post("/api/v1/core", json={"op": "version"}).status_code == 503


def test_unavailable_error_is_503_never_a_silent_fallback(client, monkeypatch):
    _stub_call_raw(client, monkeypatch, exc=CoreUnavailable("gone"))
    response = client.post("/api/v1/validate", json=_spec())
    assert response.status_code == 503
    assert "make core" in response.json()["detail"]


# ---- old manifests ----------------------------------------------------------------------


def test_old_four_field_manifest_still_loads(client, settings):
    spec = CadDocument.model_validate(_spec())
    job_id = "a" * 32
    job_dir = settings.data_dir / job_id
    job_dir.mkdir(parents=True)
    old = {
        "job_id": job_id,
        "status": "source_only",
        "created_at": "2025-01-01T00:00:00+00:00",
        "prompt": "old",
        "planner_used": "rule",
        "renderer_used": "not-run",
        "requested_formats": ["json"],
        "spec": spec.model_dump(mode="json"),
        "validation": {
            "valid": True,
            "review_required": True,
            "issues": [
                {
                    "severity": "info",
                    "code": "thread_annotation_only",
                    "message": "M6 目前以圓柱孔近似，未建立實體螺旋牙。",
                    "feature_index": 0,
                },
                {"severity": "warning", "code": "thin_wall", "message": "偏薄"},
            ],
        },
        "artifacts": [],
        "warnings": [],
        "error": None,
    }
    (job_dir / "manifest.json").write_text(json.dumps(old, ensure_ascii=False), encoding="utf-8")
    manifest = JobManifest.model_validate(old)
    assert manifest.validation.issues[1].target is None
    assert manifest.validation.issues[0].layer.value == "analytic"
    assert client.get(f"/api/v1/jobs/{job_id}").status_code == 200
    assert any(item["job_id"] == job_id for item in client.get("/api/v1/jobs").json())


# ---- CLI ---------------------------------------------------------------------------------


def test_cli_validate_exit_codes(core_binary, tmp_path, capsys):
    from app import cli

    bad = tmp_path / "bad.json"
    bad.write_text(json.dumps(_overlapping_spec()), encoding="utf-8")
    assert cli.main_args(["validate", str(V1_EXAMPLE)]) == 0
    assert cli.main_args(["validate", str(bad)]) == 2
    assert "overlapping_holes" in capsys.readouterr().out
