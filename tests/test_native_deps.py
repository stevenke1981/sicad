"""Pure tests for scripts/native_deps.py (ADR 0011 slim build).

The otool/readelf parsers run on hand-written captures in the tools' real output format,
so the macOS and Linux paths are covered on any OS.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str):
    spec = importlib.util.spec_from_file_location(f"_{name}", ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


nd = _load("native_deps")

# `otool -L` on a dylib: the first entry is the library's own LC_ID_DYLIB.
OTOOL_DYLIB = """\
/b/_internal/vtkmodules/.dylibs/libvtkFiltersCore-9.6.9.6.dylib:
\t@rpath/libvtkFiltersCore-9.6.9.6.dylib (compatibility version 0.0.0, current version 0.0.0)
\t@rpath/libvtkCommonExecutionModel-9.6.9.6.dylib (compatibility version 0.0.0, current version 0.0.0)
\t@rpath/libvtkCommonDataModel-9.6.9.6.dylib (compatibility version 0.0.0, current version 0.0.0)
\t@loader_path/libvtkCommonCore-9.6.9.6.dylib (compatibility version 0.0.0, current version 0.0.0)
\t/usr/lib/libc++.1.dylib (compatibility version 1.0.0, current version 1700.255.5)
\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1345.100.2)
"""

# `otool -L` on a Python extension (MH_BUNDLE): no own id line.
OTOOL_BUNDLE = """\
/b/_internal/vtkmodules/vtkFiltersCore.cpython-312-darwin.so:
\t@rpath/libvtkFiltersCore-9.6.9.6.dylib (compatibility version 0.0.0, current version 0.0.0)
\t@rpath/libvtkWrappingPythonCore3.12-9.6.9.6.dylib (compatibility version 0.0.0, current version 0.0.0)
\t/System/Library/Frameworks/OpenGL.framework/Versions/A/OpenGL (compatibility version 1.0.0, current version 1.0.0)
\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1345.100.2)
"""

# `otool -L` on a universal binary prints one block per architecture.
OTOOL_FAT = """\
/b/_internal/libfoo.dylib (architecture x86_64):
\t@rpath/libfoo.dylib (compatibility version 1.0.0, current version 1.0.0)
\t@rpath/libbar.dylib (compatibility version 1.0.0, current version 1.0.0)
\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1311.0.0)
/b/_internal/libfoo.dylib (architecture arm64):
\t@rpath/libfoo.dylib (compatibility version 1.0.0, current version 1.0.0)
\t@rpath/libbar.dylib (compatibility version 1.0.0, current version 1.0.0)
\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1311.0.0)
"""

READELF_D = """\

Dynamic section at offset 0x4d8d28 contains 37 entries:
  Tag        Type                         Name/Value
 0x0000000000000001 (NEEDED)             Shared library: [libvtkCommonExecutionModel-9.6.so.1]
 0x0000000000000001 (NEEDED)             Shared library: [libvtkCommonDataModel-9.6.so.1]
 0x0000000000000001 (NEEDED)             Shared library: [libvtkCommonCore-9.6.so.1]
 0x0000000000000001 (NEEDED)             Shared library: [libGL.so.1]
 0x0000000000000001 (NEEDED)             Shared library: [libX11.so.6]
 0x0000000000000001 (NEEDED)             Shared library: [libstdc++.so.6]
 0x0000000000000001 (NEEDED)             Shared library: [libm.so.6]
 0x0000000000000001 (NEEDED)             Shared library: [libgcc_s.so.1]
 0x0000000000000001 (NEEDED)             Shared library: [libc.so.6]
 0x000000000000000e (SONAME)             Library soname: [libvtkFiltersCore-9.6.so.1]
 0x000000000000001d (RUNPATH)            Library runpath: [$ORIGIN]
 0x000000000000000c (INIT)               0x5d000
 0x000000006ffffff0 (VERSYM)             0x5a2c4
 0x0000000000000000 (NULL)               0x0
