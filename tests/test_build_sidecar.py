"""Pure tests for the desktop sidecar build/smoke scripts (ADR 0010 Phase 1b)."""

from __future__ import annotations

import importlib.util
import sys
import types
from pathlib import Path

import pytest

from app.models.cad import CadDocument, HoleType
from app.services.validator import DesignValidator

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str):
    spec = importlib.util.spec_from_file_location(f"_{name}", ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module  # dataclasses resolve annotations via sys.modules
    spec.loader.exec_module(module)
    return module


build_sidecar = _load("build_sidecar")
frozen_smoke = _load("frozen_smoke")

ENTRY = Path("entry.py")


def _flag_values(args: list[str], flag: str) -> list[str]:
    return [args[i + 1] for i, arg in enumerate(args) if arg == flag]


def test_onedir_not_onefile():
    args = build_sidecar.pyinstaller_args(ENTRY)
    assert "--onedir" in args
    assert "--onefile" not in args
    assert "--noupx" in args
    assert _flag_values(args, "--name") == ["promptcad-sidecar"]


def test_output_matches_layout_contract():
    args = build_sidecar.pyinstaller_args(ENTRY)
    binaries = ROOT / "src-tauri" / "binaries"
    assert _flag_values(args, "--distpath") == [str(binaries)]
    exe = "promptcad-sidecar" + (".exe" if sys.platform == "win32" else "")
    assert build_sidecar.output_exe() == binaries / "promptcad-sidecar" / exe
    # frozen_smoke drives the same path by default.
    assert build_sidecar.output_exe() == frozen_smoke.DEFAULT_EXE


def test_cadquery_bundled_and_prune_list_applied():
    args = build_sidecar.pyinstaller_args(ENTRY)
    excluded = set(_flag_values(args, "--exclude-module"))
    hidden = set(_flag_values(args, "--hidden-import"))
    assert "cadquery" not in excluded
    assert "OCP" not in excluded
    assert "cadquery" in hidden
    assert {"app.render_worker", "uvicorn.lifespan.on", "uvicorn.protocols.http.h11_impl"} <= hidden
    pruned = {"scipy", "llvmlite", "numba", "matplotlib", "IPython", "PIL", "PyQt5", "PySide6",
              "tkinter", "pytest"}
    assert pruned <= excluded
    # The runtime import graph of a generated model.py must never be pruned.
    keep = {"vtk", "vtkmodules", "casadi", "nlopt", "ezdxf", "numpy", "multimethod", "pyparsing"}
    assert not keep & excluded


def test_no_cad_build_excludes_cadquery():
    args = build_sidecar.pyinstaller_args(ENTRY, include_cad=False)
    assert "cadquery" in _flag_values(args, "--exclude-module")
    assert "cadquery" not in _flag_values(args, "--hidden-import")


def test_pyinstaller_args_has_no_static_data(tmp_path):
    # app/static is gone and app/web_dist must never be frozen in (Tauri serves the UI),
    # even when a stale checkout still has either directory on disk.
    assert build_sidecar.DATA_DIRS == ()
    for name in ("static", "web_dist"):
        (tmp_path / "app" / name).mkdir(parents=True)
        (tmp_path / "app" / name / "index.html").write_text("x", encoding="utf-8")
    for root in (ROOT, tmp_path):
        assert "--add-data" not in build_sidecar.pyinstaller_args(ENTRY, root=root)


def test_clean_stale_outputs(tmp_path):
    onefile = tmp_path / "promptcad-sidecar-x86_64-pc-windows-msvc.exe"
    onefile.write_bytes(b"old")
    onedir = tmp_path / "promptcad-sidecar"
    (onedir / "_internal").mkdir(parents=True)
    (onedir / "promptcad-sidecar.exe").write_bytes(b"")
    unrelated = tmp_path / "README.txt"
    unrelated.write_text("keep", encoding="utf-8")

    removed = build_sidecar.clean_stale_outputs(tmp_path)

    assert set(removed) == {onefile, onedir}
    assert not onefile.exists()
    assert not onedir.exists()
    assert unrelated.exists()
    assert build_sidecar.clean_stale_outputs(tmp_path) == []


def test_refuses_without_cadquery(monkeypatch, capsys):
    calls = []
    fake_main = types.ModuleType("PyInstaller.__main__")
    fake_main.run = lambda args: calls.append(args)
    fake_pkg = types.ModuleType("PyInstaller")
    fake_pkg.__main__ = fake_main
    monkeypatch.setitem(sys.modules, "PyInstaller", fake_pkg)
    monkeypatch.setitem(sys.modules, "PyInstaller.__main__", fake_main)
    monkeypatch.setattr(build_sidecar, "_cadquery_importable", lambda: False)
    monkeypatch.setattr(build_sidecar, "clean_stale_outputs", lambda *a: pytest.fail("cleaned"))

    assert build_sidecar.main([]) == 2
    assert calls == []
    assert "--no-cad" in capsys.readouterr().err


def test_smoke_cases_cover_every_base_and_hole_type():
    cases = frozen_smoke.smoke_cases()
    validator = DesignValidator()
    bases = set()
    for label, spec in cases:
        doc = CadDocument.model_validate(spec)
        assert validator.validate(doc).valid, label
        bases.add(doc.base.kind)
        if not label.startswith("example:"):
            assert {hole.hole_type for hole in doc.holes} == set(HoleType), label
    assert bases == {"plate", "cylinder", "ring", "l_bracket", "enclosure"}
