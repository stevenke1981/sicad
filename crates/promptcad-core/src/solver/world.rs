//! World geometry of every port, given component orientations and translations.
//!
//! [`World`] is the single evaluator used both while solving (translations partially known,
//! unknown axes read as 0) and afterwards (by [`super::WorldView`], built from a
//! [`Resolved`](crate::model::Resolved)).
//!
//! # Derived geometry cache
//!
//! Derived components (§3.2) are evaluated from their sources. The frame (pose, extent,
//! outline) and the local `region` of every derived Plane / Volume / `of` are computed **once**
//! per state of the independent placements and memoized in a per-component cache
//! ([`Geo`]). The cache is filled by an explicit-stack post-order walk over the source graph
//! (no recursion), so
//!
//! * each derived component is evaluated once, whatever the shape of the derivation graph:
//!   a `section`/`extrude` chain of length `N` costs `O(N)`, not `O(2^N)` (a `section` needs
//!   both the pose and the region of its source, which used to evaluate the source frame
//!   twice per level);
//! * there is no depth limit: any acyclic chain the 256-component cap allows resolves, and the
//!   call stack does not grow with the chain length;
//! * a reference cycle (a validator error) resolves to "no geometry" for its members instead of
//!   looping.
//!
//! The independent placements are private and only change through the `set_*` methods, which
//! invalidate the cache, so a cached frame is never stale.
//!
//! Orientation knowledge and the set of independent sources are static (independent of
//! translations) and precomputed in [`World::new`] ([`OrientSrc`] and [`World::sources`] per
//! component), again by walking source chains iteratively.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::model::geom::{vadd, vcross, vdot, vscale, vsub, AxisDir, Orientation, Vec3, TOL};
use crate::model::resolved::ResolvedPort;
use crate::model::v2::{
    BodyOpKind, CadDocument2, Component, EmptyShape, Geometry, HoleType, LineShape, PlaneShape,
    VolumeShape,
};
use crate::model::{PortRef, ShapeKind};
use crate::ports::{anchor, box_port, local_port, plane_port, transform_port};

use super::{Aabb, ConeCutter, Cutter, HoleCutters, Pose};

/// The frame of a planar derived component (`profile`, `section`) or of an `extrude`.
#[derive(Debug, Clone)]
pub(crate) struct PlaneFrame {
    pub pose: Pose,
    /// `[along local x, along local y]`.
    pub extent: [f64; 2],
    /// Outline points in world coordinates (profile only; used by `revolve`).
    pub outline: Vec<Vec3>,
}

/// Cached geometry of one derived component (see the module docs).
#[derive(Debug, Clone, Default)]
struct Geo {
    pose: Option<Pose>,
    /// `profile`, `section`, `extrude`.
    frame: Option<PlaneFrame>,
    /// `region` box in the component's own pose frame (`of`, `extrude`, `revolve`).
    local_region: Option<(Vec3, Vec3)>,
    /// World `region` of a `revolve`.
    revolve_region: Option<Aabb>,
}

/// Where the world orientation of a component comes from (§5.4 step 4), independent of
/// translations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OrientSrc {
    /// Equal to the orientation of this independent component (itself when independent;
    /// the source chain's independent root for `of`, `section`, and `extrude` of a
    /// `rect`/`disc`/`section`).
    Of(usize),
    /// Always the identity (Lines, `revolve`).
    Identity,
    /// Depends on translations (`profile`, and everything derived from one).
    Positional,
    /// Does not resolve (missing source, wrong source kind, reference cycle).
    Broken,
}

pub(crate) struct World<'a> {
    pub doc: &'a CadDocument2,
    index: BTreeMap<&'a str, usize>,
    /// Orientation of each independent component (identity until solved).
    orient: Vec<Orientation>,
    /// Translation of each independent component (unknown axes are 0 while solving).
    trans: Vec<Vec3>,
    /// Whether an independent component has a placement (always true while solving; false in a
    /// [`super::WorldView`] when the `Resolved` lacks the component).
    present: Vec<bool>,
    /// Solved §7.1 cut direction per hole; `None` → default `−R·shape.axis`.
    pub hole_dirs: Vec<Option<AxisDir>>,
    /// Owning part (index into `doc.parts`) of each hole: first part that `cut`s it.
    pub hole_part: Vec<Option<usize>>,
    /// Static orientation source of every component.
    orient_src: Vec<OrientSrc>,
    /// Independent sources of every component (static).
    sources: Vec<BTreeSet<usize>>,
    /// Memoized derived geometry; `None` = not computed for the current placements.
    geo: RefCell<Vec<Option<Geo>>>,
}

