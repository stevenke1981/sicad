from __future__ import annotations

import asyncio
import mimetypes
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from app.core.config import Settings
from app.models.api import Artifact, JobListItem, JobManifest, PlanResponse
from app.models.cad import CadDocument, ValidationReport
from app.services.compiler2 import Cad2Compiler, CompileUnsupported
from app.services.core_bridge import CoreBridge
from app.services.core_validator import CoreValidator
from app.services.openscad import OpenScadCompiler
from app.services.planners.factory import PlannerFactory
from app.services.preview import SvgPreview
from app.services.renderer import Renderer
from app.services.storage import JobStorage


class JobService:
    def __init__(self, settings: Settings, core: CoreBridge | None = None):
        self.settings = settings
        self.planners = PlannerFactory(settings)
        self.core = core if core is not None else CoreBridge(settings)
        self.core_validator = CoreValidator(self.core)
        self.compiler2 = Cad2Compiler()
        self.openscad = OpenScadCompiler()
        self.preview = SvgPreview()
        self.storage = JobStorage(settings)
        self.renderer = Renderer(settings)

    async def _validate(
        self, spec: CadDocument
    ) -> tuple[ValidationReport, dict[str, Any], dict[str, Any]]:
        """One core call: ``(report, resolved, canonical 2.0 document)``.

        A missing binary is CoreUnavailable (HTTP 503).
        """
        return await self.core_validator.validate_spec(spec)

    async def _plan(self, prompt: str, planner_choice: str):
        if len(prompt) > self.settings.max_prompt_chars:
            raise ValueError(f"Prompt exceeds {self.settings.max_prompt_chars} characters")
        spec, planner_used = await self.planners.plan(prompt, planner_choice)
        validation, resolved, canonical = await self._validate(spec)
        return spec, planner_used, validation, resolved, canonical

    async def plan(self, prompt: str, planner_choice: str) -> PlanResponse:
        spec, planner_used, validation, _resolved, _canonical = await self._plan(
            prompt, planner_choice
        )
        return PlanResponse(spec=spec, validation=validation, planner_used=planner_used)

    async def generate(
        self,
        prompt: str,
        planner_choice: str,
        formats: list[str],
        render: bool,
    ) -> JobManifest:
        spec, planner_used, validation, resolved, canonical = await self._plan(
            prompt, planner_choice
        )
        return await self._materialize(
            spec=spec,
            validation=validation,
            resolved=resolved,
            canonical=canonical,
            prompt=prompt,
            planner_used=planner_used,
            formats=formats,
            render=render,
        )

    async def generate_from_spec(
        self,
        spec: CadDocument,
        formats: list[str],
        render: bool,
    ) -> JobManifest:
        validation, resolved, canonical = await self._validate(spec)
        return await self._materialize(
            spec=spec,
            validation=validation,
            resolved=resolved,
            canonical=canonical,
            prompt=spec.source_prompt,
            planner_used="manual-dsl",
            formats=formats,
            render=render,
        )

    async def _materialize(
        self,
        *,
        spec: CadDocument,
        validation: ValidationReport,
        resolved: dict[str, Any],
        canonical: dict[str, Any],
        prompt: str,
        planner_used: str,
        formats: list[str],
        render: bool,
    ) -> JobManifest:
        job_id, job_dir = self.storage.create()
        created_at = datetime.now(UTC).isoformat()

        self.storage.write_json(job_dir / "spec.json", spec.model_dump(mode="json"))
        self.storage.write_json(
            job_dir / "validation.json",
            validation.model_dump(mode="json"),
        )
        self.storage.write_text(job_dir / "model.scad", self.openscad.compile(spec))
        self.preview.write(spec, job_dir / "preview.svg")
        # Attempted even for an invalid report: geometry-invalid specs keep their source.
        failure = self._compile_model(job_dir, canonical, resolved)

        return await self._finish(
            job_id=job_id,
            job_dir=job_dir,
            created_at=created_at,
            validation=validation,
            prompt=prompt,
            planner_used=planner_used,
            formats=formats,
            render=render,
            payload={"spec": spec},
            failure=failure,
        )

    def _compile_model(
        self, job_dir: Path, canonical: dict[str, Any], resolved: dict[str, Any]
    ) -> tuple[str, str] | None:
        """Write model.py from the core's canonical document; return a failure on unsupported."""
        try:
            source = self.compiler2.compile(canonical, resolved)
        except CompileUnsupported as exc:
            detail = "; ".join(f"{item}: {reason}" for item, reason in exc.items)
            return (
                "compile-unsupported",
                f"Cannot generate geometry for this document yet: {detail or exc}",
            )
        self.storage.write_text(job_dir / "model.py", source)
        return None

    async def generate_from_document(
        self,
        document: dict[str, Any],
        formats: list[str],
        render: bool,
    ) -> JobManifest:
        """Generate from a CadDocument 2.0 (ADR 0010): the core resolves, Python only emits."""
        if document.get("schema_version") != "2.0":
            raise ValueError(
                "generate-from-document requires schema_version \"2.0\"; "
                "use generate-from-spec for 1.0 documents"
            )
        report, resolved, canonical, _version = await self.core_validator.validate_document(
            document
        )
        job_id, job_dir = self.storage.create()
        created_at = datetime.now(UTC).isoformat()
        self.storage.write_json(job_dir / "spec.json", canonical)
        self.storage.write_json(job_dir / "validation.json", report.model_dump(mode="json"))
        self.storage.write_json(job_dir / "resolved.json", resolved)

        failure = self._compile_model(job_dir, canonical, resolved) if report.valid else None
        prompt = canonical.get("source_prompt")
        return await self._finish(
            job_id=job_id,
            job_dir=job_dir,
            created_at=created_at,
            validation=report,
            prompt=prompt if isinstance(prompt, str) else "",
            planner_used="manual-dsl",
            formats=formats,
            render=render,
            payload={"spec_version": "2.0", "document": canonical},
            failure=failure,
            allow_openscad=False,
        )

    async def _finish(
        self,
        *,
        job_id: str,
        job_dir: Path,
        created_at: str,
        validation: ValidationReport,
        prompt: str,
        planner_used: str,
        formats: list[str],
        render: bool,
        payload: dict[str, Any],
        failure: tuple[str, str] | None = None,
        allow_openscad: bool = True,
    ) -> JobManifest:
        """Shared tail: render (or explain why not), list artifacts, write the manifest."""
        warnings: list[str] = []
        error: str | None = None
        renderer_used = "not-run"
        status = "source_only"

        if not validation.valid:
            status = "failed"
            renderer_used = "validation-blocked"
            error = "CAD validation failed; rendering was skipped."
            warnings.extend(
                issue.message
                for issue in validation.issues
                if issue.severity.value == "error"
            )
        elif failure is not None:
            status = "failed"
            renderer_used, error = failure
        elif render:
            try:
                result = await asyncio.to_thread(
                    self.renderer.render, job_dir, formats, allow_openscad
                )
                renderer_used = result.renderer
                status = result.status
                warnings.extend(result.warnings)
            except RuntimeError as exc:
                status = "failed"
                renderer_used = "failed"
                error = str(exc)
        else:
            renderer_used = "not-run"
            warnings.append("render=false：只產生 DSL 與 CAD 原始碼。")

        artifacts = self._artifacts(job_id, job_dir)
        manifest = JobManifest(
            job_id=job_id,
            status=status,
            created_at=created_at,
            prompt=prompt,
            planner_used=planner_used,
            renderer_used=renderer_used,
            requested_formats=list(dict.fromkeys(formats)),
            validation=validation,
            artifacts=artifacts,
            warnings=warnings,
            error=error,
            **payload,
        )
        self.storage.write_json(job_dir / "manifest.json", manifest.model_dump(mode="json"))
        return manifest

    async def validate(self, spec: CadDocument) -> ValidationReport:
        return (await self._validate(spec))[0]

    def get(self, job_id: str) -> JobManifest | None:
        value = self.storage.read_manifest(job_id)
        return JobManifest.model_validate(value) if value else None

    def list(self) -> list[JobListItem]:
        items = []
        for value in self.storage.list_manifests(self.settings.max_jobs_returned):
            try:
                manifest = JobManifest.model_validate(value)
            except ValueError:
                continue
            items.append(
                JobListItem(
                    job_id=manifest.job_id,
                    status=manifest.status,
                    created_at=manifest.created_at,
                    prompt=manifest.prompt,
                    name=self._manifest_name(manifest),
                    renderer_used=manifest.renderer_used,
                )
            )
        return items

    @staticmethod
    def _manifest_name(manifest: JobManifest) -> str:
        if manifest.spec is not None:
            return manifest.spec.name
        name = (manifest.document or {}).get("name")
        return name if isinstance(name, str) else ""

    @staticmethod
    def _artifacts(job_id: str, job_dir: Path) -> list[Artifact]:
        artifacts = []
        for path in sorted(job_dir.iterdir()):
            if not path.is_file() or path.name == "manifest.json" or path.name.startswith(".tmp-"):
                continue
            media_type = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
            artifacts.append(
                Artifact(
                    filename=path.name,
                    media_type=media_type,
                    size=path.stat().st_size,
                    url=f"/api/v1/jobs/{job_id}/files/{path.name}",
                )
            )
        return artifacts
