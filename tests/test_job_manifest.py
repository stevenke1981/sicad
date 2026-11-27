from __future__ import annotations

import pytest
from pydantic import ValidationError

from app.models.api import JobManifest

SPEC = {
    "schema_version": "1.0",
    "name": "unsafe-hole",
    "source_prompt": "manual test",
    "unit": "mm",
    "material": None,
    "base": {"kind": "plate", "length": 40, "width": 30, "thickness": 5},
    "holes": [],
    "fillets": [],
    "chamfers": [],
    "assumptions": [],
    "notes": [],
    "planner": {"planner": "manual", "confidence": 1, "review_required": False},
}


def _manifest(**overrides) -> dict:
    data = {
        "job_id": "job-1",
        "status": "completed",
        "created_at": "2026-11-27T00:00:00Z",
        "prompt": "a bracket",
        "planner_used": "rule",
        "renderer_used": "cadquery",
        "requested_formats": ["step"],
        "validation": {
            "valid": True,
            "review_required": False,
            "issues": [],
        },
    }
    data.update(overrides)
    return data


def test_old_manifest_without_spec_version_validates():
    # Four-field-era manifests: spec present, no spec_version or document keys.
    manifest = JobManifest.model_validate(_manifest(spec=SPEC))
    assert manifest.spec_version == "1.0"
    assert manifest.spec is not None
    assert manifest.document is None


def test_v2_manifest_with_document_validates():
    manifest = JobManifest.model_validate(
        _manifest(spec_version="2.0", document={"schema_version": "2.0", "name": "bracket"})
    )
    assert manifest.spec is None
    assert manifest.document == {"schema_version": "2.0", "name": "bracket"}


def test_v2_manifest_without_document_fails():
    with pytest.raises(ValidationError, match="require 'document'"):
        JobManifest.model_validate(_manifest(spec_version="2.0"))


def test_v1_manifest_without_spec_fails():
    with pytest.raises(ValidationError, match="require 'spec'"):
        JobManifest.model_validate(_manifest(spec_version="1.0"))
