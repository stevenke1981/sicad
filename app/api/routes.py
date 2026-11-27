from __future__ import annotations

import io
import zipfile
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

from fastapi import APIRouter, Depends, HTTPException, Request
from fastapi.responses import FileResponse, Response, StreamingResponse

from app.core.security import require_api_token, safe_job_file, validate_job_id
from app.models.api import (
    CapabilityResponse,
    GenerateFromSpecRequest,
    GenerateRequest,
    JobListItem,
    JobManifest,
    PlanRequest,
    PlanResponse,
)
from app.models.cad import CadDocument, ValidationReport
from app.services.core_bridge import CoreBusy, CoreFailed, CoreTimeout, CoreUnavailable

router = APIRouter(prefix="/api/v1", dependencies=[Depends(require_api_token)])


@contextmanager
def core_http_errors() -> Iterator[None]:
    """Map core transport failures to HTTP: 503 (+Retry-After when busy), 504, 502."""
    try:
        yield
    except CoreUnavailable as exc:
        raise HTTPException(status_code=503, detail=f"Core unavailable: {exc}") from exc
    except CoreBusy as exc:
        raise HTTPException(
            status_code=503,
            detail=f"Core busy: {exc}",
            headers={"Retry-After": str(exc.retry_after)},
        ) from exc
    except CoreTimeout as exc:
        raise HTTPException(status_code=504, detail=str(exc)) from exc
    except CoreFailed as exc:  # includes CoreRejected (status: error for a valid 1.0 doc)
        raise HTTPException(status_code=502, detail=str(exc)) from exc


def _service(request: Request):
    return request.app.state.jobs


@router.get("/health")
async def health(request: Request):
    return {"status": "ok", "version": request.app.version}


@router.get("/capabilities", response_model=CapabilityResponse)
async def capabilities(request: Request):
    service = _service(request)
    settings = request.app.state.settings
    core = await request.app.state.core.version()
    return CapabilityResponse(
        planners=["auto", "rule", "llm"],
        base_features=["plate", "cylinder", "ring", "l_bracket", "enclosure"],
        hole_types=["through", "blind", "clearance", "tapped", "counterbore", "countersink"],
        formats=["step", "stl", "dxf", "svg", "py", "scad", "json"],
        cadquery_available=service.renderer.cadquery_available(),
        openscad_available=service.renderer.openscad_available(),
        configured_planner_mode=settings.planner_mode,
        configured_render_backend=settings.render_backend,
        core_available=core is not None,
        core_crate_version=core.crate_version if core else None,
        core_api_version=core.api_version if core else None,
        core_schema_versions=list(core.schema_versions) if core else [],
    )


async def _read_limited_body(request: Request, limit: int) -> bytes:
    declared = request.headers.get("content-length")
    if declared and declared.isdigit() and int(declared) > limit:
        raise HTTPException(status_code=413, detail=f"Request body exceeds {limit} bytes")
    chunks: list[bytes] = []
    total = 0
    async for chunk in request.stream():
        total += len(chunk)
        if total > limit:
            raise HTTPException(status_code=413, detail=f"Request body exceeds {limit} bytes")
        chunks.append(chunk)
    return b"".join(chunks)


@router.post(
    "/core",
    include_in_schema=True,
    responses={
        200: {"description": "Core response envelope (status ok or error), unchanged"},
        413: {"description": "Request body too large"},
        422: {"description": "Body does not start with a JSON object"},
        502: {"description": "Core crashed or wrote an unparseable response"},
        503: {"description": "Core binary unavailable, or busy (Retry-After set)"},
        504: {"description": "Core timed out"},
    },
)
async def core_rpc(request: Request):
    """Transparent transport to `promptcad-core rpc` (ADR 0008 rule 4, ADR 0012).

    The body is forwarded byte for byte and the core's response is returned unchanged with
    HTTP 200 for both `status: ok` and `status: error`.
    """
    settings = request.app.state.settings
    body = await _read_limited_body(request, settings.core_max_request_bytes)
    # Cheap shape check only: the core parses and rejects anything malformed with a
    # `bad_request` envelope, so the server never parses up to 8 MiB on the event loop.
    if body.lstrip(b" \t\r\n")[:1] != b"{":
        raise HTTPException(status_code=422, detail="Body must be a JSON object")
    with core_http_errors():
        payload = await request.app.state.core.call_raw(body)
    return Response(content=payload, media_type="application/json")


@router.post("/plan", response_model=PlanResponse)
async def plan(request: Request, body: PlanRequest):
    try:
        with core_http_errors():
            return await _service(request).plan(body.prompt, body.planner)
    except (ValueError, RuntimeError) as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc


@router.post("/generate", response_model=JobManifest)
async def generate(request: Request, body: GenerateRequest):
    try:
        with core_http_errors():
            return await _service(request).generate(
                body.prompt, body.planner, body.formats, body.render
            )
    except (ValueError, RuntimeError) as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc


@router.post("/generate-from-spec", response_model=JobManifest)
async def generate_from_spec(request: Request, body: GenerateFromSpecRequest):
    try:
        with core_http_errors():
            return await _service(request).generate_from_spec(
                body.spec,
                body.formats,
                body.render,
            )
    except (ValueError, RuntimeError) as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc


@router.post("/validate", response_model=ValidationReport)
async def validate(request: Request, body: CadDocument):
    with core_http_errors():
        return await _service(request).validate(body)


@router.get("/jobs", response_model=list[JobListItem])
async def list_jobs(request: Request):
    return _service(request).list()


@router.get("/jobs/{job_id}", response_model=JobManifest)
async def get_job(request: Request, job_id: str):
    validate_job_id(job_id)
    manifest = _service(request).get(job_id)
    if not manifest:
        raise HTTPException(status_code=404, detail="Job not found")
    return manifest


@router.get("/jobs/{job_id}/files/{filename}")
async def get_file(request: Request, job_id: str, filename: str):
    validate_job_id(job_id)
    job_dir = _service(request).storage.path(job_id)
    path = safe_job_file(job_dir, filename)
    if not path.is_file():
        raise HTTPException(status_code=404, detail="File not found")
    return FileResponse(path, filename=path.name)


@router.get("/jobs/{job_id}/bundle.zip")
async def bundle(request: Request, job_id: str):
    validate_job_id(job_id)
    job_dir: Path = _service(request).storage.path(job_id)
    if not job_dir.is_dir():
        raise HTTPException(status_code=404, detail="Job not found")

    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(job_dir.iterdir()):
            if path.is_file() and not path.name.startswith(".tmp-"):
                archive.write(path, arcname=path.name)
    buffer.seek(0)
    headers = {"Content-Disposition": f'attachment; filename="promptcad-{job_id}.zip"'}
    return StreamingResponse(buffer, media_type="application/zip", headers=headers)
