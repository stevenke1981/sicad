"""Bounded core transport: concurrency gate, 503 + Retry-After, cancellation, binary lookup,
negative probe cache and the body shape check, all against FAKE promptcad-core children.

The fake is a real Python child: ``subprocess.Popen`` inside ``core_bridge`` is patched so that
``[<binary>, "rpc"]`` runs ``[sys.executable, fake.py, "rpc"]``; every spawn is counted and every
child logs its start/end so the real overlap can be computed.
"""

from __future__ import annotations

import asyncio
import json
import subprocess
import sys
import textwrap
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import httpx
import pytest
from fastapi.testclient import TestClient

from app.core.config import Settings
from app.main import create_app
from app.services import core_bridge
from app.services.core_bridge import (
    CORE_MAX_RESPONSE_BYTES,
    MAX_RESPONSE_BYTES,
    CoreBridge,
    CoreFailed,
)

OK_VERSION = (
    '{"status":"ok","result":{"op":"version","crate_version":"9.9.9",'
    '"api_version":1,"schema_versions":["1.0","2.0"]}}'
)
BAD_REQUEST = '{"status":"error","code":"bad_request","message":"x","path":"","violations":[]}'


class Fake:
    def __init__(self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
        self.binary = tmp_path / "promptcad-core"
        self.binary.write_text("placeholder")
        self.script = tmp_path / "fake_core.py"
        self.events = tmp_path / "events"
        self.events.mkdir()
        self.procs: list[subprocess.Popen] = []
        self.spawns = 0
        real_popen = subprocess.Popen

        def popen(argv, **kwargs):
            assert argv[0] == str(self.binary) and argv[1:] == ["rpc"]
            self.spawns += 1
            proc = real_popen([sys.executable, str(self.script), "rpc"], **kwargs)
            self.procs.append(proc)
            return proc

        monkeypatch.setattr(core_bridge.subprocess, "Popen", popen)

    def behave(self, body: str, sleep: float = 0.0, out: str = OK_VERSION) -> None:
        # One event file per child (concurrent appends to a shared file lose lines on Windows).
        header = (
            "import sys, os, time\n"
            "data = sys.stdin.buffer.read()\n"
            f"path = os.path.join({str(self.events)!r}, str(os.getpid()))\n"
            "t0 = time.time()\n"
            "open(path + '.s', 'w').write('%f' % t0)\n"
            f"time.sleep({sleep!r})\n"
            "open(path + '.e', 'w').write('%f' % time.time())\n"
        )
        self.script.write_text(
            header + textwrap.dedent(body or f"print({out!r})"), encoding="utf-8"
        )

    def max_overlap(self) -> int:
        events = []
        for start in self.events.glob("*.s"):
            end = start.with_suffix(".e")
            if end.exists():
                events.append((float(start.read_text()), 1))
                events.append((float(end.read_text()), -1))
        events.sort(key=lambda e: (e[0], e[1]))
        live = peak = 0
        for _, delta in events:
            live += delta
            peak = max(peak, live)
        return peak


@pytest.fixture
def fake(tmp_path, monkeypatch):
    return Fake(tmp_path, monkeypatch)


def make_app(tmp_path, fake: Fake | None, **overrides):
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
    return app


def ahttp(app):
    return httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url="http://t")


# --- 1. concurrency gate, bounded wait, queue cap --------------------------------------------


def test_concurrency_never_exceeds_limit_and_overflow_is_503_without_spawn(tmp_path, fake):
    fake.behave("", sleep=0.5)
    app = make_app(tmp_path, fake, core_max_concurrency=3, core_queue_wait_seconds=30)

    async def scenario():
        async with ahttp(app) as client:

            async def one():
                started = time.monotonic()
                response = await client.post("/api/v1/core", content=b'{"op":"version"}')
                return response, time.monotonic() - started

            return await asyncio.gather(*[one() for _ in range(40)])

    results = asyncio.run(scenario())
    ok = [r for r, _ in results if r.status_code == 200]
    busy = [(r, t) for r, t in results if r.status_code == 503]
    assert len(ok) + len(busy) == 40
    # 3 running + 4 x 3 waiting are accepted; everything else is refused at once.
    assert len(ok) == 15 and len(busy) == 25
    assert fake.max_overlap() <= 3
    assert fake.spawns == len(ok)  # rejected requests never spawned anything
    for response, elapsed in busy:
        assert response.headers["Retry-After"].isdigit()
        assert elapsed < 0.4  # rejected immediately, not after queueing behind the children


