"""CadQuery compiler for 2.0 documents (CAD_DSL_2.md §5.6, §6, §7; ADR 0006).

The input is the core's ``validate`` result: the canonical ``document`` plus ``resolved``.
This module does **no solving**: it only reads the solved world placement of every component
(``resolved.components``), the solved hole cutters (``resolved.holes``) and the hole entry
ports, applies rigid placement and the section 7.2 counterbore / countersink arithmetic, and
emits the same ``model.py`` contract as the 1.0 :class:`app.services.compiler.CadQueryCompiler`
(``build() -> (result, warnings)``, ``export_all``, ``--output-dir`` / ``--formats``,
``render-warnings.json``).

Known deviation from section 6.2: parts are built directly in world coordinates, not in the
part frame followed by a ``cq.Location`` assembly step. Consequently fillet / chamfer
selectors (which are world-axis selectors) are only supported on parts whose first body
volume has the identity orientation.

Safety: no document text (names, labels, source prompt, ids) is ever written into the
emitted code. Variable names are index based, numbers go through ``_f(float(...))`` and
directions come from a fixed table.
"""

from __future__ import annotations

import math
from typing import Any

SUPPORTED_VOLUMES = frozenset({"plate", "cylinder", "ring", "l_bracket", "enclosure"})
SUPPORTED_CUT_EMPTIES = frozenset({"hole", "box", "cylinder"})
SUPPORTED_ENVELOPES = frozenset({"box", "cylinder"})

_DIRECTIONS: dict[str, tuple[float, float, float]] = {
    "+x": (1.0, 0.0, 0.0),
    "-x": (-1.0, 0.0, 0.0),
    "+y": (0.0, 1.0, 0.0),
    "-y": (0.0, -1.0, 0.0),
    "+z": (0.0, 0.0, 1.0),
    "-z": (0.0, 0.0, -1.0),
}
_SELECTORS = {"all": None, "vertical": "|Z", "top": ">Z", "bottom": "<Z"}


class CompileUnsupported(Exception):
    """The document uses something this compiler cannot build (or the solver output is unusable).

    ``items`` lists every offender as ``(id, reason)``; all offenders are collected before
    this is raised.
    """

    def __init__(self, items: list[tuple[str, str]]):
        self.items = list(items)
        summary = "; ".join(f"{ident}: {reason}" for ident, reason in self.items[:8])
        if len(self.items) > 8:
            summary += f"; ... ({len(self.items)} in total)"
        super().__init__(f"cannot compile document: {summary}")


def _f(value: float) -> str:
    return format(float(value), ".12g")


def _vec(values: Any) -> tuple[float, float, float]:
    x, y, z = (float(v) for v in values)
    if not all(math.isfinite(v) for v in (x, y, z)):
        raise ValueError("non-finite coordinate")
    return x, y, z


def _tuple_src(values: tuple[float, float, float]) -> str:
    return f"({_f(values[0])}, {_f(values[1])}, {_f(values[2])})"