impl<'a> World<'a> {
    pub fn new(doc: &'a CadDocument2) -> Self {
        let mut index = BTreeMap::new();
        for (i, c) in doc.components.iter().enumerate() {
            index.entry(c.id.as_str()).or_insert(i);
        }
        let n = doc.components.len();
        let mut hole_part = vec![None; n];
        for (ci, c) in doc.components.iter().enumerate() {
            if c.as_hole().is_some() {
                hole_part[ci] = doc.parts.iter().position(|p| {
                    p.body
                        .iter()
                        .any(|op| op.op == BodyOpKind::Cut && op.component == c.id)
                });
            }
        }
        let mut world = World {
            doc,
            index,
            orient: vec![Orientation::IDENTITY; n],
            trans: vec![[0.0; 3]; n],
            present: vec![true; n],
            hole_dirs: vec![None; n],
            hole_part,
            orient_src: Vec::new(),
            sources: Vec::new(),
            geo: RefCell::new(vec![None; n]),
        };
        world.orient_src = (0..n).map(|ci| world.compute_orient_src(ci)).collect();
        world.sources = (0..n).map(|ci| world.compute_sources(ci)).collect();
        world
    }

    pub fn comp(&self, ci: usize) -> &'a Component {
        &self.doc.components[ci]
    }

    pub fn lookup(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }

    /// `true` for the first component with its id (later duplicates are ignored).
    pub fn is_canonical(&self, ci: usize) -> bool {
        self.lookup(self.comp(ci).id.as_str()) == Some(ci)
    }

    pub fn is_hole(&self, ci: usize) -> bool {
        self.comp(ci).as_hole().is_some()
    }

    /// The component `body[0]` of the part owning hole `ci`.
    pub fn hole_body0(&self, ci: usize) -> Option<usize> {
        let part = &self.doc.parts[self.hole_part[ci]?];
        self.lookup(part.body.first()?.component.as_str())
    }

    fn source_volume(&self, id: &crate::model::Id) -> Option<usize> {
        let si = self.lookup(id.as_str())?;
        self.comp(si).as_volume().is_some().then_some(si)
    }

    fn plane_ref(&self, id: &crate::model::Id) -> Option<usize> {
        let pi = self.lookup(id.as_str())?;
        self.comp(pi).as_plane().is_some().then_some(pi)
    }

    // ---- independent placements ---------------------------------------------------------------

    pub fn orient(&self, ci: usize) -> Orientation {
        self.orient[ci]
    }

    pub fn trans(&self, ci: usize) -> Vec3 {
        self.trans[ci]
    }

    pub fn orients(&self) -> &[Orientation] {
        &self.orient
    }

    pub fn set_orient(&mut self, ci: usize, o: Orientation) {
        if self.orient[ci] != o {
            self.orient[ci] = o;
            self.invalidate();
        }
    }

    pub fn set_trans(&mut self, ci: usize, t: Vec3) {
        if self.trans[ci] != t {
            self.trans[ci] = t;
            self.invalidate();
        }
    }

    pub fn set_trans_axis(&mut self, ci: usize, axis: usize, v: f64) {
        if self.trans[ci][axis] != v {
            self.trans[ci][axis] = v;
            self.invalidate();
        }
    }

    pub fn set_present(&mut self, ci: usize, present: bool) {
        if self.present[ci] != present {
            self.present[ci] = present;
            self.invalidate();
        }
    }

    fn invalidate(&mut self) {
        self.geo.get_mut().iter_mut().for_each(|g| *g = None);
    }

    // ---- orientation sources (§5.4 step 4) ----------------------------------------------------

    /// Static orientation source of `ci` (see [`OrientSrc`]).
    pub fn orient_src(&self, ci: usize) -> OrientSrc {
        self.orient_src[ci]
    }

    /// Walks the single-source chain `of → Volume`, `section → Volume`, `extrude → Plane`
    /// until an independent component or a terminal derived kind; at most one step per
    /// component, so a reference cycle ends as [`OrientSrc::Broken`].
    fn compute_orient_src(&self, ci: usize) -> OrientSrc {
        let mut c = ci;
        for _ in 0..=self.doc.components.len() {
            let comp = self.comp(c);
            if comp.is_independent() {
                return OrientSrc::Of(c);
            }
            let next = match &comp.geometry {
                Geometry::Empty {
                    shape: EmptyShape::Of(of),
                    ..
                } => self.source_volume(&of.source),
                Geometry::Plane {
                    shape: PlaneShape::Section(s),
                } => self.source_volume(&s.source),
                Geometry::Plane {
                    shape: PlaneShape::Profile(_),
                } => return OrientSrc::Positional,
                Geometry::Volume {
                    shape: VolumeShape::Extrude(e),
                } => self.plane_ref(&e.profile),
                Geometry::Volume {
                    shape: VolumeShape::Revolve(_),
                }
                | Geometry::Line { .. } => return OrientSrc::Identity,
                _ => return OrientSrc::Broken,
            };
            match next {
                Some(s) => c = s,
                None => return OrientSrc::Broken,
            }
        }
        OrientSrc::Broken
    }

    /// Whether the world orientation of a component is known once the orientation phase ends
    /// (i.e. does not depend on translations).
    pub fn orientation_static(&self, ci: usize) -> bool {
        matches!(self.orient_src[ci], OrientSrc::Of(_) | OrientSrc::Identity)
    }

    // ---- derived geometry cache ---------------------------------------------------------------

    /// Derived components whose cached [`Geo`] the geometry of `ci` reads.
    fn geo_deps(&self, ci: usize) -> Vec<usize> {
        let dep = match &self.comp(ci).geometry {
            Geometry::Empty {
                shape: EmptyShape::Of(of),
                ..
            } => self.source_volume(&of.source),
            Geometry::Plane {
                shape: PlaneShape::Section(s),
            } => self.source_volume(&s.source),
            Geometry::Volume {
                shape: VolumeShape::Extrude(e),
            } => self.plane_ref(&e.profile),
            Geometry::Volume {
                shape: VolumeShape::Revolve(r),
            } => self.plane_ref(&r.profile),
            _ => None,
        };
        dep.into_iter().filter(|&d| self.has_geo(d)).collect()
    }

    /// Components with a cached [`Geo`]: derived Planes, Volumes and `of`.
    fn has_geo(&self, ci: usize) -> bool {
        let comp = self.comp(ci);
        !comp.is_independent() && !matches!(comp.geometry, Geometry::Line { .. })
    }

    /// Fills the cache for `ci` and everything it derives from (iterative post-order DFS;
    /// members of a reference cycle see their in-progress dependency as "no geometry").
    fn ensure(&self, ci: usize) {
        if !self.has_geo(ci) || self.geo.borrow()[ci].is_some() {
            return;
        }
        let mut cache = self.geo.borrow_mut();
        let mut entered = vec![false; cache.len()];
        let mut stack = vec![(ci, false)];
        while let Some((c, expanded)) = stack.pop() {
            if cache[c].is_some() {
                continue;
            }
            if expanded {
                let g = self.compute_geo(c, &cache);
                cache[c] = Some(g);
                continue;
            }
            if entered[c] {
                continue;
            }
            entered[c] = true;
            stack.push((c, true));
            for d in self.geo_deps(c) {
                if cache[d].is_none() && !entered[d] {
                    stack.push((d, false));
                }
            }
        }
    }

    fn geo_field<T>(&self, ci: usize, f: impl FnOnce(&Geo) -> Option<T>) -> Option<T> {
        self.ensure(ci);
        self.geo.borrow()[ci].as_ref().and_then(f)
    }

    /// Pose of a component given the cache (no cache fill).
    fn pose_in(&self, cache: &[Option<Geo>], ci: usize) -> Option<Pose> {
        let comp = self.comp(ci);
        if comp.is_independent() {
            return self.indep_pose(ci);
        }
        match &comp.geometry {
            Geometry::Line { shape } => self.line_pose(shape),
            _ => cache[ci].as_ref()?.pose,
        }
    }

    /// Local region of a Volume-like component given the cache (no cache fill).
    fn local_region_in(&self, cache: &[Option<Geo>], ci: usize) -> Option<(Vec3, Vec3)> {
        let comp = self.comp(ci);
        if comp.is_independent() {
            return match local_port(comp, "region")? {
                ResolvedPort::Region { min, max } => Some((min, max)),
                _ => None,
            };
        }
        cache[ci].as_ref()?.local_region
    }

    /// Frame of any Plane (independent `rect`/`disc` included) given the cache — the extrude
    /// base.
    fn plane_like_frame_in(&self, cache: &[Option<Geo>], pi: usize) -> Option<PlaneFrame> {
        let extent = match self.comp(pi).as_plane()? {
            PlaneShape::Rect(r) => [r.length, r.width],
            PlaneShape::Disc(d) => [d.diameter, d.diameter],
            _ => return cache[pi].as_ref()?.frame.clone(),
        };
        Some(PlaneFrame {
            pose: self.indep_pose(pi)?,
            extent,
            outline: Vec::new(),
        })
    }

    /// One level of derived geometry, reading dependencies from `cache`.
    fn compute_geo(&self, ci: usize, cache: &[Option<Geo>]) -> Geo {
        let comp = self.comp(ci);
        let mut g = Geo::default();
        match &comp.geometry {
            Geometry::Empty {
                shape: EmptyShape::Of(of),
                ..
            } => {
                if let Some(si) = self.source_volume(&of.source) {
                    g.pose = self.pose_in(cache, si);
                    g.local_region = self.local_region_in(cache, si);
                }
            }
            Geometry::Plane {
                shape: PlaneShape::Profile(p),
            } => {
                g.frame = self
                    .lookup(p.outer.as_str())
                    .and_then(|oi| self.outline_frame(oi));
            }
            Geometry::Plane {
                shape: PlaneShape::Section(s),
            } => {
                g.frame = (|| {
                    let si = self.source_volume(&s.source)?;
                    let src = self.pose_in(cache, si)?;
                    let (lo, hi) = self.local_region_in(cache, si)?;
                    let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, s.offset];
                    Some(PlaneFrame {
                        pose: Pose {
                            orientation: src.orientation,
                            origin: vadd(src.origin, src.orientation.apply(center)),
                        },
                        extent: [hi[0] - lo[0], hi[1] - lo[1]],
                        outline: Vec::new(),
                    })
                })();
            }
            Geometry::Volume {
                shape: VolumeShape::Extrude(e),
            } => {
                g.frame = self
                    .plane_ref(&e.profile)
                    .and_then(|pi| self.plane_like_frame_in(cache, pi));
                g.local_region = g.frame.as_ref().map(|f| {
                    let (l, w) = (f.extent[0], f.extent[1]);
                    ([-l / 2.0, -w / 2.0, 0.0], [l / 2.0, w / 2.0, e.height])
                });
            }
            Geometry::Volume {
                shape: VolumeShape::Revolve(r),
            } => {
                g.pose = self.segment_ends(&r.axis).map(|(s, _)| Pose {
                    orientation: Orientation::IDENTITY,
                    origin: s,
                });
                g.revolve_region = self.revolve_region_in(cache, r);
                if let (Some(p), Some(b)) = (g.pose, g.revolve_region) {
                    g.local_region = Some((vsub(b.min, p.origin), vsub(b.max, p.origin)));
                }
            }
            _ => {}
        }
        if g.pose.is_none() {
            g.pose = g.frame.as_ref().map(|f| f.pose);
        }
        g
    }

    // ---- poses ------------------------------------------------------------------------------

    fn indep_pose(&self, ci: usize) -> Option<Pose> {
        self.present[ci].then(|| Pose {
            orientation: self.orient[ci],
            origin: self.trans[ci],
        })
    }

    fn line_pose(&self, shape: &LineShape) -> Option<Pose> {
        Some(Pose {
            orientation: Orientation::IDENTITY,
            origin: match shape {
                LineShape::Segment(s) => self.dot(&s.start)?,
                LineShape::Polyline(p) => self.dot(p.points.first()?)?,
                LineShape::Arc(a) => self.dot(&a.start)?,
                LineShape::Circle(c) => self.dot(&c.center)?,
            },
        })
    }

    /// World pose (orientation + origin) of any component.
    pub fn pose(&self, ci: usize) -> Option<Pose> {
        let comp = self.comp(ci);
        if comp.is_independent() {
            return self.indep_pose(ci);
        }
        match &comp.geometry {
            Geometry::Line { shape } => self.line_pose(shape),
            _ => self.geo_field(ci, |g| g.pose),
        }
    }

    /// World position of a Dot.
    fn dot(&self, id: &crate::model::Id) -> Option<Vec3> {
        let ci = self.lookup(id.as_str())?;
        (self.comp(ci).shape_kind() == ShapeKind::DotPoint && self.present[ci])
            .then(|| self.trans[ci])
    }

    fn segment_ends(&self, id: &crate::model::Id) -> Option<(Vec3, Vec3)> {
        let ci = self.lookup(id.as_str())?;
        match self.comp(ci).as_line()? {
            LineShape::Segment(s) => Some((self.dot(&s.start)?, self.dot(&s.end)?)),
            _ => None,
        }
    }

    /// Pose and extent of the frame of a `profile` / `section` / `extrude`.
    fn frame_pose_extent(&self, ci: usize) -> Option<(Pose, [f64; 2])> {
        self.geo_field(ci, |g| g.frame.as_ref().map(|f| (f.pose, f.extent)))
    }

    /// Frame of a closed outline Line (closed `polyline` or `circle`), §3.4 `profile` rules:
    /// all points on one axis-aligned plane, `normal` such that the outline is
    /// counter-clockwise seen from its positive side, `center` = bounding-box center, and the
    /// orientation = the first canonical orientation whose `z_axis` is the normal.
    fn outline_frame(&self, li: usize) -> Option<PlaneFrame> {
        match self.comp(li).as_line()? {
            LineShape::Circle(c) => {
                let center = self.dot(&c.center)?;
                let r = c.diameter / 2.0;
                let outline = [(-r, -r), (r, -r), (r, r), (-r, r)]
                    .iter()
                    .map(|(dx, dy)| [center[0] + dx, center[1] + dy, center[2]])
                    .collect();
                Some(PlaneFrame {
                    pose: Pose {
                        orientation: Orientation::IDENTITY,
                        origin: center,
                    },
                    extent: [c.diameter, c.diameter],
                    outline,
                })
            }
            LineShape::Polyline(p) if p.closed => {
                let pts: Vec<Vec3> = p
                    .points
                    .iter()
                    .map(|id| self.dot(id))
                    .collect::<Option<_>>()?;
                let normal = outline_normal(&pts)?;
                let orientation = Orientation::all()
                    .into_iter()
                    .find(|o| o.z_axis() == normal)?;
                let (lo, hi) = bounds(&pts)?;
                let ext = |d: AxisDir| hi[d.index()] - lo[d.index()];
                Some(PlaneFrame {
                    pose: Pose {
                        orientation,
                        origin: std::array::from_fn(|i| (lo[i] + hi[i]) / 2.0),
                    },
                    extent: [ext(orientation.x_axis()), ext(orientation.y_axis())],
                    outline: pts,
                })
            }
            _ => None,
        }
    }

    /// Full-revolution bounding box of a `revolve` (conservative for `angle < 360`), reading
    /// the profile frame from `cache`.
    fn revolve_region_in(
        &self,
        cache: &[Option<Geo>],
        r: &crate::model::v2::RevolveSpec,
    ) -> Option<Aabb> {
        let (s, e) = self.segment_ends(&r.axis)?;
        let a = axis_dir_between(s, e)?.index();
        let pi = self.plane_ref(&r.profile)?;
        // an independent rect/disc profile has no derived frame (unchanged behaviour)
        let frame = cache[pi].as_ref()?.frame.as_ref()?;
        let pts: Vec<Vec3> = if frame.outline.is_empty() {
            // section: use its face rectangle corners
            let (u, v) = (
                frame.pose.orientation.x_axis(),
                frame.pose.orientation.y_axis(),
            );
            let (hu, hv) = (frame.extent[0] / 2.0, frame.extent[1] / 2.0);
            [(-hu, -hv), (hu, -hv), (hu, hv), (-hu, hv)]
                .iter()
                .map(|(du, dv)| {
                    vadd(
                        frame.pose.origin,
                        vadd(vscale(u.unit(), *du), vscale(v.unit(), *dv)),
                    )
                })
                .collect()
        } else {
            frame.outline.clone()
        };
        if pts.is_empty() {
            return None;
        }
        let mut rmax: f64 = 0.0;
        let (mut amin, mut amax) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in &pts {
            let d = vsub(*p, s);
            let r2: f64 = (0..3).filter(|k| *k != a).map(|k| d[k] * d[k]).sum();
            rmax = rmax.max(r2.sqrt());
            amin = amin.min(p[a]);
            amax = amax.max(p[a]);
        }
        let min = std::array::from_fn(|k| if k == a { amin } else { s[k] - rmax });
        let max = std::array::from_fn(|k| if k == a { amax } else { s[k] + rmax });
        Some(Aabb { min, max })
    }

    // ---- ports ------------------------------------------------------------------------------

    pub fn port_ref(&self, port: &PortRef) -> Option<ResolvedPort> {
        self.port(self.lookup(port.id_str())?, port.port())
    }

    /// World geometry of `port` on component `ci`.
    pub fn port(&self, ci: usize, port: &str) -> Option<ResolvedPort> {
        let comp = self.comp(ci);
        if comp.is_independent() {
            if !self.present[ci] {
                return None;
            }
            if comp.as_hole().is_some() {
                return match port {
                    "axis" => Some(ResolvedPort::Axis {
                        position: self.trans[ci],
                        direction: self.hole_dir(ci),
                    }),
                    "region" => self.hole_region(ci).map(|b| ResolvedPort::Region {
                        min: b.min,
                        max: b.max,
                    }),
                    _ => local_port(comp, port)
                        .map(|l| transform_port(&l, self.orient[ci], self.trans[ci])),
                };
            }
            let local = local_port(comp, port)?;
            return Some(transform_port(&local, self.orient[ci], self.trans[ci]));
        }
        match &comp.geometry {
            Geometry::Empty {
                shape: EmptyShape::Of(of),
                ..
            } => {
                // the source is a Volume (never an `of`), so this recurses at most once
                self.port(self.source_volume(&of.source)?, port)
            }
            Geometry::Plane { .. } => {
                let (pose, extent) = self.frame_pose_extent(ci)?;
                let local = plane_port(extent, port)?;
                Some(transform_port(&local, pose.orientation, pose.origin))
            }
            Geometry::Volume {
                shape: VolumeShape::Extrude(e),
            } => {
                let (pose, extent) = self.frame_pose_extent(ci)?;
                let local = box_port(extent[0], extent[1], e.height, port)?;
                Some(transform_port(&local, pose.orientation, pose.origin))
            }
            Geometry::Volume {
                shape: VolumeShape::Revolve(_),
            } => match port {
                "origin" => Some(ResolvedPort::Point {
                    position: self.pose(ci)?.origin,
                }),
                "region" => {
                    self.geo_field(ci, |g| g.revolve_region)
                        .map(|b| ResolvedPort::Region {
                            min: b.min,
                            max: b.max,
                        })
                }
                _ => None,
            },
            Geometry::Line { shape } => self.line_port(shape, port),
            _ => None,
        }
    }

    fn line_port(&self, shape: &LineShape, port: &str) -> Option<ResolvedPort> {
        let pt = |p: Vec3| Some(ResolvedPort::Point { position: p });
        match shape {
            LineShape::Segment(s) => {
                let (a, b) = (self.dot(&s.start)?, self.dot(&s.end)?);
                match port {
                    "start" => pt(a),
                    "end" => pt(b),
                    "mid" => pt(vscale(vadd(a, b), 0.5)),
                    "axis" => Some(ResolvedPort::Axis {
                        position: a,
                        direction: axis_dir_between(a, b)?,
                    }),
                    _ => None,
                }
            }
            LineShape::Polyline(p) => match port {
                "start" => pt(self.dot(p.points.first()?)?),
                "end" => pt(self.dot(p.points.last()?)?),
                _ => None,
            },
            LineShape::Arc(a) => match port {
                "start" => pt(self.dot(&a.start)?),
                "end" => pt(self.dot(&a.end)?),
                "center" => pt(circumcenter(
                    self.dot(&a.start)?,
                    self.dot(&a.through)?,
                    self.dot(&a.end)?,
                )?),
                _ => None,
            },
            LineShape::Circle(c) => {
                let center = self.dot(&c.center)?;
                match port {
                    "center" => pt(center),
                    "axis" => Some(ResolvedPort::Axis {
                        position: center,
                        direction: AxisDir::PosZ,
                    }),
                    _ => None,
                }
            }
        }
    }

    /// World `region` AABB of a component (`None` when it has no `region` port).
    pub fn region(&self, ci: usize) -> Option<Aabb> {
        match self.port(ci, "region")? {
            ResolvedPort::Region { min, max } => Some(Aabb { min, max }),
            _ => None,
        }
    }

    /// World bounding box of all `add` components of part `pi` (§7.2).
    pub fn part_bbox(&self, pi: usize) -> Option<Aabb> {
        let mut acc: Option<Aabb> = None;
        for op in &self.doc.parts[pi].body {
            if op.op != BodyOpKind::Add {
                continue;
            }
            let Some(ci) = self.lookup(op.component.as_str()) else {
                continue;
            };
            if self.is_hole(ci) {
                continue;
            }
            if let Some(b) = self.region(ci) {
                acc = Some(match acc {
                    Some(a) => a.union(&b),
                    None => b,
                });
            }
        }
        acc
    }

    // ---- holes (§7) -------------------------------------------------------------------------

    /// World direction of the hole's `shape.axis` (positive sense) under its orientation.
    pub fn hole_axis_world(&self, ci: usize) -> Option<AxisDir> {
        let h = self.comp(ci).as_hole()?;
        Some(self.orient[ci].apply_dir(h.axis.positive()))
    }

    /// The hole's cut direction: solved (§7.1) if set, else `−R·shape.axis`.
    pub fn hole_dir(&self, ci: usize) -> AxisDir {
        self.hole_dirs[ci]
            .or_else(|| self.hole_axis_world(ci).map(AxisDir::neg))
            .unwrap_or(AxisDir::NegZ)
    }

    /// Hole `region` (§3.4): from `entry` to the main-hole end (blind: `entry + depth`, other
    /// types: far boundary of the owning part's bounding box), lateral size
    /// `max(diameter, counterbore_diameter, countersink_diameter)`.
    pub fn hole_region(&self, ci: usize) -> Option<Aabb> {
        let h = self.comp(ci).as_hole()?;
        let entry = self.trans[ci];
        let dir = self.hole_dir(ci);
        let (k, s) = (dir.index(), dir.sign());
        let far = match (h.hole_type, h.depth) {
            (HoleType::Blind, Some(d)) => entry[k] + s * d,
            _ => {
                let b = self.part_bbox(self.hole_part[ci]?)?;
                if s > 0.0 {
                    b.max[k]
                } else {
                    b.min[k]
                }
            }
        };
        let r = h.max_diameter() / 2.0;
        let mut min: Vec3 = std::array::from_fn(|i| entry[i] - r);
        let mut max: Vec3 = std::array::from_fn(|i| entry[i] + r);
        min[k] = entry[k].min(far);
        max[k] = entry[k].max(far);
        Some(Aabb { min, max })
    }

    /// Main-hole, counterbore and countersink cutters (§7.2).
    pub fn hole_cutters(&self, ci: usize) -> Option<HoleCutters> {
        let h = self.comp(ci).as_hole()?;
        if !self.present[ci] {
            return None;
        }
        let entry = self.trans[ci];
        let dir = self.hole_dir(ci);
        let du = dir.unit();
        let along = |from: Vec3, len: f64| vadd(from, vscale(du, len));
        let main = match (h.hole_type, h.depth) {
            (HoleType::Blind, Some(d)) => Cutter {
                start: along(entry, -0.1),
                end: along(entry, d + 0.1),
                diameter: h.diameter,
            },
            _ => {
                let b = self.part_bbox(self.hole_part[ci]?)?;
                let k = dir.index();
                let (near, far) = if dir.is_positive() {
                    (b.min[k] - 1.0, b.max[k] + 1.0)
                } else {
                    (b.max[k] + 1.0, b.min[k] - 1.0)
                };
                let mut start = entry;
                let mut end = entry;
                start[k] = near;
                end[k] = far;
                Cutter {
                    start,
                    end,
                    diameter: h.diameter,
                }
            }
        };
        let counterbore = match (h.hole_type, h.counterbore_diameter, h.counterbore_depth) {
            (HoleType::Counterbore, Some(d), Some(depth)) => {
                let start = along(entry, -0.1);
                Some(Cutter {
                    start,
                    end: vadd(start, vscale(du, depth + 0.2)),
                    diameter: d,
                })
            }
            _ => None,
        };
        let countersink = match (h.hole_type, h.countersink_diameter, h.countersink_angle) {
            (HoleType::Countersink, Some(d), Some(angle)) => {
                let tan = (angle / 2.0).to_radians().tan();
                let height = (d - h.diameter) / (2.0 * tan.max(0.01));
                Some(ConeCutter {
                    start: entry,
                    end: along(entry, height),
                    outer_diameter: d,
                    inner_diameter: h.diameter,
                })
            }
            _ => None,
        };
        Some(HoleCutters {
            direction: dir,
            entry,
            main,
            counterbore,
            countersink,
        })
    }

    // ---- knowledge (§5.5) -------------------------------------------------------------------

    /// Independent components a component's geometry depends on (itself if independent);
    /// precomputed in [`World::new`].
    pub fn sources(&self, ci: usize) -> &BTreeSet<usize> {
        &self.sources[ci]
    }

    fn compute_sources(&self, ci: usize) -> BTreeSet<usize> {
        let mut out = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut stack = vec![ci];
        while let Some(c) = stack.pop() {
            if !seen.insert(c) {
                continue;
            }
            let comp = self.comp(c);
            if comp.is_independent() {
                out.insert(c);
                continue;
            }
            for (_, id) in comp.geometry.shape_refs() {
                if let Some(si) = self.lookup(id.as_str()) {
                    stack.push(si);
                }
            }
        }
        out
    }

    /// Whether the anchor coordinate of a derived port on axis `k` depends only on its sources'
    /// translations on axis `k` (the §5.5 per-axis knowledge rule holds exactly).
    ///
    /// `section` and `extrude` frames are separable exactly when their orientation is static
    /// (their origin is then the root translation plus a constant rotated offset); a
    /// `profile`-based `extrude` is not (its orientation and extent follow the Dot positions).
    pub fn anchor_separable(&self, ci: usize, port: &str) -> bool {
        let comp = self.comp(ci);
        if comp.is_independent() {
            return true;
        }
        match &comp.geometry {
            Geometry::Empty {
                shape: EmptyShape::Of(of),
                ..
            } => self
                .source_volume(&of.source)
                .is_some_and(|s| s != ci && self.anchor_separable_volume(s, port)),
            Geometry::Plane {
                shape: PlaneShape::Section(_),
            } => self.orientation_static(ci),
            Geometry::Plane { .. } => true,
            Geometry::Volume { .. } => self.anchor_separable_volume(ci, port),
            Geometry::Line {
                shape: LineShape::Arc(_),
            } => port != "center",
            _ => true,
        }
    }

    fn anchor_separable_volume(&self, ci: usize, port: &str) -> bool {
        let comp = self.comp(ci);
        if comp.is_independent() {
            return true;
        }
        match &comp.geometry {
            Geometry::Volume {
                shape: VolumeShape::Revolve(_),
            } => port == "origin",
            Geometry::Volume {
                shape: VolumeShape::Extrude(_),
            } => self.orientation_static(ci),
            _ => false,
        }
    }

    /// Whether the directions of a port are known without translations.
    pub fn dirs_static(&self, ci: usize, port: &str) -> bool {
        let comp = self.comp(ci);
        if comp.is_independent() {
            return true;
        }
        match &comp.geometry {
            Geometry::Line {
                shape: LineShape::Segment(_),
            } => port != "axis",
            _ => self.orientation_static(ci),
        }
    }
}

