"""Unix stdin-EOF kill-on-close policy (app/parent_watch.py), tested with fakes on any OS.

The real pipe/process-group behaviour is covered on unix by tests/test_sidecar.py and by
scripts/frozen_kill_check.py in desktop-bundle.yml.
"""

from __future__ import annotations

import io
import os
import sys
import threading

import pytest

from app import parent_watch as pw


class Recorder:
    def __init__(self) -> None:
        self.calls: list[tuple[str, int, int]] = []
        self.logs: list[str] = []

    def killpg(self, pgid: int, sig: int) -> None:
        self.calls.append(("killpg", pgid, sig))

    def kill(self, pid: int, sig: int) -> None:
        self.calls.append(("kill", pid, sig))

    def log(self, message: str) -> None:
        self.logs.append(message)


class ChunkStream:
    """A pipe that delivers some chunks, then EOF (or raises)."""

    def __init__(self, chunks: list[bytes], error: Exception | None = None) -> None:
        self.chunks = list(chunks)
        self.error = error
        self.reads = 0

    def read(self, n: int) -> bytes:
        self.reads += 1
        if self.chunks:
            return self.chunks.pop(0)
        if self.error is not None:
            raise self.error
        return b""

    def isatty(self) -> bool:
        return False


class BlockingStream:
    """A pipe that blocks until ``close_write`` (the shell closing its end)."""

    def __init__(self) -> None:
        self._eof = threading.Event()

    def read(self, n: int) -> bytes:
        self._eof.wait(10)
        return b""

    def close_write(self) -> None:
        self._eof.set()

    def isatty(self) -> bool:
        return False


def _watch(stream, rec: Recorder, *, pid: int = 500, pgid: int | None = 500, grace: float = 0.0):
    return pw.StdinEofWatch(
        stream, pid=pid, pgid=pgid, killpg=rec.killpg, kill=rec.kill, grace=grace, log=rec.log
    )


def test_wait_for_eof_drains_until_empty_chunk():
    stream = ChunkStream([b"x", b"yz"])
    pw.wait_for_eof(stream)
    assert stream.reads == 3


@pytest.mark.parametrize("error", [OSError("broken"), ValueError("closed file")])
def test_wait_for_eof_treats_read_errors_as_eof(error):
    stream = ChunkStream([b"x"], error=error)
    pw.wait_for_eof(stream)
    assert stream.reads == 2


def test_wait_for_eof_on_a_real_closed_pipe():
    r, w = os.pipe()
    os.write(w, b"ignored")
    os.close(w)
    with os.fdopen(r, "rb") as stream:
        pw.wait_for_eof(stream)  # returns instead of blocking


@pytest.mark.parametrize(
    ("pid", "pgid", "expected"),
    [
        (500, 500, ("group", 500)),
        (500, 400, ("process", 500)),  # launched inside someone else's group: never kill it
        (500, None, ("process", 500)),
        (1, 1, ("process", 1)),  # never killpg(1)
    ],
)
def test_signal_target(pid, pgid, expected):
    assert pw.signal_target(pid, pgid) == expected


def test_become_group_leader_ignores_errors():
    calls = []

    def ok(pid, pgid):
        calls.append((pid, pgid))

    def eperm(pid, pgid):
        raise PermissionError("session leader")

    assert pw.become_group_leader(ok) is True
    assert calls == [(0, 0)]
    assert pw.become_group_leader(eperm) is False
    assert pw.become_group_leader(None) is False


def test_eof_terminates_then_kills_group_after_grace():
    rec = Recorder()
    watch = _watch(ChunkStream([]), rec, grace=0.01)
    watch.run()
    assert watch.triggered
    assert rec.calls == [("killpg", 500, pw.SIGTERM), ("killpg", 500, pw.SIGKILL)]
    assert any("stdin closed" in line for line in rec.logs)


def test_finish_during_grace_kills_group_immediately():
    rec = Recorder()
    stream = BlockingStream()
    watch = _watch(stream, rec, grace=30.0)
    thread = watch.start()
    stream.close_write()
    # Wait for SIGTERM, then the main thread returns (uvicorn shut down) and calls finish().
    for _ in range(1000):
        if rec.calls:
            break
        threading.Event().wait(0.005)
    assert rec.calls == [("killpg", 500, pw.SIGTERM)]
    watch.finish()
    thread.join(5)
    assert not thread.is_alive(), "the grace wait must end when finish() is called"
    # Exactly one SIGKILL (from finish), none from the watcher thread after the grace wait.
    assert rec.calls == [("killpg", 500, pw.SIGTERM), ("killpg", 500, pw.SIGKILL)]