def test_wait_deadline_gives_503_retry_after_without_spawning(tmp_path, fake):
    fake.behave("", sleep=2.0)
    app = make_app(tmp_path, fake, core_max_concurrency=1, core_queue_wait_seconds=0.3)

    async def scenario():
        async with ahttp(app) as client:
            first = asyncio.create_task(client.post("/api/v1/core", content=b'{"op":"a"}'))
            await asyncio.sleep(0.3)
            started = time.monotonic()
            second = await client.post("/api/v1/core", content=b'{"op":"b"}')
            waited = time.monotonic() - started
            return await first, second, waited

    first, second, waited = asyncio.run(scenario())
    assert first.status_code == 200
    assert second.status_code == 503
    assert second.headers["Retry-After"] == "1"
    assert 0.2 < waited < 1.2
    assert fake.spawns == 1


def test_settings_validation():
    with pytest.raises(ValueError):
        Settings(core_max_concurrency=0)
    with pytest.raises(ValueError):
        Settings(core_queue_wait_seconds=0)
    defaults = Settings()
    assert defaults.core_max_concurrency == 4 and defaults.core_queue_wait_seconds == 3


def test_slow_core_does_not_delay_health_or_use_default_executor(tmp_path, fake):
    fake.behave("", sleep=1.5)
    app = make_app(tmp_path, fake, core_max_concurrency=2, core_queue_wait_seconds=5)
    submitted: list[object] = []

    class SpyExecutor(ThreadPoolExecutor):
        def submit(self, fn, /, *args, **kwargs):
            submitted.append(fn)
            return super().submit(fn, *args, **kwargs)

    async def scenario():
        loop = asyncio.get_running_loop()
        spy = SpyExecutor(max_workers=1)
        loop.set_default_executor(spy)
        async with ahttp(app) as client:
            slow = [
                asyncio.create_task(client.post("/api/v1/core", content=b'{"op":"v"}'))
                for _ in range(2)
            ]
            await asyncio.sleep(0.3)
            worst = 0.0
            for _ in range(20):
                started = time.monotonic()
                assert (await client.get("/api/v1/health")).status_code == 200
                # the default executor is free for render jobs
                assert await loop.run_in_executor(None, lambda: 7) == 7
                worst = max(worst, time.monotonic() - started)
            assert all(not t.done() for t in slow)  # the core calls really were still running
            await asyncio.gather(*slow)
        spy.shutdown()
        return worst

    worst = asyncio.run(scenario())
    assert worst < 0.5
    assert all(getattr(fn, "__name__", "") != "call_bytes" for fn in submitted)


def test_gate_works_across_event_loops(tmp_path, fake):
    fake.behave("")
    bridge = make_app(tmp_path, fake, core_max_concurrency=1).state.core
    for _ in range(3):  # a fresh loop each time; no semaphore bound to a dead loop
        assert asyncio.run(bridge.call({"op": "version"}))["status"] == "ok"
    assert fake.spawns == 3


# --- cancellation / shutdown -----------------------------------------------------------------


def _wait_for(predicate, timeout=5.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.02)
    return False


def test_cancelling_the_request_kills_the_child_and_frees_the_slot(tmp_path, fake):
    fake.behave("", sleep=60)
    app = make_app(tmp_path, fake, core_max_concurrency=1, core_queue_wait_seconds=2)

    async def scenario():
        async with ahttp(app) as client:
            task = asyncio.create_task(client.post("/api/v1/core", content=b'{"op":"v"}'))
            assert await asyncio.to_thread(_wait_for, lambda: fake.spawns == 1)
            await asyncio.sleep(0.2)
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
            assert await asyncio.to_thread(_wait_for, lambda: fake.procs[0].poll() is not None)
            # the slot is free again: the next call is served, not 503
            fake.behave("")
            return await client.post("/api/v1/core", content=b'{"op":"v"}')

    pid = None
    response = asyncio.run(scenario())
    pid = fake.procs[0].pid
    assert fake.procs[0].returncode is not None, f"child {pid} still running"
    assert response.status_code == 200
    assert fake.spawns == 2


def test_shutdown_kills_running_children(tmp_path, fake):
    fake.behave("", sleep=60)
    bridge = make_app(tmp_path, fake, core_max_concurrency=2).state.core

    async def scenario():
        task = asyncio.create_task(bridge.call_raw(b'{"op":"v"}'))
        assert await asyncio.to_thread(_wait_for, lambda: fake.spawns == 1)
        await asyncio.sleep(0.2)
        started = time.monotonic()
        bridge.shutdown()
        with pytest.raises(CoreFailed):
            await task
        return time.monotonic() - started

    assert asyncio.run(scenario()) < 5
    assert fake.procs[0].poll() is not None


def test_app_lifespan_shuts_the_bridge_down(tmp_path, fake):
    fake.behave("")
    app = make_app(tmp_path, fake)
    with TestClient(app) as client:
        assert client.post("/api/v1/core", json={"op": "version"}).status_code == 200
    assert app.state.core._closed is True


# --- 2. binary lookup ------------------------------------------------------------------------


