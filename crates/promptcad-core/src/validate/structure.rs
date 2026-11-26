//! Schema extras and the structure & reference layer (§1.2, §1.3, §3, §5.3, §6, §8.6, §9.2
//! "結構與引用").

use std::collections::{BTreeMap, BTreeSet};

use crate::model::geom::{vsub, AxisDir, Vec3, TOL};
use crate::model::issue::codes::Code;
use crate::model::issue::{Severity, ValidationIssue};
use crate::model::resolved::ResolvedPort;
use crate::model::v2::{
    limits, BodyOpKind, CadDocument2, Component, ComponentKind, EmptyRole, LineShape, PlaneShape,
    VolumeShape,
};
use crate::model::{Id, ShapeKind};
use crate::ports::check_constraint_ports;
use crate::solver::WorldView;

use super::SCHEMA_INVALID;

/// Structural issues plus whether they make solving meaningless.
pub(super) struct Structure {
    pub issues: Vec<ValidationIssue>,
    pub blocks_solver: bool,
}

// ---- schema ---------------------------------------------------------------------------------

/// `check_schema()` violations plus the assembly nesting-depth cap (§2: "16，巢狀深度 4"), as
/// `schema_invalid` error issues.
pub(super) fn schema_issues(doc: &CadDocument2) -> Vec<ValidationIssue> {
    let mut out: Vec<ValidationIssue> = doc
        .check_schema()
        .into_iter()
        .map(|v| schema_issue(&v.path, &v.message))
        .collect();
    if let Some(depth) = assembly_depth(doc) {
        if depth > limits::ASSEMBLY_DEPTH {
            out.push(schema_issue(
                "/assemblies",
                &format!(
                    "assembly nesting depth must be <= {} (got {depth})",
                    limits::ASSEMBLY_DEPTH
                ),
            ));
        }
    }
    out
}

fn schema_issue(path: &str, message: &str) -> ValidationIssue {
    let path = if path.is_empty() { "/" } else { path };
    ValidationIssue {
        severity: Severity::Error,
        code: SCHEMA_INVALID.to_owned(),
        message: format!("schema violation at {path}: {message}"),
        feature_index: None,
        target: Some(path.to_owned()),
        related: Vec::new(),
        owner: None,
        layer: Default::default(),
    }
}

/// Longest chain of nested assemblies (1 = flat); `None` when the assembly graph has a cycle.
fn assembly_depth(doc: &CadDocument2) -> Option<usize> {
    let idx = assembly_index(doc);
    let n = idx.len();
    let edges = assembly_edges(doc, &idx);
    if !sccs(n, &edges).is_empty() {
        return None;
    }
    let mut memo = vec![0usize; n];
    fn depth(i: usize, edges: &[Vec<usize>], memo: &mut [usize]) -> usize {
        if memo[i] == 0 {
            let d = 1 + edges[i]
                .iter()
                .map(|&j| depth(j, edges, memo))
                .max()
                .unwrap_or(0);
            memo[i] = d;
        }
        memo[i]
    }
    (0..n)
        .map(|i| depth(i, &edges, &mut memo))
        .max()
        .or(Some(0))
}

/// First index of each assembly id.
fn assembly_index(doc: &CadDocument2) -> Vec<usize> {
    let mut seen = BTreeSet::new();
    (0..doc.assemblies.len())
        .filter(|&i| seen.insert(doc.assemblies[i].id.as_str()))
        .collect()
}

/// Edges between canonical assemblies (positions in `idx`).
fn assembly_edges(doc: &CadDocument2, idx: &[usize]) -> Vec<Vec<usize>> {
    let pos: BTreeMap<&str, usize> = idx
        .iter()
        .enumerate()
        .map(|(p, &i)| (doc.assemblies[i].id.as_str(), p))
        .collect();
    idx.iter()
        .map(|&i| {
            let mut out: Vec<usize> = doc.assemblies[i]
                .children
                .iter()
                .filter_map(|c| pos.get(c.as_str()).copied())
                .collect();
            out.sort_unstable();
            out.dedup();
            out
        })
        .collect()
}

