//! 1.0 <-> 2.0 migration (spec §10).
//!
//! * [`upgrade_1_to_2`] (§10.1) is a **total** function on schema-valid 1.0 documents; the
//!   result always passes [`CadDocument2::check_schema`] and [`CadDocument2::check_completed`].
//! * [`normalize_v1`] produces the "normalized 1.0 document" of §10.3 (the domain on which
//!   `downgrade_2_to_1(upgrade_1_to_2(d)) == d` must hold).
//! * [`downgrade_2_to_1`] (§10.2) is the partial inverse; [`downgrade_2_to_1_with`] reuses an
//!   existing solver result.
//!
//! # Deterministic document order
//!
//! `components`: `v_base`, then for each hole `i` (`n = i + 1`) `d_h{n}`, `e_h{n}`.
//! `constraints`: `k_base`, then for each hole `k_dh{n}`, `k_eh{n}`.
//! `parts[0].body`: `add v_base`, then `cut e_h1`, `cut e_h2`, ... in 1.0 hole order.
//!
//! # `thread` overflow policy
//!
//! Invalid threads (after normalization) become `null` and their original text is merged into
//! one note line `1.0 thread 原值：e_h1=<orig>；e_h3=<orig>`. The 2.0 model only caps the
//! number of notes (32), not the length of an entry, so when `notes` already holds 32 entries
//! the line is appended to the **last** entry, separated by `；`. The function therefore never
//! fails and never drops information.

use crate::model::geom::{Axis, AxisDir, Frame, Orientation};
use crate::model::issue::Severity;
use crate::model::resolved::{Resolved, ResolvedPort};
use crate::model::v1::{BaseFeature, CadDocument1, HoleFeature, HoleTag, SchemaVersion1};
use crate::model::v2::{
    is_valid_thread, limits, BodyOp, BodyOpKind, CadDocument2, Component, Constraint,
    ConstraintKind, DotShape, EmptyRole, EmptyShape, Geometry, HoleSpec, Part, PartSource,
    Provenance, ProvenanceAgent, SchemaVersion2, VolumeShape,
};
use crate::model::{Id, PortRef};
use crate::solver::{solve, WorldView};

/// Prefix of the merged note line recording discarded 1.0 `thread` values.
pub const THREAD_NOTE_PREFIX: &str = "1.0 thread 原值：";

/// `(max_x, max_y, max_z)` of the base solid, exactly 1.0 `compiler._extents()`.
///
/// 1.0 hole cutters start at the positive outer surface of the axis they drill along; these
/// are the coordinates of those surfaces.
pub fn extents_v1(base: &BaseFeature) -> [f64; 3] {
    match base {
        BaseFeature::Plate(b) => [b.length / 2.0, b.width / 2.0, b.thickness],
        BaseFeature::Cylinder(b) => [b.diameter / 2.0, b.diameter / 2.0, b.height],
        BaseFeature::Ring(b) => [b.outer_diameter / 2.0, b.outer_diameter / 2.0, b.height],
        BaseFeature::LBracket(b) => [
            b.width / 2.0,
            b.depth / 2.0,
            b.vertical_height + b.thickness,
        ],
        BaseFeature::Enclosure(b) => [b.length / 2.0, b.width / 2.0, b.height],
    }
}

fn id(s: &str) -> Id {
    Id::new(s).expect("migration ids are statically valid")
}

fn port(c: &str, p: &str) -> PortRef {
    id(c).port(&id(p))
}

/// `strip().upper().replace("X", "x")`.
fn normalize_thread_text(t: &str) -> String {
    t.trim().to_uppercase().replace('X', "x")
}

/// Normalizes every hole thread; returns the new threads and the merged note line (if any).
fn normalized_threads(holes: &[HoleFeature]) -> (Vec<Option<String>>, Option<String>) {
    let mut dropped: Vec<String> = Vec::new();
    let threads = holes
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let orig = h.thread.as_ref()?;
            let norm = normalize_thread_text(orig);
            if is_valid_thread(&norm) {
                Some(norm)
            } else {
                dropped.push(format!("e_h{}={}", i + 1, orig));
                None
            }
        })
        .collect();
    let line = (!dropped.is_empty()).then(|| format!("{THREAD_NOTE_PREFIX}{}", dropped.join("；")));
    (threads, line)
}

fn merged_notes(notes: &[String], line: Option<String>) -> Vec<String> {
    let mut out = notes.to_vec();
    if let Some(line) = line {
        if out.len() >= limits::NOTES {
            if let Some(last) = out.last_mut() {
                last.push('；');
                last.push_str(&line);
            }
        } else {
            out.push(line);
        }
    }
    out
}