// ---- geometry helpers -----------------------------------------------------------------------

fn bounds(pts: &[Vec3]) -> Option<(Vec3, Vec3)> {
    let first = *pts.first()?;
    Some(pts.iter().fold((first, first), |(lo, hi), p| {
        (
            std::array::from_fn(|i| lo[i].min(p[i])),
            std::array::from_fn(|i| hi[i].max(p[i])),
        )
    }))
}

/// Axis-aligned unit direction from `a` to `b`; `None` if degenerate or not axis-aligned.
pub(crate) fn axis_dir_between(a: Vec3, b: Vec3) -> Option<AxisDir> {
    let d = vsub(b, a);
    let len = vdot(d, d).sqrt();
    if len <= TOL {
        return None;
    }
    AxisDir::from_vec(vscale(d, 1.0 / len))
}

/// Normal of a planar closed outline (Newell); `None` when degenerate or when the points do
/// not lie on one axis-aligned plane.
fn outline_normal(pts: &[Vec3]) -> Option<AxisDir> {
    if pts.len() < 3 {
        return None;
    }
    let p0 = pts[0];
    let n = pts
        .iter()
        .zip(pts.iter().cycle().skip(1))
        .fold([0.0; 3], |n, (a, b)| {
            vadd(n, vcross(vsub(*a, p0), vsub(*b, p0)))
        });
    let k = (0..3).max_by(|a, b| n[*a].abs().total_cmp(&n[*b].abs()))?;
    if n[k].abs() <= TOL {
        return None;
    }
    if pts.iter().any(|p| (p[k] - p0[k]).abs() > TOL) {
        return None;
    }
    let axis = crate::model::geom::Axis::from_index(k)?;
    Some(AxisDir::new(axis, n[k] > 0.0))
}