/// Strongly connected components that contain a cycle (size > 1, or a self-loop); members
/// ascending, components ordered by their smallest member.
pub(super) fn sccs(n: usize, edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    struct T<'e> {
        edges: &'e [Vec<usize>],
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        on: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        out: Vec<Vec<usize>>,
    }
    fn visit(t: &mut T<'_>, v: usize) {
        t.index[v] = Some(t.next);
        t.low[v] = t.next;
        t.next += 1;
        t.stack.push(v);
        t.on[v] = true;
        for &w in &t.edges[v] {
            match t.index[w] {
                None => {
                    visit(t, w);
                    t.low[v] = t.low[v].min(t.low[w]);
                }
                Some(iw) if t.on[w] => t.low[v] = t.low[v].min(iw),
                _ => {}
            }
        }
        if Some(t.low[v]) == t.index[v] {
            let mut comp = Vec::new();
            while let Some(w) = t.stack.pop() {
                t.on[w] = false;
                comp.push(w);
                if w == v {
                    break;
                }
            }
            if comp.len() > 1 || t.edges[v].contains(&v) {
                comp.sort_unstable();
                t.out.push(comp);
            }
        }
    }
    let mut t = T {
        edges,
        index: vec![None; n],
        low: vec![0; n],
        on: vec![false; n],
        stack: Vec::new(),
        next: 0,
        out: Vec::new(),
    };
    for v in 0..n {
        if t.index[v].is_none() {
            visit(&mut t, v);
        }
    }
    t.out.sort_by_key(|c| c[0]);
    t.out
}

// ---- root (§6.3) ----------------------------------------------------------------------------

/// §6.3 root rule.
pub(super) fn compute_root(doc: &CadDocument2) -> Option<Id> {
    if doc.assemblies.is_empty() {
        let ids: BTreeSet<&str> = doc.parts.iter().map(|p| p.id.as_str()).collect();
        return match (ids.len(), doc.parts.first()) {
            (1, Some(p)) => Some(p.id.clone()),
            _ => None,
        };
    }
    let children: BTreeSet<&str> = doc
        .assemblies
        .iter()
        .flat_map(|a| a.children.iter().map(Id::as_str))
        .collect();
    let mut tops: Vec<&Id> = Vec::new();
    for a in &doc.assemblies {
        if !children.contains(a.id.as_str()) && !tops.contains(&&a.id) {
            tops.push(&a.id);
        }
    }
    match tops.as_slice() {
        [one] => Some((*one).clone()),
        _ => None,
    }
}

// ---- structure layer ------------------------------------------------------------------------

/// What a promotion source must be.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Expect {
    Dot,
    ClosedLine,
    Plane,
    Volume,
    Segment,
}

impl Expect {
    fn name(self) -> &'static str {
        match self {
            Expect::Dot => "dot",
            Expect::ClosedLine => "closed line",
            Expect::Plane => "plane",
            Expect::Volume => "volume",
            Expect::Segment => "segment line",
        }
    }
}

fn expect(shape: ShapeKind, field: &str) -> Expect {
    match (shape, field) {
        (ShapeKind::PlaneProfile, _) => Expect::ClosedLine,
        (ShapeKind::PlaneSection, _) | (ShapeKind::EmptyOf, _) => Expect::Volume,
        (ShapeKind::VolumeExtrude, _) | (ShapeKind::VolumeRevolve, "profile") => Expect::Plane,
        (ShapeKind::VolumeRevolve, _) => Expect::Segment,
        _ => Expect::Dot,
    }
}

fn kind_name(c: &Component) -> String {
    match c.role() {
        Some(r) => format!("{} empty", role_name(r)),
        None => format!("{} {}", c.shape_kind().shape_name(), c.kind().as_str()),
    }
}

