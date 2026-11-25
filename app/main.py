from __future__ import annotations

import re
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse
from fastapi.staticfiles import StaticFiles

from app import __version__
from app.api.routes import router
from app.core.config import Settings, get_settings
from app.services.job_service import JobService

STATIC_DIR = Path(__file__).parent / "static"
_ROOT_FILE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")


def _frontend_root(settings: Settings) -> Path | None:
    """Return the built frontend directory if it is usable, else None (legacy UI)."""
    root = settings.frontend_dir
    if root and (root / "index.html").is_file():
        return root.resolve()
    return None


def _mount_frontend(app: FastAPI, root: Path) -> None:
    assets = root / "assets"
    if assets.is_dir():
        app.mount("/assets", StaticFiles(directory=assets), name="assets")

    @app.get("/", include_in_schema=False)
    async def index():
        return FileResponse(root / "index.html")

    @app.get("/{filename}", include_in_schema=False)
    async def root_file(filename: str):
        # Top-level files emitted by Vite from web/public (favicon etc.). Single path
        # segment only, no traversal, never outside the frontend root.
        if _ROOT_FILE.fullmatch(filename):
            candidate = (root / filename).resolve()
            if candidate.parent == root and candidate.is_file():
                return FileResponse(candidate)
        raise HTTPException(status_code=404, detail="Not Found")


def _mount_legacy(app: FastAPI) -> None:
    app.mount("/assets", StaticFiles(directory=STATIC_DIR), name="assets")

    @app.get("/", include_in_schema=False)
    async def index():
        return FileResponse(STATIC_DIR / "index.html")


def create_app(settings: Settings | None = None) -> FastAPI:
    settings = settings or get_settings()

    @asynccontextmanager
    async def lifespan(app: FastAPI):
        settings.ensure_directories()
        yield

    app = FastAPI(
        title=settings.app_name,
        version=__version__,
        description="Prompt → validated parametric CAD DSL → CadQuery/OpenSCAD artifacts",
        lifespan=lifespan,
    )
    app.state.settings = settings
    app.state.jobs = JobService(settings)
    app.add_middleware(
        CORSMiddleware,
        allow_origins=settings.cors_origin_list,
        allow_credentials=False,
        allow_methods=["GET", "POST"],
        allow_headers=["Authorization", "Content-Type", "X-API-Key"],
    )
    app.include_router(router)

    frontend = _frontend_root(settings)
    if frontend is not None:
        _mount_frontend(app, frontend)
    else:
        _mount_legacy(app)

    return app


app = create_app()
