from __future__ import annotations

import json
import os
import queue
import subprocess
import sys
import threading
from pathlib import Path

import httpx
import pytest

from app.sidecar import READY_PREFIX, ready_line

ROOT = Path(__file__).resolve().parents[1]
TOKEN = "test-sidecar-token-0123456789abcdef"
TAURI_ORIGINS = "http://tauri.localhost,tauri://localhost"


def _env(tmp_path: Path, **extra: str) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if not k.upper().startswith("PROMPTCAD_")}
    env.update(
        PROMPTCAD_DATA_DIR=str(tmp_path / "data"),
        PROMPTCAD_PLANNER_MODE="rule",
        PROMPTCAD_RENDER_BACKEND="source_only",
        PYTHONUNBUFFERED="1",
    )
    env.update(extra)
    return env


def _spawn(tmp_path: Path, env: dict[str, str]) -> subprocess.Popen[str]:
    return subprocess.Popen(
        [sys.executable, "-m", "app.sidecar", "--host", "127.0.0.1", "--port", "0"],
        cwd=tmp_path,  # no .env from the repo; app must be importable via PYTHONPATH
        env={**env, "PYTHONPATH": str(ROOT)},
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def _first_line(stream, timeout: float = 30.0) -> str | None:
    lines: queue.Queue[str] = queue.Queue()
    threading.Thread(target=lambda: lines.put(stream.readline()), daemon=True).start()
    try:
        return lines.get(timeout=timeout)
    except queue.Empty:
        return None


@pytest.fixture
def sidecar(tmp_path):
    proc = _spawn(
        tmp_path, _env(tmp_path, PROMPTCAD_API_TOKEN=TOKEN, PROMPTCAD_CORS_ORIGINS=TAURI_ORIGINS)
    )
    try:
        line = _first_line(proc.stdout)
        assert line, (
            f"no READY line; stderr: {proc.stderr.read() if proc.poll() is not None else ''}"
        )
        yield proc, line
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)


def test_ready_line_format():
    assert ready_line(51234) == 'PROMPTCAD_SIDECAR_READY {"port": 51234}'


def test_sidecar_ready_health_and_token(sidecar):
    _proc, line = sidecar
    assert line.endswith("\n")
    prefix, _, payload = line.strip().partition(" ")
    assert prefix == READY_PREFIX
    port = json.loads(payload)["port"]
    assert isinstance(port, int) and 0 < port < 65536

    base = f"http://127.0.0.1:{port}"
    with httpx.Client(base_url=base, timeout=10) as http:
        ok = http.get("/api/v1/health", headers={"Authorization": f"Bearer {TOKEN}"})
        assert ok.status_code == 200
        assert ok.json()["status"] == "ok"

        assert http.get("/api/v1/health").status_code == 401
        wrong = http.get("/api/v1/health", headers={"Authorization": "Bearer nope"})
        assert wrong.status_code == 401

        for origin in TAURI_ORIGINS.split(","):
            preflight = http.options(
                "/api/v1/health",
                headers={
                    "Origin": origin,
                    "Access-Control-Request-Method": "GET",
                    "Access-Control-Request-Headers": "authorization",
                },
            )
            assert preflight.status_code == 200
            assert preflight.headers["access-control-allow-origin"] == origin


def test_sidecar_refuses_to_start_without_token(tmp_path):
    proc = _spawn(tmp_path, _env(tmp_path))
    try:
        out, err = proc.communicate(timeout=60)
    finally:
        if proc.poll() is None:
            proc.kill()
    assert proc.returncode == 2
    assert READY_PREFIX not in out
    assert "PROMPTCAD_API_TOKEN" in err


def test_sidecar_rejects_non_loopback_host(tmp_path):
    result = subprocess.run(
        [sys.executable, "-m", "app.sidecar", "--host", "0.0.0.0", "--port", "0"],
        cwd=ROOT,
        env=_env(tmp_path, PROMPTCAD_API_TOKEN=TOKEN),
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode == 2
    assert READY_PREFIX not in result.stdout