pub(super) fn role_name(r: EmptyRole) -> &'static str {
    match r {
        EmptyRole::Cut => "cut",
        EmptyRole::Reserve => "reserve",
        EmptyRole::KeepOut => "keep_out",
        EmptyRole::Envelope => "envelope",
    }
}

fn is_closed(c: &Component) -> bool {
    match c.as_line() {
        Some(LineShape::Polyline(p)) => p.closed && p.points.len() >= 3,
        Some(LineShape::Circle(_)) => true,
        _ => false,
    }
}

/// Pre-solve structure checks, in this order: `duplicate_id` (N1, then N2); per component
/// shape references (`dangling_reference`, `invalid_promotion`, `open_profile`); shape-graph
/// `reference_cycle`; per constraint port checks (`dangling_reference`, `unknown_port`,
/// `invalid_fixed_target`, `port_type_mismatch`); per part body checks (`body_first_not_add`,
/// `dangling_reference`, `invalid_body_component`); body `multi_owner`; assembly children
/// (`dangling_reference`, multi-parent `multi_owner`, `reference_cycle`); `root`
/// (`dangling_reference`; `invalid_root` when completed); `orphan_component`.
pub(super) fn check(doc: &CadDocument2, completed: bool) -> Structure {
    let mut out: Vec<ValidationIssue> = Vec::new();

    // ---- duplicate_id ----
    let mut seen = BTreeSet::new();
    for id in doc
        .components
        .iter()
        .map(|c| &c.id)
        .chain(doc.constraints.iter().map(|k| &k.id))
    {
        if !seen.insert(id.as_str()) {
            out.push(
                ValidationIssue::new(
                    Code::DuplicateId,
                    format!("{id}: duplicate id in namespace N1 (components and constraints)"),
                )
                .with_target(id.as_str()),
            );
        }
    }
    let mut seen = BTreeSet::new();
    for id in doc
        .parts
        .iter()
        .map(|p| &p.id)
        .chain(doc.assemblies.iter().map(|a| &a.id))
    {
        if !seen.insert(id.as_str()) {
            out.push(
                ValidationIssue::new(
                    Code::DuplicateId,
                    format!("{id}: duplicate id in namespace N2 (parts and assemblies)"),
                )
                .with_target(id.as_str()),
            );
        }
    }

    // ---- shape references ----
    for c in &doc.components {
        let shape = c.shape_kind();
        for (field, rid) in c.geometry.shape_refs() {
            let Some(src) = doc.component(rid.as_str()) else {
                out.push(
                    ValidationIssue::new(
                        Code::DanglingReference,
                        format!(
                            "component {}: shape.{field} references unknown component {rid}",
                            c.id
                        ),
                    )
                    .with_target(c.id.as_str())
                    .with_related([rid.as_str()]),
                );
                continue;
            };
            let want = expect(shape, field);
            let kind_ok = match want {
                Expect::Dot => src.kind() == ComponentKind::Dot,
                Expect::ClosedLine => src.kind() == ComponentKind::Line,
                Expect::Plane => src.kind() == ComponentKind::Plane,
                Expect::Volume => src.kind() == ComponentKind::Volume,
                Expect::Segment => src.shape_kind() == ShapeKind::LineSegment,
            };
            if !kind_ok {
                out.push(
                    ValidationIssue::new(
                        Code::InvalidPromotion,
                        format!(
                            "component {}: shape.{field} must reference a {} but {rid} is a {}",
                            c.id,
                            want.name(),
                            kind_name(src)
                        ),
                    )
                    .with_target(c.id.as_str())
                    .with_related([rid.as_str()]),
                );
            } else if want == Expect::ClosedLine && !is_closed(src) {
                out.push(
                    ValidationIssue::new(
                        Code::OpenProfile,
                        format!(
                            "profile {}: {field} {rid} is not a closed line (closed polyline \
                             with at least 3 points, or circle)",
                            c.id
                        ),
                    )
                    .with_target(c.id.as_str())
                    .with_related([rid.as_str()]),
                );
            }
        }
    }

    // ---- shape-graph cycles ----
    {
        let index: BTreeMap<&str, usize> = {
            let mut m = BTreeMap::new();
            for (i, c) in doc.components.iter().enumerate() {
                m.entry(c.id.as_str()).or_insert(i);
            }
            m
        };
        let edges: Vec<Vec<usize>> = doc
            .components
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if index.get(c.id.as_str()) != Some(&i) {
                    return Vec::new();
                }
                let mut e: Vec<usize> = c
                    .geometry
                    .shape_refs()
                    .into_iter()
                    .filter_map(|(_, id)| index.get(id.as_str()).copied())
                    .collect();
                e.sort_unstable();
                e.dedup();
                e
            })
            .collect();
        for comp in sccs(doc.components.len(), &edges) {
            let ids: Vec<&str> = comp
                .iter()
                .map(|&i| doc.components[i].id.as_str())
                .collect();
            out.push(
                ValidationIssue::new(
                    Code::ReferenceCycle,
                    format!("shape references form a cycle through {}", ids.join(", ")),
                )
                .with_target(ids[0])
                .with_related(ids[1..].iter().copied()),
            );
        }
    }

    // ---- constraint ports ----
    for k in &doc.constraints {
        if let Err(errors) = check_constraint_ports(doc, k) {
            out.extend(errors.iter().map(|e| e.to_issue(k.id.as_str())));
        }
    }

    // ---- bodies ----
    for p in &doc.parts {
        if let Some(first) = p.body.first() {
            if first.op != BodyOpKind::Add {
                out.push(
                    ValidationIssue::new(
                        Code::BodyFirstNotAdd,
                        format!(
                            "part {}: body[0] must be add (got {} {})",
                            p.id,
                            op_name(first.op),
                            first.component
                        ),
                    )
                    .with_target(p.id.as_str())
                    .with_related([first.component.as_str()]),
                );
            }
        }
        for (i, op) in p.body.iter().enumerate() {
            let Some(c) = doc.component(op.component.as_str()) else {
                out.push(
                    ValidationIssue::new(
                        Code::DanglingReference,
                        format!(
                            "part {}: body[{i}] references unknown component {}",
                            p.id, op.component
                        ),
                    )
                    .with_target(p.id.as_str())
                    .with_related([op.component.as_str()]),
                );
                continue;
            };
            let ok = match op.op {
                BodyOpKind::Add => c.kind() == ComponentKind::Volume,
                BodyOpKind::Cut => c.role() == Some(EmptyRole::Cut),
                BodyOpKind::Intersect => {
                    c.kind() == ComponentKind::Volume || c.role() == Some(EmptyRole::Envelope)
                }
            };
            if !ok {
                out.push(
                    ValidationIssue::new(
                        Code::InvalidBodyComponent,
                        format!(
                            "part {}: body[{i}] {} cannot use {} {}",
                            p.id,
                            op_name(op.op),
                            kind_name(c),
                            c.id
                        ),
                    )
                    .with_target(p.id.as_str())
                    .with_related([c.id.as_str()]),
                );
            }
        }
    }

    // ---- multi_owner (bodies) ----
    let owners = body_owners(doc);
    for (i, c) in doc.components.iter().enumerate() {
        if doc.component_index(c.id.as_str()) != Some(i) || !is_owned_kind(c) {
            continue;
        }
        if let Some(parts) = owners.get(c.id.as_str()) {
            if parts.len() > 1 {
                out.push(
                    ValidationIssue::new(
                        Code::MultiOwner,
                        format!(
                            "component {} belongs to the bodies of {} parts: {}",
                            c.id,
                            parts.len(),
                            parts.join(", ")
                        ),
                    )
                    .with_target(c.id.as_str())
                    .with_related(parts.iter().copied()),
                );
            }
        }
    }

    let solver_end = out.len();

    // ---- assemblies ----
    let n2: BTreeSet<&str> = doc
        .parts
        .iter()
        .map(|p| p.id.as_str())
        .chain(doc.assemblies.iter().map(|a| a.id.as_str()))
        .collect();
    let mut parents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for a in &doc.assemblies {
        for ch in &a.children {
            if !n2.contains(ch.as_str()) {
                out.push(
                    ValidationIssue::new(
                        Code::DanglingReference,
                        format!("assembly {}: child {ch} is not a part or assembly", a.id),
                    )
                    .with_target(a.id.as_str())
                    .with_related([ch.as_str()]),
                );
                continue;
            }
            let ps = parents.entry(ch.as_str()).or_default();
            if !ps.contains(&a.id.as_str()) {
                ps.push(a.id.as_str());
            }
        }
    }
    let mut child_order: Vec<&str> = Vec::new();
    for a in &doc.assemblies {
        for ch in &a.children {
            if !child_order.contains(&ch.as_str()) {
                child_order.push(ch.as_str());
            }
        }
    }
    for ch in child_order {
        if let Some(ps) = parents.get(ch) {
            if ps.len() > 1 {
                out.push(
                    ValidationIssue::new(
                        Code::MultiOwner,
                        format!(
                            "{ch} is a child of {} assemblies: {} (the assembly graph must be a \
                             tree)",
                            ps.len(),
                            ps.join(", ")
                        ),
                    )
                    .with_target(ch)
                    .with_related(ps.iter().copied()),
                );
            }
        }
    }
    {
        let idx = assembly_index(doc);
        let edges = assembly_edges(doc, &idx);
        for comp in sccs(idx.len(), &edges) {
            let ids: Vec<&str> = comp
                .iter()
                .map(|&p| doc.assemblies[idx[p]].id.as_str())
                .collect();
            out.push(
                ValidationIssue::new(
                    Code::ReferenceCycle,
                    format!("assembly children form a cycle through {}", ids.join(", ")),
                )
                .with_target(ids[0])
                .with_related(ids[1..].iter().copied()),
            );
        }
    }

    // ---- root ----
    if let Some(r) = &doc.root {
        if !n2.contains(r.as_str()) {
            out.push(
                ValidationIssue::new(
                    Code::DanglingReference,
                    format!("root {r} is not a part or assembly"),
                )
                .with_target(r.as_str()),
            );
        }
    }
    if completed {
        out.extend(root_issues(doc));
    }

    // ---- orphan_component ----
    for (i, c) in doc.components.iter().enumerate() {
        if doc.component_index(c.id.as_str()) != Some(i) || !is_owned_kind(c) {
            continue;
        }
        if !owners.contains_key(c.id.as_str()) {
            out.push(
                ValidationIssue::new(
                    Code::OrphanComponent,
                    format!("{} {} does not belong to any part body", kind_name(c), c.id),
                )
                .with_target(c.id.as_str()),
            );
        }
    }

    // only the checks up to body `multi_owner` feed the solver; assembly / root problems and
    // orphans do not affect positions
    let blocks = out[..solver_end]
        .iter()
        .any(|i| i.severity == Severity::Error);
    Structure {
        issues: out,
        blocks_solver: blocks,
    }
}

