from __future__ import annotations

from pathlib import Path

from fastapi.testclient import TestClient

from app.core.config import Settings
from app.main import FRONTEND_MISSING_HTML, create_app

APP_DIR = Path(__file__).resolve().parents[1] / "app"


def _settings(tmp_path, frontend_dir):
    return Settings(
        env="test",
        data_dir=tmp_path / "generated",
        planner_mode="rule",
        render_backend="source_only",
        api_token=None,
        frontend_dir=frontend_dir,
    )


def test_built_frontend_is_served(tmp_path):
    dist = tmp_path / "web_dist"
    (dist / "assets").mkdir(parents=True)
    (dist / "index.html").write_text("<!doctype html><title>react build</title>", encoding="utf-8")
    (dist / "assets" / "x.js").write_text("console.log('x');", encoding="utf-8")
    (dist / "favicon.svg").write_text("<svg/>", encoding="utf-8")

    with TestClient(create_app(_settings(tmp_path, dist))) as client:
        index = client.get("/")
        assert index.status_code == 200
        assert "react build" in index.text

        asset = client.get("/assets/x.js")
        assert asset.status_code == 200
        assert "console.log" in asset.text

        assert client.get("/favicon.svg").status_code == 200
        assert client.get("/missing.txt").status_code == 404
        assert client.get("/assets/app.js").status_code == 404  # legacy asset not mixed in
        assert client.get("/api/v1/health").status_code == 200
        assert client.get("/openapi.json").status_code == 200


def test_root_file_route_rejects_traversal(tmp_path):
    dist = tmp_path / "web_dist"
    dist.mkdir()
    (dist / "index.html").write_text("ok", encoding="utf-8")
    (tmp_path / "secret.txt").write_text("secret", encoding="utf-8")

    with TestClient(create_app(_settings(tmp_path, dist))) as client:
        for path in ("/..%2Fsecret.txt", "/%2E%2E%2Fsecret.txt", "/.env"):
            response = client.get(path)
            assert response.status_code == 404
            assert "secret" not in response.text


def test_stub_when_no_build(tmp_path):
    for frontend_dir in (tmp_path / "does-not-exist", tmp_path):  # missing dir / no index.html
        with TestClient(create_app(_settings(tmp_path, frontend_dir))) as client:
            index = client.get("/")
            assert index.status_code == 503
            assert index.headers["content-type"].startswith("text/html")
            assert index.text == FRONTEND_MISSING_HTML
            assert "npm --prefix web run build" in index.text
            assert 'href="/docs"' in index.text
            assert client.get("/assets/app.js").status_code == 404  # nothing mounted
            assert client.get("/assets/styles.css").status_code == 404
            assert client.get("/api/v1/health").status_code == 200
            assert client.get("/docs").status_code == 200


def test_static_dir_is_gone():
    # The legacy vanilla-JS UI (app/static) was removed; ADR 0003.
    assert not (APP_DIR / "static").exists()
    import app.main

    assert not hasattr(app.main, "STATIC_DIR")
