"""Pure tests for the desktop sidecar build/smoke scripts (ADR 0010 Phase 1b)."""

from __future__ import annotations

import importlib.util
import os
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


# --------------------------------------------------------------------------- slim (ADR 0011)

nd = build_sidecar.native_deps


def test_vtk_shim_searched_first_and_web_viewer_excluded():
    shim = Path("/b/vtk_shim")
    args = build_sidecar.pyinstaller_args(ENTRY, shim_dir=shim)
    paths = _flag_values(args, "--paths")
    assert paths[0] == str(shim) and str(ROOT) in paths[1:]
    assert str(shim) not in _flag_values(
        build_sidecar.pyinstaller_args(ENTRY, include_cad=False, shim_dir=shim), "--paths"
    )
    excluded = set(_flag_values(args, "--exclude-module"))
    assert {"cadquery.fig", "trame", "trame_vtk", "trame_vuetify", "wslink", "aiohttp"} <= excluded
    assert "vtk" not in excluded and "cadquery" not in excluded


def test_vtk_shim_source_and_keepset_script_compile():
    shim = compile(build_sidecar.VTK_SHIM_SOURCE, "vtk.py", "exec")
    assert build_sidecar.VTK_SHIM_MARKER in shim.co_names
    assert "vtkmodules.vtkCommonCore" in build_sidecar.VTK_SHIM_SOURCE
    compile(build_sidecar.KEEPSET_SCRIPT, "<keepset>", "exec")
    assert build_sidecar.VTK_SHIM_MARKER in build_sidecar.KEEPSET_SCRIPT


def test_write_vtk_shim(tmp_path):
    (tmp_path / "shim").mkdir()
    (tmp_path / "shim" / "stale.py").write_text("x", encoding="utf-8")
    path = build_sidecar.write_vtk_shim(tmp_path / "shim")
    assert path.name == "vtk.py"
    assert sorted(p.name for p in path.parent.iterdir()) == ["vtk.py"]


@pytest.mark.parametrize(
    ("name", "stem"),
    [
        ("vtkCommonCore.cp312-win_amd64.pyd", "vtkCommonCore"),
        ("vtkIOXML.cpython-312-x86_64-linux-gnu.so", "vtkIOXML"),
        ("vtkRenderingCore.cpython-312-darwin.so", "vtkRenderingCore"),
        ("vtkCommonCore.abi3.so", "vtkCommonCore"),
        ("vtkCommonCore.pyd", "vtkCommonCore"),
        ("libvtkCommonCore-9.6.so.1", None),
        ("vtkCommonCore-9.6.2.dll", None),
        ("__init__.py", None),
        ("_version.py", None),
    ],
)
def test_vtk_extension_stem(name, stem):
    assert build_sidecar.vtk_extension_stem(name) == stem


def test_unused_vtk_extensions():
    d = Path("/b/_internal/vtkmodules")
    files = [
        d / "vtkCommonCore.cp312-win_amd64.pyd",
        d / "vtkIOXML.cp312-win_amd64.pyd",
        d / "vtkChartsCore.cp312-win_amd64.pyd",
        d / "__init__.py",
    ]
    keep = ["vtkmodules.vtkCommonCore", "vtkmodules.vtkIOXML", "vtkmodules.util"]
    assert build_sidecar.unused_vtk_extensions(files, keep) == [files[2]]


@pytest.mark.parametrize(
    ("name", "kind", "expected"),
    [
        ("vtkCommonCore-9.6.2.dll", nd.PE, True),
        ("VTKCOMMONCORE-9.6.2.DLL", nd.PE, True),
        ("vtkWrappingPythonCore3.12-9.6.2.dll", nd.PE, True),
        ("viskores_cont-9.6.2.dll", nd.PE, True),
        ("TKIVtk-9a979cfa5c26dd16069397f8760edae0.dll", nd.PE, False),
        ("msvcp140-ba05f997e25eac0cb10acfddf6513d26.dll", nd.PE, False),
        ("vtkCommonCore.cp312-win_amd64.pyd", nd.PE, False),
        ("openxr_loader-47d29b2f0e7df7fb367af5ce36fd428d.dll", nd.PE, False),
        ("libvtkCommonCore-9.6.so.1", nd.ELF, True),
        ("libvtkCommonCore-9.6.so", nd.ELF, True),
        ("libTKIVtk.so.7.9", nd.ELF, False),
        ("vtkCommonCore.cpython-312-x86_64-linux-gnu.so", nd.ELF, False),
        ("libvtkCommonCore-9.6.9.6.dylib", nd.MACHO, True),
        ("libviskores_cont-9.6.dylib", nd.MACHO, True),
        ("libomp.dylib", nd.MACHO, False),
    ],
)
def test_is_vtk_native(name, kind, expected):
    assert build_sidecar.is_vtk_native(name, kind) is expected


