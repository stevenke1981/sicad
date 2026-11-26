"""Native (shared-library) dependency walking for the frozen sidecar slim step (ADR 0011).

``scripts/build_sidecar.py`` prunes VTK libraries nothing in the bundle links against. This
module answers "which shared libraries does this binary load at startup" on all three
platforms and computes the transitive closure inside a bundle:

* Windows - PE import + delay-import tables, read with ``pefile`` (a PyInstaller dependency,
  so it is present in the build venv; imported lazily so tests run without it);
* macOS - ``otool -L``;
* Linux - ``readelf -d`` (``DT_NEEDED``).

The tool-output parsers are pure functions so the unix paths are unit-tested on Windows.
Libraries are matched **by file name** (PE: case-insensitive; Mach-O: install-name base
name, ignoring ``@rpath``/``@loader_path``; ELF: the ``DT_NEEDED`` soname), which is how
the bundle's loader search finds them: PyInstaller keeps every collected library under its
own name.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath

PE, MACHO, ELF = "pe", "macho", "elf"

# glibc core: present on every Linux system, never worth a package dependency.
GLIBC_CORE = frozenset(
    {
        "libc.so.6",
        "libm.so.6",
        "libdl.so.2",
        "libpthread.so.0",
        "librt.so.1",
        "libutil.so.1",
        "libresolv.so.2",
        "libanl.so.1",
        "libcrypt.so.1",
        "ld-linux-x86-64.so.2",
        "ld-linux-aarch64.so.1",
        "linux-vdso.so.1",
    }
)
# macOS system locations (dyld shared cache): always present.
MACOS_SYSTEM_PREFIXES = ("/usr/lib/", "/System/Library/")


def flavour(platform: str | None = None) -> str:
    """Binary format of ``platform`` (default ``sys.platform``)."""
    platform = platform or sys.platform
    if platform == "win32":
        return PE
    if platform == "darwin":
        return MACHO
    return ELF


# --------------------------------------------------------------------------- parsers


def parse_otool_l(text: str) -> list[str]:
    """Install names from ``otool -L <file>`` output. Pure.

    The first line (``<path>:``) and per-architecture headers end with ``:``. For a dylib
    otool lists its own ``LC_ID_DYLIB`` first; it is dropped by comparing base names with
    the header path. Duplicates (fat binaries) are collapsed, order kept.
    """
    deps: list[str] = []
    own: str | None = None
    for raw in text.splitlines():
        line = raw.strip()
        if not line:
            continue
        if not raw[:1].isspace() and line.endswith(":"):
            header = line[:-1]
            if " (architecture " in header:
                header = header.split(" (architecture ", 1)[0]
            own = own or PurePosixPath(header).name
            continue
        if line.endswith(":") and line.startswith("("):  # "(architecture arm64):"
            continue
        name = line.split(" (compatibility version", 1)[0].strip()
        if not name or name in deps:
            continue
        if own is not None and PurePosixPath(name).name == own:
            continue
        deps.append(name)
    return deps


_NEEDED_RE = re.compile(r"\(NEEDED\)\s+Shared library:\s*\[([^\]]+)\]")
_SONAME_RE = re.compile(r"\(SONAME\)\s+Library soname:\s*\[([^\]]+)\]")


def parse_readelf_d(text: str) -> list[str]:
    """``DT_NEEDED`` sonames from ``readelf -d <file>`` output, in order. Pure."""
    deps: list[str] = []
    for match in _NEEDED_RE.finditer(text):
        if match.group(1) not in deps:
            deps.append(match.group(1))
    return deps


def parse_readelf_soname(text: str) -> str | None:
    """``DT_SONAME`` from ``readelf -d`` output, if any. Pure."""
    match = _SONAME_RE.search(text)
    return match.group(1) if match else None


def pe_imports(path: Path) -> list[str]:
    """DLL names from a PE file's import and delay-import tables (``pefile``)."""
    import pefile  # build venv only

    pe = pefile.PE(str(path), fast_load=True)
    try:
        pe.parse_data_directories(
            directories=[
                pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"],
                pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT"],
            ]
        )
        names: list[str] = []
        for attr in ("DIRECTORY_ENTRY_IMPORT", "DIRECTORY_ENTRY_DELAY_IMPORT"):
            for entry in getattr(pe, attr, ()):
                name = entry.dll.decode("ascii", "replace")
                if name not in names:
                    names.append(name)
        return names
    finally:
        pe.close()


def _run_tool(cmd: list[str]) -> str:
    completed = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if completed.returncode != 0:
        raise OSError(f"{' '.join(cmd)} failed ({completed.returncode}): {completed.stderr.strip()}")
    return completed.stdout