/// Circumcenter of three points; `None` when collinear.
fn circumcenter(p1: Vec3, p2: Vec3, p3: Vec3) -> Option<Vec3> {
    let a = vsub(p1, p3);
    let b = vsub(p2, p3);
    let axb = vcross(a, b);
    let den = 2.0 * vdot(axb, axb);
    if den <= TOL * TOL {
        return None;
    }
    let num = vcross(vsub(vscale(b, vdot(a, a)), vscale(a, vdot(b, b))), axb);
    Some(vadd(p3, vscale(num, 1.0 / den)))
}

/// Anchor coordinate of a port on one axis.
pub(crate) fn anchor_coord(port: &ResolvedPort, k: usize) -> Option<f64> {
    anchor(port).map(|p| p[k])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circumcenter_and_normals() {
        let c = circumcenter([1.0, 0.0, 5.0], [0.0, 1.0, 5.0], [-1.0, 0.0, 5.0]).unwrap();
        assert!(crate::model::geom::vapprox_eq(c, [0.0, 0.0, 5.0], 1e-9));
        assert!(circumcenter([0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]).is_none());
        let ccw = [
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ];
        assert_eq!(outline_normal(&ccw), Some(AxisDir::PosZ));
        let cw: Vec<_> = ccw.iter().rev().copied().collect();
        assert_eq!(outline_normal(&cw), Some(AxisDir::NegZ));
        let tilted = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.5]];
        assert_eq!(outline_normal(&tilted), None);
        assert_eq!(
            axis_dir_between([0.0; 3], [0.0, -3.0, 0.0]),
            Some(AxisDir::NegY)
        );
        assert_eq!(axis_dir_between([0.0; 3], [1.0, 1.0, 0.0]), None);
    }
}
