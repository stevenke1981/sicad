//! Analytic solid model used by the design rules: axis-aligned boxes and cylinders, and an
//! exact-where-possible overlap query.
//!
//! Every world solid the validator reasons about is a union of [`Piece`]s; a piece is one
//! positive primitive minus some negative primitives (a `ring` is a cylinder minus its bore,
//! an `enclosure` is five boxes). Each primitive carries a [`Fit`]: `Exact` or `Over` (a
//! superset of the true shape, e.g. the bounding box of an `extrude` of a polyline profile, of
//! a `revolve`, or the cylinder bounding a countersink cone). No primitive is ever built as a
//! subset of its shape.
//!
//! [`overlap`] decides whether `∩ pos − ∪ negs` has positive volume by adaptive subdivision:
//! cells are split exactly on box faces and cylinder end planes, and at the midpoint for
//! curved boundaries. The answer is three-valued; `Unknown` means the subdivision budget ran
//! out (e.g. two coincident cylinder walls) or an approximation makes the answer uncertain:
//! a `Yes` is only certain when no positive is `Over`, a `No` only when no (relevant) negative
//! is `Over`. Callers emit issues only on certain answers, so approximations never create
//! false issues; they can only hide some.

use crate::model::geom::TOL;
use crate::solver::Aabb;

/// How a primitive relates to the true shape it stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fit {
    /// Exactly the shape.
    Exact,
    /// A superset of the shape.
    Over,
}

/// An axis-aligned primitive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Prim {
    /// Box.
    Box(Aabb),
    /// Cylinder along world axis `axis`; `c` are the coordinates of the axis line on the two
    /// other world axes (ascending index order), `[lo, hi]` the range along `axis`.
    Cyl {
        axis: usize,
        c: [f64; 2],
        r: f64,
        lo: f64,
        hi: f64,
    },
}

/// The two axes perpendicular to `axis`, ascending.
pub(super) fn radial_axes(axis: usize) -> [usize; 2] {
    match axis {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Inside,
    Outside,
    Partial,
}

impl Prim {
    /// Bounding box.
    pub fn aabb(&self) -> Aabb {
        match *self {
            Prim::Box(b) => b,
            Prim::Cyl { axis, c, r, lo, hi } => {
                let [p, q] = radial_axes(axis);
                let mut min = [0.0; 3];
                let mut max = [0.0; 3];
                min[axis] = lo;
                max[axis] = hi;
                min[p] = c[0] - r;
                max[p] = c[0] + r;
                min[q] = c[1] - r;
                max[q] = c[1] + r;
                Aabb { min, max }
            }
        }
    }

    fn classify(&self, cell: &Aabb) -> Class {
        let range = |lo: f64, hi: f64, k: usize| -> Class {
            if cell.min[k] >= hi - TOL || cell.max[k] <= lo + TOL {
                Class::Outside
            } else if cell.min[k] >= lo && cell.max[k] <= hi {
                Class::Inside
            } else {
                Class::Partial
            }
        };
        let mut result = Class::Inside;
        let mut merge = |c: Class| match c {
            Class::Outside => result = Class::Outside,
            Class::Partial if result == Class::Inside => result = Class::Partial,
            _ => {}
        };
        match *self {
            Prim::Box(b) => {
                for k in 0..3 {
                    merge(range(b.min[k], b.max[k], k));
                }
            }
            Prim::Cyl { axis, c, r, lo, hi } => {
                merge(range(lo, hi, axis));
                let [p, q] = radial_axes(axis);
                // nearest / farthest distance from the axis to the cell's projected rectangle
                let near = |v: f64, a: f64, b: f64| v.clamp(a, b) - v;
                let far = |v: f64, a: f64, b: f64| (v - a).abs().max((b - v).abs());
                let dn = near(c[0], cell.min[p], cell.max[p]).hypot(near(
                    c[1],
                    cell.min[q],
                    cell.max[q],
                ));
                let df =
                    far(c[0], cell.min[p], cell.max[p]).hypot(far(c[1], cell.min[q], cell.max[q]));
                if dn >= r - TOL {
                    merge(Class::Outside);
                } else if df > r {
                    merge(Class::Partial);
                }
            }
        }
        result
    }

    /// A coordinate where this primitive's boundary cuts `cell` on some axis, preferring exact
    /// planes; `(axis, coordinate)`.
    fn split_plane(&self, cell: &Aabb) -> Option<(usize, f64)> {
        let inside = |k: usize, v: f64| v > cell.min[k] + TOL && v < cell.max[k] - TOL;
        match *self {
            Prim::Box(b) => (0..3).find_map(|k| {
                [b.min[k], b.max[k]]
                    .into_iter()
                    .find(|v| inside(k, *v))
                    .map(|v| (k, v))
            }),
            Prim::Cyl {
                axis, c, lo, hi, ..
            } => {
                if let Some(v) = [lo, hi].into_iter().find(|v| inside(axis, *v)) {
                    return Some((axis, v));
                }
                let [p, q] = radial_axes(axis);
                let k = if cell.max[p] - cell.min[p] >= cell.max[q] - cell.min[q] {
                    p
                } else {
                    q
                };
                let ck = if k == p { c[0] } else { c[1] };
                let mid = (cell.min[k] + cell.max[k]) / 2.0;
                // split through the axis when it lies well inside the cell, else at the middle
                let width = cell.max[k] - cell.min[k];
                if (ck - mid).abs() < width / 4.0 && inside(k, ck) {
                    Some((k, ck))
                } else {
                    Some((k, mid))
                }
            }
        }
    }
}

/// A primitive with its fit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Shape {
    pub prim: Prim,
    pub fit: Fit,
}