def _isolate(monkeypatch, tmp_path, path_entries):
    """No next-to-python binary, a controlled PATH, a tmp CWD, no dev fallback by default."""
    fake_python = tmp_path / "pyhome" / "python"
    fake_python.parent.mkdir(exist_ok=True)
    monkeypatch.setattr(sys, "executable", str(fake_python))
    monkeypatch.setenv("PATH", core_bridge.os.pathsep.join(path_entries))
    work = tmp_path / "cwd"
    work.mkdir(exist_ok=True)
    monkeypatch.chdir(work)
    return work


def _make_binary(directory: Path) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    binary = directory / core_bridge._binary_names()[0]
    binary.write_text("x")
    binary.chmod(0o755)
    return binary


def test_cwd_script_is_never_picked_up(tmp_path, monkeypatch):
    work = _isolate(monkeypatch, tmp_path, ["", "."])
    (work / "promptcad-core.cmd").write_text("@echo pwned")
    (work / "promptcad-core.bat").write_text("@echo pwned")
    (work / "promptcad-core").write_text("pwned")
    if sys.platform == "win32":
        (work / "promptcad-core.exe").write_text("x")  # even a CWD .exe: "." and "" are skipped
    settings = Settings(env="production")
    assert core_bridge.find_core_binary(settings) is None
    # an absolute PATH entry that *is* the CWD is skipped as well
    monkeypatch.setenv("PATH", str(work))
    assert core_bridge.find_core_binary(settings) is None


def test_cmd_and_bat_on_path_are_rejected(tmp_path, monkeypatch):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    _isolate(monkeypatch, tmp_path, [str(bin_dir)])
    (bin_dir / "promptcad-core.cmd").write_text("@echo x")
    (bin_dir / "promptcad-core.bat").write_text("@echo x")
    if sys.platform == "win32":
        (bin_dir / "promptcad-core").write_text("not an exe")
    assert core_bridge.find_core_binary(Settings(env="production")) is None


def test_path_lookup_returns_absolute_resolved_file(tmp_path, monkeypatch):
    bin_dir = tmp_path / "bin"
    expected = _make_binary(bin_dir)
    _isolate(monkeypatch, tmp_path, ["", str(bin_dir)])
    found = core_bridge.find_core_binary(Settings(env="production"))
    assert found is not None and found.is_absolute() and found.is_file()
    assert found == expected.resolve()


@pytest.mark.parametrize("kind", ["missing", "directory"])
def test_bad_core_bin_is_unavailable_without_fallback(tmp_path, monkeypatch, kind):
    _make_binary(tmp_path / "bin")
    _isolate(monkeypatch, tmp_path, [str(tmp_path / "bin")])  # a valid one exists on PATH
    bad = tmp_path / "nope"
    if kind == "directory":
        bad.mkdir()
    settings = Settings(env="test", data_dir=tmp_path / "g", core_bin=bad)
    binary, reason = core_bridge.locate_core_binary(settings)
    assert binary is None and "PROMPTCAD_CORE_BIN" in reason
    with TestClient(create_app(settings)) as client:
        response = client.post("/api/v1/core", json={"op": "version"})
        assert response.status_code == 503
        assert "PROMPTCAD_CORE_BIN" in response.json()["detail"]
        assert client.get("/api/v1/capabilities").json()["core_available"] is False


def _fake_checkout(tmp_path, monkeypatch) -> Path:
    root = tmp_path / "checkout"
    module = root / "app" / "services" / "core_bridge.py"
    module.parent.mkdir(parents=True)
    monkeypatch.setattr(core_bridge, "__file__", str(module))
    return _make_binary(root / "crates" / "promptcad-core" / "target" / "release")


def test_dev_fallback_only_in_source_checkout(tmp_path, monkeypatch):
    built = _fake_checkout(tmp_path, monkeypatch)
    _isolate(monkeypatch, tmp_path, [])
    assert core_bridge.find_core_binary(Settings(env="development")) == built.resolve()
    assert core_bridge.find_core_binary(Settings(env="production")) is None
    monkeypatch.setattr(sys, "frozen", True, raising=False)
    assert core_bridge.find_core_binary(Settings(env="development")) is None


# --- 4. negative probe cache -----------------------------------------------------------------


def test_failed_probe_is_cached_then_retried_after_ttl(tmp_path, fake, monkeypatch):
    fake.behave("sys.stderr.write('boom'); sys.exit(1)\n")
    app = make_app(tmp_path, fake)
    with TestClient(app) as client:
        for _ in range(5):
            caps = client.get("/api/v1/capabilities").json()
            assert caps["core_available"] is False
        assert fake.spawns == 1
        app.state.core._probe_failed_until = time.monotonic() - 1  # TTL elapsed
        fake.behave(f"print('{OK_VERSION}')\n")
        assert client.get("/api/v1/capabilities").json()["core_available"] is True
        assert fake.spawns == 2