"""


def test_parse_otool_dylib_drops_own_id():
    assert nd.parse_otool_l(OTOOL_DYLIB) == [
        "@rpath/libvtkCommonExecutionModel-9.6.9.6.dylib",
        "@rpath/libvtkCommonDataModel-9.6.9.6.dylib",
        "@loader_path/libvtkCommonCore-9.6.9.6.dylib",
        "/usr/lib/libc++.1.dylib",
        "/usr/lib/libSystem.B.dylib",
    ]


def test_parse_otool_bundle_keeps_all_and_paths_with_spaces():
    deps = nd.parse_otool_l(OTOOL_BUNDLE)
    assert deps[0] == "@rpath/libvtkFiltersCore-9.6.9.6.dylib"
    assert "/System/Library/Frameworks/OpenGL.framework/Versions/A/OpenGL" in deps
    assert len(deps) == 4
    spaced = "/x/a b.so:\n\t/opt/My Libs/libq.dylib (compatibility version 1.0.0, current version 1.0.0)\n"
    assert nd.parse_otool_l(spaced) == ["/opt/My Libs/libq.dylib"]


def test_parse_otool_fat_binary_collapses_architectures():
    assert nd.parse_otool_l(OTOOL_FAT) == ["@rpath/libbar.dylib", "/usr/lib/libSystem.B.dylib"]


def test_parse_readelf():
    assert nd.parse_readelf_d(READELF_D) == [
        "libvtkCommonExecutionModel-9.6.so.1",
        "libvtkCommonDataModel-9.6.so.1",
        "libvtkCommonCore-9.6.so.1",
        "libGL.so.1",
        "libX11.so.6",
        "libstdc++.so.6",
        "libm.so.6",
        "libgcc_s.so.1",
        "libc.so.6",
    ]
    assert nd.parse_readelf_soname(READELF_D) == "libvtkFiltersCore-9.6.so.1"
    assert nd.parse_readelf_d("There is no dynamic section in this file.\n") == []
    assert nd.parse_readelf_soname("") is None


def test_flavour_and_keys():
    assert nd.flavour("win32") == nd.PE
    assert nd.flavour("darwin") == nd.MACHO
    assert nd.flavour("linux") == nd.ELF
    assert nd.dep_key("VTKCommonCore-9.6.2.DLL", nd.PE) == "vtkcommoncore-9.6.2.dll"
    assert nd.dep_key("@rpath/libvtkCommonCore-9.6.9.6.dylib", nd.MACHO) == (
        "libvtkCommonCore-9.6.9.6.dylib"
    )
    assert nd.dep_key("libX11.so.6", nd.ELF) == "libX11.so.6"


@pytest.mark.parametrize(
    ("name", "kind", "expected"),
    [
        ("vtkCommonCore-9.6.2.dll", nd.PE, True),
        ("OCP.cp312-win_amd64.pyd", nd.PE, True),
        ("promptcad-sidecar.exe", nd.PE, True),
        ("base_library.zip", nd.PE, False),
        ("libvtkCommonCore-9.6.so.1", nd.ELF, True),
        ("libfoo.so", nd.ELF, True),
        ("vtkCommonCore.cpython-312-x86_64-linux-gnu.so", nd.ELF, True),
        ("libfoo.so.1.2.3", nd.ELF, True),
        ("notes.so.txt", nd.ELF, False),
        ("libvtkCommonCore-9.6.9.6.dylib", nd.MACHO, True),
        ("vtkCommonCore.cpython-312-darwin.so", nd.MACHO, True),
        ("Info.plist", nd.MACHO, False),
    ],
)
def test_is_native_binary(name, kind, expected):
    assert nd.is_native_binary(name, kind) is expected


def test_closure_follows_names_and_reports_unresolved():
    d = Path("/b/_internal")
    files = {
        "root.pyd": ["A.dll", "KERNEL32.dll"],
        "A.dll": ["b.dll"],  # case-insensitive on PE
        "B.dll": ["A.dll"],  # cycle
        "orphan.dll": ["C.dll"],
        "C.dll": [],
    }
    paths = {name: d / name for name in files}
    index = nd.build_index(paths.values(), nd.PE)

    result = nd.closure([paths["root.pyd"]], index, kind=nd.PE, deps_of=lambda p: files[p.name])

    assert {p.name for p in result.reached if p.name in files} == {"root.pyd", "A.dll", "B.dll"}
    assert set(result.unresolved) == {"KERNEL32.dll"}
    assert result.errors == {}
    assert "c.dll" not in result.reached_names(nd.PE)


def test_closure_duplicate_names_all_reached_and_errors_recorded():
    a1, a2 = Path("/b/x/liba.so.1"), Path("/b/y/liba.so.1")
    root, bad = Path("/b/root.so"), Path("/b/bad.so")
    deps = {root: ["liba.so.1", "libbad.so"], a1: [], a2: []}

    def deps_of(path):
        if path.name == "libbad.so":
            raise OSError("not an ELF file")
        return deps[path]

    index = nd.build_index([a1, a2, root, bad.with_name("libbad.so")], nd.ELF)
    result = nd.closure([root], index, kind=nd.ELF, deps_of=deps_of)
    assert {a1, a2} <= result.reached
    assert list(result.errors) == [Path("/b/libbad.so")]


def test_external_system_libs():
    linux = ["libGL.so.1", "libX11.so.6", "libc.so.6", "libm.so.6", "ld-linux-x86-64.so.2",
             "libstdc++.so.6", "libpthread.so.0"]
    assert nd.external_system_libs(linux, nd.ELF) == ["libGL.so.1", "libX11.so.6", "libstdc++.so.6"]
    assert nd.external_system_libs(["KERNEL32.dll", "OPENGL32.dll"], nd.PE) == []
    mac = ["/usr/lib/libSystem.B.dylib", "/System/Library/Frameworks/OpenGL.framework/OpenGL",
           "/opt/homebrew/lib/libomp.dylib", "@rpath/libmissing.dylib"]
    assert nd.external_system_libs(mac, nd.MACHO) == [
        "/opt/homebrew/lib/libomp.dylib",
        "@rpath/libmissing.dylib",
    ]


def test_pe_imports_on_this_interpreter():
    pytest.importorskip("pefile")
    if sys.platform != "win32":
        pytest.skip("needs a PE file")
    dll = Path(sys.base_prefix) / f"python{sys.version_info[0]}{sys.version_info[1]}.dll"
    if not dll.is_file():
        pytest.skip("no pythonXY.dll next to the base interpreter")
    names = {n.lower() for n in nd.pe_imports(dll)}
    assert "kernel32.dll" in names