/// The normalized 1.0 document of §10.3: each hole's along-axis coordinate is 0, `thread` is
/// normalized (invalid values become `null`, originals merged into one `notes` line as in
/// §10.1). Idempotent, and `upgrade_1_to_2(d) == upgrade_1_to_2(&normalize_v1(d))`.
pub fn normalize_v1(doc: &CadDocument1) -> CadDocument1 {
    let (threads, line) = normalized_threads(&doc.holes);
    let mut out = doc.clone();
    for (h, t) in out.holes.iter_mut().zip(threads) {
        h.thread = t;
        match h.axis {
            Axis::X => h.x = 0.0,
            Axis::Y => h.y = 0.0,
            Axis::Z => h.z = 0.0,
        }
    }
    out.notes = merged_notes(&doc.notes, line);
    out
}

fn migration_provenance() -> Option<Provenance> {
    Some(Provenance {
        agent: ProvenanceAgent::Migration,
        patch_id: None,
    })
}

fn component(cid: &str, geometry: Geometry) -> Component {
    Component {
        id: id(cid),
        geometry,
        label: None,
        provenance: migration_provenance(),
    }
}

fn fixed(kid: &str, a: PortRef, frame: Frame) -> Constraint {
    Constraint {
        id: id(kid),
        kind: ConstraintKind::Fixed,
        a,
        b: None,
        value: None,
        u: None,
        v: None,
        frame: Some(frame),
    }
}

/// §10.1: upgrades a 1.0 document to a completed 2.0 document (see the module docs for the
/// deterministic record order).
pub fn upgrade_1_to_2(doc: &CadDocument1) -> CadDocument2 {
    let ext = extents_v1(&doc.base);
    let (threads, line) = normalized_threads(&doc.holes);
    let n_holes = doc.holes.len();

    let volume = match &doc.base {
        BaseFeature::Plate(s) => VolumeShape::Plate(s.clone()),
        BaseFeature::Cylinder(s) => VolumeShape::Cylinder(s.clone()),
        BaseFeature::Ring(s) => VolumeShape::Ring(s.clone()),
        BaseFeature::LBracket(s) => VolumeShape::LBracket(s.clone()),
        BaseFeature::Enclosure(s) => VolumeShape::Enclosure(s.clone()),
    };

    let mut components = Vec::with_capacity(1 + 2 * n_holes);
    let mut constraints = Vec::with_capacity(1 + 2 * n_holes);
    let mut body = Vec::with_capacity(1 + n_holes);

    components.push(component("v_base", Geometry::Volume { shape: volume }));
    constraints.push(fixed(
        "k_base",
        port("v_base", "origin"),
        Frame::at([0.0, 0.0, 0.0]),
    ));
    body.push(BodyOp {
        op: BodyOpKind::Add,
        component: id("v_base"),
    });

    for (i, (h, thread)) in doc.holes.iter().zip(threads).enumerate() {
        let n = i + 1;
        let (d, e) = (format!("d_h{n}"), format!("e_h{n}"));
        let origin = match h.axis {
            Axis::Z => [h.x, h.y, ext[2]],
            Axis::X => [ext[0], h.y, h.z],
            Axis::Y => [h.x, ext[1], h.z],
        };
        components.push(component(
            &d,
            Geometry::Dot {
                shape: DotShape::Point {},
            },
        ));
        components.push(component(
            &e,
            Geometry::Empty {
                role: EmptyRole::Cut,
                shape: EmptyShape::Hole(HoleSpec {
                    axis: h.axis,
                    diameter: h.diameter,
                    hole_type: h.hole_type,
                    depth: h.depth,
                    thread,
                    counterbore_diameter: h.counterbore_diameter,
                    counterbore_depth: h.counterbore_depth,
                    countersink_diameter: h.countersink_diameter,
                    countersink_angle: h.countersink_angle,
                }),
            },
        ));
        constraints.push(fixed(
            &format!("k_dh{n}"),
            port(&d, "point"),
            Frame::at(origin),
        ));
        constraints.push(Constraint {
            id: id(&format!("k_eh{n}")),
            kind: ConstraintKind::Coincident,
            a: port(&e, "entry"),
            b: Some(port(&d, "point")),
            value: None,
            u: None,
            v: None,
            frame: None,
        });
        body.push(BodyOp {
            op: BodyOpKind::Cut,
            component: id(&e),
        });
    }

    CadDocument2 {
        schema_version: SchemaVersion2::V2_0,
        name: doc.name.clone(),
        source_prompt: doc.source_prompt.clone(),
        unit: doc.unit,
        revision: 0,
        parameters: Vec::new(),
        bindings: Vec::new(),
        components,
        constraints,
        parts: vec![Part {
            id: id("p_main"),
            name: doc.name.clone(),
            material: doc.material,
            quantity: 1,
            source: PartSource::Made,
            body,
            fillets: doc.fillets.clone(),
            chamfers: doc.chamfers.clone(),
        }],
        assemblies: Vec::new(),
        root: Some(id("p_main")),
        assumptions: doc.assumptions.clone(),
        notes: merged_notes(&doc.notes, line),
        planner: doc.planner.clone(),
    }
}

/// Tolerance of the identity-pose / origin checks of [`downgrade_2_to_1`].
const DOWNGRADE_TOL: f64 = 1e-6;

