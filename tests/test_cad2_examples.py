"""Paper validation of the frozen CadDocument 2.0 examples (docs/CAD_DSL_2.md).

This is *not* a 2.0 implementation: per ADR 0010 the 2.0 model, validator and solver are
implemented in the Rust core in Phase 2. The checks below only use ``json`` and ``re`` and
re-derive, by the frozen rules, what the hand-written examples claim.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
EXAMPLE_DIR = ROOT / "examples" / "cad2"
PLATE = "plate-four-holes.cad2.json"
ENCLOSURE = "enclosure-with-lid.cad2.json"
EXAMPLES = [PLATE, ENCLOSURE]

TOL = 1e-6
ID_RE = re.compile(r"^[a-z][a-z0-9_]{0,31}$")
PORT_REF_RE = re.compile(r"^([a-z][a-z0-9_]{0,31})\.([a-z][a-z0-9_]{0,31})$")
FIELD_REF_RE = re.compile(r"^([a-z][a-z0-9_]{0,31}):([a-z][a-z0-9_]{0,31})$")
THREAD_RE = re.compile(r"^M\d{1,3}(\.\d)?(x\d(\.\d{1,2})?)?$")

TOP_LEVEL_KEYS = [
    "schema_version", "name", "source_prompt", "unit", "revision", "parameters", "bindings",
    "components", "constraints", "parts", "assemblies", "root", "assumptions", "notes", "planner",
]
EXAMPLE_ONLY_KEYS = {"expected_resolved"}
COMPONENT_KEYS = {"id", "kind", "shape", "label", "provenance"}
CONSTRAINT_KEYS = {"id", "kind", "a", "b", "value", "u", "v", "frame"}
PART_KEYS = {"id", "name", "material", "quantity", "source", "body", "fillets", "chamfers"}
PROVENANCE_AGENTS = {
    "space_planner", "component_smith", "joiner", "assembler", "human", "migration",
}
EMPTY_ROLES = {"cut", "reserve", "keep_out", "envelope"}
AXIS_DIRS = {"+x", "-x", "+y", "-y", "+z", "-z"}

# ---- frozen port tables (docs/CAD_DSL_2.md §3.4), for the shapes these tests know ----------
BOX_PORTS = {
    "origin": "point", "center": "point", "axis_z": "axis",
    "top": "face", "bottom": "face",
    "x_pos": "face", "x_neg": "face", "y_pos": "face", "y_neg": "face",
    "region": "region",
}
CYLINDER_PORTS = {
    "origin": "point", "center": "point", "axis": "axis",
    "top": "face", "bottom": "face", "region": "region",
}
PORT_TABLE: dict[tuple[str, str], dict[str, str]] = {
    ("volume", "plate"): BOX_PORTS,
    ("volume", "enclosure"): {**BOX_PORTS, "cavity_floor": "face", "cavity": "region"},
    ("volume", "l_bracket"): {**BOX_PORTS, "base_top": "face", "upright_front": "face"},
    ("volume", "cylinder"): CYLINDER_PORTS,
    ("volume", "ring"): {**CYLINDER_PORTS, "bore": "region"},
    ("empty", "box"): BOX_PORTS,
    ("empty", "cylinder"): CYLINDER_PORTS,
    ("empty", "hole"): {"entry": "point", "axis": "axis", "region": "region"},
    ("dot", "point"): {"point": "point"},
}
FRAME_PORT = {key: "origin" for key in PORT_TABLE}
FRAME_PORT[("empty", "hole")] = "entry"
FRAME_PORT[("dot", "point")] = "point"

BINDABLE_FIELDS: dict[tuple[str, str], set[str]] = {
    ("volume", "plate"): {"length", "width", "thickness"},
    ("volume", "enclosure"): {"length", "width", "height", "wall_thickness"},
    ("volume", "l_bracket"): {"width", "depth", "vertical_height", "thickness"},
    ("volume", "cylinder"): {"diameter", "height"},
    ("volume", "ring"): {"outer_diameter", "inner_diameter", "height"},
    ("empty", "box"): {"length", "width", "height"},
    ("empty", "cylinder"): {"diameter", "height"},
    ("empty", "hole"): {
        "diameter", "depth", "counterbore_diameter", "counterbore_depth",
        "countersink_diameter", "countersink_angle",
    },
    ("dot", "point"): set(),
}
CONSTRAINT_FIELDS = {"value", "u", "v"}

# §5.3 port compatibility matrix
COMPATIBLE = {
    ("point", "point"): {"coincident"},
    ("point", "face"): {"on_face", "distance"},
    ("axis", "axis"): {"coaxial"},
    ("face", "face"): {"mate", "flush", "distance"},
    ("region", "region"): {"inside", "clearance"},
}
CONSTRAINT_KINDS = {"fixed"} | set().union(*COMPATIBLE.values())


def load(name: str) -> dict:
    return json.loads((EXAMPLE_DIR / name).read_text(encoding="utf-8"))


def shape_key(component: dict) -> tuple[str, str]:
    return component["kind"], component["shape"]["kind"]


def index(doc: dict) -> dict[str, dict]:
    """Namespace N1: components and constraints by id."""
    table: dict[str, dict] = {}
    for record in doc["components"] + doc["constraints"]:
        table[record["id"]] = record
    return table


def components(doc: dict) -> dict[str, dict]:
    return {c["id"]: c for c in doc["components"]}


def port_type(doc: dict, ref: str) -> str:
    match = PORT_REF_RE.match(ref)
    assert match, f"not a port reference: {ref!r}"
    cid, port = match.groups()
    comps = components(doc)
    assert cid in comps, f"dangling_reference: {ref}"
    table = PORT_TABLE[shape_key(comps[cid])]
    assert port in table, f"unknown_port: {ref}"
    return table[port]


def field_value(doc: dict, ref: str) -> float:
    match = FIELD_REF_RE.match(ref)
    assert match, f"not a field reference: {ref!r}"
    rid, field = match.groups()
    records = index(doc)
    assert rid in records, f"dangling_reference: {ref}"
    record = records[rid]
    if "a" in record:  # constraints carry port refs a/b; components never do
        assert field in CONSTRAINT_FIELDS, f"unknown_field: {ref}"
        value = record[field]
    else:
        assert field in BINDABLE_FIELDS[shape_key(record)], f"unknown_field: {ref}"
        value = record["shape"][field]
    assert isinstance(value, int | float) and not isinstance(value, bool), f"null field: {ref}"
    return float(value)


# ---- §4.3 closed expression grammar: tiny recursive-descent evaluator (no eval) ------------
class ExprError(ValueError):
    pass


_TOKEN_RE = re.compile(
    r"\s*(?:(?P<num>\d+(?:\.\d+)?)"
    r"|(?P<field>[a-z][a-z0-9_]{0,31}:[a-z][a-z0-9_]{0,31})"
    r"|(?P<ident>[a-z][a-z0-9_]{0,31})"
    r"|(?P<op>[-+*/(),]))"
)


def _tokenize(text: str) -> list[tuple[str, str]]:
    if not 1 <= len(text) <= 160:
        raise ExprError("expr length must be 1..160")
    tokens: list[tuple[str, str]] = []
    pos = 0
    while pos < len(text):
        if text[pos:].strip() == "":
            break
        match = _TOKEN_RE.match(text, pos)
        if not match or match.end() == pos:
            raise ExprError(f"binding_parse_error at {pos}")
        kind = match.lastgroup
        assert kind is not None
        tokens.append((kind, match.group(kind)))
        pos = match.end()
    return tokens


def evaluate(text: str, params: dict[str, float], field_lookup) -> float:
    tokens = _tokenize(text)
    pos = 0

    def peek() -> tuple[str, str] | None:
        return tokens[pos] if pos < len(tokens) else None

    def take(expected: str | None = None) -> tuple[str, str]:
        nonlocal pos
        token = peek()
        if token is None or (expected is not None and token[1] != expected):
            raise ExprError(f"binding_parse_error: expected {expected!r}")
        pos += 1
        return token

    def expr() -> float:
        value = term()
        while (token := peek()) and token[0] == "op" and token[1] in ("+", "-"):
            take()
            value = value + term() if token[1] == "+" else value - term()
        return value

    def term() -> float:
        value = unary()
        while (token := peek()) and token[0] == "op" and token[1] in ("*", "/"):
            take()
            rhs = unary()
            if token[1] == "*":
                value *= rhs
            else:
                if rhs == 0:
                    raise ExprError("binding_eval_error: division by zero")
                value /= rhs
        return value

    def unary() -> float:
        token = peek()
        if token == ("op", "-"):
            take()
            return -primary()
        return primary()

    def primary() -> float:
        token = take()
        kind, text_ = token
        if kind == "num":
            return float(text_)
        if kind == "field":
            return float(field_lookup(text_))
        if kind == "ident" and text_ in ("min", "max"):
            take("(")
            args = [expr()]
            while peek() == ("op", ","):
                take()
                args.append(expr())
            take(")")
            if len(args) > 8:
                raise ExprError("binding_parse_error: too many arguments")
            return min(args) if text_ == "min" else max(args)
        if kind == "ident":
            if text_ not in params:
                raise ExprError(f"binding_unknown_name: {text_}")
            return params[text_]
        if token == ("op", "("):
            value = expr()
            take(")")
            return value
        raise ExprError(f"binding_parse_error: unexpected {text_!r}")

    result = expr()
    if pos != len(tokens):
        raise ExprError("binding_parse_error: trailing tokens")
    return result


def doc_params(doc: dict) -> dict[str, float]:
    return {p["name"]: float(p["value"]) for p in doc["parameters"]}


# ---- local port geometry (identity orientation; §3.4) ---------------------------------------
_AXIS_INDEX = {"x": 0, "y": 1, "z": 2}


def unit(direction: str) -> tuple[float, float, float]:
    vec = [0.0, 0.0, 0.0]
    vec[_AXIS_INDEX[direction[1]]] = 1.0 if direction[0] == "+" else -1.0
    return vec[0], vec[1], vec[2]


def box_dims(shape: dict) -> tuple[float, float, float]:
    if shape["kind"] == "plate":
        return shape["length"], shape["width"], shape["thickness"]
    if shape["kind"] in ("enclosure", "box"):
        return shape["length"], shape["width"], shape["height"]
    raise NotImplementedError(shape["kind"])


def local_ports(component: dict) -> dict[str, dict]:
    shape = component["shape"]
    if shape_key(component) == ("dot", "point"):
        return {"point": {"type": "point", "position": (0.0, 0.0, 0.0)}}
    if shape_key(component) == ("empty", "hole"):
        origin = {"type": "point", "position": (0.0, 0.0, 0.0)}
        return {"entry": origin, "axis": {"type": "axis", "position": (0.0, 0.0, 0.0)}}
    length, width, height = box_dims(shape)

    def face(origin, normal, u, v, extent):
        return {"type": "face", "origin": origin, "normal": normal, "u": u, "v": v,
                "extent": extent}

    ports = {
        "origin": {"type": "point", "position": (0.0, 0.0, 0.0)},
        "center": {"type": "point", "position": (0.0, 0.0, height / 2)},
        "axis_z": {"type": "axis", "position": (0.0, 0.0, 0.0), "direction": "+z"},
        "top": face((0.0, 0.0, height), "+z", "+x", "+y", (length, width)),
        "bottom": face((0.0, 0.0, 0.0), "-z", "+x", "-y", (length, width)),
        "x_pos": face((length / 2, 0.0, height / 2), "+x", "+y", "+z", (width, height)),
        "x_neg": face((-length / 2, 0.0, height / 2), "-x", "-y", "+z", (width, height)),
        "y_pos": face((0.0, width / 2, height / 2), "+y", "-x", "+z", (length, height)),
        "y_neg": face((0.0, -width / 2, height / 2), "-y", "+x", "+z", (length, height)),
        "region": {"type": "region", "min": (-length / 2, -width / 2, 0.0),
                   "max": (length / 2, width / 2, height)},
    }
    if shape["kind"] == "enclosure":
        wall = shape["wall_thickness"]
        ports["cavity_floor"] = face(
            (0.0, 0.0, wall), "+z", "+x", "+y", (length - 2 * wall, width - 2 * wall)
        )
        ports["cavity"] = {"type": "region", "min": (-length / 2 + wall, -width / 2 + wall, wall),
                           "max": (length / 2 - wall, width / 2 - wall, height)}
    return ports


def anchor(port: dict) -> tuple[float, float, float]:
    return port["origin"] if port["type"] == "face" else port["position"]


# ---- §5.5 fixpoint translation solver (identity orientations only) --------------------------
def solve(doc: dict) -> dict:
    comps = components(doc)
    order = [c["id"] for c in doc["components"]]
    ports = {cid: local_ports(comps[cid]) for cid in order}
    t: dict[str, list[float | None]] = {cid: [None, None, None] for cid in order}
    source: dict[str, list[str | None]] = {cid: [None, None, None] for cid in order}
    issues: list[dict] = []
    defaulted: dict[str, list[str]] = {cid: [] for cid in order}

    def split(ref: str) -> tuple[str, dict]:
        cid, port = PORT_REF_RE.match(ref).groups()
        return cid, ports[cid][port]

    fixed = [k for k in doc["constraints"] if k["kind"] == "fixed"]
    if not fixed:
        return {"issues": [{"code": "no_fixed_root"}]}

    # equations: (constraint id, axis, comp_a, local_a, comp_b, local_b, c)
    #   meaning (t_a + local_a) - (t_b + local_b) = c on that world axis
    equations: list[tuple] = []
    for k in doc["constraints"]:
        if k["kind"] == "fixed":
            frame = k["frame"]
            assert (frame["x_axis"], frame["z_axis"]) == ("+x", "+z"), "test solver: identity only"
            cid, port = split(k["a"])
            assert anchor(port) == (0.0, 0.0, 0.0), "fixed must target the frame port"
            for axis in range(3):
                if t[cid][axis] is None:
                    t[cid][axis] = float(frame["origin"][axis])
                    source[cid][axis] = k["id"]
                else:
                    equations.append((k["id"], axis, cid, 0.0, None, 0.0,
                                      float(frame["origin"][axis])))
            continue
        ca, pa = split(k["a"])
        cb, pb = split(k["b"])
        a_xyz, b_xyz = anchor(pa), anchor(pb)
        if k["kind"] == "coincident":
            for axis in range(3):
                equations.append((k["id"], axis, ca, a_xyz[axis], cb, b_xyz[axis], 0.0))
        elif k["kind"] == "on_face":
            u_vec, v_vec = unit(pb["u"]), unit(pb["v"])
            for axis in range(3):
                offset = k["u"] * u_vec[axis] + k["v"] * v_vec[axis]
                equations.append((k["id"], axis, ca, a_xyz[axis], cb, b_xyz[axis], offset))
        elif k["kind"] == "coaxial":
            assert pa["direction"][1] == pb["direction"][1], "orientation_unsatisfiable"
            along = _AXIS_INDEX[pb["direction"][1]]
            for axis in range(3):
                if axis != along:
                    equations.append((k["id"], axis, ca, a_xyz[axis], cb, b_xyz[axis], 0.0))
        elif k["kind"] in ("mate", "flush", "distance"):
            normal_b = pb["normal"]
            if k["kind"] == "mate":
                assert k["value"] >= 0
                assert pa["normal"][1] == normal_b[1] and pa["normal"][0] != normal_b[0]
            elif k["kind"] == "flush":
                assert pa["normal"] == normal_b, "orientation_unsatisfiable"
            elif pa["type"] == "face":
                assert pa["normal"][1] == normal_b[1], "orientation_unsatisfiable"
            axis = _AXIS_INDEX[normal_b[1]]
            sign = 1.0 if normal_b[0] == "+" else -1.0
            equations.append((k["id"], axis, ca, a_xyz[axis], cb, b_xyz[axis],
                              k["value"] * sign))
        # inside / clearance are check-only: no equations

    used: set[int] = set()

    def known(cid: str | None, axis: int) -> bool:
        return cid is None or t[cid][axis] is not None

    def value(cid: str | None, axis: int, local: float) -> float:
        return local if cid is None else t[cid][axis] + local

    while True:
        changed = True
        while changed:
            changed = False
            for i, (kid, axis, ca, la, cb, lb, c) in enumerate(equations):
                if i in used or ca == cb:
                    continue
                if known(ca, axis) and not known(cb, axis):
                    t[cb][axis] = value(ca, axis, la) - c - lb
                    source[cb][axis] = kid
                elif known(cb, axis) and not known(ca, axis):
                    t[ca][axis] = value(cb, axis, lb) + c - la
                    source[ca][axis] = kid
                else:
                    continue
                used.add(i)
                changed = True
        pending = [cid for cid in order if None in t[cid]]
        if not pending:
            break
        cid = pending[0]
        for axis in range(3):
            if t[cid][axis] is None:
                t[cid][axis] = 0.0
                defaulted[cid].append("xyz"[axis])
        issues.append({"code": "under_constrained", "target": cid, "axes": defaulted[cid]})

    per_constraint: dict[str, list[float]] = {}
    for i, (kid, axis, ca, la, cb, lb, c) in enumerate(equations):
        if i not in used:
            residual = abs(value(ca, axis, la) - value(cb, axis, lb) - c)
            per_constraint.setdefault(kid, []).append(residual)
    for kid, residuals in per_constraint.items():
        code = "redundant_constraint" if max(residuals) <= TOL else "conflicting_constraints"
        issues.append({"code": code, "target": kid})

    return {"t": t, "ports": ports, "issues": issues, "defaulted": defaulted, "source": source}


def world_port(solution: dict, doc: dict, ref: str) -> dict:
    cid, port = PORT_REF_RE.match(ref).groups()
    offset = solution["t"][cid]

    def shift(xyz):
        return [offset[i] + xyz[i] for i in range(3)]

    if port in ("axis", "region") and shape_key(components(doc)[cid]) == ("empty", "hole"):
        # direction and span depend on the owning part (§7), not only on the local table
        hole = hole_geometry(solution, doc, cid)
        if port == "axis":
            return {"type": "axis", "position": shift((0.0, 0.0, 0.0)),
                    "direction": hole["direction"]}
        return {"type": "region", "min": hole["region_min"], "max": hole["region_max"]}
    local = solution["ports"][cid][port]
    result = {"type": local["type"]}
    for key, item in local.items():
        if key in ("position", "origin", "min", "max"):
            result[key] = shift(item)
        elif key == "extent":
            result[key] = list(item)
        elif key != "type":
            result[key] = item
    return result


def owning_part(doc: dict, cid: str) -> dict:
    owners = [p for p in doc["parts"] if any(op["component"] == cid for op in p["body"])]
    assert len(owners) == 1
    return owners[0]


def part_aabb(solution: dict, doc: dict, part: dict) -> tuple[list[float], list[float]]:
    lows, highs = [], []
    for op in part["body"]:
        if op["op"] == "add":
            region = world_port(solution, doc, f"{op['component']}.region")
            lows.append(region["min"])
            highs.append(region["max"])
    return ([min(p[i] for p in lows) for i in range(3)],
            [max(p[i] for p in highs) for i in range(3)])


def hole_geometry(solution: dict, doc: dict, hid: str) -> dict:
    """§7.1 direction and §7.2 cutter span for an Empty hole (identity orientation)."""
    comps = components(doc)
    shape = comps[hid]["shape"]
    entry_ref = f"{hid}.entry"
    hosts = {entry_ref}
    for k in doc["constraints"]:
        if k["kind"] == "coincident" and entry_ref in (k["a"], k["b"]):
            other = k["b"] if k["a"] == entry_ref else k["a"]
            if shape_key(comps[PORT_REF_RE.match(other).group(1)]) == ("dot", "point"):
                hosts.add(other)
    normals = {
        world_port(solution, doc, k["b"])["normal"]
        for k in doc["constraints"]
        if k["kind"] == "on_face" and k["a"] in hosts
    }
    assert len(normals) <= 1, "hole_direction_conflict"
    if normals:
        normal = normals.pop()
        direction = ("-" if normal[0] == "+" else "+") + normal[1]
    else:
        direction = "-" + shape["axis"]
    assert direction[1] == shape["axis"], "hole_axis_mismatch"

    entry = world_port(solution, doc, entry_ref)["position"]
    along = _AXIS_INDEX[direction[1]]
    sign = 1.0 if direction[0] == "+" else -1.0
    low, high = part_aabb(solution, doc, owning_part(doc, hid))
    start, end = list(entry), list(entry)
    if shape["hole_type"] == "blind":
        start[along] = entry[along] - sign * 0.1
        end[along] = entry[along] + sign * (shape["depth"] + 0.1)
        far = entry[along] + sign * shape["depth"]
    else:
        start[along] = (low[along] - 1.0) if sign > 0 else (high[along] + 1.0)
        end[along] = (high[along] + 1.0) if sign > 0 else (low[along] - 1.0)
        far = high[along] if sign > 0 else low[along]
    radius = max(
        d for d in (shape["diameter"], shape["counterbore_diameter"],
                    shape["countersink_diameter"]) if d is not None
    ) / 2
    region_min = [entry[i] - radius for i in range(3)]
    region_max = [entry[i] + radius for i in range(3)]
    region_min[along], region_max[along] = min(entry[along], far), max(entry[along], far)
    return {"direction": direction, "cutter_start": start, "cutter_end": end,
            "diameter": shape["diameter"], "region_min": region_min, "region_max": region_max}


def assert_close(actual, expected, where: str) -> None:
    if isinstance(expected, list | tuple):
        assert len(actual) == len(expected), where
        for i, (a, e) in enumerate(zip(actual, expected, strict=True)):
            assert_close(a, e, f"{where}[{i}]")
    elif isinstance(expected, int | float) and not isinstance(expected, bool):
        assert abs(float(actual) - float(expected)) <= TOL, f"{where}: {actual} != {expected}"
    else:
        assert actual == expected, f"{where}: {actual!r} != {expected!r}"


# ---- tests ----------------------------------------------------------------------------------
@pytest.mark.parametrize("name", EXAMPLES)
def test_example_parses_with_frozen_top_level(name):
    doc = load(name)
    assert doc["schema_version"] == "2.0"
    assert doc["unit"] == "mm"
    assert [k for k in doc if k not in EXAMPLE_ONLY_KEYS] == TOP_LEVEL_KEYS
    assert set(doc) - set(TOP_LEVEL_KEYS) <= EXAMPLE_ONLY_KEYS
    assert "expected_resolved" in doc
    assert isinstance(doc["revision"], int) and doc["revision"] >= 0


@pytest.mark.parametrize("name", EXAMPLES)
def test_ids_are_unique_per_namespace_and_well_formed(name):
    doc = load(name)
    namespaces = {
        "N1 components+constraints": [r["id"] for r in doc["components"] + doc["constraints"]],
        "N2 parts+assemblies": [r["id"] for r in doc["parts"] + doc["assemblies"]],
        "N3 parameters": [p["name"] for p in doc["parameters"]],
    }
    for label, ids in namespaces.items():
        assert len(ids) == len(set(ids)), f"duplicate_id in {label}: {ids}"
        for item in ids:
            assert ID_RE.match(item), f"bad id {item!r} in {label}"
    assert not {"min", "max"} & set(namespaces["N3 parameters"])


@pytest.mark.parametrize("name", EXAMPLES)
def test_components_use_provenance_and_have_no_origin_field(name):
    doc = load(name)
    for comp in doc["components"]:
        assert "origin" not in comp, f"{comp['id']}: 'origin' is a port name, not a field"
        allowed = COMPONENT_KEYS | ({"role"} if comp["kind"] == "empty" else set())
        assert set(comp) == allowed, f"{comp['id']}: keys {sorted(comp)}"
        if comp["kind"] == "empty":
            assert comp["role"] in EMPTY_ROLES
        assert shape_key(comp) in PORT_TABLE, f"{comp['id']}: shape not in frozen tables"
        provenance = comp["provenance"]
        assert set(provenance) == {"agent", "patch_id"}
        assert provenance["agent"] in PROVENANCE_AGENTS
        assert re.match(r"^r\d+$", provenance["patch_id"])
        assert int(provenance["patch_id"][1:]) <= doc["revision"]
        if shape_key(comp) == ("empty", "hole") and comp["shape"]["thread"] is not None:
            assert THREAD_RE.match(comp["shape"]["thread"])


@pytest.mark.parametrize("name", EXAMPLES)
def test_constraint_port_references_resolve_and_are_compatible(name):
    doc = load(name)
    comps = components(doc)
    for k in doc["constraints"]:
        assert set(k) == CONSTRAINT_KEYS, k["id"]
        assert k["kind"] in CONSTRAINT_KINDS
        if k["kind"] == "fixed":
            assert k["b"] is None and k["frame"] is not None
            assert k["value"] is None and k["u"] is None and k["v"] is None
            cid, port = PORT_REF_RE.match(k["a"]).groups()
            port_type(doc, k["a"])
            assert port == FRAME_PORT[shape_key(comps[cid])], "invalid_fixed_target"
            frame = k["frame"]
            assert frame["x_axis"] in AXIS_DIRS and frame["z_axis"] in AXIS_DIRS
            assert frame["x_axis"][1] != frame["z_axis"][1]
            continue
        assert k["frame"] is None
        pair = (port_type(doc, k["a"]), port_type(doc, k["b"]))
        assert k["kind"] in COMPATIBLE.get(pair, set()), f"port_type_mismatch: {k['id']}"
        if k["kind"] != "on_face":
            assert k["u"] is None and k["v"] is None
        if k["kind"] in ("coincident", "on_face", "coaxial", "inside"):
            assert k["value"] is None
        if k["kind"] == "mate":
            assert k["value"] >= 0


@pytest.mark.parametrize("name", EXAMPLES)
def test_parts_assemblies_and_root_resolve(name):
    doc = load(name)
    comps = components(doc)
    owners: dict[str, str] = {}
    for part in doc["parts"]:
        assert set(part) == PART_KEYS
        assert part["body"], f"{part['id']}: empty body"
        assert part["body"][0]["op"] == "add", f"body_first_not_add: {part['id']}"
        for op in part["body"]:
            assert set(op) == {"op", "component"}
            comp = comps[op["component"]]  # dangling_reference -> KeyError
            if op["op"] == "add":
                assert comp["kind"] == "volume"
            elif op["op"] == "cut":
                assert comp["kind"] == "empty" and comp["role"] == "cut"
            else:
                assert op["op"] == "intersect"
                assert comp["kind"] == "volume" or comp.get("role") == "envelope"
            if op["op"] != "intersect":
                assert op["component"] not in owners, f"multi_owner: {op['component']}"
                owners[op["component"]] = part["id"]
    for comp in doc["components"]:
        if comp["kind"] == "volume" or comp.get("role") == "cut":
            assert comp["id"] in owners, f"orphan_component: {comp['id']}"

    n2 = {r["id"] for r in doc["parts"] + doc["assemblies"]}
    children = [c for a in doc["assemblies"] for c in a["children"]]
    assert set(children) <= n2
    assert len(children) == len(set(children)), "an item may have only one parent assembly"
    if doc["assemblies"]:
        tops = [a["id"] for a in doc["assemblies"] if a["id"] not in children]
        assert tops == [doc["root"]], "invalid_root"
    else:
        assert [p["id"] for p in doc["parts"]] == [doc["root"]], "invalid_root"


@pytest.mark.parametrize("name", EXAMPLES)
def test_bindings_evaluate_to_cached_field_values(name):
    doc = load(name)
    params = doc_params(doc)
    targets = [b["target"] for b in doc["bindings"]]
    assert len(targets) == len(set(targets)), "duplicate_binding"
    assert doc["bindings"], "examples are expected to exercise bindings"
    for binding in doc["bindings"]:
        cached = field_value(doc, binding["target"])
        result = evaluate(binding["expr"], params, lambda ref: field_value(doc, ref))
        assert abs(result - cached) <= TOL, f"{binding['target']}: {result} != cached {cached}"
    for param in doc["parameters"]:
        if param["min"] is not None:
            assert param["value"] >= param["min"], "parameter_out_of_range"
        if param["max"] is not None:
            assert param["value"] <= param["max"], "parameter_out_of_range"


@pytest.mark.parametrize("name", EXAMPLES)
def test_expected_resolved_matches_fixpoint_recomputation(name):
    doc = load(name)
    expected = doc["expected_resolved"]
    solution = solve(doc)
    assert solution["issues"] == expected["issues"]

    assert set(expected["components"]) == {c["id"] for c in doc["components"]}
    for cid, frame in expected["components"].items():
        assert_close(solution["t"][cid], frame["origin"], f"{cid}.origin")
        assert (frame["x_axis"], frame["z_axis"]) == ("+x", "+z")
        assert frame["defaulted_axes"] == solution["defaulted"][cid]

    for ref, port in expected["ports"].items():
        assert port["type"] == port_type(doc, ref)
        actual = world_port(solution, doc, ref)
        assert set(actual) == set(port), f"{ref}: keys {sorted(actual)} vs {sorted(port)}"
        for key, item in port.items():
            assert_close(actual[key], item, f"{ref}.{key}")

    for hid, hole in expected["holes"].items():
        actual = hole_geometry(solution, doc, hid)
        for key, item in hole.items():
            assert_close(actual[key], item, f"{hid}.{key}")


def test_example_a_hole_dots_sit_on_plate_top_corners():
    doc = load(PLATE)
    solution = solve(doc)
    dots = sorted(tuple(solution["t"][f"d_h{n}"]) for n in range(1, 5))
    assert dots == sorted((x, y, 10.0) for x in (-50.0, 50.0) for y in (-20.0, 20.0))
    # 1.0 order of _default_positions(four_corners): (-50,-20), (50,-20), (50,20), (-50,20)
    assert [tuple(solution["t"][f"d_h{n}"][:2]) for n in range(1, 5)] == [
        (-50.0, -20.0), (50.0, -20.0), (50.0, 20.0), (-50.0, 20.0)
    ]
    for n in range(1, 5):
        assert solution["t"][f"e_h{n}"] == solution["t"][f"d_h{n}"]
        hole = hole_geometry(solution, doc, f"e_h{n}")
        assert hole["direction"] == "-z"
        assert hole["cutter_start"][2] == 11.0 and hole["cutter_end"][2] == -1.0
    # parity with the 1.0 example: same hole XY and plate size
    legacy = json.loads(
        (ROOT / "examples" / "generated" / "plate-four-holes" / "spec.json").read_text("utf-8")
    )
    legacy_xy = [(h["x"], h["y"]) for h in legacy["holes"]]
    assert legacy_xy == [tuple(solution["t"][f"d_h{n}"][:2]) for n in range(1, 5)]
    plate = components(doc)["v_plate"]["shape"]
    assert {k: plate[k] for k in ("length", "width", "thickness")} == {
        k: legacy["base"][k] for k in ("length", "width", "thickness")
    }


def test_example_b_lid_sits_on_bound_body_height_and_clears_pcb_reserve():
    doc = load(ENCLOSURE)
    params = doc_params(doc)
    height = evaluate("wall + standoff_h + pcb_stack + 1", params, lambda ref: 0.0)
    assert height == 33.0
    assert field_value(doc, "v_body:height") == 33.0
    solution = solve(doc)
    assert solution["t"]["v_lid"] == [0.0, 0.0, 33.0]
    reserve = world_port(solution, doc, "e_pcb.region")
    assert reserve["min"][2] == 7.0 and reserve["max"][2] == 32.0
    lid_bottom = world_port(solution, doc, "v_lid.bottom")["origin"][2]
    assert lid_bottom - reserve["max"][2] == 1.0  # 1 mm clearance; no reserve_violated
    cavity = world_port(solution, doc, "v_body.cavity")
    for axis in range(3):
        assert cavity["min"][axis] <= reserve["min"][axis]
        assert reserve["max"][axis] <= cavity["max"][axis]
    # the superseded assumption was removed explicitly; its replacement is present
    assert "未提供外殼高度，暫用 30 mm" not in doc["assumptions"]
    assert any("33 mm" in a for a in doc["assumptions"])


def test_constraint_cycle_is_not_an_error_but_redundancy_or_conflict():
    doc = load(ENCLOSURE)
    extra = {"id": "k_lid_z", "kind": "flush", "a": "v_lid.top", "b": "v_body.top",
             "value": 2, "u": None, "v": None, "frame": None}
    doc["constraints"].append(extra)
    assert solve(doc)["issues"] == [{"code": "redundant_constraint", "target": "k_lid_z"}]
    extra["value"] = 3
    assert solve(doc)["issues"] == [{"code": "conflicting_constraints", "target": "k_lid_z"}]


def test_unconstrained_component_defaults_to_origin_with_warning():
    doc = load(ENCLOSURE)
    doc["constraints"] = [k for k in doc["constraints"] if k["id"] != "k_pcb_z"]
    solution = solve(doc)
    assert solution["issues"] == [
        {"code": "under_constrained", "target": "e_pcb", "axes": ["z"]}
    ]
    assert solution["t"]["e_pcb"] == [0.0, 0.0, 0.0]


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("margin - len / 2", -50.0),
        ("len / 2 - margin", 50.0),
        ("-margin + 2 * (len - wid)", 110.0),
        ("max(margin, wid / 4, 3) - min(len, 1.5)", 13.5),
        ("k_d1:u * 2", -100.0),
    ],
)
def test_expression_evaluator_accepts_closed_grammar(text, expected):
    doc = load(PLATE)
    assert evaluate(text, doc_params(doc), lambda ref: field_value(doc, ref)) == expected


@pytest.mark.parametrize(
    "text",
    [
        "__import__('os')",
        "len ** 2",
        "len % 7",
        "len.real",
        "1e3",
        ".5",
        "--len",
        "len wid",
        "min",
        "max()",
        "(len",
        "abs(len)",
        "unknown_param + 1",
        "len / 0",
        "x" * 161,
    ],
)
def test_expression_evaluator_rejects_everything_else(text):
    doc = load(PLATE)
    with pytest.raises((ExprError, AssertionError, KeyError)):
        evaluate(text, doc_params(doc), lambda ref: field_value(doc, ref))