def _bundle(tmp_path: Path, files: dict[str, bytes]) -> Path:
    onedir = tmp_path / "promptcad-sidecar"
    for rel, data in files.items():
        path = onedir / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    return onedir


def test_prune_bundle_pe(tmp_path):
    onedir = _bundle(
        tmp_path,
        {
            "promptcad-sidecar.exe": b"e",
            "_internal/OCP/OCP.cp312-win_amd64.pyd": b"o",
            "_internal/cadquery_ocp.libs/TKIVtk-abc.dll": b"t",
            "_internal/vtkmodules/vtkCommonCore.cp312-win_amd64.pyd": b"c" * 3,
            "_internal/vtkmodules/vtkChartsCore.cp312-win_amd64.pyd": b"h" * 5,
            "_internal/vtkmodules/__init__.py": b"",
            "_internal/vtk.libs/vtkCommonCore-9.6.2.dll": b"C" * 7,
            "_internal/vtk.libs/vtkRenderingOpenGL2-9.6.2.dll": b"G" * 11,
            "_internal/vtk.libs/vtkglad-9.6.2.dll": b"g" * 13,
            "_internal/vtk.libs/vtkChartsCore-9.6.2.dll": b"X" * 17,
            "_internal/vtk.libs/viskores_cont-9.6.2.dll": b"V" * 19,
            "_internal/vtk.libs/msvcp140-hash.dll": b"m",  # not VTK-named: never deleted
        },
    )
    deps = {
        "promptcad-sidecar.exe": ["KERNEL32.dll"],
        "OCP.cp312-win_amd64.pyd": ["TKIVtk-abc.dll"],
        "TKIVtk-abc.dll": ["vtkCommonCore-9.6.2.dll", "VTKRENDERINGOPENGL2-9.6.2.DLL"],
        "vtkCommonCore.cp312-win_amd64.pyd": ["vtkCommonCore-9.6.2.dll", "python312.dll"],
        "vtkChartsCore.cp312-win_amd64.pyd": ["vtkChartsCore-9.6.2.dll"],
        "vtkCommonCore-9.6.2.dll": ["msvcp140-hash.dll"],
        "vtkRenderingOpenGL2-9.6.2.dll": ["vtkCommonCore-9.6.2.dll", "vtkglad-9.6.2.dll"],
        "vtkglad-9.6.2.dll": [],
        "vtkChartsCore-9.6.2.dll": ["viskores_cont-9.6.2.dll"],
        "viskores_cont-9.6.2.dll": [],
        "msvcp140-hash.dll": [],
    }
    seen = []

    def deps_of(path):
        seen.append(path.name)
        return deps[path.name]

    keep = ["vtkmodules.vtkCommonCore", "vtkmodules.util"]
    dry = build_sidecar.prune_bundle(onedir, keep, kind=nd.PE, deps_of=deps_of, dry_run=True)
    assert (onedir / "_internal/vtk.libs/vtkChartsCore-9.6.2.dll").exists()

    result = build_sidecar.prune_bundle(onedir, keep, kind=nd.PE, deps_of=deps_of)
    assert result.removed_native == dry.removed_native == [
        "_internal/vtkmodules/vtkChartsCore.cp312-win_amd64.pyd",
        "_internal/vtk.libs/viskores_cont-9.6.2.dll",
        "_internal/vtk.libs/vtkChartsCore-9.6.2.dll",
    ]
    assert result.removed_bytes == 5 + 19 + 17
    assert result.kept_native == [
        "_internal/vtk.libs/vtkCommonCore-9.6.2.dll",
        "_internal/vtk.libs/vtkRenderingOpenGL2-9.6.2.dll",
        "_internal/vtk.libs/vtkglad-9.6.2.dll",
        "_internal/vtkmodules/vtkCommonCore.cp312-win_amd64.pyd",
    ]
    for rel in result.removed_native:
        assert not (onedir / rel).exists()
    for rel in [*result.kept_native, "_internal/vtk.libs/msvcp140-hash.dll",
                "_internal/vtkmodules/__init__.py"]:
        assert (onedir / rel).exists()
    assert result.unresolved == ["KERNEL32.dll", "python312.dll"]
    assert result.external_system_libs == []  # Windows
    assert result.keep_modules == sorted(keep)
    # A deleted extension module is no root, so its library is not reached through it.
    assert "vtkChartsCore.cp312-win_amd64.pyd" not in seen


