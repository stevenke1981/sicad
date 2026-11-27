"""POST /api/v1/core and the core bridge, against a FAKE promptcad-core (no Rust needed).

The fake is a real Python child process: ``subprocess.Popen`` inside ``core_bridge`` is
patched so that ``[<binary>, "rpc"]`` runs ``[sys.executable, fake.py, "rpc"]``. That keeps
the stdin/stdout/kill behaviour real on every platform without a shell wrapper.
"""

from __future__ import annotations

import subprocess
import sys
import textwrap
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from app.core.config import Settings
from app.main import create_app
from app.services import core_bridge
from app.services.core_bridge import CoreBridge

OK_VERSION = (
    '{"status":"ok","result":{"op":"version","crate_version":"9.9.9",'
    '"api_version":1,"schema_versions":["1.0","2.0"]}}'
)


class FakeCore:
    """Installs a fake child process; records the Popen objects and the bytes it received."""

    def __init__(self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
        self.tmp_path = tmp_path
        self.binary = tmp_path / "promptcad-core"
        self.binary.write_text("placeholder")
        self.procs: list[subprocess.Popen] = []
        self.spawns = 0
        self.script = tmp_path / "fake_core.py"
        self.stdin_dump = tmp_path / "stdin.bin"
        real_popen = subprocess.Popen

        def popen(argv, **kwargs):
            assert argv[0] == str(self.binary) and argv[1:] == ["rpc"]
            assert kwargs.get("shell") is False
            self.spawns += 1
            proc = real_popen([sys.executable, str(self.script), *argv[1:]], **kwargs)
            self.procs.append(proc)
            return proc

        monkeypatch.setattr(core_bridge.subprocess, "Popen", popen)

    def behave(self, body: str) -> None:
        header = f"import sys, pathlib\ndata = sys.stdin.buffer.read()\npathlib.Path({str(self.stdin_dump)!r}).write_bytes(data)\n"
        self.script.write_text(header + textwrap.dedent(body), encoding="utf-8")


@pytest.fixture
def fake(tmp_path, monkeypatch):
    return FakeCore(tmp_path, monkeypatch)


def make_client(tmp_path, fake: FakeCore | None, **overrides):
    settings = Settings(
        env="test",
        data_dir=tmp_path / "generated",
        planner_mode="rule",
        render_backend="source_only",
        **overrides,
    )
    app = create_app(settings)
    if fake is not None:
        app.state.core.override_binary(fake.binary)
    return TestClient(app)


def test_missing_binary_is_503_and_capabilities_unavailable(tmp_path):
    settings_kwargs = {"core_bin": tmp_path / "does-not-exist"}
    with make_client(tmp_path, None, **settings_kwargs) as client:
        response = client.post("/api/v1/core", json={"op": "version"})
        assert response.status_code == 503
        assert "unavailable" in response.json()["detail"].lower()
        caps = client.get("/api/v1/capabilities").json()
        assert caps["core_available"] is False
        assert caps["core_crate_version"] is None
        assert caps["core_schema_versions"] == []


def test_unstartable_binary_is_503(tmp_path):
    broken = tmp_path / "promptcad-core"
    broken.write_text("not an executable")
    with make_client(tmp_path, None, core_bin=broken) as client:
        assert client.post("/api/v1/core", json={"op": "version"}).status_code == 503


def test_ok_and_error_envelopes_are_http_200_and_bytes_forwarded(tmp_path, fake):
    fake.behave(
        """
        import json
        req = json.loads(data)
        if req.get("op") == "version":
            print('@VERSION@')
        else:
            print('{"status":"error","code":"bad_request","message":"x","path":"","violations":[]}')
        """.replace("@VERSION@", OK_VERSION)
    )
    # Deliberately odd spacing / key order / unicode / big number: must reach the core untouched.
    raw = '{ "op" :  "version",\n  "z":1e400, "a":"高" }'.encode()
    with make_client(tmp_path, fake) as client:
        ok = client.post("/api/v1/core", content=raw)
        assert ok.status_code == 200
        assert ok.json()["status"] == "ok"
        assert ok.text == OK_VERSION  # response forwarded unchanged (trailing newline stripped)
        assert fake.stdin_dump.read_bytes() == raw

        err = client.post("/api/v1/core", json={"op": "nope"})
        assert err.status_code == 200
        assert err.json()["status"] == "error"
        assert err.json()["code"] == "bad_request"


def test_capabilities_reports_core_versions_and_caches_probe(tmp_path, fake):
    fake.behave(f"print('{OK_VERSION}')")
    with make_client(tmp_path, fake) as client:
        for _ in range(3):
            caps = client.get("/api/v1/capabilities").json()
        assert caps["core_available"] is True
        assert caps["core_crate_version"] == "9.9.9"
        assert caps["core_api_version"] == 1
        assert caps["core_schema_versions"] == ["1.0", "2.0"]
        assert fake.spawns == 1


@pytest.mark.parametrize("content", [b"[1,2]", b"3", b'"s"', b"null", b"not json", b""])
def test_non_object_body_is_422_without_spawning(tmp_path, fake, content):
    fake.behave(f"print('{OK_VERSION}')")
    with make_client(tmp_path, fake) as client:
        assert client.post("/api/v1/core", content=content).status_code == 422
    assert fake.spawns == 0


def test_oversize_body_is_413_without_spawning(tmp_path, fake):
    fake.behave(f"print('{OK_VERSION}')")
    with make_client(tmp_path, fake, core_max_request_bytes=1024) as client:
        big = b'{"op":"validate","pad":"' + b"a" * 2000 + b'"}'
        assert client.post("/api/v1/core", content=big).status_code == 413
        # chunked upload without a Content-Length is capped as well
        assert client.post("/api/v1/core", content=iter([big[:1500], big[1500:]])).status_code == 413
    assert fake.spawns == 0


def test_timeout_is_504_and_child_is_killed(tmp_path, fake):
    fake.behave("import time\ntime.sleep(60)\n")
    with make_client(tmp_path, fake, core_timeout_seconds=1) as client:
        response = client.post("/api/v1/core", json={"op": "version"})
    assert response.status_code == 504
    assert len(fake.procs) == 1
    assert fake.procs[0].poll() is not None  # killed and reaped, not left sleeping


@pytest.mark.parametrize(
    "body",
    [
        "print('garbage not json')",
        "print('[1,2,3]')",
        "print('{\"no_status\":1}')",
        "pass",  # no output at all
        "sys.stderr.write('boom ' * 10); sys.exit(1)",
        "sys.stderr.write('usage'); sys.exit(2)",
        "print('" + OK_VERSION + "'); sys.exit(1)",
    ],
)
def test_bad_child_is_502(tmp_path, fake, body):
    fake.behave(body + "\n")
    with make_client(tmp_path, fake) as client:
        response = client.post("/api/v1/core", json={"op": "version"})
    assert response.status_code == 502


def test_502_detail_has_truncated_stderr_tail_only(tmp_path, fake):
    fake.behave("sys.stderr.write('x' * 50000 + 'TAIL'); sys.exit(1)\n")
    with make_client(tmp_path, fake) as client:
        detail = client.post("/api/v1/core", json={"op": "version"}).json()["detail"]
    assert detail.startswith("promptcad-core exited with 1")
    assert len(detail) <= 2000


def test_child_environment_has_no_secrets(tmp_path, fake, monkeypatch):
    monkeypatch.setenv("PROMPTCAD_API_TOKEN", "super-secret")
    monkeypatch.setenv("PROMPTCAD_LLM_API_KEY", "llm-secret")
    fake.behave(
        """
        import os
        leaked = [k for k in os.environ if "SECRET" in k.upper() or k.startswith("PROMPTCAD_")]
        print('{"status":"ok","result":{"leaked":%s}}' % repr(leaked).replace("'", '"'))
        """
    )
    with make_client(tmp_path, fake, api_token="super-secret") as client:
        response = client.post(
            "/api/v1/core", json={"op": "version"}, headers={"X-API-Key": "super-secret"}
        )
        assert response.json()["result"]["leaked"] == []


def test_auth_required_like_sibling_endpoints(tmp_path, fake):
    fake.behave(f"print('{OK_VERSION}')")
    headers = {"X-API-Key": "top-secret"}
    with make_client(tmp_path, fake, api_token="top-secret") as client:
        assert client.post("/api/v1/core", json={"op": "version"}).status_code == 401
        assert client.post("/api/v1/core", json={"op": "version"}, headers={"X-API-Key": "bad"}).status_code == 401
        bearer = {"Authorization": "Bearer top-secret"}
        assert client.post("/api/v1/core", json={"op": "version"}, headers=bearer).status_code == 200
        assert client.post("/api/v1/core", json={"op": "version"}, headers=headers).status_code == 200
        assert client.get("/api/v1/capabilities").status_code == 401
    assert fake.spawns == 2  # the rejected calls never spawned a child


def test_find_core_binary_prefers_setting(tmp_path):
    binary = tmp_path / "pc"
    binary.write_text("x")
    settings = Settings(env="test", core_bin=binary)
    assert core_bridge.find_core_binary(settings) == binary
    assert core_bridge.find_core_binary(Settings(env="test", core_bin=tmp_path / "missing")) is None
    bridge = CoreBridge(settings)
    assert bridge.binary() == binary
    bridge.override_binary(None)
    assert bridge.binary() is None
