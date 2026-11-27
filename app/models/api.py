from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, model_validator

from app.models.cad import CadDocument, ValidationReport

OutputFormat = Literal["step", "stl", "dxf", "svg", "py", "scad", "json"]
PlannerChoice = Literal["auto", "rule", "llm"]


def default_formats() -> list[OutputFormat]:
    return ["step", "stl", "dxf", "svg", "py", "scad", "json"]


class StrictApiModel(BaseModel):
    model_config = ConfigDict(extra="forbid")


class PlanRequest(StrictApiModel):
    prompt: str = Field(min_length=3, max_length=20_000)
    planner: PlannerChoice = "auto"


class GenerateRequest(PlanRequest):
    formats: list[OutputFormat] = Field(
        default_factory=default_formats,
        min_length=1,
        max_length=7,
    )
    render: bool = True


class GenerateFromSpecRequest(StrictApiModel):
    spec: CadDocument
    formats: list[OutputFormat] = Field(
        default_factory=default_formats,
        min_length=1,
        max_length=7,
    )
    render: bool = True


class GenerateFromDocumentRequest(StrictApiModel):
    document: dict[str, Any]
    formats: list[OutputFormat] = Field(
        default_factory=default_formats,
        min_length=1,
        max_length=7,
    )
    render: bool = True


class PlanResponse(StrictApiModel):
    spec: CadDocument
    validation: ValidationReport
    planner_used: str


class Artifact(StrictApiModel):
    filename: str
    media_type: str
    size: int
    url: str


class JobManifest(StrictApiModel):
    job_id: str
    status: Literal["completed", "source_only", "failed"]
    created_at: str
    prompt: str
    planner_used: str
    renderer_used: str
    requested_formats: list[OutputFormat]
    # 1.0 jobs carry `spec`; 2.0 jobs carry `document` (see the validator below).
    spec: CadDocument | None = None
    spec_version: Literal["1.0", "2.0"] = "1.0"
    document: dict[str, Any] | None = None
    validation: ValidationReport
    artifacts: list[Artifact] = Field(default_factory=list)
    warnings: list[str] = Field(default_factory=list)
    error: str | None = None

    @model_validator(mode="after")
    def _check_payload_for_version(self) -> JobManifest:
        if self.spec_version == "1.0" and self.spec is None:
            raise ValueError("spec_version 1.0 manifests require 'spec'")
        if self.spec_version == "2.0" and self.document is None:
            raise ValueError("spec_version 2.0 manifests require 'document'")
        return self


class JobListItem(StrictApiModel):
    job_id: str
    status: str
    created_at: str
    prompt: str
    name: str
    renderer_used: str


class CapabilityResponse(StrictApiModel):
    planners: list[str]
    base_features: list[str]
    hole_types: list[str]
    formats: list[str]
    cadquery_available: bool
    openscad_available: bool
    configured_planner_mode: str
    configured_render_backend: str
    # Rust core (POST /api/v1/core); optional so older clients and servers stay compatible.
    core_available: bool = False
    core_crate_version: str | None = None
    core_api_version: int | None = None
    core_schema_versions: list[str] = Field(default_factory=list)
