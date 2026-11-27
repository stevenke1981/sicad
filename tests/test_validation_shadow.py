"""Golden regression for 1.0 validation through the Rust core (ADR 0005).

The 1.0 Python validator is removed; the gate that compared it with the core (G1, 49
corpus specs + frozen smoke / kill-check specs + rule-planner output for documented prompts)
passed before removal and wrote ``tests/fixtures/v1_validation_golden.json``. That file is the
oracle now: the core's report for every known 1.0 input must stay equal to it.

Regenerate deliberately with ``PROMPTCAD_UPDATE_GOLDEN=1 pytest tests/test_validation_shadow.py``.
"""

from __future__ import annotations

import asyncio
import hashlib
import importlib.util
import json
import os
import sys
from collections import Counter
from pathlib import Path

import pytest

from app.models.cad import CadDocument
from app.services.planners.rule_based import RuleBasedPlanner

ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "crates" / "promptcad-core" / "tests" / "fixtures" / "v1_parity.json"
GOLDEN = ROOT / "tests" / "fixtures" / "v1_validation_golden.json"

# Prompts taken from tests/test_rule_planner.py, tests/test_api.py, README.md and
# docs/PROMPT_GUIDE.md (the guide's hole phrases are put on a plate / L bracket).
PROMPTS: list[tuple[str, str]] = [
    ("test_rule_planner", "鋁合金固定板，長120mm、寬60mm、厚10mm，四角 M6 通孔，邊距10mm，R5"),
    ("test_rule_planner", "做一個墊圈，外徑30mm、內徑15mm、厚5mm"),
    ("test_rule_planner", "做一個100x70x30mm的開口盒，壁厚2mm"),
    (
        "test_rule_planner",
        "Create an L bracket 80 mm wide, 50 mm deep, 60 mm tall, 4 mm thick, with two 5 mm holes.",
    ),
    ("test_rule_planner", "長100寬50厚8的板，兩個 M6 孔"),
    ("test_rule_planner", "做一個長80寬40厚10的板，中央一個直徑5mm、深6mm盲孔"),
    ("test_rule_planner", "100x60x10mm plate with two 5mm blind holes 6mm deep"),
    ("test_rule_planner", "做一個L型支架，寬80、深50、高60、厚4，立板兩個M5孔"),
    ("test_rule_planner", "長100寬60厚10，兩個5mm沉頭孔"),
    ("test_api", "長120mm寬60mm厚10mm固定板，四角M6通孔，R5"),
    ("test_api", "長100mm寬50mm厚8mm固定板，兩個5mm孔"),
    ("test_api", "長80寬40厚6的板"),
    ("README", "畫一個長120、寬60、厚10的固定板，四角M6孔，R5"),
    ("README", "外徑30、內徑15、厚5的墊圈"),
    ("README", "鋁合金固定座，長120mm、寬60mm、厚10mm，四角 M6 通孔，R5"),
    ("PROMPT_GUIDE", "鋁合金固定板，長120mm、寬60mm、厚10mm，四角 M6 通孔，孔中心離邊10mm，垂直邊 R5。"),
    ("PROMPT_GUIDE", "長120、寬60、厚10的板，兩個直徑5mm通孔"),
    ("PROMPT_GUIDE", "長120、寬60、厚10的板，四角 M6 通孔，邊距10mm"),
    ("PROMPT_GUIDE", "長120、寬60、厚10的板，兩個5mm沉頭孔，沉頭徑10mm，角度90度"),
    ("PROMPT_GUIDE", "長120、寬60、厚10的板，兩個5mm沉孔，沉孔直徑9mm，沉孔深3mm"),
    ("PROMPT_GUIDE", "長120、寬60、厚10的板，直徑5mm孔，x=20mm, y=-10mm"),
    ("PROMPT_GUIDE", "長100寬60厚10的板，兩個 M6 攻牙孔"),
    ("PROMPT_GUIDE", "L 支架：寬80、深50、高60、厚4，立板兩個 M5 孔"),
    ("PROMPT_GUIDE", "圓柱：直徑30、高50，中央一個直徑5mm通孔"),
    ("PROMPT_GUIDE", "外殼：100x70x30 mm，壁厚2，兩個5mm孔"),
]