def test_finish_without_eof_does_nothing():
    rec = Recorder()
    watch = _watch(BlockingStream(), rec)
    watch.finish()
    assert rec.calls == []
    assert not watch.triggered


def test_eof_after_finish_skips_grace():
    rec = Recorder()
    watch = _watch(ChunkStream([]), rec, grace=30.0)
    watch.finish()
    watch.run()
    assert rec.calls == [("killpg", 500, pw.SIGKILL)]


def test_non_leader_only_signals_itself():
    rec = Recorder()
    watch = _watch(ChunkStream([]), rec, pid=500, pgid=77, grace=0.0)
    watch.run()
    assert rec.calls == [("kill", 500, pw.SIGTERM), ("kill", 500, pw.SIGKILL)]


def test_signal_errors_are_logged_not_raised():
    rec = Recorder()

    def gone(pgid, sig):
        raise ProcessLookupError

    def denied(pgid, sig):
        raise PermissionError("nope")

    watch = pw.StdinEofWatch(
        ChunkStream([]), pid=9, pgid=9, killpg=gone, kill=rec.kill, grace=0.0, log=rec.log
    )
    watch.run()  # an empty group is fine
    watch = pw.StdinEofWatch(
        ChunkStream([]), pid=9, pgid=9, killpg=denied, kill=rec.kill, grace=0.0, log=rec.log
    )
    watch.run()
    assert any("cannot send signal" in line for line in rec.logs)


def _install(stream, rec: Recorder, **overrides):
    kwargs = dict(
        platform="linux",
        log=rec.log,
        setpgid=lambda pid, pgid: rec.calls.append(("setpgid", pid, pgid)),
        getpgrp=lambda: 4242,
        getpid=lambda: 4242,
        killpg=rec.killpg,
        kill=rec.kill,
        grace=0.0,
        start=False,
    )
    kwargs.update(overrides)
    return pw.install(stream, **kwargs)


def test_install_on_unix_becomes_leader_and_targets_group():
    rec = Recorder()
    watch = _install(ChunkStream([]), rec)
    assert watch is not None
    assert rec.calls == [("setpgid", 0, 0)]
    assert (watch.kind, watch.target) == ("group", 4242)
    watch.run()
    assert rec.calls[1:] == [("killpg", 4242, pw.SIGTERM), ("killpg", 4242, pw.SIGKILL)]


def test_install_starts_a_daemon_thread():
    rec = Recorder()
    stream = BlockingStream()
    watch = _install(stream, rec, start=True)
    assert watch is not None and watch._thread is not None
    assert watch._thread.daemon and watch._thread.is_alive()
    stream.close_write()
    watch._thread.join(5)
    assert ("killpg", 4242, pw.SIGKILL) in rec.calls


def test_install_when_setpgid_fails_falls_back_to_self():
    rec = Recorder()

    def eperm(pid, pgid):
        raise PermissionError

    watch = _install(ChunkStream([]), rec, setpgid=eperm, getpgrp=lambda: 1000)
    assert (watch.kind, watch.target) == ("process", 4242)
    assert any("not a process group leader" in line for line in rec.logs)


def test_install_is_a_logged_noop_on_windows():
    rec = Recorder()
    assert _install(ChunkStream([]), rec, platform="win32") is None
    assert rec.calls == []
    assert any("ignored on Windows" in line for line in rec.logs)


class TtyStream(ChunkStream):
    def isatty(self) -> bool:
        return True


def test_install_refuses_a_terminal():
    rec = Recorder()
    assert _install(TtyStream([]), rec) is None
    assert rec.calls == []  # no setpgid either: a background group reading a tty gets SIGTTIN
    assert any("terminal" in line for line in rec.logs)


def test_install_without_stdin(monkeypatch):
    rec = Recorder()
    monkeypatch.setattr(sys, "stdin", None)
    assert _install(None, rec) is None
    assert any("no stdin" in line for line in rec.logs)


def test_install_defaults_to_stdin_buffer(monkeypatch):
    rec = Recorder()
    fake = io.TextIOWrapper(io.BytesIO(b""))
    monkeypatch.setattr(sys, "stdin", fake)
    watch = _install(None, rec)
    assert watch is not None and watch.stream is fake.buffer


def test_signal_constants():
    assert pw.SIGKILL == 9
    assert pw.SIGTERM == 15
    assert pw.FLAG == "--exit-on-stdin-eof"