fn op_name(op: BodyOpKind) -> &'static str {
    match op {
        BodyOpKind::Add => "add",
        BodyOpKind::Cut => "cut",
        BodyOpKind::Intersect => "intersect",
    }
}

/// Volumes and `cut` Empties (subject to `multi_owner` / `orphan_component`).
fn is_owned_kind(c: &Component) -> bool {
    c.kind() == ComponentKind::Volume || c.role() == Some(EmptyRole::Cut)
}

/// Component id → distinct part ids (document order) whose body references it.
fn body_owners(doc: &CadDocument2) -> BTreeMap<&str, Vec<&str>> {
    let mut m: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for p in &doc.parts {
        for op in &p.body {
            let v = m.entry(op.component.as_str()).or_default();
            if !v.contains(&p.id.as_str()) {
                v.push(p.id.as_str());
            }
        }
    }
    m
}

/// `invalid_root` for completed documents (§6.3, §8.6 `check_completed`).
fn root_issues(doc: &CadDocument2) -> Vec<ValidationIssue> {
    let mut out = Vec::new();
    let computed = compute_root(doc);
    let mut parts_reported = false;
    for v in doc.check_completed() {
        let msg = match v.path.as_str() {
            "/root" => match &computed {
                Some(c) => format!("root is missing; the computed root is {c}"),
                None => "root is missing and cannot be computed".to_owned(),
            },
            "/parts" => {
                parts_reported = true;
                format!("a completed document needs at least 1 part ({})", v.message)
            }
            "/components" => format!(
                "a completed document needs at least 1 component ({})",
                v.message
            ),
            _ => format!("completed-document rule failed at {}", v.path),
        };
        out.push(ValidationIssue::new(Code::InvalidRoot, msg));
    }
    match (&doc.root, &computed) {
        (Some(r), Some(c)) if r != c => out.push(
            ValidationIssue::new(
                Code::InvalidRoot,
                format!("root {r} does not match the computed root {c}"),
            )
            .with_target(r.as_str())
            .with_related([c.as_str()]),
        ),
        (_, None) if !parts_reported => {
            let msg = if doc.assemblies.is_empty() {
                format!(
                    "no root: {} parts and no assembly (need exactly 1 part or 1 top-level \
                     assembly)",
                    doc.parts.len()
                )
            } else {
                let children: BTreeSet<&str> = doc
                    .assemblies
                    .iter()
                    .flat_map(|a| a.children.iter().map(Id::as_str))
                    .collect();
                let tops: Vec<&str> = doc
                    .assemblies
                    .iter()
                    .map(|a| a.id.as_str())
                    .filter(|a| !children.contains(a))
                    .collect();
                format!(
                    "no root: {} top-level assemblies ({}), exactly 1 is required",
                    tops.len(),
                    tops.join(", ")
                )
            };
            let issue = ValidationIssue::new(Code::InvalidRoot, msg);
            out.push(match &doc.root {
                Some(r) => issue.with_target(r.as_str()),
                None => issue,
            });
        }
        _ => {}
    }
    out
}

