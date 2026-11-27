"""1.0 validation through the Rust core (ADR 0005, 0012).

All rule logic lives in ``promptcad-core``; this module only sends the 1.0 document to the
``validate`` op and parses the ``report`` of the response. It never rewrites issues.
"""

from __future__ import annotations

from typing import Any

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


class DocumentRejected(ValueError):
    """The core refused a submitted document because of the user's input (HTTP 422).

    Carries the core's error code, JSON path and violations unchanged. Core ``internal`` and
    ``response_too_large`` errors are not the caller's fault and stay :class:`CoreFailed`.
    """

    def __init__(self, code: str, path: str, message: str, violations: list[Any] | None = None):
        super().__init__(f"{code} at {path or '/'}: {message}")
        self.code = code
        self.path = path
        self.violations = list(violations or [])


# Core error codes that are server-side problems rather than invalid input.
_CORE_SERVER_CODES = frozenset({"internal", "response_too_large"})


class CoreValidator:
    def __init__(self, bridge: CoreBridge):
        self.bridge = bridge

    async def validate(self, spec: CadDocument) -> ValidationReport:
        return (await self.validate_spec(spec))[0]

    async def validate_spec(
        self, spec: CadDocument
    ) -> tuple[ValidationReport, dict[str, Any], dict[str, Any]]:
        """Validate a 1.0 document: ``(report, resolved, canonical 2.0 document)``.

        The report is parsed strictly; ``resolved`` and the canonical document are parsed
        leniently (empty when the core did not return them).
        """
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
            result = response["result"]
            report = ValidationReport.model_validate(result["report"])
            resolved = result.get("resolved") or {}
            canonical = result.get("document") or {}
        except (KeyError, TypeError, ValueError, AttributeError) as exc:
            raise CoreFailed(f"promptcad-core returned an unusable validate result: {exc}") from exc
        if not isinstance(resolved, dict) or not isinstance(canonical, dict):
            raise CoreFailed("promptcad-core returned an unusable validate result: bad shapes")
        return report, resolved, canonical

    async def validate_document(
        self, document: dict[str, Any]
    ) -> tuple[ValidationReport, dict[str, Any], dict[str, Any], str]:
        """Validate a 1.0 or 2.0 document: ``(report, resolved, canonical document, version)``.

        ``resolved`` is empty when structure errors gate the solver; the caller decides.
        """
        try:
            response = await self.bridge.call({"op": "validate", "document": document})
        except CoreUnavailable as exc:
            raise CoreUnavailable(f"{exc}. {CORE_MISSING_HINT}") from exc
        if response.get("status") != "ok":
            code = str(response.get("code", "unknown"))
            message = str(response.get("message", ""))
            if code in _CORE_SERVER_CODES:
                raise CoreFailed(f"promptcad-core failed: {code}: {message}")
            violations = response.get("violations")
            raise DocumentRejected(
                code,
                str(response.get("path", "")),
                message,
                violations if isinstance(violations, list) else [],
            )
        try:
            result = response["result"]
            report = ValidationReport.model_validate(result["report"])
            resolved = result.get("resolved") or {}
            canonical = result["document"]
            version = str(result.get("input_version", ""))
        except (KeyError, TypeError, ValueError) as exc:
            raise CoreFailed(f"promptcad-core returned an unusable validate result: {exc}") from exc
        if not isinstance(resolved, dict) or not isinstance(canonical, dict):
            raise CoreFailed("promptcad-core returned an unusable validate result: bad shapes")
        return report, resolved, canonical, version