def _load_script(name: str):
    spec = importlib.util.spec_from_file_location(f"_{name}", ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _plan_all() -> list[tuple[str, str, dict]]:
    async def run() -> list[tuple[str, str, dict]]:
        planner = RuleBasedPlanner()
        out = []
        for index, (source, prompt) in enumerate(PROMPTS):
            doc = await planner.plan(prompt)
            out.append((f"prompt:{source}", f"{index:02d} {prompt}", doc.model_dump(mode="json")))
        return out

    return asyncio.run(run())


def collect_inputs() -> list[tuple[str, str, dict]]:
    """``(source, id, spec)`` for every known 1.0 input."""
    inputs: list[tuple[str, str, dict]] = []
    for case in json.loads(CORPUS.read_text(encoding="utf-8")):
        inputs.append(("corpus", case["name"], case["spec"]))
    smoke = _load_script("frozen_smoke")
    for label, spec in smoke.smoke_cases():
        inputs.append(("frozen_smoke", label, spec))
    kill = _load_script("frozen_kill_check")
    for attempt in (1, 2, 3):
        inputs.append(("kill_check", f"kill_spec_{attempt}", kill.kill_spec(attempt)))
    inputs.extend(_plan_all())
    ids = [i for _, i, _ in inputs]
    assert len(ids) == len(set(ids)), "input ids must be unique"
    return inputs


def _spec_hash(spec: dict) -> str:
    blob = json.dumps(spec, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    return hashlib.sha256(blob.encode("utf-8")).hexdigest()


def _golden_entries(inputs, reports) -> list[dict]:
    return [
        {
            "id": case_id,
            "source": source,
            "spec_sha256": _spec_hash(spec),
            "report": report.model_dump(mode="json"),
        }
        for (source, case_id, spec), report in zip(inputs, reports, strict=True)
    ]


@pytest.fixture(scope="module")
def inputs():
    return collect_inputs()


@pytest.fixture(scope="module")
def core_reports(inputs, core_binary):
    from app.core.config import Settings
    from app.services.core_bridge import CoreBridge
    from app.services.core_validator import CoreValidator

    bridge = CoreBridge(Settings(env="test"))
    bridge.override_binary(core_binary)

    async def run():
        validator = CoreValidator(bridge)
        return [await validator.validate(CadDocument.model_validate(spec)) for _, _, spec in inputs]

    try:
        return asyncio.run(run())
    finally:
        bridge.shutdown()


def test_input_counts(inputs):
    counts = Counter(source for source, _, _ in inputs)
    assert counts["corpus"] == 49
    assert counts["kill_check"] == 3
    assert counts["frozen_smoke"] >= 5
    assert sum(1 for s in counts if s.startswith("prompt:")) >= 4
    assert sum(n for s, n in counts.items() if s.startswith("prompt:")) >= 20


def test_golden_file_matches_core(inputs, core_reports):
    entries = _golden_entries(inputs, core_reports)
    rendered = json.dumps(entries, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    if os.environ.get("PROMPTCAD_UPDATE_GOLDEN") == "1":
        GOLDEN.parent.mkdir(parents=True, exist_ok=True)
        GOLDEN.write_bytes(rendered.encode("utf-8"))
    committed = json.loads(GOLDEN.read_text(encoding="utf-8"))
    assert [e["id"] for e in committed] == [e["id"] for e in entries], "input set changed"
    for want, got in zip(committed, entries, strict=True):
        assert want["spec_sha256"] == got["spec_sha256"], f"{got['id']}: input spec changed"
        assert want["report"] == got["report"], f"{got['id']}: core report changed"


def test_golden_file_is_canonical():
    raw = GOLDEN.read_bytes()
    assert b"\r\n" not in raw and raw.endswith(b"\n")
    entries = json.loads(raw)
    assert raw.decode("utf-8") == json.dumps(
        entries, indent=2, sort_keys=True, ensure_ascii=False
    ) + "\n"