// ---- post-solve: profile_not_axis_aligned ---------------------------------------------------

/// `profile_not_axis_aligned` on solved positions: every closed polyline used by a `profile`
/// (outer and inners) must lie on one axis-aligned plane and not be degenerate; inners must
/// lie on the outer's plane; every `segment` used as a `revolve` axis or through its `axis`
/// port in a constraint must be axis-aligned. Outlines whose Dots have no solved position are
/// skipped.
pub(super) fn check_profiles(doc: &CadDocument2, view: &WorldView<'_>) -> Vec<ValidationIssue> {
    let mut out = Vec::new();
    let dot = |id: &Id| match view.component_port(id.as_str(), "point") {
        Some(ResolvedPort::Point { position }) => Some(position),
        _ => None,
    };
    // plane of a closed outline: (normal axis, coordinate)
    let plane_of = |line: &Component| -> Option<Result<(usize, f64), ()>> {
        match line.as_line()? {
            LineShape::Circle(c) => Some(Ok((2, dot(&c.center)?[2]))),
            LineShape::Polyline(p) if p.closed && p.points.len() >= 3 => {
                let pts: Vec<Vec3> = p.points.iter().map(dot).collect::<Option<_>>()?;
                Some(planar(&pts).ok_or(()))
            }
            _ => None,
        }
    };
    for (i, c) in doc.components.iter().enumerate() {
        if doc.component_index(c.id.as_str()) != Some(i) {
            continue;
        }
        let Some(PlaneShape::Profile(p)) = c.as_plane() else {
            continue;
        };
        let Some(outer) = doc.component(p.outer.as_str()) else {
            continue;
        };
        let outer_plane = match plane_of(outer) {
            Some(Ok(pl)) => Some(pl),
            Some(Err(())) => {
                out.push(profile_issue(
                    c,
                    &p.outer,
                    "does not lie on one axis-aligned plane",
                ));
                None
            }
            None => None,
        };
        for inner_id in &p.inners {
            let Some(inner) = doc.component(inner_id.as_str()) else {
                continue;
            };
            match (plane_of(inner), outer_plane) {
                (Some(Err(())), _) => out.push(profile_issue(
                    c,
                    inner_id,
                    "does not lie on one axis-aligned plane",
                )),
                (Some(Ok((k, v))), Some((ko, vo))) if k != ko || (v - vo).abs() > TOL => {
                    out.push(profile_issue(
                        c,
                        inner_id,
                        "does not lie on the plane of the outer outline",
                    ))
                }
                _ => {}
            }
        }
    }
    // segments whose axis is used
    let mut used: Vec<(&Id, &Id)> = Vec::new(); // (segment, user)
    for c in &doc.components {
        if let Some(VolumeShape::Revolve(r)) = c.as_volume() {
            used.push((&r.axis, &c.id));
        }
    }
    let ids: Vec<(Id, Id)> = doc
        .constraints
        .iter()
        .flat_map(|k| {
            k.ports()
                .filter(|p| p.port() == "axis")
                .map(|p| (p.id(), k.id.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    for (seg, user) in &ids {
        used.push((seg, user));
    }
    let mut reported = BTreeSet::new();
    for (seg, user) in used {
        let Some(s) = doc.component(seg.as_str()) else {
            continue;
        };
        let Some(LineShape::Segment(sg)) = s.as_line() else {
            continue;
        };
        let (Some(a), Some(b)) = (dot(&sg.start), dot(&sg.end)) else {
            continue;
        };
        let d = vsub(b, a);
        let len = d.iter().map(|x| x * x).sum::<f64>().sqrt();
        let aligned = len > TOL && AxisDir::from_vec(d.map(|x| x / len)).is_some();
        if !aligned && reported.insert(seg.as_str()) {
            out.push(
                ValidationIssue::new(
                    Code::ProfileNotAxisAligned,
                    format!(
                        "segment {seg} (used by {user}) is not axis-aligned or has zero length"
                    ),
                )
                .with_target(seg.as_str())
                .with_related([user.as_str()]),
            );
        }
    }
    out
}

fn profile_issue(profile: &Component, line: &Id, what: &str) -> ValidationIssue {
    ValidationIssue::new(
        Code::ProfileNotAxisAligned,
        format!("profile {}: outline {line} {what}", profile.id),
    )
    .with_target(profile.id.as_str())
    .with_related([line.as_str()])
}

/// `(axis, coordinate)` of the axis-aligned plane through all points, if the outline is planar
/// on such a plane and not degenerate (non-zero area).
fn planar(pts: &[Vec3]) -> Option<(usize, f64)> {
    let p0 = *pts.first()?;
    let k = (0..3).find(|&k| pts.iter().all(|p| (p[k] - p0[k]).abs() <= TOL))?;
    // Newell normal component along k must be non-zero (non-degenerate outline)
    let [a, b] = super::solid::radial_axes(k);
    let area2: f64 = pts
        .iter()
        .zip(pts.iter().cycle().skip(1))
        .map(|(p, q)| p[a] * q[b] - q[a] * p[b])
        .sum();
    (area2.abs() > TOL).then_some((k, p0[k]))
}
