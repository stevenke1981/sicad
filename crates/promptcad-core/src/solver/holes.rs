//! §7.1 hole cut direction.

use crate::model::geom::AxisDir;
use crate::model::resolved::ResolvedPort;
use crate::model::v2::ConstraintKind;
use crate::model::ShapeKind;

use super::world::World;

/// Result of §7.1 for one hole.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HoleDirection {
    /// Inward cut direction.
    pub dir: AxisDir,
    /// `(constraint index, world face normal)` of every candidate face, document order.
    pub candidates: Vec<(usize, AxisDir)>,
    /// Candidate normals are not all equal (`hole_direction_conflict`).
    pub conflict: bool,
    /// `dir` is not parallel to the hole's world `shape.axis` (`hole_axis_mismatch`).
    pub mismatch: bool,
}

/// §7.1 steps 1–4 for hole `ci`, using the world's current orientations/translations.
pub(crate) fn hole_direction(world: &World<'_>, ci: usize) -> Option<HoleDirection> {
    let comp = world.comp(ci);
    comp.as_hole()?;
    let axis_world = world.hole_axis_world(ci)?;
    let entry = format!("{}.entry", comp.id);

    // 1. hosts: the entry itself plus Dots coincident with it (one level, either end)
    let mut hosts = vec![entry.clone()];
    for k in &world.doc.constraints {
        if k.kind != ConstraintKind::Coincident {
            continue;
        }
        let Some(b) = k.b.as_ref() else { continue };
        let other = if k.a.as_str() == entry {
            b
        } else if b.as_str() == entry {
            &k.a
        } else {
            continue;
        };
        let is_dot_point = other.port() == "point"
            && world
                .lookup(other.id_str())
                .is_some_and(|d| world.comp(d).shape_kind() == ShapeKind::DotPoint);
        if is_dot_point && !hosts.iter().any(|h| h == other.as_str()) {
            hosts.push(other.as_str().to_owned());
        }
    }
    // candidate faces: `on_face` constraints whose `a` is a host
    let mut candidates = Vec::new();
    for (ki, k) in world.doc.constraints.iter().enumerate() {
        if k.kind != ConstraintKind::OnFace || !hosts.iter().any(|h| h == k.a.as_str()) {
            continue;
        }
        let Some(b) = k.b.as_ref() else { continue };
        if let Some(ResolvedPort::Face { normal, .. }) = world.port_ref(b) {
            candidates.push((ki, normal));
        }
    }
    // 2./3. direction
    let (dir, conflict) = match candidates.first() {
        Some(&(_, n)) => (n.neg(), candidates.iter().any(|(_, m)| *m != n)),
        None => (axis_world.neg(), false),
    };
    // 4. parallel to the world shape.axis
    let mismatch = !dir.is_parallel(axis_world);
    Some(HoleDirection {
        dir,
        candidates,
        conflict,
        mismatch,
    })
}
