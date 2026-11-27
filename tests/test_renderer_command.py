from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

import pytest

from app.services import renderer as renderer_module
from app.services.renderer import RENDER_ROOT_ENV, Renderer, _cadquery_command


def test_unfrozen_command_runs_model_with_interpreter(tmp_path, monkeypatch):
    monkeypatch.delattr(sys, "frozen", raising=False)
    job = tmp_path / "job"

    command = _cadquery_command(job, {"svg", "step", "stl", "dxf", "obj"})

    # Byte-identical to the pre-ADR-0010-Phase-1b command used by the server and Docker.
    assert command == [
        sys.executable,
        str(job / "model.py"),
        "--output-dir",
        str(job),
        "--formats",
        "dxf",
        "step",
        "stl",
        "svg",
    ]


def test_unfrozen_command_keeps_relative_job_dir(monkeypatch):
    monkeypatch.delattr(sys, "frozen", raising=False)
    job = Path("generated") / "abc"

    command = _cadquery_command(job, ["stl"])

    assert command[1:4] == [str(job / "model.py"), "--output-dir", str(job)]


def test_frozen_command_reinvokes_sidecar_as_render_worker(tmp_path, monkeypatch):
    monkeypatch.setattr(sys, "frozen", True, raising=False)
    job = tmp_path / "job"
    job.mkdir()

    command = _cadquery_command(job, {"stl", "step", "png"})

    resolved = job.resolve()
    assert command == [
        sys.executable,
        "--render-worker",
        str(resolved / "model.py"),
        "--output-dir",
        str(resolved),
        "--formats",
        "step",
        "stl",
    ]


def test_frozen_command_uses_absolute_paths(tmp_path, monkeypatch):
    monkeypatch.setattr(sys, "frozen", True, raising=False)
    monkeypatch.chdir(tmp_path)
    (tmp_path / "generated" / "abc").mkdir(parents=True)

    command = _cadquery_command(Path("generated") / "abc", ["svg"])

    assert Path(command[2]).is_absolute()
    assert Path(command[4]).is_absolute()
    assert Path(command[2]).parent == Path(command[4])


def test_cadquery_env_drops_token_and_sets_render_root(settings, monkeypatch):
    monkeypatch.setenv("PROMPTCAD_API_TOKEN", "secret-token")
    monkeypatch.setenv("PROMPTCAD_LLM_API_KEY", "secret-llm-key")
    monkeypatch.setenv("PROMPTCAD_SOMETHING_ELSE", "kept")

    env = Renderer(settings)._cadquery_env()

    assert "PROMPTCAD_API_TOKEN" not in {key.upper() for key in env}
    assert "PROMPTCAD_LLM_API_KEY" not in {key.upper() for key in env}
    assert "secret-token" not in env.values()
    assert "secret-llm-key" not in env.values()
    assert env["PROMPTCAD_SOMETHING_ELSE"] == "kept"
    assert env[RENDER_ROOT_ENV] == str(settings.data_dir.resolve())
    assert Path(env[RENDER_ROOT_ENV]).is_absolute()


def test_render_passes_command_env_and_flags_to_subprocess(settings, tmp_path, monkeypatch):
    monkeypatch.delattr(sys, "frozen", raising=False)
    monkeypatch.setenv("PROMPTCAD_API_TOKEN", "secret-token")
    settings.render_backend = "cadquery"
    monkeypatch.setattr(Renderer, "cadquery_available", staticmethod(lambda: True))
    job = tmp_path / "job"
    job.mkdir()
    calls = []

    def fake_run(command, **kwargs):
        calls.append((command, kwargs))
        return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

    monkeypatch.setattr(renderer_module.subprocess, "run", fake_run)

    result = Renderer(settings).render(job, ["step", "stl"])

    assert result.renderer == "cadquery"
    (command, kwargs) = calls[0]
    assert command == _cadquery_command(job, {"step", "stl"})
    assert kwargs["cwd"] == job.resolve()
    assert kwargs["shell"] is False
    assert kwargs["stdin"] is subprocess.DEVNULL
    assert kwargs["timeout"] == settings.render_timeout_seconds
    assert "PROMPTCAD_API_TOKEN" not in kwargs["env"]
    assert kwargs["env"][RENDER_ROOT_ENV] == str(settings.data_dir.resolve())
    if sys.platform == "win32":
        assert kwargs["creationflags"] == subprocess.CREATE_NO_WINDOW
    else:
        assert kwargs["creationflags"] == 0


def test_render_resolves_relative_job_dir(settings, tmp_path, monkeypatch):
    # Regression: with the default relative data_dir ("generated"), the subprocess ran with
    # cwd=job_dir *and* a job_dir-relative model path, so model.py was looked up twice deep.
    monkeypatch.delattr(sys, "frozen", raising=False)
    monkeypatch.chdir(tmp_path)
    settings.render_backend = "cadquery"
    monkeypatch.setattr(Renderer, "cadquery_available", staticmethod(lambda: True))
    job = Path("generated") / "abc"
    job.mkdir(parents=True)
    calls = []

    def fake_run(command, **kwargs):
        calls.append((command, kwargs))
        return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

    monkeypatch.setattr(renderer_module.subprocess, "run", fake_run)

    Renderer(settings).render(job, ["stl"])

    (command, kwargs) = calls[0]
    model = Path(command[1])
    assert model.is_absolute()
    assert model == (tmp_path / "generated" / "abc" / "model.py").resolve()
    assert Path(kwargs["cwd"]).is_absolute()


@pytest.mark.parametrize("error", [ImportError("broken"), ValueError("no spec")])
def test_cadquery_available_survives_find_spec_raising(monkeypatch, error):
    def raising(name, *args, **kwargs):
        raise error

    monkeypatch.setattr(importlib.util, "find_spec", raising)

    assert Renderer.cadquery_available() is False


def test_openscad_is_skipped_when_not_allowed(settings, tmp_path, monkeypatch):
    settings.render_backend = "openscad"
    monkeypatch.setattr(Renderer, "openscad_available", staticmethod(lambda: True))

    def boom(*args, **kwargs):
        raise AssertionError("openscad must not run")

    monkeypatch.setattr(renderer_module.subprocess, "run", boom)
    result = Renderer(settings).render(tmp_path, ["step", "stl"], allow_openscad=False)

    assert result.renderer == "source_only"
    assert result.status == "source_only"
    assert any("OpenSCAD" in warning for warning in result.warnings)


def test_cadquery_failure_does_not_fall_back_to_openscad_when_not_allowed(
    settings, tmp_path, monkeypatch
):
    settings.render_backend = "cadquery"
    settings.allow_source_fallback = True
    monkeypatch.setattr(Renderer, "cadquery_available", staticmethod(lambda: True))
    monkeypatch.setattr(Renderer, "openscad_available", staticmethod(lambda: True))
    calls = []

    def fake_run(command, **kwargs):
        calls.append(command)
        return subprocess.CompletedProcess(command, 1, stdout="", stderr="cq broke")

    monkeypatch.setattr(renderer_module.subprocess, "run", fake_run)
    result = Renderer(settings).render(tmp_path, ["stl"], allow_openscad=False)

    assert result.renderer == "source_only"
    assert len(calls) == 1  # only the CadQuery attempt
    assert any("cq broke" in warning for warning in result.warnings)
