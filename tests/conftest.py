from __future__ import annotations

import asyncio
import os
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from app.core.config import Settings
from app.main import create_app
from app.models.cad import CadDocument
from app.models.generated.validation_report import ValidationReport
from app.services.core_bridge import CoreBridge, find_core_binary
from app.services.core_validator import CoreValidator


@pytest.fixture
def settings(tmp_path):
    return Settings(
        env="test",
        data_dir=tmp_path / "generated",
        planner_mode="rule",
        render_backend="source_only",
        allow_source_fallback=True,
        api_token=None,
    )


@pytest.fixture
def client(settings):
    with TestClient(create_app(settings)) as test_client:
        yield test_client


@pytest.fixture(scope="session")
def core_binary() -> Path:
    """The real promptcad-core binary: skip when absent, fail when PROMPTCAD_CORE_BIN is bad.

    Build locally with ``cargo build --release --locked --bins`` in crates/promptcad-core.
    """
    binary = find_core_binary(Settings(env="test"))
    env_value = os.environ.get("PROMPTCAD_CORE_BIN")
    if env_value and binary is None:
        pytest.fail(f"PROMPTCAD_CORE_BIN={env_value!r} does not point to a file")
    if binary is None:
        pytest.skip(
            "promptcad-core binary not found (cargo build --release --locked --bins in "
            "crates/promptcad-core, or set PROMPTCAD_CORE_BIN)"
        )
    return binary


@pytest.fixture
def core_validate(core_binary):
    """``core_validate(spec) -> ValidationReport`` through the real core (no fallback)."""

    def validate(spec: CadDocument | dict) -> ValidationReport:
        document = spec if isinstance(spec, CadDocument) else CadDocument.model_validate(spec)
        bridge = CoreBridge(Settings(env="test"))
        bridge.override_binary(core_binary)
        try:
            return asyncio.run(CoreValidator(bridge).validate(document))
        finally:
            bridge.shutdown()

    return validate
