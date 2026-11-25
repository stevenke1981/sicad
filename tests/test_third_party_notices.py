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
