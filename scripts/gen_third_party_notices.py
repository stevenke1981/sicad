"""Generate ``THIRD_PARTY_NOTICES.md`` for the desktop bundle (ADR 0006, ADR 0010 Phase 1).

Usage - run with the desktop build venv so the Python section matches what PyInstaller
freezes into ``sidecar/_internal``::

    .venv-desktop/Scripts/python.exe scripts/gen_third_party_notices.py \\
        --python .venv-desktop/Scripts/python.exe --strict

Sources (all read from what is actually installed / locked, nothing from the network):

* Python - the ``--python`` interpreter is run with a small inline probe (``-I -c``) that
  dumps ``importlib.metadata`` for every distribution: name, version, ``License-Expression``,
  ``License``, Trove licence classifiers, licence file texts (``License-File`` entries in
  ``.dist-info/licenses/``; otherwise ``LICENSE*`` / ``COPYING*`` / ``NOTICE*`` files from the
  RECORD) and the native libraries vendored by the wheel: ``<pkg>.libs/*.dll`` (delvewheel),
  ``<pkg>.libs/*.so*`` (auditwheel), ``*/.dylibs/*.dylib`` (delocate) and in-package
  ``lib*.so*`` / ``lib*.dylib`` (e.g. ``vtkmodules/libvtk*.so`` on Linux). Build-only tools,
  the modules ``scripts/build_sidecar.py`` excludes (:data:`PRUNED_DISTS`) and the
  distributions it verified absent from ``_internal`` (``build_sidecar.PRUNED_DISTS``) are
  left out.
* npm - production (non-dev) entries of ``web/package-lock.json``; texts from
  ``web/node_modules/<name>/LICENSE*``.
* Rust - ``cargo metadata --format-version 1 --filter-platform <host target>`` in
  ``src-tauri`` (``x86_64-pc-windows-msvc`` / ``aarch64-apple-darwin`` /
  ``x86_64-unknown-linux-gnu``, chosen from the probed interpreter's ``sys.platform``); normal (non-dev, non-build) dependencies reachable from the root crate,
  deduplicated by name+version; texts from the crate directories.
* A hand-kept table for what no package manager reports: OCCT, VTK, CPython, the
  PyInstaller bootloader, nlopt and, on Windows only, the MSVC runtime, the WebView2
  bootstrapper and NSIS.

The committed ``THIRD_PARTY_NOTICES.md`` is the Windows one (ADR 0011): Windows CI asserts it
with ``--check --strict`` against the locked env; macOS / Linux CI regenerate it in place with
``--strict`` and ship that file in their bundle (paths and library names follow the platform).

Licence texts are deduplicated by SHA-256 of the normalised text. Output is sorted and
contains no timestamps, so ``--check`` can compare it byte for byte with the committed file.

``--strict`` exits 1 (without writing) when any component's licence resolves to
unknown/empty, when a vendored DLL is not in :data:`VENDORED_DLLS`, or when the
interpreter lacks CadQuery/OCP (wrong venv). Packages whose metadata is missing or
misleading are fixed in :data:`LICENSE_OVERRIDES`, each with the reason.

This is a compliance aid, not legal advice. Only the standard library is used.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import importlib.util
import inspect
import json
import re
import subprocess
import sys
from collections.abc import Iterable, Mapping
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT = ROOT / "THIRD_PARTY_NOTICES.md"
WEB_DIR = ROOT / "web"
TAURI_DIR = ROOT / "src-tauri"
RUST_TARGET = "x86_64-pc-windows-msvc"  # the committed (Windows) notices

# Tools that build the bundle but are not shipped in it. setuptools is deliberately NOT
# listed: PyInstaller's analysis of the real sidecar (build/sidecar/work/*/PYZ-00.toc)
# freezes setuptools and its _vendor packages, so they are shipped.
BUILD_ONLY_DISTS = frozenset(
    {
        "pyinstaller",  # its bootloader IS shipped - see the hand table
        "pyinstaller-hooks-contrib",
        "altgraph",
        "pefile",
        "pywin32-ctypes",
        "macholib",  # PyInstaller's Mach-O analysis on macOS
        "pip",
        "wheel",
        "uv",
    }
)

# Distributions whose top-level module scripts/build_sidecar.py passes to
# ``--exclude-module`` (PRUNED_MODULES): installed in .venv-desktop, not in the bundle.
# dist name -> module name; tests keep this in sync with build_sidecar.PRUNED_MODULES.
PRUNED_DISTS: Mapping[str, str] = {
    "llvmlite": "llvmlite",
    "matplotlib": "matplotlib",
    "numba": "numba",
    "pillow": "PIL",
    "scipy": "scipy",
}


def _literal_assignment(source: str, name: str) -> tuple[bool, object]:
    """``(found, value)`` of a top-level ``NAME = <literal>`` (or annotated) in ``source``."""
    for node in ast.parse(source).body:
        if isinstance(node, ast.Assign):
            targets, value = node.targets, node.value
        elif isinstance(node, ast.AnnAssign) and node.value is not None:
            targets, value = [node.target], node.value
        else:
            continue
        if any(isinstance(t, ast.Name) and t.id == name for t in targets):
            try:
                return True, ast.literal_eval(value)
            except ValueError:
                return False, None  # computed: needs a real import
    return False, None


def _load_sidecar_pruned_dists(path: Path | None = None) -> tuple[frozenset[str], str]:
    """``build_sidecar.PRUNED_DISTS`` (dists verified absent from ``_internal``), normalised.

    Read as a literal from the source when possible (no import side effects, works in the
    CadQuery-free test venv); otherwise the module is imported and ``getattr(..., ())`` used,
    so an older build_sidecar without the attribute yields an empty set. Returns
    ``(names, error)``: a build_sidecar that cannot be read is reported by ``--strict``
    (silently listing pruned dists would make the Windows ``--check`` flap).
    """
    path = path or Path(__file__).resolve().parent / "build_sidecar.py"
    scripts_dir = str(path.parent)
    added = False
    try:
        found, value = _literal_assignment(path.read_text(encoding="utf-8"), "PRUNED_DISTS")
        if not found:
            added = scripts_dir not in sys.path
            if added:  # build_sidecar imports its sibling helper modules by bare name
                sys.path.insert(0, scripts_dir)
            spec = importlib.util.spec_from_file_location("_build_sidecar_for_notices", path)
            module = importlib.util.module_from_spec(spec)
            sys.modules[spec.name] = module
            spec.loader.exec_module(module)
            value = getattr(module, "PRUNED_DISTS", ())
        return frozenset(normalize_name(str(n)) for n in value or ()), ""
    except Exception as exc:  # only on a broken checkout
        return frozenset(), f"{type(exc).__name__}: {exc}"
    finally:
        if added:
            sys.path.remove(scripts_dir)


# This project itself (MIT, see LICENSE) - not a third party.
OWN_DISTS = frozenset({"promptcad-studio"})

# Dists that must be present for --strict: a notices file without them means the wrong
# interpreter was passed (the LGPL section depends on them).
REQUIRED_PYTHON_DISTS = ("cadquery", "cadquery-ocp")

# (ecosystem, normalised name) -> (licence expression, reason). Applied before metadata.
LICENSE_OVERRIDES: Mapping[tuple[str, str], tuple[str, str]] = {
    ("python", "casadi"): (
        "LGPL-3.0-or-later",
        "metadata 的 License 欄為自由文字（LGPLv3+），classifier 只有 OSI Approved",
    ),
    ("python", "nlopt"): (
        "LGPL-2.1-or-later AND MIT",
        "wheel metadata 標示 MIT，但其 LICENSE 說明編譯後的函式庫（含 luksan 例程）受 LGPL-2.1+ 規範",
    ),
    ("python", "vtk"): (
        "BSD-3-Clause",
        "metadata 只寫 BSD；VTK 本體採 BSD-3-Clause",
    ),
}

# Trove classifier -> SPDX-ish id. Unmapped classifiers fall back to their last segment.
CLASSIFIER_LICENSES: Mapping[str, str] = {
    "License :: OSI Approved :: Apache Software License": "Apache-2.0",
    "License :: OSI Approved :: BSD License": "BSD",
    "License :: OSI Approved :: GNU General Public License v2 (GPLv2)": "GPL-2.0",
    "License :: OSI Approved :: GNU General Public License v3 (GPLv3)": "GPL-3.0",
    "License :: OSI Approved :: GNU Lesser General Public License v2 or later (LGPLv2+)": (
        "LGPL-2.0-or-later"
    ),
    "License :: OSI Approved :: GNU Lesser General Public License v3 or later (LGPLv3+)": (
        "LGPL-3.0-or-later"
    ),
    "License :: OSI Approved :: ISC License (ISCL)": "ISC",
    "License :: OSI Approved :: MIT License": "MIT",
    "License :: OSI Approved :: Mozilla Public License 2.0 (MPL 2.0)": "MPL-2.0",
    "License :: OSI Approved :: Python Software Foundation License": "PSF-2.0",
    "License :: OSI Approved :: The Unlicense (Unlicense)": "Unlicense",
    "License :: OSI Approved :: zlib/libpng License": "Zlib",
    "License :: Public Domain": "LicenseRef-Public-Domain",
}

# Free-text ``License:`` values seen in practice -> SPDX id.
LICENSE_ALIASES: Mapping[str, str] = {
    "apache 2.0": "Apache-2.0",
    "apache license 2.0": "Apache-2.0",
    "apache public license 2.0": "Apache-2.0",
    "apache software license": "Apache-2.0",
    "bsd license": "BSD",
    "mit license": "MIT",
}

UNKNOWN_VALUES = frozenset({"", "unknown", "none", "n/a", "osi approved", "other/proprietary"})

# Native libraries vendored by wheels (delvewheel ``<pkg>.libs/*.dll``; auditwheel
# ``<pkg>.libs/*.so*``; delocate ``.dylibs/*.dylib``; in-package ``lib*.so*``). Matched on the
# normalised stem (see :func:`normalize_lib_stem`: lower case, hash / version / suffix removed).
# Unix stems are tried as-is and then without their ``lib`` prefix, so ``libTKernel`` hits
# ``tk*`` and ``libpng16`` hits ``libpng16``. Entries ending in "*" are prefixes. Unix-only
# entries come from the first macOS / Linux CI ``--strict`` runs (ADR 0011).
VENDORED_DLLS: tuple[tuple[str, str, str], ...] = (
    ("tk*", "Open CASCADE Technology (OCCT)", "LGPL-2.1-only WITH OCCT-exception-1.0"),
    ("vtk*", "VTK（含其內建 ThirdParty 模組）", "BSD-3-Clause"),
    ("viskores*", "Viskores（VTK-m）", "BSD-3-Clause"),
    ("openxr_loader", "OpenXR SDK loader", "Apache-2.0"),
    ("msvcp140*", "Microsoft Visual C++ Runtime", "LicenseRef-Microsoft-VC-Redist"),
    ("vcomp140*", "Microsoft Visual C++ Runtime", "LicenseRef-Microsoft-VC-Redist"),
    ("vcruntime140*", "Microsoft Visual C++ Runtime", "LicenseRef-Microsoft-VC-Redist"),
    ("concrt140*", "Microsoft Visual C++ Runtime", "LicenseRef-Microsoft-VC-Redist"),
    (
        "libscipy_openblas*",
        "OpenBLAS（scipy-openblas 建置，含 LAPACK 與 GCC runtime）",
        "BSD-3-Clause AND BSD-3-Clause-Open-MPI AND GPL-3.0-or-later WITH GCC-exception-3.1",
    ),
    ("freeimage", "FreeImage", "FreeImage OR GPL-2.0-only OR GPL-3.0-only"),
    ("iex", "OpenEXR", "BSD-3-Clause"),
    ("ilmthread", "OpenEXR", "BSD-3-Clause"),
    ("openexr", "OpenEXR", "BSD-3-Clause"),
    ("openexrcore", "OpenEXR", "BSD-3-Clause"),
    ("imath", "Imath", "BSD-3-Clause"),
    ("lerc", "LERC", "Apache-2.0"),
    ("deflate", "libdeflate", "MIT"),
    ("freetype", "FreeType", "FTL OR GPL-2.0-or-later"),
    ("jpeg8", "libjpeg-turbo", "IJG AND BSD-3-Clause AND Zlib"),
    ("lcms2", "Little CMS", "MIT"),
    ("liblzma", "XZ Utils liblzma", "0BSD"),
    ("libpng16", "libpng", "libpng-2.0"),
    ("libsharpyuv", "libwebp", "BSD-3-Clause"),
    ("libwebp", "libwebp", "BSD-3-Clause"),
    ("libwebpmux", "libwebp", "BSD-3-Clause"),
    ("openjp2", "OpenJPEG", "BSD-2-Clause"),
    ("openjph", "OpenJPH", "BSD-2-Clause"),
    ("raw", "LibRaw", "LGPL-2.1-only OR CDDL-1.0"),
    ("tiff", "libtiff", "libtiff"),
    ("zlib", "zlib", "Zlib"),
    ("zstd", "Zstandard", "BSD-3-Clause OR GPL-2.0-only"),
    # --- unix (auditwheel / delocate / in-package) -----------------------------------
    ("casadi*", "CasADi", "LGPL-3.0-or-later"),
    ("nlopt*", "NLopt", "LGPL-2.1-or-later AND MIT"),
    ("libgomp", "GCC OpenMP runtime（libgomp）", "GPL-3.0-or-later WITH GCC-exception-3.1"),
    (
        "libgfortran",
        "GCC Fortran runtime（libgfortran）",
        "GPL-3.0-or-later WITH GCC-exception-3.1",
    ),
    ("libgcc_s", "GCC runtime（libgcc_s）", "GPL-3.0-or-later WITH GCC-exception-3.1"),
    ("libstdc++", "GCC C++ runtime（libstdc++）", "GPL-3.0-or-later WITH GCC-exception-3.1"),
    ("libquadmath", "GCC libquadmath", "LGPL-2.1-or-later"),
    ("libomp", "LLVM OpenMP runtime（libomp）", "Apache-2.0 WITH LLVM-exception"),
    ("libc++", "LLVM libc++", "Apache-2.0 WITH LLVM-exception"),
    ("libc++abi", "LLVM libc++abi", "Apache-2.0 WITH LLVM-exception"),
    ("libz", "zlib", "Zlib"),
    ("bz2", "bzip2", "bzip2-1.0.6"),
    ("expat", "Expat", "MIT"),
    ("jpeg", "libjpeg-turbo", "IJG AND BSD-3-Clause AND Zlib"),
    ("turbojpeg", "libjpeg-turbo", "IJG AND BSD-3-Clause AND Zlib"),
    ("raw_r", "LibRaw", "LGPL-2.1-only OR CDDL-1.0"),
    ("tbb*", "oneTBB", "Apache-2.0"),
    ("openblas*", "OpenBLAS", "BSD-3-Clause"),
)

LICENSE_FILE_RE = re.compile(r"^(licen[cs]e|copying|notice|unlicense|copyright)([.\-_].*)?$", re.I)
COPYLEFT_RE = re.compile(r"\b(A?GPL|LGPL|MPL|EPL|CDDL)")


def is_vendored_lib(parts) -> bool:  # parts: tuple[str, ...]; no annotations (probe)
    """Is this wheel RECORD path (split into parts) a vendored native library?

    * ``<pkg>.libs/*.dll`` (delvewheel) and ``<pkg>.libs/*.so*`` / ``*.dylib`` (auditwheel);
    * ``.../.dylibs/*.dylib`` (delocate);
    * in-package ``lib*.so[.N...]`` / ``lib*.dylib`` that are not Python extension modules
      (``vtkmodules/libvtk*.so`` on Linux). In-package Windows DLLs are not counted: they are
      covered by their distribution's own entry, as before.

    Pure and stdlib-only: its source is also embedded in :data:`PY_PROBE`.
    """
    import re as _re

    if len(parts) < 2:
        return False
    lower = parts[-1].lower()
    shared = bool(_re.search(r"\.so(\.\d+)*$", lower)) or lower.endswith(".dylib")
    if parts[0].endswith(".libs"):
        return lower.endswith(".dll") or shared
    if ".dylibs" in parts[:-1]:
        return shared
    if ".cpython-" in lower or ".abi3." in lower or not lower.startswith("lib"):
        return False
    return shared


# Executed by the ``--python`` interpreter (``-I -c``): stdlib only, prints one JSON object.
PY_PROBE = (
    inspect.getsource(is_vendored_lib)
    + r"""
import importlib.metadata as md, json, os, platform, re, sys
PAT = re.compile(r"^(licen[cs]e|copying|notice|unlicense|copyright)([.\-_].*)?$", re.I)
SKIP_DIRS = {"test", "tests", "doc", "docs", "example", "examples"}
CODE_EXT = (".py", ".pyc", ".pyi", ".pyd", ".dll", ".so", ".h", ".c", ".cpp")

def read_path(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            return fh.read()
    except OSError:
        return None

dists = []
for d in md.distributions():
    meta = d.metadata
    name = meta.get("Name")
    if not name:
        continue
    texts = []
    for lf in meta.get_all("License-File") or []:
        for cand in ("licenses/" + lf, lf):
            try:
                text = d.read_text(cand)
            except Exception:
                text = None
            if text:
                texts.append([lf, text])
                break
    files = list(d.files or [])
    if not texts:
        for f in files:
            parts = f.parts
            base = parts[-1]
            if not PAT.match(base) or base.lower().endswith(CODE_EXT):
                continue
            if SKIP_DIRS.intersection(p.lower() for p in parts[:-1]):
                continue
            text = read_path(f.locate())
            if text:
                texts.append([str(f).replace(os.sep, "/"), text])
    else:
        # Vendored dists (e.g. setuptools/_vendor/*.dist-info) carry their own licences.
        for f in files:
            parts = f.parts
            if len(parts) < 3 or not any(p.endswith(".dist-info") for p in parts[1:-1]):
                continue
            if not PAT.match(parts[-1]) or parts[-1].lower().endswith(CODE_EXT):
                continue
            text = read_path(f.locate())
            if text:
                texts.append([str(f).replace(os.sep, "/"), text])
    libs = sorted(f.parts[-1] for f in files if is_vendored_lib(f.parts))
    dists.append({
        "name": name,
        "version": meta.get("Version") or "",
        "license_expression": meta.get("License-Expression") or "",
        "license": meta.get("License") or "",
        "classifiers": [c for c in (meta.get_all("Classifier") or []) if c.startswith("License ::")],
        "texts": texts,
        "libs": libs,
    })
cpython = None
for cand in ("LICENSE.txt", "LICENSE"):
    cpython = read_path(os.path.join(sys.base_prefix, cand))
    if cpython:
        break
json.dump({"python_version": platform.python_version(), "cpython_license": cpython or "",
           "platform": sys.platform, "dists": dists}, sys.stdout, ensure_ascii=True)
"""
)


# --------------------------------------------------------------------------------------
# Model


@dataclass(frozen=True)
class LicenseText:
    source: str  # file name / path inside the package, for the "used by" list
    text: str


@dataclass
class Component:
    ecosystem: str  # python | npm | rust | hand
    name: str
    version: str
    license: str
    texts: list[LicenseText] = field(default_factory=list)
    note: str = ""
    url: str = ""


@dataclass
class VendoredDll:
    dist: str
    dist_version: str
    file: str
    label: str | None  # None = not in VENDORED_DLLS
    license: str | None


@dataclass(frozen=True)
class Target:
    """Where and how the bundle for one OS lays out its files (wording of the notices)."""

    key: str  # sys.platform prefix: win32 | darwin | linux
    bundle: str  # what the notices describe, e.g. "Windows x64 NSIS 安裝程式"
    rust_target: str  # cargo metadata --filter-platform
    sep: str  # path separator shown in the notices
    notices_path: str  # where the installed notices file lives
    install_note: str  # where sidecar/_internal ends up after installation
    lib_ext: str  # glob suffix of a shared library: ".dll" | ".dylib" | ".so*"
    ext_ext: str  # Python extension module suffix: ".pyd" | ".so"
    lib_prefix: str  # "" on Windows (TK*.dll), "lib" on unix (libTK*.so*)
    exe_suffix: str
    windows_only_hand: bool

    def internal(self, *parts: str) -> str:
        """``sidecar/_internal/<parts>/`` spelled with this OS's separator (trailing sep)."""
        return self.sep.join(("sidecar", "_internal", *parts)) + self.sep

    def lib(self, stem_glob: str) -> str:
        return f"{self.lib_prefix}{stem_glob}{self.lib_ext}"

    def libs_dir(self, wheel_libs: str) -> str:
        """Typical directory of vendored libraries: ``<pkg>.libs`` on Windows, generic on unix."""
        return self.internal(wheel_libs) if self.key == "win32" else self.internal()


TARGETS: Mapping[str, Target] = {
    "win32": Target(
        key="win32",
        bundle="Windows x64 NSIS 安裝程式",
        rust_target="x86_64-pc-windows-msvc",
        sep="\\",
        notices_path="<安裝目錄>\\THIRD_PARTY_NOTICES.md",
        install_note="安裝目錄的 `sidecar\\_internal\\`",
        lib_ext=".dll",
        ext_ext=".pyd",
        lib_prefix="",
        exe_suffix=".exe",
        windows_only_hand=True,
    ),
    "darwin": Target(
        key="darwin",
        bundle="macOS arm64 .app／.dmg",
        rust_target="aarch64-apple-darwin",
        sep="/",
        notices_path="PromptCAD Studio.app/Contents/Resources/THIRD_PARTY_NOTICES.md",
        install_note="安裝位置 `PromptCAD Studio.app/Contents/Resources/sidecar/_internal/`",
        lib_ext=".dylib",
        ext_ext=".so",
        lib_prefix="lib",
        exe_suffix="",
        windows_only_hand=False,
    ),
    "linux": Target(
        key="linux",
        bundle="Linux x86_64 .deb／AppImage",
        rust_target="x86_64-unknown-linux-gnu",
        sep="/",
        notices_path="/usr/lib/PromptCAD Studio/THIRD_PARTY_NOTICES.md",
        install_note=(
            "安裝位置 `/usr/lib/PromptCAD Studio/sidecar/_internal/`（.deb；AppImage 內為相同相對位置）"
        ),
        lib_ext=".so*",
        ext_ext=".so",
        lib_prefix="lib",
        exe_suffix="",
        windows_only_hand=False,
    ),
}


def target_for(sys_platform: str | None) -> Target:
    """The :class:`Target` of a probed ``sys.platform`` (Windows when unknown / missing)."""
    for key, target in TARGETS.items():
        if (sys_platform or "win32").startswith(key):
            return target
    return TARGETS["win32"]


@dataclass
class Collected:
    python: list[Component]
    native: list[VendoredDll]
    npm: list[Component]
    rust: list[Component]
    hand: list[Component]
    python_version: str
    dist_versions: dict[str, str]
    target: Target = TARGETS["win32"]


def normalize_name(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


SIDECAR_PRUNED_DISTS, SIDECAR_PRUNED_ERROR = _load_sidecar_pruned_dists()


def excluded_pruned_dists() -> list[str]:
    """Every dist left out because the sidecar build drops it (both lists, normalised)."""
    return sorted(set(PRUNED_DISTS) | SIDECAR_PRUNED_DISTS)


def is_unknown(expr: str | None) -> bool:
    return expr is None or expr.strip().lower() in UNKNOWN_VALUES


def normalize_text(text: str) -> str:
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    return "\n".join(line.rstrip() for line in lines).strip("\n")


def text_id(text: str) -> str:
    return "L-" + hashlib.sha256(normalize_text(text).encode("utf-8")).hexdigest()[:12]


# --------------------------------------------------------------------------------------
# Python


def resolve_python_license(dist: Mapping) -> str:
    """Licence expression for one probed distribution (override > metadata > classifiers)."""
    override = LICENSE_OVERRIDES.get(("python", normalize_name(dist["name"])))
    if override:
        return override[0]
    expr = (dist.get("license_expression") or "").strip()
    if not is_unknown(expr):
        return expr
    raw = (dist.get("license") or "").strip()
    short = raw if ("\n" not in raw and len(raw) <= 80) else ""
    if short and not is_unknown(short):
        return LICENSE_ALIASES.get(short.lower(), short)
    mapped = []
    for classifier in sorted(dist.get("classifiers") or []):
        parts = [p.strip() for p in classifier.split("::")]
        if len(parts) < 3 and classifier not in CLASSIFIER_LICENSES:
            continue  # bare "License :: OSI Approved" says nothing
        value = CLASSIFIER_LICENSES.get(classifier, parts[-1])
        if value not in mapped:
            mapped.append(value)
    if mapped:
        return " OR ".join(mapped)
    return "UNKNOWN"


def shipped_python_dists(dists: Iterable[Mapping]) -> list[Mapping]:
    """Drop build-only tools, pruned modules and this project; dedupe by name+version."""
    seen: dict[tuple[str, str], Mapping] = {}
    for dist in dists:
        key = normalize_name(dist["name"])
        if key in BUILD_ONLY_DISTS or key in OWN_DISTS:
            continue
        if key in PRUNED_DISTS or key in SIDECAR_PRUNED_DISTS:
            continue
        seen.setdefault((key, dist.get("version", "")), dist)
    return [seen[k] for k in sorted(seen)]


def python_components(probe: Mapping) -> list[Component]:
    comps = []
    for dist in shipped_python_dists(probe.get("dists", [])):
        texts = [LicenseText(src, txt) for src, txt in dist.get("texts") or [] if txt.strip()]
        if normalize_name(dist["name"]) in GPL_3_REFERENCED_BY:
            # LGPL-3.0 incorporates GPL-3.0 by reference; ship the full text explicitly.
            texts.append(LicenseText("GPL-3.0.txt", GPL_3_TEXT))
        override = LICENSE_OVERRIDES.get(("python", normalize_name(dist["name"])))
        comps.append(
            Component(
                ecosystem="python",
                name=dist["name"],
                version=dist.get("version", ""),
                license=resolve_python_license(dist),
                texts=texts,
                note=f"覆寫：{override[1]}" if override else "",
            )
        )
    return comps


def is_unix_lib(file_name: str) -> bool:
    lower = file_name.lower()
    return bool(re.search(r"\.so(\.\d+)*$", lower)) or lower.endswith(".dylib")


def normalize_lib_stem(file_name: str) -> str:
    """Stable name of a vendored library: no suffix, wheel-repair hash, soname or version.

    ``TKernel-<32 hex>.dll`` -> ``tkernel`` (delvewheel); ``libgomp-a34b3233.so.1.0.0`` ->
    ``libgomp`` (auditwheel); ``libgfortran.5.dylib`` -> ``libgfortran`` (delocate);
    ``libvtkCommonCore-9.6.so`` -> ``libvtkcommoncore``; ``libIex-3_2.so.31`` -> ``libiex``.
    """
    lower = file_name.lower()
    if not is_unix_lib(lower):  # Windows: unchanged since the committed notices
        stem = lower.rsplit(".", 1)[0]
        stem = re.sub(r"-[0-9a-f]{32}$", "", stem)
        return re.sub(r"-\d+(\.\d+)*$", "", stem)
    stem = re.sub(r"(\.so(\.\d+)*|\.dylib)$", "", lower)
    stem = re.sub(r"\.\d+(\.\d+)*$", "", stem)  # libfoo.1.2(.dylib)
    stem = re.sub(r"-[0-9a-f]{8}$", "", stem)  # auditwheel hash
    return re.sub(r"-\d+([._]\d+)*$", "", stem)


normalize_dll_stem = normalize_lib_stem  # former name


def _match_vendored(stem: str) -> tuple[str, str] | None:
    for pattern, label, licence in VENDORED_DLLS:
        if pattern.endswith("*"):
            if stem.startswith(pattern[:-1]):
                return label, licence
        elif stem == pattern:
            return label, licence
    return None


def classify_dll(file_name: str) -> tuple[str, str] | None:
    """``(label, licence)`` of a vendored library, or None when :data:`VENDORED_DLLS` lacks it."""
    stem = normalize_lib_stem(file_name)
    hit = _match_vendored(stem)
    if hit is None and is_unix_lib(file_name) and stem.startswith("lib") and len(stem) > 3:
        hit = _match_vendored(stem[3:])
    return hit


def vendored_dlls(probe: Mapping) -> list[VendoredDll]:
    out = []
    for dist in shipped_python_dists(probe.get("dists", [])):
        for lib in dist.get("libs") or []:
            hit = classify_dll(lib)
            out.append(
                VendoredDll(
                    dist=dist["name"],
                    dist_version=dist.get("version", ""),
                    file=lib,
                    label=hit[0] if hit else None,
                    license=hit[1] if hit else None,
                )
            )
    return sorted(out, key=lambda d: (normalize_name(d.dist), d.file.lower()))


# --------------------------------------------------------------------------------------
# npm / Rust


def license_files_in(directory: Path) -> list[LicenseText]:
    if not directory.is_dir():
        return []
    texts = []
    for path in sorted(directory.iterdir(), key=lambda p: p.name.lower()):
        if path.is_file() and LICENSE_FILE_RE.match(path.name):
            text = path.read_text(encoding="utf-8", errors="replace")
            if text.strip():
                texts.append(LicenseText(path.name, text))
    return texts


def _npm_license(info: Mapping, package_dir: Path) -> str:
    value = info.get("license")
    if not value:
        manifest = package_dir / "package.json"
        if manifest.is_file():
            data = json.loads(manifest.read_text(encoding="utf-8"))
            value = data.get("license") or data.get("licenses")
    if isinstance(value, list):  # legacy {"licenses": [{"type": ...}]}
        value = " OR ".join(v.get("type", "") if isinstance(v, dict) else str(v) for v in value)
    if isinstance(value, dict):
        value = value.get("type", "")
    return str(value or "UNKNOWN")


def npm_components(lock: Mapping, web_dir: Path) -> list[Component]:
    """Production packages of a lockfileVersion 2/3 ``package-lock.json``."""
    seen: dict[tuple[str, str], Component] = {}
    for key, info in sorted((lock.get("packages") or {}).items()):
        if "node_modules/" not in key or info.get("link"):
            continue
        if info.get("dev") or info.get("devOptional"):
            continue
        name = info.get("name") or key.rsplit("node_modules/", 1)[1]
        version = info.get("version", "")
        package_dir = web_dir / key
        if (name, version) in seen:
            continue
        seen[(name, version)] = Component(
            ecosystem="npm",
            name=name,
            version=version,
            license=_npm_license(info, package_dir),
            texts=license_files_in(package_dir),
        )
    return [seen[k] for k in sorted(seen, key=lambda k: (k[0].lower(), k[1]))]


def _is_normal_dep(dep: Mapping) -> bool:
    kinds = dep.get("dep_kinds")
    if not kinds:
        return True  # very old cargo: no kind information
    return any(k.get("kind") is None for k in kinds)


def rust_components(metadata: Mapping) -> list[Component]:
    """Normal deps reachable from the root crate of ``cargo metadata`` output."""
    packages = {p["id"]: p for p in metadata.get("packages", [])}
    resolve = metadata.get("resolve") or {}
    nodes = {n["id"]: n for n in resolve.get("nodes", [])}
    roots = (
        [resolve["root"]] if resolve.get("root") else list(metadata.get("workspace_members", []))
    )
    reachable: set[str] = set()
    stack = list(roots)
    while stack:
        pkg_id = stack.pop()
        if pkg_id in reachable or pkg_id not in nodes:
            continue
        reachable.add(pkg_id)
        stack.extend(d["pkg"] for d in nodes[pkg_id].get("deps", []) if _is_normal_dep(d))
    seen: dict[tuple[str, str], Component] = {}
    for pkg_id in reachable:
        pkg = packages.get(pkg_id)
        if pkg is None or pkg.get("source") is None:  # workspace / path crates are ours
            continue
        key = (pkg["name"], pkg["version"])
        if key in seen:
            continue
        crate_dir = Path(pkg["manifest_path"]).parent
        texts = license_files_in(crate_dir)
        if pkg.get("license_file"):
            extra = crate_dir / pkg["license_file"]
            if extra.is_file() and all(t.source != extra.name for t in texts):
                texts.append(
                    LicenseText(extra.name, extra.read_text(encoding="utf-8", errors="replace"))
                )
        licence = pkg.get("license") or ("LicenseRef-file" if pkg.get("license_file") else "")
        seen[key] = Component(
            ecosystem="rust",
            name=pkg["name"],
            version=pkg["version"],
            license=licence or "UNKNOWN",
            texts=texts,
        )
    return [seen[k] for k in sorted(seen)]


# --------------------------------------------------------------------------------------
# Hand-kept components


def _version(dist_versions: Mapping[str, str], dist: str, parts: int | None = None) -> str:
    version = dist_versions.get(dist)
    if not version:
        return "（未偵測到）"
    return ".".join(version.split(".")[:parts]) if parts else version


def hand_components(probe: Mapping, target: Target | None = None) -> list[Component]:
    target = target or target_for(probe.get("platform"))
    win = target.key == "win32"
    versions = {normalize_name(d["name"]): d.get("version", "") for d in probe.get("dists", [])}
    texts_of = {
        normalize_name(d["name"]): [LicenseText(s, t) for s, t in d.get("texts") or [] if t.strip()]
        for d in probe.get("dists", [])
    }
    cpython_text = probe.get("cpython_license") or ""
    common = [
        Component(
            "hand",
            "Open CASCADE Technology (OCCT)",
            _version(versions, "cadquery-ocp", 3),
            "LGPL-2.1-only WITH OCCT-exception-1.0",
            [
                LicenseText("LICENSE_LGPL_21.txt", LGPL_2_1_TEXT),
                LicenseText("OCCT_LGPL_EXCEPTION.txt", OCCT_EXCEPTION_TEXT),
            ],
            note=(
                "由 OCP（cadquery-ocp wheel，綁定層為 Apache-2.0）以動態連結的 "
                f"{target.lib('TK*')} 提供。"
            ),
            url="https://github.com/Open-Cascade-SAS/OCCT",
        ),
        Component(
            "hand",
            "VTK (Visualization Toolkit)",
            _version(versions, "vtk"),
            "BSD-3-Clause",
            texts_of.get("vtk", []),
            note=(
                f"vtk wheel 隨附 {target.lib('vtk*')}；VTK 內建的 ThirdParty 模組（hdf5、netCDF、libxml2、"
                "expat、freetype、gl2ps、sqlite、libharu、libproj、pugixml、lz4、theora 等）各有授權，"
                "見 VTK 原始碼 ThirdParty/ 目錄；其中 gl2ps 為 LGPL-2.0-or-later 或 GL2PS 授權擇一。"
            ),
            url="https://gitlab.kitware.com/vtk/vtk",
        ),
        Component(
            "hand",
            "CPython",
            probe.get("python_version") or "（未偵測到）",
            "PSF-2.0",
            [LicenseText("LICENSE.txt", cpython_text)] if cpython_text.strip() else [],
            note=(
                f"PyInstaller 隨附 {'python3*.dll' if win else target.lib('python3*')} 與標準"
                "函式庫；CPython 的 LICENSE 亦列出其內含的"
                "第三方元件（OpenSSL、libffi、SQLite、zlib、bzip2、xz 等）。"
            ),
            url="https://www.python.org/",
        ),
        Component(
            "hand",
            "PyInstaller bootloader",
            _version(versions, "pyinstaller"),
            "GPL-2.0-or-later WITH Bootloader-exception",
            texts_of.get("pyinstaller", []),
            note=(
                f"promptcad-sidecar{target.exe_suffix} 內含 PyInstaller bootloader 與執行期模組（pyimod*、pyi_rth_*）；"
                "其 GPL 附加例外允許以任意授權散布由其封裝的程式。PyInstaller 的分析／建置工具本身不隨附。"
            ),
            url="https://github.com/pyinstaller/pyinstaller",
        ),
        Component(
            "hand",
            "NLopt",
            _version(versions, "nlopt"),
            "LGPL-2.1-or-later AND MIT",
            texts_of.get("nlopt", []),
            note=(
                "nlopt wheel 的 metadata 標示 MIT，但其 LICENSE 說明編譯後的函式庫（luksan 例程）"
                "受 LGPL-2.1+ 規範，故以 LGPL 處理。"
            ),
            url="https://github.com/stevengj/nlopt",
        ),
    ]
    if not target.windows_only_hand:
        # macOS / Linux: the system WebView (WKWebView / WebKitGTK) and the C/C++ runtimes
        # are OS libraries, not shipped; vendored GCC / LLVM runtimes are listed per file.
        return common
    return common + [
        Component(
            "hand",
            "Microsoft Visual C++ Runtime",
            "14.x",
            "LicenseRef-Microsoft-VC-Redist",
            note=(
                "vcruntime140*.dll、msvcp140*.dll、vcomp140.dll 等由 CPython 與各 wheel 隨附，"
                "依 Visual Studio 授權條款之「可散布程式碼（Distributable Code）」條款散布。"
            ),
            url="https://visualstudio.microsoft.com/license-terms/",
        ),
        Component(
            "hand",
            "Microsoft Edge WebView2 Runtime（Evergreen Bootstrapper）",
            "Evergreen",
            "LicenseRef-Microsoft-WebView2",
            note=(
                "tauri.conf.json 設定 webviewInstallMode=downloadBootstrapper：安裝程式在缺少 "
                "WebView2 時下載並執行 Microsoft 的 bootstrapper；Runtime 由 Microsoft 依其授權"
                "條款安裝，不包含在本安裝程式內。"
            ),
            url="https://developer.microsoft.com/microsoft-edge/webview2/",
        ),
        Component(
            "hand",
            "NSIS (Nullsoft Scriptable Install System)",
            "3.x",
            "Zlib",
            note="安裝程式（Tauri NSIS 目標）的 stub 與外掛；壓縮元件含 bzip2 與 LZMA（public domain）。",
            url="https://nsis.sourceforge.io/License",
        ),
    ]


# --------------------------------------------------------------------------------------
# Rendering


class TextRegistry:
    """Licence texts deduplicated by hash, with the components that use each one."""

    def __init__(self) -> None:
        self.texts: dict[str, str] = {}
        self.users: dict[str, set[str]] = {}

    def add(self, text: str, user: str) -> str:
        tid = text_id(text)
        self.texts.setdefault(tid, normalize_text(text))
        self.users.setdefault(tid, set()).add(user)
        return tid


def _cell(value: str) -> str:
    return value.replace("|", "\\|").replace("\n", " ")


def _links(ids: Iterable[str]) -> str:
    unique = sorted(set(ids))
    if not unique:
        return "—（套件未附授權檔，依授權運算式）"
    return "、".join(f"[{i}](#{i.lower()})" for i in unique)


def _fence(text: str) -> str:
    longest = max((len(m) for m in re.findall(r"`+", text)), default=0)
    return "`" * max(3, longest + 1)


def _register(registry: TextRegistry, comp: Component) -> list[str]:
    label = f"{comp.name} {comp.version}".strip()
    return [registry.add(t.text, f"{label}（{t.source}）") for t in comp.texts]


def _component_table(comps: list[Component], registry: TextRegistry, *, notes: bool) -> list[str]:
    head = "| 套件 | 版本 | 授權 | 授權全文 |" + (" 備註 |" if notes else "")
    sep = "|---|---|---|---|" + ("---|" if notes else "")
    lines = [head, sep]
    for comp in comps:
        row = (
            f"| {_cell(comp.name)} | {_cell(comp.version)} | {_cell(comp.license)} | "
            f"{_links(_register(registry, comp))} |"
        )
        if notes:
            row += f" {_cell(comp.note)} |"
        lines.append(row)
    return lines


def _lgpl_section(collected: Collected, registry: TextRegistry) -> list[str]:
    v = collected.dist_versions
    occt = _version(v, "cadquery-ocp", 3)
    occt_tag = "V" + occt.replace(".", "_") if occt[0].isdigit() else ""
    casadi = _version(v, "casadi")
    nlopt = _version(v, "nlopt")
    vtk = _version(v, "vtk")
    lgpl21 = text_id(LGPL_2_1_TEXT)
    exception = text_id(OCCT_EXCEPTION_TEXT)
    gpl3 = text_id(GPL_3_TEXT)
    occt_src = f"https://github.com/Open-Cascade-SAS/OCCT/tree/{occt_tag}" if occt_tag else ""
    t = collected.target
    exe = t.exe_suffix
    files = {"win32": "`.dll` / `.pyd` 檔", "darwin": "`.dylib` / `.so` 檔"}.get(t.key, "`.so` 檔")
    raw = t.lib("raw*")
    ocp_libs = t.libs_dir("cadquery_ocp.libs")
    return [
        "## LGPL 元件與替換權利",
        "",
        "桌面版的 sidecar 以 PyInstaller **onedir** 模式封裝。下列以 LGPL（或含 LGPL 選項）授權的"
        f"函式庫**未**靜態連結進 `promptcad-sidecar{exe}` 或 `promptcad-studio{exe}`，而是以獨立的 "
        f"{files}存放在{t.install_note} 之下。使用者可依 LGPL 以相容"
        "（相同 ABI）的修改版本替換這些檔案，sidecar 下次啟動即會載入替換後的檔案。PromptCAD "
        "Studio 未修改這些函式庫；對應版本的原始碼可由下列網址取得。",
        "",
        "| 元件 | 版本 | 授權 | 安裝目錄中的檔案（典型位置） | 原始碼 |",
        "|---|---|---|---|---|",
        f"| Open CASCADE Technology（OCCT，經 OCP / cadquery-ocp 載入） | {occt} | "
        f"LGPL-2.1-only WITH OCCT-exception-1.0 | `{ocp_libs}{t.lib('TK*')}`、"
        f"`{t.internal('OCP')}` | {occt_src or 'https://github.com/Open-Cascade-SAS/OCCT'}；"
        "OCP 綁定層：https://github.com/CadQuery/OCP |",
        f"| CasADi | {casadi} | LGPL-3.0-or-later | `{t.internal('casadi')}`（`_casadi{t.ext_ext}`、"
        f"`libcasadi*{t.lib_ext}` 等） | https://github.com/casadi/casadi/tree/{casadi} |",
        f"| NLopt | {nlopt} | LGPL-2.1-or-later AND MIT | `{t.internal('nlopt')}` | "
        "https://github.com/stevengj/nlopt；Python wheel：https://github.com/DanielBok/nlopt-python |",
        f"| LibRaw（cadquery-ocp 隨附 `{raw}`） | 隨 cadquery-ocp | LGPL-2.1-only OR CDDL-1.0 | "
        f"`{ocp_libs}{raw}` | https://www.libraw.org/ |",
        f"| gl2ps（VTK 內建 `vtkgl2ps`） | 隨 VTK {vtk} | LGPL-2.0-or-later OR GL2PS | "
        f"`{t.libs_dir('vtk.libs')}{t.lib('vtkgl2ps*')}` | "
        f"https://gitlab.kitware.com/vtk/vtk/-/tree/v{vtk}/ThirdParty/gl2ps |",
        "",
        f"- OCCT 授權全文：GNU LGPL 2.1 見 [{lgpl21}](#{lgpl21.lower()})，Open CASCADE 例外條款見 "
        f"[{exception}](#{exception.lower()})。依例外條款聲明：本軟體使用 Open CASCADE Technology "
        "軟體提供的功能。",
        "- CasADi、NLopt 的授權全文見下方 Python 套件表所連結的全文；LGPL-3.0 所引用的 GNU GPL "
        f"3.0 全文見 [{gpl3}](#{gpl3.lower()})。",
        "- 公開發布時，請在安裝檔旁一併提供（或連結封存）上述 LGPL 元件於所附版本的對應原始碼，"
        "以滿足 LGPL-2.1 §6 / LGPL-3.0 §4 的要求；僅附上游網址屬一般作法，並非嚴格遵循。",
        "- FreeType、FreeImage、Zstandard 為多重授權，本軟體依其非 GPL 選項（FTL、FreeImage "
        "Public License、BSD-3-Clause）散布。",
        "",
    ]


def _native_section(dlls: list[VendoredDll], target: Target = TARGETS["win32"]) -> list[str]:
    groups: dict[tuple[str, str], dict[str, list[str]]] = {}
    for dll in dlls:
        if dll.label is None:
            continue
        key = (dll.label, dll.license or "")
        groups.setdefault(key, {}).setdefault(f"{dll.dist} {dll.dist_version}", []).append(dll.file)
    if target.key == "win32":
        intro = "以下 DLL 由 wheel 以 delvewheel 打包在 `<套件>.libs/`，隨 sidecar 安裝於 "
    else:
        intro = (
            "以下共享函式庫由 wheel 以 auditwheel（`<套件>.libs/`）或 delocate（`.dylibs/`）打包，"
            "或直接放在套件目錄內，隨 sidecar 安裝於 "
        )
    lines = [
        "## Python wheel 隨附的原生函式庫",
        "",
        intro
        + f"`{target.internal()}`。授權依各上游專案；授權全文若未隨 wheel 提供，請見上游網站。",
        "",
        "| 元件 | 授權 | 來源 wheel | 檔案 |",
        "|---|---|---|---|",
    ]
    for (label, licence), by_dist in sorted(groups.items()):
        for dist, files in sorted(by_dist.items()):
            names = sorted({normalize_dll_stem(f) for f in files})
            shown = "、".join(f"`{n}`" for n in names[:6])
            if len(names) > 6:
                shown += f" 等 {len(names)} 個"
            lines.append(f"| {_cell(label)} | {_cell(licence)} | {_cell(dist)} | {shown} |")
    lines.append("")
    return lines


def _copyleft_section(comps: list[Component]) -> list[str]:
    flagged = [c for c in comps if COPYLEFT_RE.search(c.license)]
    lines = [
        "## 需特別留意的授權（copyleft / 弱 copyleft）",
        "",
        "下表自動列出授權運算式含 GPL、LGPL、MPL、EPL 或 CDDL 的元件，供發行前人工複核。",
        "",
        "| 區段 | 元件 | 版本 | 授權 |",
        "|---|---|---|---|",
    ]
    for c in sorted(flagged, key=lambda c: (c.ecosystem, c.name.lower(), c.version)):
        lines.append(
            f"| {c.ecosystem} | {_cell(c.name)} | {_cell(c.version)} | {_cell(c.license)} |"
        )
    lines.append("")
    return lines


def render(collected: Collected) -> str:
    registry = TextRegistry()
    t = collected.target
    all_comps = collected.hand + collected.python + collected.npm + collected.rust
    native_label = "DLL" if t.key == "win32" else "共享函式庫"
    lines = [
        "# 第三方軟體授權聲明（Third-Party Notices）",
        "",
        "<!-- 本檔由 scripts/gen_third_party_notices.py 產生，請勿手動編輯。 -->",
        "",
        f"PromptCAD Studio 桌面版（{t.bundle}）隨附下列第三方軟體。本檔由 "
        "`scripts/gen_third_party_notices.py` 依實際安裝／鎖定的相依套件產生，安裝後位於 "
        f"`{t.notices_path}`。本檔僅為授權資訊彙整，並非法律意見。",
        "",
        "PromptCAD Studio 本身以 MIT 授權釋出（見 `LICENSE`）。",
        "",
        "## 摘要",
        "",
        "| 區段 | 元件數 |",
        "|---|---|",
        f"| 原生／手動維護元件 | {len(collected.hand)} |",
        f"| Python 套件（凍結 sidecar，CPython {collected.python_version}） | "
        f"{len(collected.python)} |",
        f"| Python wheel 隨附 {native_label} | {len([d for d in collected.native if d.label])} |",
        f"| npm 套件（前端正式相依） | {len(collected.npm)} |",
        f"| Rust crates（桌面殼層，{t.rust_target}） | {len(collected.rust)} |",
        "",
    ]
    lines += _lgpl_section(collected, registry)
    dll_comps = {
        (d.label, d.license): Component("dll", d.label, f"隨 {d.dist} {d.dist_version}", d.license)
        for d in collected.native
        if d.label and d.license
    }
    lines += _copyleft_section(all_comps + [dll_comps[k] for k in sorted(dll_comps)])
    lines += ["## 原生／手動維護元件", ""]
    lines += [
        "| 元件 | 版本 | 授權 | 授權全文 | 說明 |",
        "|---|---|---|---|---|",
    ]
    for comp in collected.hand:
        note = comp.note + (f" {comp.url}" if comp.url else "")
        lines.append(
            f"| {_cell(comp.name)} | {_cell(comp.version)} | {_cell(comp.license)} | "
            f"{_links(_register(registry, comp))} | {_cell(note)} |"
        )
    lines.append("")
    lines += [
        "## Python 套件（凍結 sidecar）",
        "",
        "不含建置工具（" + "、".join(sorted(BUILD_ONLY_DISTS)) + "）與 "
        "`scripts/build_sidecar.py` 排除的模組（" + "、".join(excluded_pruned_dists()) + "）。",
        "",
    ]
    lines += _component_table(collected.python, registry, notes=True)
    lines.append("")
    lines += _native_section(collected.native, t)
    lines += ["## npm 套件（前端正式相依，打包進 Web UI）", ""]
    lines += _component_table(collected.npm, registry, notes=False)
    lines.append("")
    lines += [f"## Rust crates（桌面殼層，{t.rust_target}）", ""]
    lines += _component_table(collected.rust, registry, notes=False)
    lines.append("")
    lines += ["## 授權全文", ""]
    order = sorted(registry.texts, key=lambda t: (min(registry.users[t]).lower(), t))
    for tid in order:
        text = registry.texts[tid]
        fence = _fence(text)
        lines += [
            f'<a id="{tid.lower()}"></a>',
            "",
            f"### {tid}",
            "",
            "使用者：" + "；".join(sorted(registry.users[tid], key=lambda u: (u.lower(), u))),
            "",
            fence + "text",
            text,
            fence,
            "",
        ]
    return "\n".join(lines).rstrip("\n") + "\n"


# --------------------------------------------------------------------------------------
# Checks / collection / CLI


def strict_errors(collected: Collected) -> list[str]:
    errors = []
    for comp in collected.hand + collected.python + collected.npm + collected.rust:
        if is_unknown(comp.license):
            errors.append(
                f"{comp.ecosystem}: {comp.name} {comp.version} 授權不明（請補 LICENSE_OVERRIDES）"
            )
    for dll in collected.native:
        if dll.label is None:
            errors.append(f"python: {dll.dist} 隨附未知的 DLL {dll.file}（請補 VENDORED_DLLS）")
    for dist in REQUIRED_PYTHON_DISTS:
        if dist not in collected.dist_versions:
            errors.append(f"python: 找不到 {dist}；--python 應指向含 CadQuery 的 .venv-desktop")
    if SIDECAR_PRUNED_ERROR:
        errors.append(f"無法載入 scripts/build_sidecar.py 的 PRUNED_DISTS：{SIDECAR_PRUNED_ERROR}")
    return errors


def missing_text_warnings(collected: Collected) -> list[str]:
    return [
        f"{c.ecosystem}: {c.name} {c.version} 未附授權檔"
        for c in collected.python + collected.npm + collected.rust
        if not c.texts
    ]


def collect(probe: Mapping, lock: Mapping, web_dir: Path, cargo_meta: Mapping) -> Collected:
    target = target_for(probe.get("platform"))
    return Collected(
        python=python_components(probe),
        native=vendored_dlls(probe),
        npm=npm_components(lock, web_dir),
        rust=rust_components(cargo_meta),
        hand=hand_components(probe, target),
        python_version=probe.get("python_version", ""),
        dist_versions={
            normalize_name(d["name"]): d.get("version", "") for d in probe.get("dists", [])
        },
        target=target,
    )


def run_python_probe(python: str) -> dict:
    if Path(python).exists():  # CreateProcess does not resolve relative "a/b/python.exe"
        python = str(Path(python).resolve())
    proc = subprocess.run(
        [python, "-I", "-c", PY_PROBE],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    if proc.returncode != 0:
        raise SystemExit(f"Python probe failed ({python}):\n{proc.stderr}")
    return json.loads(proc.stdout)


def run_cargo_metadata(cargo: str, tauri_dir: Path, rust_target: str = RUST_TARGET) -> dict:
    proc = subprocess.run(
        [cargo, "metadata", "--format-version", "1", "--filter-platform", rust_target, "--locked"],
        cwd=tauri_dir,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    if proc.returncode != 0:
        raise SystemExit(f"cargo metadata failed:\n{proc.stderr}")
    return json.loads(proc.stdout)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--python", required=True, help="interpreter whose packages are frozen")
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--strict", action="store_true", help="fail on unknown/empty licences")
    parser.add_argument("--check", action="store_true", help="compare with --out, do not write")
    parser.add_argument("--cargo", default="cargo")
    args = parser.parse_args(argv)
    for stream in (sys.stdout, sys.stderr):  # CI consoles may be cp1252; messages are zh-TW
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(errors="replace")

    probe = run_python_probe(args.python)
    lock = json.loads((WEB_DIR / "package-lock.json").read_text(encoding="utf-8"))
    cargo_meta = run_cargo_metadata(
        args.cargo, TAURI_DIR, target_for(probe.get("platform")).rust_target
    )
    collected = collect(probe, lock, WEB_DIR, cargo_meta)

    for warning in missing_text_warnings(collected):
        print(f"warning: {warning}", file=sys.stderr)
    errors = strict_errors(collected)
    for error in errors:
        print(f"{'error' if args.strict else 'warning'}: {error}", file=sys.stderr)
    if args.strict and errors:
        return 1

    document = render(collected)
    if args.check:
        existing = args.out.read_text(encoding="utf-8") if args.out.is_file() else ""
        if existing.replace("\r\n", "\n") != document:
            print(
                f"{args.out} is out of date; rerun scripts/gen_third_party_notices.py",
                file=sys.stderr,
            )
            return 1
        print(f"{args.out} is up to date")
        return 0
    args.out.write_text(document, encoding="utf-8", newline="\n")
    print(
        f"wrote {args.out} ({len(document.encode('utf-8')):,} bytes): "
        f"{len(collected.python)} python, {len(collected.npm)} npm, {len(collected.rust)} rust, "
        f"{len(collected.hand)} hand-kept"
    )
    return 0


# --------------------------------------------------------------------------------------
# Verbatim licence texts for components that ship no text of their own. Source:
# https://github.com/Open-Cascade-SAS/OCCT (tag V7_9_3), OCCT_LGPL_EXCEPTION.txt and
# LICENSE_LGPL_21.txt.

# GNU GPL 3.0 (FSF verbatim copy, scripts/licenses/GPL-3.0.txt). LGPL-3.0 components such as
# casadi incorporate it by reference, so it ships as its own text instead of relying on a copy
# embedded in another package's licence file.
GPL_3_TEXT = (Path(__file__).resolve().parent / "licenses" / "GPL-3.0.txt").read_text(
    encoding="utf-8"
)
GPL_3_REFERENCED_BY = frozenset({"casadi"})

OCCT_EXCEPTION_TEXT = """\
Open CASCADE exception (version 1.0) to GNU LGPL version 2.1.

The object code (i.e. not a source) form of a "work that uses the Library"
can incorporate material from a header file that is part of the Library.
As a special exception to the GNU Lesser General Public License version 2.1,
you may distribute such object code incorporating material from header files
provided with the Open CASCADE Technology libraries (including code of CDL
generic classes) under terms of your choice, provided that you give
prominent notice in supporting documentation to this code that it makes use
of or is based on facilities provided by the Open CASCADE Technology software."""

LGPL_2_1_TEXT = """\
\t\t  GNU LESSER GENERAL PUBLIC LICENSE
\t\t       Version 2.1, February 1999

 Copyright (C) 1991, 1999 Free Software Foundation, Inc.
 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 Everyone is permitted to copy and distribute verbatim copies
 of this license document, but changing it is not allowed.

[This is the first released version of the Lesser GPL.  It also counts
 as the successor of the GNU Library Public License, version 2, hence
 the version number 2.1.]

\t\t\t    Preamble

  The licenses for most software are designed to take away your
freedom to share and change it.  By contrast, the GNU General Public
Licenses are intended to guarantee your freedom to share and change
free software--to make sure the software is free for all its users.

  This license, the Lesser General Public License, applies to some
specially designated software packages--typically libraries--of the
Free Software Foundation and other authors who decide to use it.  You
can use it too, but we suggest you first think carefully about whether
this license or the ordinary General Public License is the better
strategy to use in any particular case, based on the explanations below.

  When we speak of free software, we are referring to freedom of use,
not price.  Our General Public Licenses are designed to make sure that
you have the freedom to distribute copies of free software (and charge
for this service if you wish); that you receive source code or can get
it if you want it; that you can change the software and use pieces of
it in new free programs; and that you are informed that you can do
these things.

  To protect your rights, we need to make restrictions that forbid
distributors to deny you these rights or to ask you to surrender these
rights.  These restrictions translate to certain responsibilities for
you if you distribute copies of the library or if you modify it.

  For example, if you distribute copies of the library, whether gratis
or for a fee, you must give the recipients all the rights that we gave
you.  You must make sure that they, too, receive or can get the source
code.  If you link other code with the library, you must provide
complete object files to the recipients, so that they can relink them
with the library after making changes to the library and recompiling
it.  And you must show them these terms so they know their rights.

  We protect your rights with a two-step method: (1) we copyright the
library, and (2) we offer you this license, which gives you legal
permission to copy, distribute and/or modify the library.

  To protect each distributor, we want to make it very clear that
there is no warranty for the free library.  Also, if the library is
modified by someone else and passed on, the recipients should know
that what they have is not the original version, so that the original
author's reputation will not be affected by problems that might be
introduced by others.

  Finally, software patents pose a constant threat to the existence of
any free program.  We wish to make sure that a company cannot
effectively restrict the users of a free program by obtaining a
restrictive license from a patent holder.  Therefore, we insist that
any patent license obtained for a version of the library must be
consistent with the full freedom of use specified in this license.

  Most GNU software, including some libraries, is covered by the
ordinary GNU General Public License.  This license, the GNU Lesser
General Public License, applies to certain designated libraries, and
is quite different from the ordinary General Public License.  We use
this license for certain libraries in order to permit linking those
libraries into non-free programs.

  When a program is linked with a library, whether statically or using
a shared library, the combination of the two is legally speaking a
combined work, a derivative of the original library.  The ordinary
General Public License therefore permits such linking only if the
entire combination fits its criteria of freedom.  The Lesser General
Public License permits more lax criteria for linking other code with
the library.

  We call this license the "Lesser" General Public License because it
does Less to protect the user's freedom than the ordinary General
Public License.  It also provides other free software developers Less
of an advantage over competing non-free programs.  These disadvantages
are the reason we use the ordinary General Public License for many
libraries.  However, the Lesser license provides advantages in certain
special circumstances.

  For example, on rare occasions, there may be a special need to
encourage the widest possible use of a certain library, so that it becomes
a de-facto standard.  To achieve this, non-free programs must be
allowed to use the library.  A more frequent case is that a free
library does the same job as widely used non-free libraries.  In this
case, there is little to gain by limiting the free library to free
software only, so we use the Lesser General Public License.

  In other cases, permission to use a particular library in non-free
programs enables a greater number of people to use a large body of
free software.  For example, permission to use the GNU C Library in
non-free programs enables many more people to use the whole GNU
operating system, as well as its variant, the GNU/Linux operating
system.

  Although the Lesser General Public License is Less protective of the
users' freedom, it does ensure that the user of a program that is
linked with the Library has the freedom and the wherewithal to run
that program using a modified version of the Library.

  The precise terms and conditions for copying, distribution and
modification follow.  Pay close attention to the difference between a
"work based on the library" and a "work that uses the library".  The
former contains code derived from the library, whereas the latter must
be combined with the library in order to run.

\t\t  GNU LESSER GENERAL PUBLIC LICENSE
   TERMS AND CONDITIONS FOR COPYING, DISTRIBUTION AND MODIFICATION

  0. This License Agreement applies to any software library or other
program which contains a notice placed by the copyright holder or
other authorized party saying it may be distributed under the terms of
this Lesser General Public License (also called "this License").
Each licensee is addressed as "you".

  A "library" means a collection of software functions and/or data
prepared so as to be conveniently linked with application programs
(which use some of those functions and data) to form executables.

  The "Library", below, refers to any such software library or work
which has been distributed under these terms.  A "work based on the
Library" means either the Library or any derivative work under
copyright law: that is to say, a work containing the Library or a
portion of it, either verbatim or with modifications and/or translated
straightforwardly into another language.  (Hereinafter, translation is
included without limitation in the term "modification".)

  "Source code" for a work means the preferred form of the work for
making modifications to it.  For a library, complete source code means
all the source code for all modules it contains, plus any associated
interface definition files, plus the scripts used to control compilation
and installation of the library.

  Activities other than copying, distribution and modification are not
covered by this License; they are outside its scope.  The act of
running a program using the Library is not restricted, and output from
such a program is covered only if its contents constitute a work based
on the Library (independent of the use of the Library in a tool for
writing it).  Whether that is true depends on what the Library does
and what the program that uses the Library does.

  1. You may copy and distribute verbatim copies of the Library's
complete source code as you receive it, in any medium, provided that
you conspicuously and appropriately publish on each copy an
appropriate copyright notice and disclaimer of warranty; keep intact
all the notices that refer to this License and to the absence of any
warranty; and distribute a copy of this License along with the
Library.

  You may charge a fee for the physical act of transferring a copy,
and you may at your option offer warranty protection in exchange for a
fee.

  2. You may modify your copy or copies of the Library or any portion
of it, thus forming a work based on the Library, and copy and
distribute such modifications or work under the terms of Section 1
above, provided that you also meet all of these conditions:

    a) The modified work must itself be a software library.

    b) You must cause the files modified to carry prominent notices
    stating that you changed the files and the date of any change.

    c) You must cause the whole of the work to be licensed at no
    charge to all third parties under the terms of this License.

    d) If a facility in the modified Library refers to a function or a
    table of data to be supplied by an application program that uses
    the facility, other than as an argument passed when the facility
    is invoked, then you must make a good faith effort to ensure that,
    in the event an application does not supply such function or
    table, the facility still operates, and performs whatever part of
    its purpose remains meaningful.

    (For example, a function in a library to compute square roots has
    a purpose that is entirely well-defined independent of the
    application.  Therefore, Subsection 2d requires that any
    application-supplied function or table used by this function must
    be optional: if the application does not supply it, the square
    root function must still compute square roots.)

These requirements apply to the modified work as a whole.  If
identifiable sections of that work are not derived from the Library,
and can be reasonably considered independent and separate works in
themselves, then this License, and its terms, do not apply to those
sections when you distribute them as separate works.  But when you
distribute the same sections as part of a whole which is a work based
on the Library, the distribution of the whole must be on the terms of
this License, whose permissions for other licensees extend to the
entire whole, and thus to each and every part regardless of who wrote
it.

Thus, it is not the intent of this section to claim rights or contest
your rights to work written entirely by you; rather, the intent is to
exercise the right to control the distribution of derivative or
collective works based on the Library.

In addition, mere aggregation of another work not based on the Library
with the Library (or with a work based on the Library) on a volume of
a storage or distribution medium does not bring the other work under
the scope of this License.

  3. You may opt to apply the terms of the ordinary GNU General Public
License instead of this License to a given copy of the Library.  To do
this, you must alter all the notices that refer to this License, so
that they refer to the ordinary GNU General Public License, version 2,
instead of to this License.  (If a newer version than version 2 of the
ordinary GNU General Public License has appeared, then you can specify
that version instead if you wish.)  Do not make any other change in
these notices.

  Once this change is made in a given copy, it is irreversible for
that copy, so the ordinary GNU General Public License applies to all
subsequent copies and derivative works made from that copy.

  This option is useful when you wish to copy part of the code of
the Library into a program that is not a library.

  4. You may copy and distribute the Library (or a portion or
derivative of it, under Section 2) in object code or executable form
under the terms of Sections 1 and 2 above provided that you accompany
it with the complete corresponding machine-readable source code, which
must be distributed under the terms of Sections 1 and 2 above on a
medium customarily used for software interchange.

  If distribution of object code is made by offering access to copy
from a designated place, then offering equivalent access to copy the
source code from the same place satisfies the requirement to
distribute the source code, even though third parties are not
compelled to copy the source along with the object code.

  5. A program that contains no derivative of any portion of the
Library, but is designed to work with the Library by being compiled or
linked with it, is called a "work that uses the Library".  Such a
work, in isolation, is not a derivative work of the Library, and
therefore falls outside the scope of this License.

  However, linking a "work that uses the Library" with the Library
creates an executable that is a derivative of the Library (because it
contains portions of the Library), rather than a "work that uses the
library".  The executable is therefore covered by this License.
Section 6 states terms for distribution of such executables.

  When a "work that uses the Library" uses material from a header file
that is part of the Library, the object code for the work may be a
derivative work of the Library even though the source code is not.
Whether this is true is especially significant if the work can be
linked without the Library, or if the work is itself a library.  The
threshold for this to be true is not precisely defined by law.

  If such an object file uses only numerical parameters, data
structure layouts and accessors, and small macros and small inline
functions (ten lines or less in length), then the use of the object
file is unrestricted, regardless of whether it is legally a derivative
work.  (Executables containing this object code plus portions of the
Library will still fall under Section 6.)

  Otherwise, if the work is a derivative of the Library, you may
distribute the object code for the work under the terms of Section 6.
Any executables containing that work also fall under Section 6,
whether or not they are linked directly with the Library itself.

  6. As an exception to the Sections above, you may also combine or
link a "work that uses the Library" with the Library to produce a
work containing portions of the Library, and distribute that work
under terms of your choice, provided that the terms permit
modification of the work for the customer's own use and reverse
engineering for debugging such modifications.

  You must give prominent notice with each copy of the work that the
Library is used in it and that the Library and its use are covered by
this License.  You must supply a copy of this License.  If the work
during execution displays copyright notices, you must include the
copyright notice for the Library among them, as well as a reference
directing the user to the copy of this License.  Also, you must do one
of these things:

    a) Accompany the work with the complete corresponding
    machine-readable source code for the Library including whatever
    changes were used in the work (which must be distributed under
    Sections 1 and 2 above); and, if the work is an executable linked
    with the Library, with the complete machine-readable "work that
    uses the Library", as object code and/or source code, so that the
    user can modify the Library and then relink to produce a modified
    executable containing the modified Library.  (It is understood
    that the user who changes the contents of definitions files in the
    Library will not necessarily be able to recompile the application
    to use the modified definitions.)

    b) Use a suitable shared library mechanism for linking with the
    Library.  A suitable mechanism is one that (1) uses at run time a
    copy of the library already present on the user's computer system,
    rather than copying library functions into the executable, and (2)
    will operate properly with a modified version of the library, if
    the user installs one, as long as the modified version is
    interface-compatible with the version that the work was made with.

    c) Accompany the work with a written offer, valid for at
    least three years, to give the same user the materials
    specified in Subsection 6a, above, for a charge no more
    than the cost of performing this distribution.

    d) If distribution of the work is made by offering access to copy
    from a designated place, offer equivalent access to copy the above
    specified materials from the same place.

    e) Verify that the user has already received a copy of these
    materials or that you have already sent this user a copy.

  For an executable, the required form of the "work that uses the
Library" must include any data and utility programs needed for
reproducing the executable from it.  However, as a special exception,
the materials to be distributed need not include anything that is
normally distributed (in either source or binary form) with the major
components (compiler, kernel, and so on) of the operating system on
which the executable runs, unless that component itself accompanies
the executable.

  It may happen that this requirement contradicts the license
restrictions of other proprietary libraries that do not normally
accompany the operating system.  Such a contradiction means you cannot
use both them and the Library together in an executable that you
distribute.

  7. You may place library facilities that are a work based on the
Library side-by-side in a single library together with other library
facilities not covered by this License, and distribute such a combined
library, provided that the separate distribution of the work based on
the Library and of the other library facilities is otherwise
permitted, and provided that you do these two things:

    a) Accompany the combined library with a copy of the same work
    based on the Library, uncombined with any other library
    facilities.  This must be distributed under the terms of the
    Sections above.

    b) Give prominent notice with the combined library of the fact
    that part of it is a work based on the Library, and explaining
    where to find the accompanying uncombined form of the same work.

  8. You may not copy, modify, sublicense, link with, or distribute
the Library except as expressly provided under this License.  Any
attempt otherwise to copy, modify, sublicense, link with, or
distribute the Library is void, and will automatically terminate your
rights under this License.  However, parties who have received copies,
or rights, from you under this License will not have their licenses
terminated so long as such parties remain in full compliance.

  9. You are not required to accept this License, since you have not
signed it.  However, nothing else grants you permission to modify or
distribute the Library or its derivative works.  These actions are
prohibited by law if you do not accept this License.  Therefore, by
modifying or distributing the Library (or any work based on the
Library), you indicate your acceptance of this License to do so, and
all its terms and conditions for copying, distributing or modifying
the Library or works based on it.

  10. Each time you redistribute the Library (or any work based on the
Library), the recipient automatically receives a license from the
original licensor to copy, distribute, link with or modify the Library
subject to these terms and conditions.  You may not impose any further
restrictions on the recipients' exercise of the rights granted herein.
You are not responsible for enforcing compliance by third parties with
this License.

  11. If, as a consequence of a court judgment or allegation of patent
infringement or for any other reason (not limited to patent issues),
conditions are imposed on you (whether by court order, agreement or
otherwise) that contradict the conditions of this License, they do not
excuse you from the conditions of this License.  If you cannot
distribute so as to satisfy simultaneously your obligations under this
License and any other pertinent obligations, then as a consequence you
may not distribute the Library at all.  For example, if a patent
license would not permit royalty-free redistribution of the Library by
all those who receive copies directly or indirectly through you, then
the only way you could satisfy both it and this License would be to
refrain entirely from distribution of the Library.

If any portion of this section is held invalid or unenforceable under any
particular circumstance, the balance of the section is intended to apply,
and the section as a whole is intended to apply in other circumstances.

It is not the purpose of this section to induce you to infringe any
patents or other property right claims or to contest validity of any
such claims; this section has the sole purpose of protecting the
integrity of the free software distribution system which is
implemented by public license practices.  Many people have made
generous contributions to the wide range of software distributed
through that system in reliance on consistent application of that
system; it is up to the author/donor to decide if he or she is willing
to distribute software through any other system and a licensee cannot
impose that choice.

This section is intended to make thoroughly clear what is believed to
be a consequence of the rest of this License.

  12. If the distribution and/or use of the Library is restricted in
certain countries either by patents or by copyrighted interfaces, the
original copyright holder who places the Library under this License may add
an explicit geographical distribution limitation excluding those countries,
so that distribution is permitted only in or among countries not thus
excluded.  In such case, this License incorporates the limitation as if
written in the body of this License.

  13. The Free Software Foundation may publish revised and/or new
versions of the Lesser General Public License from time to time.
Such new versions will be similar in spirit to the present version,
but may differ in detail to address new problems or concerns.

Each version is given a distinguishing version number.  If the Library
specifies a version number of this License which applies to it and
"any later version", you have the option of following the terms and
conditions either of that version or of any later version published by
the Free Software Foundation.  If the Library does not specify a
license version number, you may choose any version ever published by
the Free Software Foundation.

  14. If you wish to incorporate parts of the Library into other free
programs whose distribution conditions are incompatible with these,
write to the author to ask for permission.  For software which is
copyrighted by the Free Software Foundation, write to the Free
Software Foundation; we sometimes make exceptions for this.  Our
decision will be guided by the two goals of preserving the free status
of all derivatives of our free software and of promoting the sharing
and reuse of software generally.

\t\t\t    NO WARRANTY

  15. BECAUSE THE LIBRARY IS LICENSED FREE OF CHARGE, THERE IS NO
WARRANTY FOR THE LIBRARY, TO THE EXTENT PERMITTED BY APPLICABLE LAW.
EXCEPT WHEN OTHERWISE STATED IN WRITING THE COPYRIGHT HOLDERS AND/OR
OTHER PARTIES PROVIDE THE LIBRARY "AS IS" WITHOUT WARRANTY OF ANY
KIND, EITHER EXPRESSED OR IMPLIED, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
PURPOSE.  THE ENTIRE RISK AS TO THE QUALITY AND PERFORMANCE OF THE
LIBRARY IS WITH YOU.  SHOULD THE LIBRARY PROVE DEFECTIVE, YOU ASSUME
THE COST OF ALL NECESSARY SERVICING, REPAIR OR CORRECTION.

  16. IN NO EVENT UNLESS REQUIRED BY APPLICABLE LAW OR AGREED TO IN
WRITING WILL ANY COPYRIGHT HOLDER, OR ANY OTHER PARTY WHO MAY MODIFY
AND/OR REDISTRIBUTE THE LIBRARY AS PERMITTED ABOVE, BE LIABLE TO YOU
FOR DAMAGES, INCLUDING ANY GENERAL, SPECIAL, INCIDENTAL OR
CONSEQUENTIAL DAMAGES ARISING OUT OF THE USE OR INABILITY TO USE THE
LIBRARY (INCLUDING BUT NOT LIMITED TO LOSS OF DATA OR DATA BEING
RENDERED INACCURATE OR LOSSES SUSTAINED BY YOU OR THIRD PARTIES OR A
FAILURE OF THE LIBRARY TO OPERATE WITH ANY OTHER SOFTWARE), EVEN IF
SUCH HOLDER OR OTHER PARTY HAS BEEN ADVISED OF THE POSSIBILITY OF SUCH
DAMAGES.

\t\t     END OF TERMS AND CONDITIONS

           How to Apply These Terms to Your New Libraries

  If you develop a new library, and you want it to be of the greatest
possible use to the public, we recommend making it free software that
everyone can redistribute and change.  You can do so by permitting
redistribution under these terms (or, alternatively, under the terms of the
ordinary General Public License).

  To apply these terms, attach the following notices to the library.  It is
safest to attach them to the start of each source file to most effectively
convey the exclusion of warranty; and each file should have at least the
"copyright" line and a pointer to where the full notice is found.

    <one line to give the library's name and a brief idea of what it does.>
    Copyright (C) <year>  <name of author>

    This library is free software; you can redistribute it and/or
    modify it under the terms of the GNU Lesser General Public
    License as published by the Free Software Foundation; either
    version 2.1 of the License, or (at your option) any later version.

    This library is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
    Lesser General Public License for more details.

    You should have received a copy of the GNU Lesser General Public
    License along with this library; if not, write to the Free Software
    Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA

Also add information on how to contact you by electronic and paper mail.

You should also get your employer (if you work as a programmer) or your
school, if any, to sign a "copyright disclaimer" for the library, if
necessary.  Here is a sample; alter the names:

  Yoyodyne, Inc., hereby disclaims all copyright interest in the
  library `Frob' (a library for tweaking knobs) written by James Random Hacker.

  <signature of Ty Coon>, 1 April 1990
  Ty Coon, President of Vice

That's all there is to it!"""


if __name__ == "__main__":
    raise SystemExit(main())