class Cad2Compiler:
    def compile(self, document: dict, resolved: dict) -> str:
        items: list[tuple[str, str]] = []
        if not isinstance(document, dict) or document.get("schema_version") != "2.0":
            raise CompileUnsupported([("document", "schema_version must be '2.0'")])
        if not isinstance(resolved, dict) or not resolved.get("components"):
            raise CompileUnsupported(
                [
                    (
                        "resolved",
                        "the solver produced no placement (structural errors gate the solver); "
                        "fix the validation errors first",
                    )
                ]
            )
        parts = document.get("parts") or []
        if not parts:
            raise CompileUnsupported([("document", "the document has no parts")])

        self._components = {c["id"]: c for c in document.get("components", [])}
        self._resolved = resolved
        self._items = items

        body_lines: list[str] = []
        used: set[str] = set()
        for pi, part in enumerate(parts):
            body_lines.extend(self._part_lines(pi, part, used))

        orphans = [
            c["id"]
            for c in document.get("components", [])
            if c.get("kind") == "volume" and c["id"] not in used
        ]
        if orphans:
            body_lines.append(
                f"    warnings.append('{len(orphans)} volume(s) belong to no part and were not modelled')"
            )
        if items:
            raise CompileUnsupported(items)

        if len(parts) == 1:
            body_lines.append("    result = part_0")
        else:
            body_lines.extend(
                [
                    "    solids = []",
                    f"    for part in [{', '.join(f'part_{i}' for i in range(len(parts)))}]:",
                    "        solids.extend(part.solids().vals())",
                    "    result = cq.Workplane('XY').newObject([cq.Compound.makeCompound(solids)])",
                ]
            )
        return "\n".join(self._header() + body_lines + self._footer())

    # ------------------------------------------------------------------ file skeleton
    @staticmethod
    def _header() -> list[str]:
        return [
            '"""Generated by PromptCAD Studio from a 2.0 document. Edit numeric parameters, '
            'then run this file."""',
            "from __future__ import annotations",
            "",
            "import argparse",
            "import json",
            "from pathlib import Path",
            "",
            "import cadquery as cq",
            "",
            "",
            "def place(shape, origin, x_axis, z_axis):",
            "    plane = cq.Plane(origin=origin, xDir=x_axis, normal=z_axis)",
            "    return cq.Workplane('XY').newObject([shape.val().moved(plane.location)])",
            "",
            "",
            "def build():",
            "    warnings = []",
        ]

    @staticmethod
    def _footer() -> list[str]:
        return [
            "    return result, warnings",
            "",
            "",
            "def export_all(result, output_dir: Path, formats: set[str], warnings: list[str]):",
            "    output_dir.mkdir(parents=True, exist_ok=True)",
            "    if 'step' in formats:",
            "        result.export(str(output_dir / 'model.step'))",
            "    if 'stl' in formats:",
            "        result.export(str(output_dir / 'model.stl'), tolerance=0.05, angularTolerance=0.1)",
            "    if 'svg' in formats:",
            "        result.export(str(output_dir / 'model.svg'), opt={",
            "            'width': 900, 'height': 650, 'marginLeft': 20, 'marginTop': 20,",
            "            'showAxes': False, 'projectionDir': (1, -1, 0.75),",
            "            'showHidden': True, 'strokeWidth': 0.7,",
            "        })",
            "    if 'dxf' in formats:",
            "        try:",
            "            box = result.val().BoundingBox()",
            "            section = result.section((box.zmin + box.zmax) / 2)",
            "            section.export(str(output_dir / 'model.dxf'), opt={'approx': 'arc', 'tolerance': 0.01})",
            "        except Exception as exc:",
            "            warnings.append('DXF section export skipped: ' + str(exc))",
            "    (output_dir / 'render-warnings.json').write_text(",
            "        json.dumps(warnings, ensure_ascii=False, indent=2), encoding='utf-8'",
            "    )",
            "",
            "",
            "def main():",
            "    parser = argparse.ArgumentParser()",
            "    parser.add_argument('--output-dir', default='.')",
            "    parser.add_argument('--formats', nargs='*', default=['step', 'stl', 'dxf', 'svg'])",
            "    args = parser.parse_args()",
            "    result, warnings = build()",
            "    export_all(result, Path(args.output_dir), set(args.formats), warnings)",
            "",
            "",
            "if __name__ == '__main__':",
            "    main()",
            "",
        ]

    # ------------------------------------------------------------------ parts
    def _part_lines(self, pi: int, part: dict, used: set[str]) -> list[str]:
        pid = part.get("id", f"part#{pi}")
        lines: list[str] = []
        if part.get("source") == "purchased":
            lines.append(
                f"    warnings.append('part {pi + 1} is purchased; placeholder geometry included')"
            )
        body = part.get("body") or []
        first_identity = True
        for bi, op in enumerate(body):
            cid = op.get("component", "?")
            comp = self._components.get(cid)
            if comp is None:
                self._items.append((cid, f"body of {pid} references an unknown component"))
                continue
            used.add(cid)
            var = f"b_{pi}_{bi}"
            kind = op.get("op")
            shape = comp.get("shape") or {}
            skind = shape.get("kind")
            ckind = comp.get("kind")
            if bi == 0:
                first_identity = self._is_identity(cid)
            if kind == "add":
                if ckind != "volume" or skind not in SUPPORTED_VOLUMES:
                    self._items.append((cid, f"add of {ckind} '{skind}' is not supported"))
                    continue
                lines.extend(self._solid_lines(var, cid, comp))
                lines.append(
                    f"    part_{pi} = {var}"
                    if bi == 0
                    else f"    part_{pi} = part_{pi}.union({var})"
                )
            elif kind == "cut":
                if (
                    ckind != "empty"
                    or comp.get("role") != "cut"
                    or skind not in SUPPORTED_CUT_EMPTIES
                ):
                    self._items.append((cid, f"cut of {ckind} '{skind}' is not supported"))
                    continue
                if bi == 0:
                    self._items.append((cid, "body must start with an add"))
                    continue
                if skind == "hole":
                    lines.extend(self._hole_lines(pi, bi, cid, shape))
                else:
                    lines.extend(self._solid_lines(var, cid, comp))
                    lines.append(f"    part_{pi} = part_{pi}.cut({var})")
            elif kind == "intersect":
                volume_ok = ckind == "volume" and skind in SUPPORTED_VOLUMES
                envelope_ok = (
                    ckind == "empty"
                    and comp.get("role") == "envelope"
                    and skind in SUPPORTED_ENVELOPES
                )
                if not (volume_ok or envelope_ok) or bi == 0:
                    self._items.append((cid, f"intersect of {ckind} '{skind}' is not supported"))
                    continue
                lines.extend(self._solid_lines(var, cid, comp))
                lines.append(f"    part_{pi} = part_{pi}.intersect({var})")
            else:
                self._items.append((cid, f"unknown body op {kind!r}"))

        features = [
            ("fillet", fi, f.get("selector"), f.get("radius"))
            for fi, f in enumerate(part.get("fillets") or [])
        ] + [
            ("chamfer", ci, f.get("selector"), f.get("distance"))
            for ci, f in enumerate(part.get("chamfers") or [])
        ]
        if features and not first_identity:
            self._items.append(
                (pid, "fillets/chamfers on a rotated part are not supported (world-frame build)")
            )
            features = []
        for name, fi, selector_name, amount in features:
            if selector_name not in _SELECTORS:
                self._items.append((pid, f"unknown {name} selector {selector_name!r}"))
                continue
            selector = _SELECTORS[selector_name]
            edges = "edges()" if selector is None else f"edges({selector!r})"
            lines.extend(
                [
                    "    try:",
                    f"        part_{pi} = part_{pi}.{edges}.{name}({self._num(pid, amount)})",
                    "    except Exception as exc:",
                    f"        warnings.append('part {pi + 1} {name} {fi + 1} skipped: ' + str(exc))",
                ]
            )
        return lines

    # ------------------------------------------------------------------ geometry
    def _num(self, ident: str, value: Any) -> str:
        try:
            number = float(value)
        except (TypeError, ValueError):
            self._items.append((ident, f"non-numeric value {value!r}"))
            return "0"
        if not math.isfinite(number):
            self._items.append((ident, "non-finite number"))
            return "0"
        return _f(number)

    def _placement(self, cid: str) -> tuple[tuple[float, float, float], str, str] | None:
        entry = (self._resolved.get("components") or {}).get(cid)
        if not isinstance(entry, dict):
            self._items.append((cid, "no solved placement in resolved.components"))
            return None
        try:
            origin = _vec(entry["origin"])
            x_axis, z_axis = entry["x_axis"], entry["z_axis"]
            if x_axis not in _DIRECTIONS or z_axis not in _DIRECTIONS:
                raise ValueError("unknown axis direction")
        except (KeyError, TypeError, ValueError) as exc:
            self._items.append((cid, f"unusable solved placement: {exc}"))
            return None
        return origin, x_axis, z_axis

    def _is_identity(self, cid: str) -> bool:
        entry = (self._resolved.get("components") or {}).get(cid) or {}
        return entry.get("x_axis") == "+x" and entry.get("z_axis") == "+z"

    def _solid_lines(self, var: str, cid: str, comp: dict) -> list[str]:
        try:
            return self._solid_lines_raw(var, cid, comp)
        except (KeyError, TypeError, ValueError) as exc:
            self._items.append((cid, f"unusable shape dimensions: {exc!r}"))
            return []

    def _solid_lines_raw(self, var: str, cid: str, comp: dict) -> list[str]:
        shape = comp["shape"]
        kind = shape.get("kind")

        def n(key: str) -> str:
            return self._num(cid, shape.get(key))

        if kind in ("plate", "box"):
            thickness = "thickness" if kind == "plate" else "height"
            height = n(thickness)
            lines = [
                f"    {var} = cq.Workplane('XY').box({n('length')}, {n('width')}, {height}, "
                "centered=(True, True, False))"
            ]
        elif kind == "cylinder":
            lines = [
                f"    {var} = cq.Workplane('XY').circle({self._half(cid, shape['diameter'])})"
                f".extrude({n('height')})"
            ]
        elif kind == "ring":
            lines = [
                f"    {var} = cq.Workplane('XY').circle({self._half(cid, shape['outer_diameter'])})"
                f".circle({self._half(cid, shape['inner_diameter'])}).extrude({n('height')})"
            ]
        elif kind == "l_bracket":
            width, depth = float(shape["width"]), float(shape["depth"])
            thick, vheight = float(shape["thickness"]), float(shape["vertical_height"])
            lines = [
                f"    {var}_base = cq.Workplane('XY').box({_f(width)}, {_f(depth)}, {_f(thick)}, "
                "centered=(True, True, False))",
                f"    {var}_up = cq.Workplane('XY').box({_f(width)}, {_f(thick)}, {_f(vheight)}, "
                f"centered=(True, True, False)).translate((0, {_f(-depth / 2 + thick / 2)}, "
                f"{_f(thick)}))",
                f"    {var} = {var}_base.union({var}_up)",
            ]
        elif kind == "enclosure":
            length, width = float(shape["length"]), float(shape["width"])
            height, wall = float(shape["height"]), float(shape["wall_thickness"])
            inner_h = height - wall
            lines = [
                f"    {var}_outer = cq.Workplane('XY').box({_f(length)}, {_f(width)}, {_f(height)}, "
                "centered=(True, True, False))",
                f"    {var}_inner = cq.Workplane('XY').box({_f(length - 2 * wall)}, "
                f"{_f(width - 2 * wall)}, {_f(inner_h + 0.2)}, centered=(True, True, False))"
                f".translate((0, 0, {_f(wall)}))",
                f"    {var} = {var}_outer.cut({var}_inner)",
            ]
        else:
            self._items.append((cid, f"unsupported shape '{kind}'"))
            return []
        placed = self._placement(cid)
        if placed is None:
            return lines
        origin, x_axis, z_axis = placed
        if x_axis == "+x" and z_axis == "+z":
            if origin != (0.0, 0.0, 0.0):
                lines.append(f"    {var} = {var}.translate({_tuple_src(origin)})")
        else:
            lines.append(
                f"    {var} = place({var}, {_tuple_src(origin)}, "
                f"{_tuple_src(_DIRECTIONS[x_axis])}, {_tuple_src(_DIRECTIONS[z_axis])})"
            )
        return lines

    def _half(self, cid: str, value: Any) -> str:
        try:
            return _f(float(value) / 2)
        except (TypeError, ValueError):
            self._items.append((cid, f"non-numeric value {value!r}"))
            return "0"

    def _hole_lines(self, pi: int, bi: int, cid: str, shape: dict) -> list[str]:
        hole = (self._resolved.get("holes") or {}).get(cid)
        entry_port = (self._resolved.get("ports") or {}).get(f"{cid}.entry")
        if not isinstance(hole, dict) or not isinstance(entry_port, dict):
            self._items.append((cid, "no solved hole cutter / entry port in resolved"))
            return []
        try:
            start = _vec(hole["cutter_start"])
            end = _vec(hole["cutter_end"])
            direction = _DIRECTIONS[hole["direction"]]
            diameter = float(hole["diameter"])
            entry = _vec(entry_port["position"])
            length = math.dist(start, end)
        except (KeyError, TypeError, ValueError) as exc:
            self._items.append((cid, f"unusable solved hole: {exc}"))
            return []
        var = f"h_{pi}_{bi}"
        blind = shape.get("hole_type") == "blind" and shape.get("depth") is not None
        if not blind and sum(direction) < 0:
            # The same cylinder, parametrised from its negative end along a positive axis (the
            # 1.0 through-hole form). OCC's boolean and volume integration depend on the
            # parametrisation at the 1e-6 level on curved host surfaces.
            start, axis = end, tuple(0.0 - c for c in direction)
        else:
            axis = direction
        lines = [
            f"    {var} = cq.Solid.makeCylinder({_f(diameter / 2)}, {_f(length)}, "
            f"cq.Vector{_tuple_src(start)}, cq.Vector{_tuple_src(axis)})",
            f"    part_{pi} = part_{pi}.cut({var})",
        ]
        hole_type = shape.get("hole_type")
        try:
            if (
                hole_type == "counterbore"
                and shape.get("counterbore_diameter")
                and shape.get("counterbore_depth")
            ):
                cstart = tuple(entry[i] - direction[i] * 0.1 for i in range(3))
                lines.extend(
                    [
                        f"    {var}_cb = cq.Solid.makeCylinder("
                        f"{_f(float(shape['counterbore_diameter']) / 2)}, "
                        f"{_f(float(shape['counterbore_depth']) + 0.2)}, "
                        f"cq.Vector{_tuple_src(cstart)}, cq.Vector{_tuple_src(direction)})",
                        f"    part_{pi} = part_{pi}.cut({var}_cb)",
                    ]
                )
            if (
                hole_type == "countersink"
                and shape.get("countersink_diameter")
                and shape.get("countersink_angle")
            ):
                outer = float(shape["countersink_diameter"])
                depth = (outer - diameter) / (
                    2 * max(0.01, math.tan(math.radians(float(shape["countersink_angle"]) / 2)))
                )
                lines.extend(
                    [
                        f"    {var}_cs = cq.Solid.makeCone({_f(outer / 2)}, {_f(diameter / 2)}, "
                        f"{_f(depth)}, cq.Vector{_tuple_src(entry)}, "
                        f"cq.Vector{_tuple_src(direction)})",
                        f"    part_{pi} = part_{pi}.cut({var}_cs)",
                    ]
                )
        except (TypeError, ValueError) as exc:
            self._items.append((cid, f"unusable hole dimensions: {exc}"))
            return []
        return lines
