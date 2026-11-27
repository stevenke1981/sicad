"""Source tier of the frozen 1.0 geometry oracle (needs the core binary, never CadQuery).

Every corpus case is validated by the real core and compiled by ``Cad2Compiler``; the sha256
of the generated ``model.py`` text must equal ``model_sha256`` in
``tests/fixtures/v1_geometry_golden.json``. The geometry tier (volume, area, bbox, ...) lives
in ``scripts/geometry_equivalence.py`` and needs CadQuery.

An intended compiler change is re-baselined with ``PROMPTCAD_UPDATE_GOLDEN=1`` (rewrites only
the ``model_sha256`` fields) and, for geometry, ``scripts/geometry_equivalence.py``.
"""

from __future__ import annotations

import copy
import importlib.util
import os
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "geometry_equivalence.py"


def _load():
    spec = importlib.util.spec_from_file_location("_geometry_equivalence_golden", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


geo = _load()
CASES, _ = geo.corpus()
UPDATE = os.environ.get("PROMPTCAD_UPDATE_GOLDEN") == "1"


@pytest.fixture(scope="module")
def golden() -> dict:
    return geo.load_golden()


@pytest.fixture(scope="module")
def core(core_binary):
    instance = geo.Core()
    try:
        yield instance
    finally:
        instance.shutdown()


def test_golden_covers_exactly_the_corpus(golden):
    assert set(golden["cases"]) == {name for name, _ in CASES}
    assert golden["meta"]["case_count"] == len(CASES)


@pytest.mark.parametrize(("name", "spec"), CASES, ids=[name for name, _ in CASES])
def test_compiled_source_matches_golden(core, golden, name, spec):
    entry = golden["cases"][name]
    assert entry["spec_sha256"] == geo.spec_sha256(spec), "corpus spec changed; re-baseline"
    source = geo._compile_result(core.validate(copy.deepcopy(spec)))
    digest = geo.source_sha256(source)
    if UPDATE and entry["model_sha256"] != digest:
        entry["model_sha256"] = digest
        geo.write_text_lf(geo.GOLDEN_PATH, geo.dump_golden(golden))
    assert entry["model_sha256"] == digest, (
        f"{name}: Cad2Compiler output changed; if intended run with PROMPTCAD_UPDATE_GOLDEN=1 "
        "and re-run scripts/geometry_equivalence.py"
    )