def test_prune_bundle_elf_reports_system_libs_and_keeps_symlinked(tmp_path):
    onedir = _bundle(
        tmp_path,
        {
            "promptcad-sidecar": b"e",
            "_internal/vtkmodules/vtkCommonCore.cpython-312-x86_64-linux-gnu.so": b"c",
            "_internal/vtkmodules/libvtkCommonCore-9.6.so.1": b"C" * 3,
            "_internal/vtkmodules/libvtkIOMovie-9.6.so.1": b"M" * 5,
        },
    )
    link: Path | None = onedir / "_internal" / "libvtkCommonCore-9.6.so"
    try:
        os.symlink(Path("vtkmodules") / "libvtkCommonCore-9.6.so.1", link)
    except (OSError, NotImplementedError):
        link = None
    deps = {
        "promptcad-sidecar": ["libc.so.6", "libz.so.1"],
        "vtkCommonCore.cpython-312-x86_64-linux-gnu.so": ["libvtkCommonCore-9.6.so.1"],
        "libvtkCommonCore-9.6.so.1": ["libGL.so.1", "libX11.so.6", "libm.so.6", "libc.so.6"],
        "libvtkIOMovie-9.6.so.1": ["libvtkCommonCore-9.6.so.1"],
    }
    keep = ["vtkmodules.vtkCommonCore"]
    result = build_sidecar.prune_bundle(
        onedir, keep, kind=nd.ELF, deps_of=lambda p: deps[p.resolve().name]
    )
    assert result.removed_native == ["_internal/vtkmodules/libvtkIOMovie-9.6.so.1"]
    assert result.external_system_libs == ["libGL.so.1", "libX11.so.6", "libz.so.1"]
    if link is not None:  # a link to a reached library survives with it
        assert link.is_symlink()
        assert "_internal/libvtkCommonCore-9.6.so" in result.kept_native


def test_prune_bundle_refuses_on_unreadable_root(tmp_path):
    onedir = _bundle(
        tmp_path,
        {"promptcad-sidecar.exe": b"e", "_internal/vtk.libs/vtkCommonCore-9.6.2.dll": b"c"},
    )

    def deps_of(path):
        raise OSError("not a PE file")

    with pytest.raises(build_sidecar.BuildError, match="--no-prune"):
        build_sidecar.prune_bundle(onedir, [], kind=nd.PE, deps_of=deps_of)
    assert (onedir / "_internal/vtk.libs/vtkCommonCore-9.6.2.dll").exists()


def test_bundled_top_levels_and_pruned_dists():
    bundled = build_sidecar.bundled_top_levels(
        ["cadquery", "cadquery.occ_impl", "OCP.gp", "vtk", "fastapi"],
        ["vtkmodules", "numpy.libs", "python312.dll", "base_library.zip"],
        ["encodings/__init__.pyc", "collections/abc.pyc", "abc.pyc"],
    )
    assert {"cadquery", "ocp", "vtk", "vtkmodules", "numpy", "encodings", "abc"} <= bundled
    record = [
        "trame_client/__init__.py",
        "trame/modules/client.py",
        "trame_client-4.1.4.dist-info/RECORD",
        "../../Scripts/trame.exe",
        "PIL/Image.py",
        "x.pth",
        "six.py",
    ]
    assert build_sidecar.dist_top_levels(record) == {"trame_client", "trame", "pil", "six"}
    installed = {
        "aiohttp": ["aiohttp/__init__.py"],
        "fastapi": ["fastapi/__init__.py"],
        "trame-vtk": ["trame/widgets/vtk.py", "trame_vtk/__init__.py"],
    }
    absent, present = build_sidecar.split_pruned_dists(
        ["aiohttp", "fastapi", "trame-vtk", "not-installed"], installed, bundled
    )
    assert absent == ["aiohttp", "trame-vtk", "not-installed"]
    assert present == {"fastapi": ["fastapi"]}