/// §10.2: downgrades a 2.0 document to 1.0 when it is expressible there (partial function).
///
/// Solves `doc` with [`solve`] and delegates to [`downgrade_2_to_1_with`].
pub fn downgrade_2_to_1(doc: &CadDocument2) -> Option<CadDocument1> {
    downgrade_2_to_1_with(doc, &solve(doc))
}

/// §10.2 with an already solved document (`resolved` must be `solve(doc)`).
///
/// Returns `Some` only when **all** of the following hold, otherwise `None`:
///
/// * exactly one part;
/// * its `body` is one `add` of a shortcut Volume (plate / cylinder / ring / l_bracket /
///   enclosure; not extrude / revolve) followed by zero or more `cut`s of `hole` Empties;
/// * the solver reported no `error` issues;
/// * the Volume and every cut hole have identity orientation, and the Volume origin is the
///   world origin (tolerance 1e-6);
/// * every hole's resolved cut direction (§7.1) equals `-shape.axis`.
///
/// Lossy (but still `Some`) cases, decided as follows:
///
/// * `parameters` and `bindings` are dropped; the cached component values are used.
/// * Components and constraints other than the Volume and the cut holes of the body (helper
///   Dots such as hole anchors, reserve Empties, Lines, Planes, un-cut holes, ...) and all
///   constraints are dropped; they only matter through the solved hole positions.
/// * `label`s and `provenance` are dropped.
/// * Hole coordinates are the solved `entry` world position with the along-axis coordinate
///   projected to 0; `holes` follow the order of the `cut`s in `body`; hole fields are copied.
/// * `fillets`, `chamfers`, `material` come from the part; the part's `name`, `quantity`,
///   `source`, and `revision`, `root`, `assemblies` are dropped (assemblies are ignored: with a
///   single part they carry no 1.0-visible meaning).
/// * `name`, `source_prompt`, `unit`, `assumptions`, `notes`, `planner` are carried over.
pub fn downgrade_2_to_1_with(doc: &CadDocument2, resolved: &Resolved) -> Option<CadDocument1> {
    let [part] = doc.parts.as_slice() else {
        return None;
    };
    if resolved
        .issues
        .iter()
        .any(|i| i.severity == Severity::Error)
    {
        return None;
    }
    let (first, cuts) = part.body.split_first()?;
    if first.op != BodyOpKind::Add || cuts.iter().any(|o| o.op != BodyOpKind::Cut) {
        return None;
    }
    let base_comp = doc.component(first.component.as_str())?;
    let base = match base_comp.as_volume()? {
        VolumeShape::Plate(s) => BaseFeature::Plate(s.clone()),
        VolumeShape::Cylinder(s) => BaseFeature::Cylinder(s.clone()),
        VolumeShape::Ring(s) => BaseFeature::Ring(s.clone()),
        VolumeShape::LBracket(s) => BaseFeature::LBracket(s.clone()),
        VolumeShape::Enclosure(s) => BaseFeature::Enclosure(s.clone()),
        VolumeShape::Extrude(_) | VolumeShape::Revolve(_) => return None,
    };

    let view = WorldView::new(doc, resolved);
    let base_pose = view.pose(base_comp.id.as_str())?;
    if base_pose.orientation != Orientation::IDENTITY
        || base_pose.origin.iter().any(|c| c.abs() > DOWNGRADE_TOL)
    {
        return None;
    }

    let mut holes = Vec::with_capacity(cuts.len());
    for op in cuts {
        let c = doc.component(op.component.as_str())?;
        let spec = c.as_hole()?;
        let hid = c.id.as_str();
        if view.pose(hid)?.orientation != Orientation::IDENTITY
            || view.hole_direction(hid)? != AxisDir::new(spec.axis, false)
        {
            return None;
        }
        let mut xyz = match view.component_port(hid, "entry")? {
            ResolvedPort::Point { position } | ResolvedPort::Axis { position, .. } => position,
            _ => return None,
        };
        xyz[spec.axis.index()] = 0.0;
        holes.push(HoleFeature {
            kind: HoleTag::default(),
            x: xyz[0],
            y: xyz[1],
            z: xyz[2],
            axis: spec.axis,
            diameter: spec.diameter,
            hole_type: spec.hole_type,
            depth: spec.depth,
            thread: spec.thread.clone(),
            counterbore_diameter: spec.counterbore_diameter,
            counterbore_depth: spec.counterbore_depth,
            countersink_diameter: spec.countersink_diameter,
            countersink_angle: spec.countersink_angle,
        });
    }

    Some(CadDocument1 {
        schema_version: SchemaVersion1::V1_0,
        name: doc.name.clone(),
        source_prompt: doc.source_prompt.clone(),
        unit: doc.unit,
        material: part.material,
        base,
        holes,
        fillets: part.fillets.clone(),
        chamfers: part.chamfers.clone(),
        assumptions: doc.assumptions.clone(),
        notes: doc.notes.clone(),
        planner: doc.planner.clone(),
    })
}