def direct_deps(path: Path, platform: str | None = None) -> list[str]:
    """Libraries ``path`` asks the loader for at startup (raw names as recorded)."""
    kind = flavour(platform)
    if kind == PE:
        return pe_imports(path)
    if kind == MACHO:
        return parse_otool_l(_run_tool(["otool", "-L", str(path)]))
    return parse_readelf_d(_run_tool(["readelf", "-d", "--wide", str(path)]))


# --------------------------------------------------------------------------- bundle index


def dep_key(name: str, kind: str) -> str:
    """Lookup key of a dependency or bundle file name. Pure."""
    base = PurePosixPath(name.replace("\\", "/")).name
    return base.lower() if kind == PE else base


_ELF_LIB_RE = re.compile(r"\.so(\.\d+)*$")


def is_native_binary(path: str | os.PathLike[str], kind: str) -> bool:
    """Whether a bundle file is a shared library / extension module of ``kind``. Pure."""
    name = PurePosixPath(str(path).replace("\\", "/")).name
    lowered = name.lower()
    if kind == PE:
        return lowered.endswith((".dll", ".pyd", ".exe"))
    if kind == MACHO:
        return lowered.endswith((".dylib", ".so"))
    return _ELF_LIB_RE.search(name) is not None


def build_index(files: Iterable[Path], kind: str) -> dict[str, list[Path]]:
    """dependency key -> bundle files carrying that name (duplicates are all kept)."""
    index: dict[str, list[Path]] = {}
    for path in files:
        index.setdefault(dep_key(path.name, kind), []).append(path)
    return index


@dataclass
class Closure:
    """Result of :func:`closure`."""

    reached: set[Path] = field(default_factory=set)
    # dependency name (as recorded) -> bundle files that need it but it is not in the bundle
    unresolved: dict[str, set[Path]] = field(default_factory=dict)
    errors: dict[Path, str] = field(default_factory=dict)

    def reached_names(self, kind: str) -> set[str]:
        return {dep_key(p.name, kind) for p in self.reached}


def _real(path: Path) -> Path:
    try:
        return path.resolve()
    except OSError:
        return path


def closure(
    roots: Iterable[Path],
    index: Mapping[str, list[Path]],
    *,
    kind: str | None = None,
    deps_of: Callable[[Path], list[str]] | None = None,
) -> Closure:
    """Transitive dependency closure of ``roots`` within ``index``.

    ``deps_of`` defaults to :func:`direct_deps` for ``kind``. Every file a dependency
    name maps to is reached (and, for symlinks, its target as well). A file whose
    dependencies cannot be read is recorded in ``errors`` and treated as a leaf - the
    caller decides whether that is fatal.
    """
    kind = kind or flavour()
    if deps_of is None:
        platform = {PE: "win32", MACHO: "darwin", ELF: "linux"}[kind]

        def deps_of(path: Path) -> list[str]:
            return direct_deps(path, platform)

    result = Closure()
    stack: list[Path] = []

    def reach(path: Path) -> None:
        # Both spellings count as reached (a symlink keeps its target); only the
        # bundle spelling is scanned, once per real file.
        if path not in result.reached:
            stack.append(path)
        result.reached.update((path, _real(path)))

    for root in roots:
        reach(root)
    scanned: set[Path] = set()
    while stack:
        current = stack.pop()
        real = _real(current)
        if real in scanned:
            continue
        scanned.add(real)
        try:
            deps = deps_of(current)  # tools follow symlinks themselves
        except Exception as exc:  # unreadable or not a binary after all
            result.errors[current] = str(exc)
            continue
        for dep in deps:
            targets = index.get(dep_key(dep, kind))
            if not targets:
                result.unresolved.setdefault(dep, set()).add(current)
                continue
            for target in targets:
                reach(target)
    return result


def external_system_libs(unresolved: Iterable[str], kind: str) -> list[str]:
    """Unresolved dependencies a target system must provide (package dependencies). Pure.

    Windows: always empty (system DLLs / API sets ship with the OS). Linux: everything
    but the glibc core. macOS: anything outside ``/usr/lib`` and ``/System/Library``
    (normally empty - an entry means a bundle that leans on e.g. Homebrew, or an
    ``@rpath`` library missing from the bundle).
    """
    if kind == PE:
        return []
    if kind == ELF:
        return sorted({dep_key(n, kind) for n in unresolved} - GLIBC_CORE)
    return sorted({n for n in unresolved if not n.startswith(MACOS_SYSTEM_PREFIXES)})
