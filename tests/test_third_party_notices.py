"""Pure tests for scripts/gen_third_party_notices.py (fakes only: no cadquery, no network)."""

from __future__ import annotations

import importlib.util
import json
import random
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str):
    spec = importlib.util.spec_from_file_location(f"_{name}", ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module  # dataclasses resolve annotations via sys.modules
    spec.loader.exec_module(module)
    return module


gen = _load("gen_third_party_notices")

MIT_TEXT = "MIT License\n\nCopyright (c) Example\n\nPermission is hereby granted..."
APACHE_TEXT = "Apache License\nVersion 2.0, January 2004\n..."


def _dist(name, version="1.0", *, expr="", lic="", classifiers=(), texts=(), libs=()):
    return {
        "name": name,
        "version": version,
        "license_expression": expr,
        "license": lic,
        "classifiers": list(classifiers),
        "texts": [list(t) for t in texts],
        "libs": list(libs),
    }


def _probe(*extra):
    dists = [
        _dist(
            "cadquery", "2.8.0", lic="Apache Public License 2.0", texts=[("LICENSE", APACHE_TEXT)]
        ),
        _dist(
            "cadquery-ocp",
            "7.9.3.1.1",
            lic="Apache-2.0",
            texts=[("cadquery_ocp/LICENSE", APACHE_TEXT)],
            libs=[
                "TKernel-0123456789abcdef0123456789abcdef.dll",
                "zlib-0123456789abcdef0123456789abcdef.dll",
            ],
        ),
        _dist("casadi", "3.8.2", lic="GNU Lesser General Public License v3 or later (LGPLv3+)"),
        _dist("nlopt", "2.11.0", lic="MIT", texts=[("LICENSE", "NLopt ... LGPL ...")]),
        _dist("vtk", "9.6.2", lic="BSD", libs=["vtkCommonCore-9.6.2.dll"]),
        _dist("pyinstaller", "6.22.3", texts=[("COPYING.txt", "GPL bootloader exception")]),
        _dist("setuptools", "84.2.1", expr="MIT", texts=[("LICENSE", MIT_TEXT)]),
        _dist("scipy", "1.18.1", lic="BSD"),
        _dist("promptcad-studio", "0.1.0", lic="MIT"),
        *extra,
    ]
    return {"python_version": "3.12.10", "cpython_license": "PSF LICENSE AGREEMENT", "dists": dists}


def _write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def _lock():
    return {
        "lockfileVersion": 3,
        "packages": {
            "": {"name": "promptcad-web", "dependencies": {"react": "19.3.1"}},
            "node_modules/react": {"version": "19.3.1", "license": "MIT"},
            "node_modules/three": {"version": "0.187.0"},  # licence only in package.json
            "node_modules/vite": {"version": "8.3.8", "license": "MIT", "dev": True},
            "node_modules/fsevents": {"version": "2.3.3", "license": "MIT", "devOptional": True},
            "node_modules/@scope/pkg/node_modules/inner": {"version": "1.0.0", "license": "ISC"},
        },
    }


def _web(tmp_path: Path) -> Path:
    web = tmp_path / "web"
    _write(web / "node_modules/react/LICENSE", MIT_TEXT)
    _write(web / "node_modules/three/package.json", json.dumps({"license": "MIT"}))
    _write(web / "node_modules/three/LICENSE.md", "three.js MIT")
    _write(web / "node_modules/vite/LICENSE.md", "vite MIT")
    return web


def _cargo_meta(tmp_path: Path):
    def pkg(
        name, version, *, source="registry+https://github.com/rust-lang/crates.io-index", lic="MIT"
    ):
        crate = tmp_path / "registry" / f"{name}-{version}"
        _write(crate / "Cargo.toml", "")
        _write(crate / "LICENSE-MIT", f"MIT for {name}")
        return {
            "id": f"{name} {version}",
            "name": name,
            "version": version,
            "license": lic,
            "license_file": None,
            "source": source,
            "manifest_path": str(crate / "Cargo.toml"),
        }

    def dep(target, kind=None):
        return {"pkg": target, "dep_kinds": [{"kind": kind, "target": None}]}

    packages = [
        pkg("app", "0.1.0", source=None),
        pkg("serde", "1.0.0", lic="MIT OR Apache-2.0"),
        pkg("serde_derive", "1.0.0"),
        pkg("tauri-build", "2.7.0"),  # build-dependency only
        pkg("tempfile", "3.0.0"),  # dev-dependency only
        pkg("local-helper", "0.1.0", source=None),  # path dependency: ours
        pkg("windows-sys", "0.61.0"),
        pkg("windows-sys", "0.59.0"),
    ]
    nodes = [
        {
            "id": "app 0.1.0",
            "deps": [
                dep("serde 1.0.0"),
                dep("tauri-build 2.7.0", "build"),
                dep("tempfile 3.0.0", "dev"),
                dep("local-helper 0.1.0"),
                dep("windows-sys 0.61.0"),
            ],
        },
        {"id": "serde 1.0.0", "deps": [dep("serde_derive 1.0.0")]},
        {"id": "serde_derive 1.0.0", "deps": []},
        {"id": "tauri-build 2.7.0", "deps": [dep("windows-sys 0.59.0")]},
        {"id": "tempfile 3.0.0", "deps": []},
        {"id": "local-helper 0.1.0", "deps": [dep("serde 1.0.0")]},
        {"id": "windows-sys 0.61.0", "deps": []},
        {"id": "windows-sys 0.59.0", "deps": []},
    ]
    return {"packages": packages, "resolve": {"root": "app 0.1.0", "nodes": nodes}}


def _collected(tmp_path: Path, probe=None):
    return gen.collect(probe or _probe(), _lock(), _web(tmp_path), _cargo_meta(tmp_path))


def test_python_section_from_fake_metadata():
    probe = _probe(
        _dist(
            "classified",
            classifiers=["License :: OSI Approved", "License :: OSI Approved :: MIT License"],
        ),
        _dist(
            "longtext",
            lic="Copyright (c) 2001\nAll rights reserved.\n" * 5,
            classifiers=["License :: OSI Approved :: BSD License"],
        ),
        _dist("expr-wins", expr="Apache-2.0 OR MIT", lic="MIT"),
        _dist("Dupe_Name", "2.0", expr="MIT"),
        _dist("dupe-name", "2.0", expr="MIT"),
    )
    comps = {c.name: c for c in gen.python_components(probe)}

    # build-only tools, pruned modules and this project are not third-party payload
    assert "pyinstaller" not in comps
    assert "scipy" not in comps
    assert "promptcad-studio" not in comps
    # setuptools is frozen by PyInstaller (its _vendor packages), so it is listed
    assert comps["setuptools"].license == "MIT"

    assert comps["cadquery"].license == "Apache-2.0"  # free-text alias
    assert comps["casadi"].license == "LGPL-3.0-or-later"  # override
    assert comps["casadi"].note.startswith("覆寫")
    assert comps["nlopt"].license == "LGPL-2.1-or-later AND MIT"  # override beats "MIT"
    assert comps["vtk"].license == "BSD-3-Clause"
    assert comps["classified"].license == "MIT"  # bare "OSI Approved" ignored
    assert comps["longtext"].license == "BSD"  # licence text in License: -> classifier
    assert comps["expr-wins"].license == "Apache-2.0 OR MIT"
    assert len([c for c in comps if gen.normalize_name(c) == "dupe-name"]) == 1
    assert [t.source for t in comps["cadquery-ocp"].texts] == ["cadquery_ocp/LICENSE"]

    dlls = {d.file: d for d in gen.vendored_dlls(probe)}
    assert dlls["TKernel-0123456789abcdef0123456789abcdef.dll"].license.startswith("LGPL-2.1-only")
    assert dlls["zlib-0123456789abcdef0123456789abcdef.dll"].license == "Zlib"
    assert dlls["vtkCommonCore-9.6.2.dll"].license == "BSD-3-Clause"


def test_npm_prod_only_excludes_dev(tmp_path):
    comps = gen.npm_components(_lock(), _web(tmp_path))
    names = [c.name for c in comps]
    assert names == ["inner", "react", "three"]  # nested prod dep kept; dev/devOptional dropped
    by_name = {c.name: c for c in comps}
    assert by_name["three"].license == "MIT"  # fallback to node_modules/<pkg>/package.json
    assert [t.source for t in by_name["react"].texts] == ["LICENSE"]
    assert [t.source for t in by_name["three"].texts] == ["LICENSE.md"]
    assert by_name["inner"].texts == []


def test_cargo_metadata_parsing(tmp_path):
    comps = gen.rust_components(_cargo_meta(tmp_path))
    keys = [(c.name, c.version) for c in comps]
    # normal deps reachable from root, transitively (also through our own path crate);
    # build-only / dev-only deps and their subtrees are out, workspace/path crates are ours
    assert keys == [("serde", "1.0.0"), ("serde_derive", "1.0.0"), ("windows-sys", "0.61.0")]
    serde = comps[0]
    assert serde.license == "MIT OR Apache-2.0"
    assert [t.text for t in serde.texts] == ["MIT for serde"]


def test_strict_fails_on_unknown_licence(tmp_path):
    assert gen.strict_errors(_collected(tmp_path)) == []

    probe = _probe(
        _dist("mystery", "0.1"),  # no expression, no licence, no classifier
        _dist("osi-only", classifiers=["License :: OSI Approved"]),
        _dist("odd-libs", expr="MIT", libs=["unheardof-0123456789abcdef0123456789abcdef.dll"]),
    )
    errors = gen.strict_errors(_collected(tmp_path, probe))
    assert any("mystery" in e for e in errors)
    assert any("osi-only" in e for e in errors)
    assert any("unheardof" in e for e in errors)

    wrong_venv = {"python_version": "3.12.0", "cpython_license": "", "dists": []}
    errors = gen.strict_errors(_collected(tmp_path, wrong_venv))
    assert any("cadquery" in e for e in errors)

    for value in ("", "UNKNOWN", " none ", "OSI Approved"):
        assert gen.is_unknown(value)
    assert not gen.is_unknown("MIT")


def test_strict_cli_exits_nonzero_without_writing(tmp_path, monkeypatch):
    out = tmp_path / "NOTICES.md"
    monkeypatch.setattr(gen, "WEB_DIR", _web(tmp_path))
    _write(gen.WEB_DIR / "package-lock.json", json.dumps(_lock()))
    monkeypatch.setattr(gen, "run_cargo_metadata", lambda *_: _cargo_meta(tmp_path))
    monkeypatch.setattr(gen, "run_python_probe", lambda _: _probe(_dist("mystery")))
    assert gen.main(["--python", "py", "--out", str(out), "--strict"]) == 1
    assert not out.exists()

    monkeypatch.setattr(gen, "run_python_probe", lambda _: _probe())
    assert gen.main(["--python", "py", "--out", str(out), "--strict"]) == 0
    assert gen.main(["--python", "py", "--out", str(out), "--check"]) == 0
    out.write_text(out.read_text(encoding="utf-8") + "drift\n", encoding="utf-8")
    assert gen.main(["--python", "py", "--out", str(out), "--check"]) == 1


def test_output_is_deterministic(tmp_path):
    first = gen.render(_collected(tmp_path))

    shuffled = _probe()
    random.Random(7).shuffle(shuffled["dists"])
    lock = _lock()
    lock["packages"] = dict(reversed(list(lock["packages"].items())))
    meta = _cargo_meta(tmp_path)
    meta["packages"].reverse()
    meta["resolve"]["nodes"].reverse()
    second = gen.render(gen.collect(shuffled, lock, tmp_path / "web", meta))

    assert first == second
    assert not re.search(r"\d{4}-\d{2}-\d{2}", first)  # no dates / timestamps
    assert "\r" not in first
    # the same Apache text used by cadquery and cadquery-ocp appears once
    apache_id = gen.text_id(APACHE_TEXT)
    assert first.count(f"### {apache_id}") == 1
    assert gen.text_id(APACHE_TEXT + "  \r\n") == apache_id  # normalised before hashing
    # every link has a matching anchor
    for link in set(re.findall(r"\]\(#(l-[0-9a-f]{12})\)", first)):
        assert f'<a id="{link}"></a>' in first


def test_lgpl_section_present(tmp_path):
    text = gen.render(_collected(tmp_path))
    section = text.split("## LGPL 元件與替換權利", 1)[1].split("\n## ", 1)[0]
    assert "OCCT" in section and "OCP" in section and "CasADi" in section
    assert "sidecar\\_internal\\" in section  # files stay replaceable in the onedir layout
    assert "onedir" in section
    assert "https://github.com/Open-Cascade-SAS/OCCT/tree/V7_9_3" in section
    assert "https://github.com/casadi/casadi/tree/3.8.2" in section
    assert gen.text_id(gen.LGPL_2_1_TEXT) in section
    assert "GNU LESSER GENERAL PUBLIC LICENSE" in gen.LGPL_2_1_TEXT
    assert "Version 2.1, February 1999" in gen.LGPL_2_1_TEXT
    assert gen.OCCT_EXCEPTION_TEXT.startswith("Open CASCADE exception (version 1.0)")
    # copyleft review table lists the bootloader and the LGPL libraries
    review = text.split("## 需特別留意的授權", 1)[1].split("\n## ", 1)[0]
    assert "PyInstaller bootloader" in review and "casadi" in review


def test_pruned_dists_match_build_sidecar():
    build_sidecar = _load("build_sidecar")
    assert set(gen.PRUNED_DISTS.values()) <= set(build_sidecar.PRUNED_MODULES)
    assert not (gen.BUILD_ONLY_DISTS & set(gen.PRUNED_DISTS))
    sidecar_pruned = {gen.normalize_name(d) for d in getattr(build_sidecar, "PRUNED_DISTS", ())}
    assert sidecar_pruned == gen.SIDECAR_PRUNED_DISTS
    assert not (sidecar_pruned & set(gen.REQUIRED_PYTHON_DISTS))


def test_tauri_resource_mapping_exists():
    conf = json.loads((ROOT / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))
    resources = conf["bundle"]["resources"]
    assert resources["../THIRD_PARTY_NOTICES.md"] == "THIRD_PARTY_NOTICES.md"
    assert resources["binaries/promptcad-sidecar/"] == "sidecar/"

    notices = ROOT / "THIRD_PARTY_NOTICES.md"
    assert notices.stat().st_size > 10 * 1024
    text = notices.read_text(encoding="utf-8")
    assert "## LGPL 元件與替換權利" in text
    for link in set(re.findall(r"\]\(#(l-[0-9a-f]{12})\)", text)):
        assert f'<a id="{link}"></a>' in text


# --------------------------------------------------------------------------------------
# Cross-platform (ADR 0011): unix vendored libraries, platform wording, sidecar pruning


def test_is_vendored_lib_sample_paths():
    yes = [
        "cadquery_ocp.libs/TKernel-0123456789abcdef0123456789abcdef.dll",  # delvewheel
        "numpy.libs/libscipy_openblas64_-6bb31eeb.so",  # auditwheel
        "cadquery_ocp.libs/libTKernel-1a2b3c4d.so.7.9.3",
        "numpy.libs/libgfortran-040039e1.so.5.0.0",
        "numpy/.dylibs/libgfortran.5.dylib",  # delocate
        "casadi/.dylibs/libc++.1.0.dylib",
        "vtkmodules/libvtkCommonCore-9.6.so",  # in-package (vtk on Linux)
        "vtkmodules/libvtkCommonCore-9.6.9.6.dylib",
        "casadi/libcasadi.so.3.7",
    ]
    no = [
        "casadi/libcasadi.dll",  # in-package Windows DLLs: covered by the dist entry
        "cadquery_ocp.libs/.load-order-cadquery_ocp-7.9.3.1.1",
        "OCP.cpython-312-x86_64-linux-gnu.so",
        "vtkmodules/vtkCommonCore.cpython-312-darwin.so",
        "vtkmodules/libvtkfoo.abi3.so",
        "casadi/_casadi.so",  # extension module, no lib prefix
        "numpy/core/_multiarray_umath.cpython-312-x86_64-linux-gnu.so",
        "libtop.so",  # not inside a package
        "pkg/libnotes.txt",
        "pkg/library.py",
    ]
    for path in yes:
        assert gen.is_vendored_lib(tuple(path.split("/"))), path
    for path in no:
        assert not gen.is_vendored_lib(tuple(path.split("/"))), path


def test_normalize_and_classify_unix_libs():
    cases = {
        "TKernel-0123456789abcdef0123456789abcdef.dll": "tkernel",
        "vtkCommonCore-9.6.2.dll": "vtkcommoncore",
        "libgomp-a34b3233.so.1.0.0": "libgomp",
        "libgfortran.5.dylib": "libgfortran",
        "libTKernel-1a2b3c4d.so.7.9.3": "libtkernel",
        "libvtkCommonCore-9.6.so": "libvtkcommoncore",
        "libvtkCommonCore-9.6.9.6.dylib": "libvtkcommoncore",
        "libIex-3_2.so.31": "libiex",
        "libpng16-1b2c3d4e.so.16.43.0": "libpng16",
        "libscipy_openblas64_-6bb31eeb.so": "libscipy_openblas64_",
    }
    for name, stem in cases.items():
        assert gen.normalize_lib_stem(name) == stem, name
    assert gen.normalize_dll_stem is gen.normalize_lib_stem

    def licence(name):
        hit = gen.classify_dll(name)
        return hit and hit[1]

    assert licence("libTKernel-1a2b3c4d.so.7.9.3").startswith("LGPL-2.1-only")  # lib prefix off
    assert licence("libvtkCommonCore-9.6.so") == "BSD-3-Clause"
    assert licence("libpng16-1b2c3d4e.so.16.43.0") == "libpng-2.0"  # as-is first
    assert licence("libgomp-a34b3233.so.1.0.0") == "GPL-3.0-or-later WITH GCC-exception-3.1"
    assert licence("libquadmath.0.dylib") == "LGPL-2.1-or-later"
    assert licence("libomp.dylib") == "Apache-2.0 WITH LLVM-exception"
    assert licence("libz-eb09ad1d.so.1.2.13") == "Zlib"
    assert licence("libcasadi_nlpsol_ipopt.so.3.7") == "LGPL-3.0-or-later"
    assert licence("libscipy_openblas64_-6bb31eeb.so").startswith("BSD-3-Clause")
    assert gen.classify_dll("libunheardof-0123abcd.so.1") is None
    # the lib-prefix fallback is unix-only: Windows stems keep their exact meaning
    assert gen.classify_dll("libtiff.dll") is None
    assert gen.classify_dll("tiff.dll")[1] == "libtiff"


def _linux_probe(*extra):
    probe = _probe(
        _dist(
            "numpy",
            "2.5.5",
            expr="BSD-3-Clause",
            libs=["libgfortran-040039e1.so.5.0.0", "libscipy_openblas64_-6bb31eeb.so"],
        ),
        *extra,
    )
    probe["platform"] = "linux"
    for dist in probe["dists"]:
        if dist["name"] == "vtk":
            dist["libs"] = ["libvtkCommonCore-9.6.so"]
        if dist["name"] == "cadquery-ocp":
            dist["libs"] = ["libTKernel-1a2b3c4d.so.7.9.3"]
    return probe


def test_linux_render_uses_unix_paths(tmp_path):
    collected = _collected(tmp_path, _linux_probe())
    assert collected.target.key == "linux"
    assert gen.strict_errors(collected) == []
    text = gen.render(collected)
    lgpl = text.split("## LGPL 元件與替換權利", 1)[1].split("\n## ", 1)[0]
    assert "sidecar/_internal/" in lgpl and "sidecar\\_internal\\" not in lgpl
    assert "libTK*.so*" in lgpl and "TK*.dll" not in lgpl
    assert "`promptcad-sidecar` 或 `promptcad-studio`" in lgpl
    assert "Linux x86_64 .deb" in text
    assert "x86_64-unknown-linux-gnu" in text
    native = text.split("## Python wheel 隨附的原生函式庫", 1)[1].split("\n## ", 1)[0]
    assert "auditwheel" in native and "`libgfortran`" in native
    hand = {c.name for c in collected.hand}
    assert not any("Microsoft" in n or "NSIS" in n for n in hand)
    assert "Open CASCADE Technology (OCCT)" in hand


def test_macos_target_and_default_platform(tmp_path):
    probe = _probe()
    probe["platform"] = "darwin"
    text = gen.render(_collected(tmp_path, probe))
    assert "PromptCAD Studio.app/Contents/Resources/sidecar/_internal/" in text
    assert "aarch64-apple-darwin" in text and "libTK*.dylib" in text
    # fakes without "platform" (and the committed file) are the Windows notices
    windows = gen.render(_collected(tmp_path))
    assert "Windows x64 NSIS" in windows and "x86_64-pc-windows-msvc" in windows
    assert "`sidecar\\_internal\\cadquery_ocp.libs\\TK*.dll`" in windows
    assert gen.target_for(None).key == "win32"
    assert gen.target_for("linux2").key == "linux"


def test_sidecar_pruned_dists_are_filtered(tmp_path, monkeypatch):
    probe = _probe(_dist("trame-vtk", "2.11.17", expr="BSD-3-Clause"), _dist("wslink", expr="BSD"))
    monkeypatch.setattr(gen, "SIDECAR_PRUNED_DISTS", frozenset())
    assert "trame-vtk" in {c.name for c in gen.python_components(probe)}
    monkeypatch.setattr(gen, "SIDECAR_PRUNED_DISTS", frozenset({"trame-vtk", "wslink"}))
    names = {c.name for c in gen.python_components(probe)}
    assert "trame-vtk" not in names and "wslink" not in names
    text = gen.render(_collected(tmp_path, probe))
    assert "trame-vtk" in text.split("排除的模組（", 1)[1].split("）", 1)[0]


def test_load_sidecar_pruned_dists(tmp_path):
    literal = _write(
        tmp_path / "a" / "build_sidecar.py", 'PRUNED_DISTS = ("Trame_VTK", "wslink")\n'
    )
    assert gen._load_sidecar_pruned_dists(literal) == (frozenset({"trame-vtk", "wslink"}), "")
    missing = _write(tmp_path / "b" / "build_sidecar.py", "PRUNED_MODULES = ('scipy',)\n")
    assert gen._load_sidecar_pruned_dists(missing) == (frozenset(), "")
    computed = _write(
        tmp_path / "c" / "build_sidecar.py",
        "BASE = ('aiohttp',)\nPRUNED_DISTS: tuple[str, ...] = BASE + ('wslink',)\n",
    )
    assert gen._load_sidecar_pruned_dists(computed) == (frozenset({"aiohttp", "wslink"}), "")
    broken = _write(tmp_path / "d" / "build_sidecar.py", "PRUNED_DISTS = (\n")
    names, error = gen._load_sidecar_pruned_dists(broken)
    assert names == frozenset() and "SyntaxError" in error
    # the real build_sidecar loads cleanly (whether or not it defines PRUNED_DISTS yet)
    assert gen.SIDECAR_PRUNED_ERROR == ""
    assert not (gen.SIDECAR_PRUNED_DISTS & {"cadquery", "cadquery-ocp", "vtk", "casadi"})


def test_probe_runs_and_reports_platform():
    compile(gen.PY_PROBE, "<probe>", "exec")
    probe = gen.run_python_probe(sys.executable)
    assert probe["platform"] == sys.platform
    assert probe["python_version"].count(".") == 2
    assert all(isinstance(d["libs"], list) for d in probe["dists"])


def test_unix_cadquery_ocp_libs_and_casadi_fallback():
    probe = {
        "dists": [
            {"name": "cadquery-ocp", "version": "7.9.3.1.1", "libs": ["libjbig-0f1087b4.so.0"]},
            {"name": "casadi", "version": "3.8.2", "libs": ["libCbc.so.3", "libipopt.3.dylib"]},
        ]
    }
    by_file = {d.file: d for d in gen.vendored_dlls(probe)}
    assert by_file["libjbig-0f1087b4.so.0"].license == "GPL-2.0-or-later"
    assert by_file["libCbc.so.3"].label and "EPL-2.0" in by_file["libCbc.so.3"].license
    assert by_file["libipopt.3.dylib"].label is not None