impl Shape {
    pub fn exact(prim: Prim) -> Self {
        Shape {
            prim,
            fit: Fit::Exact,
        }
    }

    pub fn over(prim: Prim) -> Self {
        Shape {
            prim,
            fit: Fit::Over,
        }
    }
}

/// One positive primitive minus negatives.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Piece {
    pub pos: Shape,
    pub negs: Vec<Shape>,
}

impl Piece {
    pub fn solid(pos: Shape) -> Self {
        Piece {
            pos,
            negs: Vec::new(),
        }
    }

    /// This piece as a single subtracted shape: exact only without own negatives.
    pub fn as_neg(&self) -> Shape {
        if self.negs.is_empty() {
            self.pos
        } else {
            Shape::over(self.pos.prim)
        }
    }

    /// This piece as a single clipping shape (material ⊂ clip): same rule as [`Piece::as_neg`].
    pub fn as_clip(&self) -> Shape {
        self.as_neg()
    }
}

/// Three-valued answer of [`overlap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Overlap {
    /// Certainly a positive-volume overlap.
    Yes,
    /// Certainly none.
    No,
    /// Undecided (budget or approximation).
    Unknown,
}

const BUDGET: usize = 20_000;
const MIN_CELL: f64 = 1e-4;

/// Whether `∩ pos − ∪ negs` has positive volume (see the module docs).
pub(super) fn overlap(pos: &[Shape], negs: &[Shape]) -> Overlap {
    if pos.is_empty() {
        return Overlap::No;
    }
    let mut start = pos[0].prim.aabb();
    for s in &pos[1..] {
        let b = s.prim.aabb();
        start = Aabb {
            min: std::array::from_fn(|i| start.min[i].max(b.min[i])),
            max: std::array::from_fn(|i| start.max[i].min(b.max[i])),
        };
    }
    if (0..3).any(|i| start.max[i] - start.min[i] <= TOL) {
        return Overlap::No;
    }
    let negs: Vec<&Shape> = negs
        .iter()
        .filter(|n| n.prim.aabb().intersects(&start, TOL))
        .collect();
    let yes_certain = pos.iter().all(|s| s.fit == Fit::Exact);
    let no_certain = negs.iter().all(|s| s.fit == Fit::Exact);

    let mut stack: Vec<(Aabb, Vec<usize>, Vec<usize>)> =
        vec![(start, (0..pos.len()).collect(), (0..negs.len()).collect())];
    let mut uncertain = false;
    let mut found = false;
    let mut steps = 0;
    while let Some((cell, pp, nn)) = stack.pop() {
        steps += 1;
        if steps > BUDGET {
            uncertain = true;
            break;
        }
        let mut p_left = Vec::with_capacity(pp.len());
        let mut outside = false;
        for i in pp {
            match pos[i].prim.classify(&cell) {
                Class::Outside => {
                    outside = true;
                    break;
                }
                Class::Partial => p_left.push(i),
                Class::Inside => {}
            }
        }
        if outside {
            continue;
        }
        let mut n_left = Vec::with_capacity(nn.len());
        let mut removed = false;
        for j in nn {
            match negs[j].prim.classify(&cell) {
                Class::Inside => {
                    removed = true;
                    break;
                }
                Class::Partial => n_left.push(j),
                Class::Outside => {}
            }
        }
        if removed {
            continue;
        }
        if p_left.is_empty() && n_left.is_empty() {
            found = true;
            break;
        }
        let size = (0..3)
            .map(|i| cell.max[i] - cell.min[i])
            .fold(0.0, f64::max);
        if size < MIN_CELL {
            uncertain = true;
            continue;
        }
        let plane = p_left
            .iter()
            .map(|&i| &pos[i].prim)
            .chain(n_left.iter().map(|&j| &negs[j].prim))
            .filter(|p| matches!(p, Prim::Box(_)))
            .chain(
                p_left
                    .iter()
                    .map(|&i| &pos[i].prim)
                    .chain(n_left.iter().map(|&j| &negs[j].prim))
                    .filter(|p| matches!(p, Prim::Cyl { .. })),
            )
            .find_map(|p| p.split_plane(&cell));
        let (k, v) = plane.unwrap_or_else(|| {
            let k = (0..3)
                .max_by(|a, b| {
                    (cell.max[*a] - cell.min[*a]).total_cmp(&(cell.max[*b] - cell.min[*b]))
                })
                .unwrap_or(0);
            (k, (cell.min[k] + cell.max[k]) / 2.0)
        });
        let mut lo = cell;
        let mut hi = cell;
        lo.max[k] = v;
        hi.min[k] = v;
        stack.push((lo, p_left.clone(), n_left.clone()));
        stack.push((hi, p_left, n_left));
    }
    if found {
        if yes_certain {
            Overlap::Yes
        } else {
            Overlap::Unknown
        }
    } else if uncertain || !no_certain {
        Overlap::Unknown
    } else {
        Overlap::No
    }
}