def test_probe_uses_short_timeout_and_never_blocks_capabilities(tmp_path, fake, monkeypatch):
    fake.behave("", sleep=60)
    monkeypatch.setattr(core_bridge, "PROBE_TIMEOUT_SECONDS", 0.5)
    app = make_app(tmp_path, fake, core_timeout_seconds=15)
    with TestClient(app) as client:
        started = time.monotonic()
        caps = client.get("/api/v1/capabilities").json()
        assert time.monotonic() - started < 3
        assert caps["core_available"] is False
        client.get("/api/v1/capabilities")
        assert fake.spawns == 1  # negative cache
    assert fake.procs[0].poll() is not None


def test_probe_when_busy_is_not_cached(tmp_path, fake):
    fake.behave("", sleep=1.5)
    app = make_app(tmp_path, fake, core_max_concurrency=1, core_queue_wait_seconds=5)
    bridge = app.state.core

    async def scenario():
        busy = asyncio.create_task(bridge.call_raw(b'{"op":"v"}'))
        await asyncio.to_thread(_wait_for, lambda: fake.spawns == 1)
        assert await bridge.version() is None  # slot taken, probe gives up after ~1 s
        assert bridge._probe_failed_until == 0.0
        await busy

    asyncio.run(scenario())


# --- 3. body shape check / nested JSON -------------------------------------------------------


def test_deeply_nested_body_is_not_a_500(tmp_path, fake):
    fake.behave(f"print({BAD_REQUEST!r})\n")
    app = make_app(tmp_path, fake)
    depth = 100_000  # ~200 KB
    body = b'{"op":"validate","document":' + b"[" * depth + b"]" * depth + b"}"
    assert len(body) >= 200_000
    with TestClient(app) as client:
        response = client.post("/api/v1/core", content=body)
    assert response.status_code in (200, 502)
    assert response.status_code != 500
    assert response.json()["code"] == "bad_request"
    assert fake.spawns == 1


def test_malformed_object_body_is_forwarded_and_core_answers(tmp_path, fake):
    fake.behave(f"print({BAD_REQUEST!r})\n")
    app = make_app(tmp_path, fake)
    with TestClient(app) as client:
        response = client.post("/api/v1/core", content=b'  \n{"op": oops')
        assert response.status_code == 200
        assert response.json()["code"] == "bad_request"
        assert client.post("/api/v1/core", content=b'[{"op":"v"}]').status_code == 422
    assert fake.spawns == 1


def test_body_is_not_parsed_on_the_event_loop(tmp_path, fake, monkeypatch):
    fake.behave(f"print('{OK_VERSION}')\n")
    app = make_app(tmp_path, fake)
    REQUEST = b'{"op":"version"}'

    real_loads = json.loads

    def guarded(data, *args, **kwargs):
        if data == REQUEST:
            raise AssertionError("the route must not parse the request body")
        return real_loads(data, *args, **kwargs)

    monkeypatch.setattr("json.loads", guarded)
    with TestClient(app) as client:
        response = client.post("/api/v1/core", content=b'{"op":"version"}')
    assert response.status_code == 200


# --- 6. response size caps -------------------------------------------------------------------


def test_bridge_cap_is_above_the_core_cap_and_accepts_a_max_size_response(tmp_path, fake):
    assert MAX_RESPONSE_BYTES > CORE_MAX_RESPONSE_BYTES
    size = CORE_MAX_RESPONSE_BYTES - 1024
    fake.behave(
        f"""
        sys.stdout.write('{{"status":"ok","result":"' + 'a' * {size} + '"}}\\n')
        """
    )
    app = make_app(tmp_path, fake, core_timeout_seconds=60)
    with TestClient(app) as client:
        response = client.post("/api/v1/core", content=b'{"op":"x"}')
    assert response.status_code == 200
    assert len(response.content) >= size


def test_bridge_still_refuses_absurd_output(tmp_path, fake, monkeypatch):
    monkeypatch.setattr(core_bridge, "MAX_RESPONSE_BYTES", 1024)
    fake.behave("sys.stdout.write('{\"status\":\"ok\",\"result\":\"' + 'a' * 5000 + '\"}')\n")
    bridge = CoreBridge(Settings(env="test"))
    bridge.override_binary(fake.binary)
    with pytest.raises(CoreFailed):
        bridge.call_bytes(b"{}")


# --- 5. Makefile -----------------------------------------------------------------------------


def test_make_core_selects_host_toolchain_off_windows():
    text = (Path(__file__).resolve().parents[1] / "Makefile").read_text(encoding="utf-8")
    assert "ifneq ($(OS),Windows_NT)" in text
    assert "core: export RUSTUP_TOOLCHAIN ?= stable" in text
    assert text.index("RUSTUP_TOOLCHAIN") < text.index("cargo build --release --locked --bins")