def test_pruned_dists_cover_excluded_web_stack():
    dists = set(build_sidecar.PRUNED_DISTS)
    assert {"trame", "trame-vtk", "trame-vuetify", "wslink", "aiohttp", "scipy", "matplotlib",
            "pillow"} <= dists
    runtime = {"cadquery", "cadquery-ocp", "vtk", "numpy", "casadi", "nlopt", "ezdxf", "fastapi",
               "uvicorn", "pydantic"}
    assert not runtime & dists
    assert list(build_sidecar.PRUNED_DISTS) == sorted(build_sidecar.PRUNED_DISTS)


def test_build_manifest_schema():
    prune = build_sidecar.PruneResult(
        keep_modules=["vtkmodules.vtkCommonCore"],
        kept_native=["_internal/vtk.libs/vtkCommonCore-9.6.2.dll"],
        removed_native=["_internal/vtk.libs/vtkChartsCore-9.6.2.dll"],
        unresolved=["KERNEL32.dll"],
        external_system_libs=[],
    )
    manifest = build_sidecar.build_manifest(
        platform="win32",
        machine="AMD64",
        stats=(400 * 1_048_576, 880, 0),
        pruned=True,
        max_mib=460.0,
        prune=prune,
        pruned_dists=["wslink", "aiohttp"],
        before_bytes=1,
    )
    required = {"platform", "total_bytes", "kept_native", "removed_native",
                "external_system_libs", "pruned_dists"}
    assert required <= set(manifest)
    assert manifest["schema"] == 1
    assert manifest["total_mib"] == 400.0
    assert manifest["pruned_dists"] == ["aiohttp", "wslink"]
    assert manifest["removed_native"] == prune.removed_native
    unpruned = build_sidecar.build_manifest(
        platform="linux",
        machine="x86_64",
        stats=(1, 1, 3),
        pruned=False,
        max_mib=None,
        prune=None,
        pruned_dists=[],
    )
    assert unpruned["kept_native"] == unpruned["external_system_libs"] == []
    assert unpruned["symlinks"] == 3


def test_effective_max_mib():
    f = build_sidecar.effective_max_mib
    assert f(None, no_prune=False, platform="win32") == 460.0
    assert f(None, no_prune=False, platform="linux") is None
    assert f(None, no_prune=True, platform="win32") is None
    assert f(500.0, no_prune=True, platform="linux") == 500.0
    assert f(0.0, no_prune=False, platform="win32") is None


def test_tree_stats_matches_frozen_smoke(tmp_path):
    onedir = _bundle(tmp_path, {"a.exe": b"1" * 4, "_internal/x/y.dll": b"2" * 6})
    assert build_sidecar.tree_stats(onedir) == frozen_smoke.tree_stats(onedir) == (10, 2, 0)


def test_compute_keep_set_requires_models(tmp_path):
    with pytest.raises(build_sidecar.BuildError, match="no example"):
        build_sidecar.compute_keep_set(tmp_path, models=[])


def test_prune_bundle_refuses_on_unresolved_vtk_dependency(tmp_path):
    onedir = _bundle(
        tmp_path,
        {
            "promptcad-sidecar": b"e",
            "_internal/vtkmodules/vtkCommonCore.cpython-312-x86_64-linux-gnu.so": b"c",
            # file name differs from the soname the extension records
            "_internal/vtkmodules/libvtkCommonCore-9.6.so.1.2": b"C",
            "_internal/vtkmodules/libvtkIOMovie-9.6.so.1": b"M",
        },
    )
    deps = {
        "promptcad-sidecar": ["libc.so.6"],
        "vtkCommonCore.cpython-312-x86_64-linux-gnu.so": ["libvtkCommonCore-9.6.so.1"],
        "libvtkCommonCore-9.6.so.1.2": ["libc.so.6"],
        "libvtkIOMovie-9.6.so.1": ["libc.so.6"],
    }
    with pytest.raises(build_sidecar.BuildError, match="libvtkCommonCore-9.6.so.1"):
        build_sidecar.prune_bundle(
            onedir, ["vtkmodules.vtkCommonCore"], kind=nd.ELF, deps_of=lambda p: deps[p.name]
        )
    assert (onedir / "_internal/vtkmodules/libvtkIOMovie-9.6.so.1").exists()