/// Whether the rectangle `[a0, a1] × [b0, b1]` on world axes `(p, q)` (ascending) lies inside
/// the projection of `prim` along axis `k` (`{p, q, k}` = all axes).
pub(super) fn projection_covers(prim: &Prim, k: usize, rect: [[f64; 2]; 2]) -> bool {
    let [p, q] = radial_axes(k);
    let within = |lo: f64, hi: f64, r: [f64; 2]| r[0] >= lo - TOL && r[1] <= hi + TOL;
    match *prim {
        Prim::Box(b) => within(b.min[p], b.max[p], rect[0]) && within(b.min[q], b.max[q], rect[1]),
        // along its own axis a cylinder projects to its disc: every corner inside the circle
        Prim::Cyl { axis, c, r, .. } if axis == k => rect[0].iter().all(|x| {
            rect[1]
                .iter()
                .all(|y| (x - c[0]).hypot(y - c[1]) <= r + TOL)
        }),
        // across its axis it projects to a rectangle (axial range × diameter)
        Prim::Cyl { .. } => {
            let b = prim.aabb();
            within(b.min[p], b.max[p], rect[0]) && within(b.min[q], b.max[q], rect[1])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(min: [f64; 3], max: [f64; 3]) -> Shape {
        Shape::exact(Prim::Box(Aabb { min, max }))
    }

    fn cyl(axis: usize, c: [f64; 2], r: f64, lo: f64, hi: f64) -> Shape {
        Shape::exact(Prim::Cyl { axis, c, r, lo, hi })
    }

    #[test]
    fn boxes() {
        let a = bx([0.0; 3], [2.0; 3]);
        let b = bx([1.0; 3], [3.0; 3]);
        let c = bx([2.0, 0.0, 0.0], [3.0, 2.0, 2.0]);
        assert_eq!(overlap(&[a, b], &[]), Overlap::Yes);
        assert_eq!(overlap(&[a, c], &[]), Overlap::No, "touching faces");
        assert_eq!(overlap(&[a], &[bx([-1.0; 3], [3.0; 3])]), Overlap::No);
        assert_eq!(
            overlap(
                &[a],
                &[
                    bx([-1.0; 3], [3.0, 3.0, 1.0]),
                    bx([-1.0, -1.0, 1.0], [3.0; 3])
                ]
            ),
            Overlap::No,
            "two negatives jointly cover"
        );
        assert_eq!(
            overlap(&[Shape::over(a.prim), b], &[]),
            Overlap::Unknown,
            "over-approximated positive"
        );
    }

    #[test]
    fn cylinders() {
        let plate = bx([-10.0, -10.0, 0.0], [10.0, 10.0, 5.0]);
        let hole = cyl(2, [0.0, 0.0], 3.3, -1.0, 6.0);
        let bolt_small = cyl(2, [0.0, 0.0], 3.0, -5.0, 10.0);
        let bolt_big = cyl(2, [0.0, 0.0], 3.5, -5.0, 10.0);
        assert_eq!(overlap(&[plate, bolt_small], &[hole]), Overlap::No);
        assert_eq!(overlap(&[plate, bolt_big], &[hole]), Overlap::Yes);
        assert_eq!(overlap(&[plate, hole], &[]), Overlap::Yes);
        // coincident walls: undecidable within the budget, never Yes
        let bolt_same = cyl(2, [0.0, 0.0], 3.3, -5.0, 10.0);
        assert_ne!(overlap(&[plate, bolt_same], &[hole]), Overlap::Yes);
        // perpendicular cylinders
        let x = cyl(0, [0.0, 0.0], 1.0, -5.0, 5.0);
        let far = cyl(2, [0.0, 5.0], 1.0, -5.0, 5.0);
        assert_eq!(overlap(&[x, far], &[]), Overlap::No);
        assert_eq!(overlap(&[x, hole], &[]), Overlap::Yes);
        // ring void
        let ring = Piece {
            pos: cyl(2, [0.0, 0.0], 20.0, 0.0, 5.0),
            negs: vec![cyl(2, [0.0, 0.0], 10.0, 0.0, 5.0)],
        };
        let in_void = cyl(2, [4.0, 0.0], 3.0, -1.0, 6.0);
        assert_eq!(overlap(&[ring.pos, in_void], &ring.negs), Overlap::No);
        let crossing = cyl(2, [9.0, 0.0], 3.0, -1.0, 6.0);
        assert_eq!(overlap(&[ring.pos, crossing], &ring.negs), Overlap::Yes);
    }

    #[test]
    fn projections() {
        let c = Prim::Cyl {
            axis: 2,
            c: [0.0, 0.0],
            r: 5.0,
            lo: 0.0,
            hi: 1.0,
        };
        assert!(projection_covers(&c, 2, [[-3.0, 3.0], [-3.0, 3.0]]));
        assert!(!projection_covers(&c, 2, [[-4.0, 4.0], [-4.0, 4.0]]));
        assert!(projection_covers(&c, 0, [[-5.0, 5.0], [0.0, 1.0]]));
    }
}
