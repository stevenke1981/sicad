from __future__ import annotations

import re
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse, HTMLResponse
from fastapi.staticfiles import StaticFiles

from app import __version__
from app.api.routes import router
from app.core.config import Settings, get_settings
from app.services.job_service import JobService

_ROOT_FILE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")

# Served at "/" when the React build (app/web_dist) is missing, e.g. a plain checkout or the
# frozen desktop sidecar, whose UI comes from the Tauri shell. The API is unaffected.
FRONTEND_MISSING_HTML = """<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>PromptCAD Studio - frontend not built</title></head>
<body style="font-family:system-ui,sans-serif;max-width:40rem;margin:3rem auto;line-height:1.5">
<h1>PromptCAD Studio</h1>
<p>The web frontend has not been built, so there is no UI to serve here.
The API is running normally.</p>
<p>Build it once with <code>npm --prefix web ci</code> and
<code>npm --prefix web run build</code> (output: <code>app/web_dist</code>),
then restart the server.</p>
<p>API documentation: <a href="/docs">/docs</a></p>
</body>
</html>
"""


def _frontend_root(settings: Settings) -> Path | None:
    """Return the built frontend directory if it is usable, else None (503 stub at "/")."""
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


def _mount_frontend_missing(app: FastAPI) -> None:
    @app.get("/", include_in_schema=False)
    async def index():
        return HTMLResponse(FRONTEND_MISSING_HTML, status_code=503)


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
        _mount_frontend_missing(app)

    return app


app = create_app()
