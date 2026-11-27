"""1.0 validation through the Rust core (ADR 0005, 0012).

All rule logic lives in ``promptcad-core``; this module only sends the 1.0 document to the
``validate`` op and parses the ``report`` of the response. It never rewrites issues.
"""

from __future__ import annotations

from app.models.cad import CadDocument
from app.models.generated.validation_report import ValidationReport
from app.services.core_bridge import CoreBridge, CoreFailed, CoreUnavailable

CORE_MISSING_HINT = (
    "Validation requires the promptcad-core binary: run `make core` "
    "(cargo build --release --locked --bins in crates/promptcad-core) "
    "or set PROMPTCAD_CORE_BIN to its path."
)


class CoreRejected(CoreFailed):
    """The core answered ``status: error`` for a document Pydantic had accepted."""

    def __init__(self, code: str, path: str, message: str):
        super().__init__(f"core rejected the document: {code} at {path or '/'}: {message}")
        self.code = code
        self.path = path


class CoreValidator:
    def __init__(self, bridge: CoreBridge):
        self.bridge = bridge

    async def validate(self, spec: CadDocument) -> ValidationReport:
        try:
            response = await self.bridge.call(
                {"op": "validate", "document": spec.model_dump(mode="json")}
            )
        except CoreUnavailable as exc:
            raise CoreUnavailable(f"{exc}. {CORE_MISSING_HINT}") from exc
        if response.get("status") != "ok":
            raise CoreRejected(
                str(response.get("code", "unknown")),
                str(response.get("path", "")),
                str(response.get("message", "")),
            )
        try:
            return ValidationReport.model_validate(response["result"]["report"])
        except (KeyError, TypeError, ValueError) as exc:
            raise CoreFailed(f"promptcad-core returned an unusable validate result: {exc}") from exc
