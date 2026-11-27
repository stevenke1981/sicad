"""scripts/geometry_equivalence.py: pure helper tests plus the real corpus run.

The corpus needs CadQuery (and the core binary). It runs in-process when CadQuery imports,
otherwise through ``$PROMPTCAD_CADQUERY_PYTHON`` or ``.venv-desktop``; with neither it is
skipped, and failed when ``PROMPTCAD_REQUIRE_GEOMETRY=1``.
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "geometry_equivalence.py"


def _load():
    spec = importlib.util.spec_from_file_location("_geometry_equivalence", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


geo = _load()

BASE = {
    "volume": 1000.0,
    "bbox": [0, 0, 0, 10, 10, 10],
    "com": [5, 5, 5],
    "solids": 1,
    "warnings": 0,
}


def test_equal_metrics_have_no_differences():
    assert geo.compare_metrics(BASE, dict(BASE), symdiff=0.0) == []


def test_each_metric_is_checked():
    assert geo.compare_metrics(BASE, {**BASE, "volume": 1000.01})  # rel 1e-5
    assert geo.compare_metrics(BASE, {**BASE, "bbox": [0, 0, 0, 10, 10, 10.00001]})
    assert geo.compare_metrics(BASE, {**BASE, "com": [5, 5.00001, 5]})
    assert geo.compare_metrics(BASE, {**BASE, "solids": 2})
    assert geo.compare_metrics(BASE, {**BASE, "warnings": 1})
    assert geo.compare_metrics(BASE, dict(BASE), symdiff=0.01)


def test_tolerances_are_not_loosened():
    assert geo.VOLUME_REL_TOL == 1e-6
    assert geo.BBOX_ABS_TOL == 1e-6
    assert geo.COM_ABS_TOL == 1e-6
    assert geo.SYMDIFF_TOL == 1e-3
    assert geo.ALLOWED_DIFFERENCES == {}  # an entry needs a documented reason
    assert geo.compare_metrics(BASE, {**BASE, "volume": 1000.0 + 1e-4}) == []


def test_classify():
    ok = {"ok": True, **BASE}
    fail = {"ok": False, "error": "boom"}
    assert geo.classify(ok, dict(ok), 0.0) == ("equal", [])
    assert geo.classify(fail, dict(fail))[0] == "both_failed"
    assert geo.classify(ok, fail)[0] == "mismatch"
    assert geo.classify(fail, ok)[0] == "mismatch"
    assert geo.classify(ok, {**ok, "volume": 2000.0})[0] == "mismatch"


def test_poses_are_the_24_canonical_orientations():
    assert len(geo.POSES) == 24
    assert len(set(geo.POSES)) == 24
    assert geo.POSES[0] == ("+x", "+z")


def test_rotation_matches_the_core_convention():
    assert geo.rotate("+x", "+z", (1, 2, 3)) == (1, 2, 3)
    # x_axis +z, z_axis +x: y = z x x = -y; local (1, 0, 0) -> +z, local (0, 0, 1) -> +x
    assert geo.rotate("+z", "+x", (1, 0, 0)) == (0, 0, 1)
    assert geo.rotate("+z", "+x", (0, 0, 1)) == (1, 0, 0)
    assert geo.rotate("+z", "+x", (0, 1, 0)) == (0, -1, 0)
    # x_axis +x, z_axis -z: y = (-z) x (+x) = -y
    assert geo.rotate("+x", "-z", (0, 1, 0)) == (0, -1, 0)


def test_rotated_bbox():
    box = geo.rotated_bbox("+z", "+x", (10, 20, 30), (-1, -2, 0), (1, 2, 5))
    assert box == [10, 18, 29, 15, 22, 31]


def test_corpus_inputs_are_found():
    cases, _ = geo.corpus()
    names = [name for name, _ in cases]
    assert any(n.startswith("example:") for n in names)
    assert (
        sum(n.startswith("parity:") for n in names) >= 48
    )  # 49 specs, one identical to the example
    assert any(n.startswith("smoke:") for n in names)


def _cadquery_python() -> str | None:
    configured = os.environ.get("PROMPTCAD_CADQUERY_PYTHON")
    if configured and Path(configured).is_file():
        return configured
    for candidate in (
        ROOT / ".venv-desktop" / "Scripts" / "python.exe",
        ROOT / ".venv-desktop" / "bin" / "python",
    ):
        if candidate.is_file():
            return str(candidate)
    return None


def _assert_summary(summary: dict) -> None:
    assert summary["counts"]["mismatch"] == 0, summary["problems"]
    assert summary["counts"]["equal"] + summary["counts"]["both_failed"] > 0
    assert summary["cases"] >= 50
    assert all(n["status"] == "equal" for n in summary["native"]), summary["native"]
    assert len(summary["native"]) == 3


def test_corpus_geometry_equivalence(core_binary):
    if importlib.util.find_spec("cadquery") is not None:
        import contextlib
        import io

        buffer, errors = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(buffer), contextlib.redirect_stderr(errors):
            code = geo.main([])
        assert code == 0, errors.getvalue()[-2000:]
        _assert_summary(json.loads(buffer.getvalue()))
        return
    python = _cadquery_python()
    if python is None:
        if os.environ.get("PROMPTCAD_REQUIRE_GEOMETRY") == "1":
            pytest.fail("PROMPTCAD_REQUIRE_GEOMETRY=1 but no CadQuery interpreter is available")
        pytest.skip(
            "CadQuery not importable (set PROMPTCAD_CADQUERY_PYTHON or create .venv-desktop)"
        )
    completed = subprocess.run(
        [python, str(SCRIPT)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=900,
        env=os.environ.copy(),
    )
    assert completed.returncode == 0, completed.stderr[-2000:] + completed.stdout[-2000:]
    _assert_summary(json.loads(completed.stdout))
