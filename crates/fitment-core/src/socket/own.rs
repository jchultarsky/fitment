//! U11 (the socket PR, `docs/research/m4-match.md` SR5): a bore or shaft
//! whose span's core holds the original's own material is unverifiable,
//! for M4 asks a candidate's core to be clear (VR22) and the original
//! would fail its own requirement there. The core is VR22's for the
//! original as candidate, or wider: a bore's solid cylinder of radius
//! c/2 + τ + g (clearance) or d/2 − τ (pinned, or a zone), a shaft's
//! outside of radius c/2 − τ − g or d/2 + τ, over the span widened by the
//! default tolerance at both ends, stopping short of the stack's own root
//! or bottom. The stack's own cylinders, and its transitions a segment may
//! stand for (X7), are no intrusion. Every other face of the original is
//! tested soundly: a plane square to the axis by its region's distance
//! from it, a cylinder, cone or torus on the axis by its radius over its
//! stations, anything else by its box. A post in a bore, a skirt round a
//! spigot or a second pin beside a pin makes its requirement unverifiable
//! until a reviewer removes it (s50, s54, s64).

use std::collections::{BTreeMap, BTreeSet};

use super::geo::{V, add, cross, dot, norm, perpendiculars, scale, sub};
use super::model::{Flag, Requirement, Requires, SpanBasis};
use super::overlap::{P2, distance_to_segment, flatten, hole_polygon, inside, outer_polygon};
use super::place::{Aabb, Tree};
use crate::features::extent::FaceBox;
use crate::features::{CylinderClass, FeatureId, Outline, Part, Segment, StackEnd};

/// The default linear tolerance (D9): the core's reach past a span's ends.
const REACH_MM: f64 = 0.01;

/// What of a face may reach a core: its stations along the axis and its
/// least and greatest distance from it there.
#[derive(Debug, Clone, Copy)]
struct Reach {
    from: f64,
    to: f64,
    near: f64,
    far: f64,
}

/// One face of the original, as tested.
enum Shape {
    /// A plane face square to the axis, of known outline: its station, and
    /// the sine of its tilt from square (its region is asked of each core).
    Square { at: f64, tilt: f64 },
    /// A face on the axis, or a box: what it reaches.
    Reach(Reach),
    /// A cone on the axis: its radius at two stations, straight between.
    Taper { lo: (f64, f64), hi: (f64, f64) },
    /// A plane face along or across the axis: a polygon holding it, as
    /// (station, across) points.
    Polygon(Vec<(f64, P2)>),
    /// Nothing is known of where it lies.
    Anywhere,
}

/// Where a feature lies in its part.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Plane(usize),
    Cylinder(usize),
    Cone(usize),
    Torus(usize),
}

/// A plane face's hole loops, by index among its outline's loops, and a
/// tree over their boxes.
struct Holes {
    loops: Vec<usize>,
    tree: Tree,
}

/// The original part, its faces and a tree over their boxes, built once.
pub(crate) struct Own<'p> {
    part: &'p Part,
    /// Each face's `#id` and the feature it belongs to, in tree order.
    faces: Vec<(u64, FeatureId)>,
    /// Each feature's faces, as indices into `faces`.
    of: BTreeMap<FeatureId, Vec<usize>>,
    boxes: Vec<Option<Aabb>>,
    tree: Tree,
    kinds: BTreeMap<FeatureId, Kind>,
    /// Each plane face's index among its plane's faces, by `#id`.
    plane_faces: BTreeMap<u64, usize>,
    /// Each plane face's holes, by `#id`, so a core asks only of those
    /// near it.
    holes: BTreeMap<u64, Holes>,
}

/// A loop's box: its lines' ends, its arcs' whole circles.
pub(crate) fn loop_box(segments: &[Segment]) -> Option<Aabb> {
    let mut out: Option<Aabb> = None;
    let mut take = |low: V, high: V| {
        out = Some(out.map_or((low, high), |(lo, hi)| {
            (
                [0, 1, 2].map(|k| lo[k].min(low[k])),
                [0, 1, 2].map(|k| hi[k].max(high[k])),
            )
        }));
    };
    for segment in segments {
        match segment {
            Segment::Line { from_mm, to_mm, .. } => {
                take(from_mm.to_array(), from_mm.to_array());
                take(to_mm.to_array(), to_mm.to_array());
            }
            Segment::Arc {
                center_mm,
                radius_mm,
                ..
            } => {
                let c = center_mm.to_array();
                take(c.map(|x| x - radius_mm), c.map(|x| x + radius_mm));
            }
        }
    }
    out
}

impl<'p> Own<'p> {
    pub(crate) fn of(part: &'p Part, boxes: &[FaceBox]) -> Self {
        let mut faces: Vec<(u64, FeatureId)> = Vec::new();
        for plane in &part.planes {
            faces.extend(plane.faces.iter().map(|f| (f.face, plane.id)));
        }
        for c in &part.cylinders {
            faces.extend(c.faces.iter().map(|f| (*f, c.id)));
        }
        for c in &part.cones {
            faces.extend(c.faces.iter().map(|f| (*f, c.id)));
        }
        for t in &part.tori {
            faces.extend(t.faces.iter().map(|f| (*f, t.id)));
        }
        faces.extend(part.other_faces.iter().map(|o| (o.face, o.id)));
        let by_face: BTreeMap<u64, Option<Aabb>> =
            boxes.iter().map(|b| (b.face, b.bounds)).collect();
        let boxes: Vec<Option<Aabb>> = faces
            .iter()
            .map(|(face, _)| by_face.get(face).copied().flatten())
            .collect();
        let tree = Tree::new(boxes.clone());
        let mut of: BTreeMap<FeatureId, Vec<usize>> = BTreeMap::new();
        for (k, (_, feature)) in faces.iter().enumerate() {
            of.entry(*feature).or_default().push(k);
        }
        let mut kinds = BTreeMap::new();
        let mut plane_faces = BTreeMap::new();
        let mut holes = BTreeMap::new();
        for (i, plane) in part.planes.iter().enumerate() {
            kinds.insert(plane.id, Kind::Plane(i));
            for (j, face) in plane.faces.iter().enumerate() {
                plane_faces.insert(face.face, j);
                if let Outline::Known { loops } = &face.outline {
                    let inner: Vec<usize> = (0..loops.len())
                        .filter(|l| loops.get(*l).is_some_and(|l| !l.outer))
                        .collect();
                    let tree = Tree::new(
                        inner
                            .iter()
                            .map(|l| loops.get(*l).and_then(|l| loop_box(&l.segments)))
                            .collect(),
                    );
                    holes.insert(face.face, Holes { loops: inner, tree });
                }
            }
        }
        kinds.extend(
            part.cylinders
                .iter()
                .enumerate()
                .map(|(i, c)| (c.id, Kind::Cylinder(i))),
        );
        kinds.extend(
            part.cones
                .iter()
                .enumerate()
                .map(|(i, c)| (c.id, Kind::Cone(i))),
        );
        kinds.extend(
            part.tori
                .iter()
                .enumerate()
                .map(|(i, t)| (t.id, Kind::Torus(i))),
        );
        Self {
            part,
            faces,
            of,
            boxes,
            tree,
            kinds,
            plane_faces,
            holes,
        }
    }
}

/// The axis frame of a requirement.
pub(crate) struct Frame {
    point: V,
    dir: V,
    u: V,
    w: V,
}

impl Frame {
    /// The frame of the line through `point` along unit `dir`.
    pub(crate) fn new(point: V, dir: V) -> Self {
        let (u, w) = perpendiculars(dir);
        Self { point, dir, u, w }
    }

    /// Its axis direction.
    pub(crate) fn dir(&self) -> V {
        self.dir
    }

    /// The distance of `p` from the axis.
    pub(crate) fn distance(&self, p: V) -> f64 {
        let [x, y] = self.across(p);
        (x * x + y * y).sqrt()
    }

    pub(crate) fn station(&self, p: V) -> f64 {
        dot(sub(p, self.point), self.dir)
    }

    fn across(&self, p: V) -> P2 {
        let d = sub(p, self.point);
        [dot(d, self.u), dot(d, self.w)]
    }

    /// True if a line through `p` along `d` is the axis.
    fn on_axis(&self, p: V, d: V) -> bool {
        let [x, y] = self.across(p);
        norm(cross(d, self.dir)) <= 1e-9 && (x * x + y * y).sqrt() <= 1e-6
    }

    /// A box's stations and distances from the axis: a sound bound of
    /// whatever it holds, from its corners.
    fn boxed(&self, (low, high): &Aabb) -> Reach {
        let mut out = (f64::INFINITY, f64::NEG_INFINITY);
        let mut across = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for i in 0..8 {
            let c = [
                if i & 1 == 0 { low[0] } else { high[0] },
                if i & 2 == 0 { low[1] } else { high[1] },
                if i & 4 == 0 { low[2] } else { high[2] },
            ];
            let s = self.station(c);
            out = (out.0.min(s), out.1.max(s));
            let [x, y] = self.across(c);
            across = (
                [across.0[0].min(x), across.0[1].min(y)],
                [across.1[0].max(x), across.1[1].max(y)],
            );
        }
        let gap = |lo: f64, hi: f64| {
            if lo > 0.0 {
                lo
            } else if hi < 0.0 {
                -hi
            } else {
                0.0
            }
        };
        let (gx, gy) = (gap(across.0[0], across.1[0]), gap(across.0[1], across.1[1]));
        let (fx, fy) = (
            across.0[0].abs().max(across.1[0].abs()),
            across.0[1].abs().max(across.1[1].abs()),
        );
        Reach {
            from: out.0,
            to: out.1,
            near: (gx * gx + gy * gy).sqrt(),
            far: (fx * fx + fy * fy).sqrt(),
        }
    }
}

impl Frame {
    /// The stations and distances from the axis of what lies within the
    /// hull of circles (centre, unit normal, radius): each circle's extent
    /// along the axis and across it, in this frame.
    fn circles(&self, circles: &[(V, V, f64)]) -> Reach {
        let mut out = (f64::INFINITY, f64::NEG_INFINITY);
        let mut across = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for (c, d, r) in circles {
            let reach = |axis: V| r * (1.0 - dot(*d, axis) * dot(*d, axis)).max(0.0).sqrt();
            let s = self.station(*c);
            let [x, y] = self.across(*c);
            let (rs, ru, rw) = (reach(self.dir), reach(self.u), reach(self.w));
            out = (out.0.min(s - rs), out.1.max(s + rs));
            across = (
                [across.0[0].min(x - ru), across.0[1].min(y - rw)],
                [across.1[0].max(x + ru), across.1[1].max(y + rw)],
            );
        }
        let gap = |lo: f64, hi: f64| {
            if lo > 0.0 {
                lo
            } else if hi < 0.0 {
                -hi
            } else {
                0.0
            }
        };
        let (gx, gy) = (gap(across.0[0], across.1[0]), gap(across.0[1], across.1[1]));
        let (fx, fy) = (
            across.0[0].abs().max(across.1[0].abs()),
            across.0[1].abs().max(across.1[1].abs()),
        );
        Reach {
            from: out.0,
            to: out.1,
            near: (gx * gx + gy * gy).sqrt(),
            far: (fx * fx + fy * fy).sqrt(),
        }
    }
}

/// One edge's least and greatest distance from the axis, across it:
/// exact for a line and an arc. `turn` is the plane's normal against the
/// frame's (an arc's sweep is about the face's normal).
fn segment_reach(frame: &Frame, turn: f64, segment: &Segment) -> (f64, f64) {
    let length = |p: P2| (p[0] * p[0] + p[1] * p[1]).sqrt();
    match segment {
        Segment::Line { from_mm, to_mm, .. } => {
            let (a, b) = (
                frame.across(from_mm.to_array()),
                frame.across(to_mm.to_array()),
            );
            (
                distance_to_segment([0.0, 0.0], a, b),
                length(a).max(length(b)),
            )
        }
        Segment::Arc {
            from_mm,
            to_mm,
            center_mm,
            radius_mm,
            sweep_rad,
            ..
        } => {
            let (a, b) = (
                frame.across(from_mm.to_array()),
                frame.across(to_mm.to_array()),
            );
            let (low, high) = arc(
                frame.across(center_mm.to_array()),
                a,
                *radius_mm,
                turn * sweep_rad,
            );
            (
                low.min(length(a)).min(length(b)),
                high.max(length(a)).max(length(b)),
            )
        }
    }
}

impl Own<'_> {
    /// Face `k`, if a plane's, with its plane.
    fn plane_face(
        &self,
        k: usize,
    ) -> Option<(&crate::features::Plane, &crate::features::PlaneFace)> {
        let &(face, feature) = self.faces.get(k)?;
        let Some(Kind::Plane(i)) = self.kinds.get(&feature) else {
            return None;
        };
        let plane = self.part.planes.get(*i)?;
        Some((plane, plane.faces.get(*self.plane_faces.get(&face)?)?))
    }

    /// A square plane face's region against `core`: its outer loop's
    /// reach, and for a bore the holes near the axis (asked of a tree),
    /// exact; whether the axis passes through it asked of a region holding
    /// it (its outer loop circumscribed, those holes inscribed), so a doubt
    /// reads as through. Its tilt from square widens its station by its
    /// reach. Every edge examined counts toward `pairs`.
    fn square_hit(
        &self,
        k: usize,
        frame: &Frame,
        core: &Core,
        (at, tilt): (f64, f64),
        (pairs, limit): (&mut u64, u64),
    ) -> Result<Option<f64>, super::rules::TooMany> {
        let Some((plane, face)) = self.plane_face(k) else {
            return Ok(Some(f64::NAN));
        };
        let Outline::Known { loops } = &face.outline else {
            return Ok(Some(f64::NAN));
        };
        let normal = plane.normal.to_array();
        let turn = dot(normal, cross(frame.u, frame.w)).signum();
        let mut count = |n: usize| {
            *pairs += n as u64;
            if *pairs > limit {
                Err(super::rules::TooMany)
            } else {
                Ok(())
            }
        };
        let (mut near, mut far) = (f64::INFINITY, 0.0_f64);
        for outer in loops.iter().filter(|l| l.outer) {
            count(outer.segments.len())?;
            for segment in &outer.segments {
                let (low, high) = segment_reach(frame, turn, segment);
                near = near.min(low);
                far = far.max(high);
            }
        }
        let slack = tilt * far;
        if !core.meets(at - slack, at + slack) {
            return Ok(None);
        }
        if !core.hole {
            return Ok((far >= core.rho).then_some(far));
        }
        let point = add(frame.point, scale(frame.dir, at));
        let reach = core.rho.max(0.0) + slack;
        let query = (point.map(|x| x - reach), point.map(|x| x + reach));
        let mut through = outer_polygon(&face.outline, normal)
            .is_none_or(|p| inside([0.0, 0.0], &flatten(&p, frame.point, frame.u, frame.w)));
        if let Some(holes) = self.holes.get(&face.face) {
            for h in holes.tree.meeting(&query, 0.0) {
                let Some(hole) = holes.loops.get(h).and_then(|l| loops.get(*l)) else {
                    continue;
                };
                count(hole.segments.len())?;
                for segment in &hole.segments {
                    near = near.min(segment_reach(frame, turn, segment).0);
                }
                if hole_polygon(&hole.segments, normal).is_some_and(|p| {
                    inside([0.0, 0.0], &flatten(&p, frame.point, frame.u, frame.w))
                }) {
                    through = false;
                }
            }
        }
        Ok(if through {
            Some(0.0)
        } else {
            (near <= core.rho).then_some(near)
        })
    }

    /// A square plane face's whole reach from the axis, for X7's test of a
    /// transition: (least, greatest), the least zero where the axis passes
    /// through it.
    fn square_reach(&self, k: usize, frame: &Frame) -> Option<(f64, f64)> {
        let (plane, face) = self.plane_face(k)?;
        let Outline::Known { loops } = &face.outline else {
            return None;
        };
        let normal = plane.normal.to_array();
        let turn = dot(normal, cross(frame.u, frame.w)).signum();
        let (mut near, mut far) = (f64::INFINITY, 0.0_f64);
        for segment in loops.iter().flat_map(|l| l.segments.iter()) {
            let (low, high) = segment_reach(frame, turn, segment);
            near = near.min(low);
            far = far.max(high);
        }
        let flat = |p: Vec<V>| flatten(&p, frame.point, frame.u, frame.w);
        let through = outer_polygon(&face.outline, normal)
            .map(flat)
            .is_none_or(|p| inside([0.0, 0.0], &p))
            && !loops.iter().filter(|l| !l.outer).any(|l| {
                hole_polygon(&l.segments, normal)
                    .map(flat)
                    .is_some_and(|p| inside([0.0, 0.0], &p))
            });
        near.is_finite()
            .then_some((if through { 0.0 } else { near }, far))
    }
}

/// A polygon (station, across) cut to the stations `from` to `to`
/// (Sutherland and Hodgman, against each bound in turn).
fn slab(points: &[(f64, P2)], from: f64, to: f64) -> Vec<(f64, P2)> {
    let cut = |points: Vec<(f64, P2)>, keep: &dyn Fn(f64) -> bool, at: f64| {
        let mut out = Vec::new();
        for (i, a) in points.iter().enumerate() {
            let b = points[(i + 1) % points.len()];
            if keep(a.0) {
                out.push(*a);
            }
            if keep(a.0) != keep(b.0) && (b.0 - a.0).abs() > 0.0 {
                let t = (at - a.0) / (b.0 - a.0);
                out.push((
                    at,
                    [
                        a.1[0] + t * (b.1[0] - a.1[0]),
                        a.1[1] + t * (b.1[1] - a.1[1]),
                    ],
                ));
            }
        }
        out
    };
    let lower = cut(points.to_vec(), &|s| s >= from, from);
    if lower.is_empty() {
        return lower;
    }
    cut(lower, &|s| s <= to, to)
}

/// The least and greatest distance from the axis of a polygon (station,
/// across): zero where the axis passes through it, as its shadow across
/// the axis then holds the axis's point.
fn polygon_reach(points: &[(f64, P2)]) -> (f64, f64) {
    let shadow: Vec<P2> = points.iter().map(|p| p.1).collect();
    let far = shadow
        .iter()
        .map(|p| (p[0] * p[0] + p[1] * p[1]).sqrt())
        .fold(0.0, f64::max);
    if shadow.len() >= 3 && inside([0.0, 0.0], &shadow) {
        return (0.0, far);
    }
    let near = (0..shadow.len())
        .map(|i| distance_to_segment([0.0, 0.0], shadow[i], shadow[(i + 1) % shadow.len()]))
        .fold(f64::INFINITY, f64::min);
    (near, far)
}

/// The box of where plane `n`·x = `offset` crosses box `b`: of the points
/// where it meets the box's edges, the vertices of that section. `None`
/// if it misses the box.
fn cut((low, high): &Aabb, normal: V, offset: f64) -> Option<Aabb> {
    let corner = |index: usize| {
        [
            if index & 1 == 0 { low[0] } else { high[0] },
            if index & 2 == 0 { low[1] } else { high[1] },
            if index & 4 == 0 { low[2] } else { high[2] },
        ]
    };
    let mut out: Option<Aabb> = None;
    for start in 0..8 {
        for bit in [1, 2, 4] {
            let end = start | bit;
            if end == start {
                continue;
            }
            let (from, to) = (corner(start), corner(end));
            let (side_from, side_to) = (dot(normal, from) - offset, dot(normal, to) - offset);
            if side_from * side_to > 0.0 {
                continue;
            }
            let points = if side_from == 0.0 && side_to == 0.0 {
                vec![from, to]
            } else {
                let along = side_from / (side_from - side_to);
                vec![add(from, scale(sub(to, from), along))]
            };
            for point in points {
                out = Some(out.map_or((point, point), |(lo, hi)| {
                    (
                        [0, 1, 2].map(|axis| lo[axis].min(point[axis])),
                        [0, 1, 2].map(|axis| hi[axis].max(point[axis])),
                    )
                }));
            }
        }
    }
    out
}

/// The least and greatest distance from the origin of an arc about `c` of
/// radius `r` from `a`, sweeping `sweep` (counterclockwise positive), its
/// ends aside.
fn arc(c: P2, a: P2, r: f64, sweep: f64) -> (f64, f64) {
    let d = (c[0] * c[0] + c[1] * c[1]).sqrt();
    if d <= 1e-12 * r.max(1.0) {
        return (r, r);
    }
    let start = libm::atan2(a[1] - c[1], a[0] - c[0]);
    let tau = 2.0 * std::f64::consts::PI;
    let on = |angle: f64| {
        let turned = if sweep >= 0.0 {
            (angle - start).rem_euclid(tau)
        } else {
            (start - angle).rem_euclid(tau)
        };
        turned <= sweep.abs() + 1e-12
    };
    // Toward the origin and away from it.
    let toward = libm::atan2(-c[1], -c[0]);
    let low = if on(toward) {
        (d - r).abs()
    } else {
        f64::INFINITY
    };
    let high = if on(toward + std::f64::consts::PI) {
        d + r
    } else {
        0.0
    };
    (low, high)
}

impl Own<'_> {
    /// Face `k`'s box: its own, or with none (an edge on an unread curve)
    /// the part's, a plane face's section of it.
    fn boxed(&self, k: usize, frame: &Frame, plane: Option<&crate::features::Plane>) -> Shape {
        let extent = self
            .part
            .extent
            .map(|e| (e.min_mm.to_array(), e.max_mm.to_array()));
        let within = match (plane, extent) {
            (Some(p), Some(e)) => cut(&e, p.normal.to_array(), p.offset_mm).or(Some(e)),
            (None, e) => e,
            (Some(_), None) => None,
        };
        self.boxes
            .get(k)
            .copied()
            .flatten()
            .or(within)
            .map_or(Shape::Anywhere, |b| Shape::Reach(frame.boxed(&b)))
    }

    /// How face `k` lies in `frame`.
    fn shape(&self, k: usize, frame: &Frame) -> Shape {
        let Some(&(_, feature)) = self.faces.get(k) else {
            return Shape::Anywhere;
        };
        let part = self.part;
        let found = match self.kinds.get(&feature) {
            Some(Kind::Plane(_)) => {
                let plane = self.plane_face(k);
                return plane
                    .and_then(|(plane, f)| plane_shape(frame, plane, f))
                    .unwrap_or_else(|| self.boxed(k, frame, plane.map(|(p, _)| p)));
            }
            Some(Kind::Cylinder(i)) => part
                .cylinders
                .get(*i)
                .and_then(|c| cylinder_shape(frame, c).or_else(|| self.coaxial(k, frame, c))),
            Some(Kind::Cone(i)) => part.cones.get(*i).and_then(|c| cone_shape(frame, c)),
            Some(Kind::Torus(i)) => part.tori.get(*i).map(|t| torus_shape(frame, t)),
            None => None,
        };
        found.unwrap_or_else(|| self.boxed(k, frame, None))
    }
}

impl Own<'_> {
    /// A cylinder on the axis whose stations do not bound it (a partial
    /// one, or one with an irregular end): every point of it lies at its
    /// radius, along its box's stations, or anywhere along the axis with
    /// no box.
    fn coaxial(
        &self,
        k: usize,
        frame: &Frame,
        cylinder: &crate::features::Cylinder,
    ) -> Option<Shape> {
        let point = cylinder.axis.point_mm.to_array();
        let dir = cylinder.axis.direction.to_array();
        if !frame.on_axis(point, dir) {
            return None;
        }
        let (from, to) = match self.boxed(k, frame, None) {
            Shape::Reach(r) => (r.from, r.to),
            _ => (f64::NEG_INFINITY, f64::INFINITY),
        };
        Some(Shape::Reach(Reach {
            from,
            to,
            near: cylinder.radius_mm,
            far: cylinder.radius_mm,
        }))
    }
}

/// A plane face: square to the axis, by its region; otherwise by its outer
/// loop, circumscribed, which holds it. `None` for an unknown outline.
fn plane_shape(
    frame: &Frame,
    plane: &crate::features::Plane,
    face: &crate::features::PlaneFace,
) -> Option<Shape> {
    let normal = plane.normal.to_array();
    let along = dot(normal, frame.dir);
    if (along.abs() - 1.0).abs() <= 1e-9 && matches!(face.outline, Outline::Known { .. }) {
        let at = (plane.offset_mm - dot(normal, frame.point)) / along;
        let tilt = (1.0 - along * along).max(0.0).sqrt();
        return Some(Shape::Square { at, tilt });
    }
    outer_polygon(&face.outline, normal).map(|points| {
        Shape::Polygon(
            points
                .iter()
                .map(|point| (frame.station(*point), frame.across(*point)))
                .collect(),
        )
    })
}

/// A whole cylinder bounded by its end circles: on the axis, by its radius
/// over its stations; off it, between its end circles. `None` otherwise:
/// a partial one's wall, or one with an irregular end, may run past the
/// stations of its circles.
fn cylinder_shape(frame: &Frame, cylinder: &crate::features::Cylinder) -> Option<Shape> {
    if !matches!(cylinder.class, CylinderClass::Full { .. }) {
        return None;
    }
    let point = cylinder.axis.point_mm.to_array();
    let dir = cylinder.axis.direction.to_array();
    let (low, high) = cylinder
        .stations_mm
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), at| {
            (lo.min(*at), hi.max(*at))
        });
    if low > high {
        return None;
    }
    let (first, last) = (add(point, scale(dir, low)), add(point, scale(dir, high)));
    if !frame.on_axis(point, dir) {
        let radius = cylinder.radius_mm;
        return Some(Shape::Reach(
            frame.circles(&[(first, dir, radius), (last, dir, radius)]),
        ));
    }
    let (from, to) = (frame.station(first), frame.station(last));
    Some(Shape::Reach(Reach {
        from: from.min(to),
        to: from.max(to),
        near: cylinder.radius_mm,
        far: cylinder.radius_mm,
    }))
}

/// A cone with known radii: on the axis, by its radius along it; off it,
/// between its end circles. `None` otherwise.
fn cone_shape(frame: &Frame, cone: &crate::features::Cone) -> Option<Shape> {
    let (apex, dir) = (cone.apex_mm.to_array(), cone.axis.direction.to_array());
    let tan = libm::tan(cone.semi_angle_rad);
    let [inner, outer] = cone.radii_mm?;
    if !(tan > 0.0 && tan.is_finite()) {
        return None;
    }
    let centre = |radius: f64| add(apex, scale(dir, radius / tan));
    Some(if frame.on_axis(apex, dir) {
        let at = |radius: f64| frame.station(centre(radius));
        Shape::Taper {
            lo: (at(inner), inner),
            hi: (at(outer), outer),
        }
    } else {
        Shape::Reach(frame.circles(&[(centre(inner), dir, inner), (centre(outer), dir, outer)]))
    })
}

/// A torus: on the axis, by its tube's reach; off it, within the cylinder
/// round it.
fn torus_shape(frame: &Frame, torus: &crate::features::Torus) -> Shape {
    let (centre, dir) = (
        torus.axis.point_mm.to_array(),
        torus.axis.direction.to_array(),
    );
    let (big, small) = (torus.major_radius_mm, torus.minor_radius_mm);
    if frame.on_axis(centre, dir) {
        let at = frame.station(centre);
        return Shape::Reach(Reach {
            from: at - small,
            to: at + small,
            near: (big - small).max(0.0),
            far: big + small,
        });
    }
    Shape::Reach(frame.circles(&[
        (add(centre, scale(dir, -small)), dir, big + small),
        (add(centre, scale(dir, small)), dir, big + small),
    ]))
}

/// What of the stack a requirement anchors may stand in its cores: its
/// cylinders' faces, and its transitions on the safe side of its
/// cylinders (X7: outside a hole's narrowest, inside a shaft's widest);
/// and where its root or bottom lies.
struct Stack {
    own: BTreeSet<u64>,
    /// The stations of its root and bottom, with their features.
    closed: Vec<(f64, FeatureId)>,
}

impl Own<'_> {
    fn stack(&self, feature: FeatureId, frame: &Frame, hole: bool) -> Option<Stack> {
        let part = self.part;
        let (point, dir, segments, transitions, ends) =
            if let Some(h) = part.holes.iter().find(|h| h.id == feature) {
                (
                    h.axis.point_mm,
                    h.axis.direction,
                    &h.segments,
                    &h.transitions,
                    &h.ends,
                )
            } else {
                let s = part.shafts.iter().find(|s| s.id == feature)?;
                (
                    s.axis.point_mm,
                    s.axis.direction,
                    &s.segments,
                    &s.transitions,
                    &s.ends,
                )
            };
        let (p, d) = (point.to_array(), dir.to_array());
        let at = |s: f64| frame.station(add(p, scale(d, s)));
        // Each segment's stations and radius, where both its ends are
        // known.
        let walls: Vec<(f64, f64, f64)> = segments
            .iter()
            .filter_map(|g| {
                let (a, b) = (at(g.from_mm?), at(g.to_mm?));
                Some((a.min(b), a.max(b), g.diameter_mm / 2.0))
            })
            .collect();
        let mut own: BTreeSet<u64> = BTreeSet::new();
        let cylinders = segments.iter().map(|s| (s.cylinder, true));
        let tapers = transitions.iter().map(|t| (*t, false));
        for (id, cylinder) in cylinders.chain(tapers) {
            for &k in self.of.get(&id).into_iter().flatten() {
                let Some(&(face, _)) = self.faces.get(k) else {
                    continue;
                };
                if cylinder || self.safe(k, frame, hole, &walls) {
                    own.insert(face);
                }
            }
        }
        let closed = ends
            .iter()
            .filter_map(|e| match e {
                StackEnd::Bottom {
                    at_mm, closed_by, ..
                } => Some((*at_mm, *closed_by)),
                StackEnd::Root {
                    at_mm, rises_from, ..
                } => Some((*at_mm, *rises_from)),
                _ => None,
            })
            .map(|(at, face)| (frame.station(add(p, scale(d, at))), face))
            .collect();
        Some(Stack { own, closed })
    }
}

/// A face's stations and its least and greatest distance from an axis, and
/// for a cone on the axis its radius at two stations, straight between.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FaceReach {
    /// Its lowest station.
    pub(crate) from: f64,
    /// Its highest.
    pub(crate) to: f64,
    /// Its least distance from the axis.
    pub(crate) near: f64,
    /// Its greatest.
    pub(crate) far: f64,
    /// A cone on the axis: (station, radius) at its two circles.
    pub(crate) taper: Option<((f64, f64), (f64, f64))>,
}

impl Own<'_> {
    /// Face `k`'s stations and reach in `frame`: `None` where nothing bounds
    /// it.
    pub(crate) fn reach(&self, k: usize, frame: &Frame) -> Option<FaceReach> {
        let reach = |(from, to): (f64, f64), (near, far): (f64, f64)| FaceReach {
            from,
            to,
            near,
            far,
            taper: None,
        };
        match self.shape(k, frame) {
            Shape::Square { at, .. } => self.square_reach(k, frame).map(|r| reach((at, at), r)),
            Shape::Reach(r) => Some(reach((r.from, r.to), (r.near, r.far))),
            Shape::Taper { lo, hi } => Some(FaceReach {
                taper: Some((lo, hi)),
                ..reach(
                    (lo.0.min(hi.0), lo.0.max(hi.0)),
                    (lo.1.min(hi.1), lo.1.max(hi.1)),
                )
            }),
            Shape::Polygon(ref points) => {
                let stations = points.iter().map(|p| p.0);
                let range = stations.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), s| {
                    (a.min(s), b.max(s))
                });
                Some(reach(range, polygon_reach(points)))
            }
            Shape::Anywhere => None,
        }
    }

    /// The indices of feature `feature`'s faces.
    pub(crate) fn faces_of(&self, feature: FeatureId) -> &[usize] {
        self.of.get(&feature).map_or(&[], Vec::as_slice)
    }

    /// Face `k`'s `#id`.
    pub(crate) fn face(&self, k: usize) -> Option<u64> {
        self.faces.get(k).map(|f| f.0)
    }

    /// X7: a transition's face on the safe side of the cylinder that stands
    /// for its zone: the narrower of the two segments it joins for a hole,
    /// the wider for a shaft, or at an end the end segment. `walls` are the
    /// stack's segments (stations, radius) whose ends are known; with no
    /// such segment beside it, it is not safe.
    fn safe(&self, k: usize, frame: &Frame, hole: bool, walls: &[(f64, f64, f64)]) -> bool {
        let Some(FaceReach {
            from,
            to,
            near,
            far,
            ..
        }) = self.reach(k, frame)
        else {
            return false;
        };
        let wide = walls.iter().map(|w| w.2).fold(1.0, f64::max);
        let slack = 1e-6 * wide;
        // The segments ending where it begins, and beginning where it ends.
        let before = walls
            .iter()
            .filter(|w| w.1 <= from + slack)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        let after = walls
            .iter()
            .filter(|w| w.0 >= to - slack)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let radii = before.into_iter().chain(after).map(|w| w.2);
        let bound = if hole {
            radii.fold(f64::INFINITY, f64::min)
        } else {
            radii.fold(f64::NEG_INFINITY, f64::max)
        };
        if !bound.is_finite() {
            return false;
        }
        if hole {
            near >= bound - slack
        } else {
            far <= bound + slack
        }
    }
}

impl Own<'_> {
    /// VR22 on a candidate: the first face not `exempt` that may meet
    /// `core` about `frame`'s axis, with its `#id` and its distance from
    /// the axis (NaN where nothing bounds it). Every face tested and every
    /// edge examined counts toward `pairs`, which may not pass `limit`.
    pub(crate) fn intrusion(
        &self,
        frame: &Frame,
        core: &Core,
        exempt: &BTreeSet<u64>,
        (pairs, limit): (&mut u64, u64),
    ) -> Result<Option<(u64, f64)>, super::rules::TooMany> {
        for k in self.candidates(frame, core) {
            *pairs += 1;
            if *pairs > limit {
                return Err(super::rules::TooMany);
            }
            let Some(face) = self.face(k) else {
                continue;
            };
            if exempt.contains(&face) {
                continue;
            }
            let hit = match self.shape(k, frame) {
                Shape::Square { at, tilt } => {
                    self.square_hit(k, frame, core, (at, tilt), (&mut *pairs, limit))?
                }
                shape => core.intruded(&shape),
            };
            if let Some(distance) = hit {
                return Ok(Some((face, distance)));
            }
        }
        Ok(None)
    }
}

/// One span's core: its radius, which side of it is held clear, and its
/// stations, each end open or closed (a root's or bottom's, exclusive).
pub(crate) struct Core {
    /// Its radius.
    pub(crate) rho: f64,
    /// True for a bore's: the inside is held clear; else a shaft's outside.
    pub(crate) hole: bool,
    /// Where it starts, and whether that end is exclusive.
    pub(crate) from: (f64, bool),
    /// Where it ends.
    pub(crate) to: (f64, bool),
}

impl Core {
    /// True if stations `a` to `b` reach into the core's.
    fn meets(&self, a: f64, b: f64) -> bool {
        let (lo, hi) = (a.min(b), a.max(b));
        let after = if self.from.1 {
            hi > self.from.0
        } else {
            hi >= self.from.0
        };
        let before = if self.to.1 {
            lo < self.to.0
        } else {
            lo <= self.to.0
        };
        after && before
    }

    /// Where a face of `shape` may hold material inside the core, the edge
    /// counting: its distance from the axis there (its least for a bore,
    /// its greatest for a shaft; NaN where nothing bounds it), or `None` if
    /// it stays clear.
    fn intruded(&self, shape: &Shape) -> Option<f64> {
        let radial = |near: f64, far: f64| {
            if self.hole {
                (near <= self.rho).then_some(near)
            } else {
                (far >= self.rho).then_some(far)
            }
        };
        match *shape {
            // A square plane's region is asked by `Own::square_hit`.
            Shape::Anywhere | Shape::Square { .. } => Some(f64::NAN),
            Shape::Reach(r) => {
                if self.meets(r.from, r.to) {
                    radial(r.near, r.far)
                } else {
                    None
                }
            }
            Shape::Polygon(ref points) => {
                let clipped = slab(points, self.from.0, self.to.0);
                if clipped.is_empty() {
                    return None;
                }
                let (near, far) = polygon_reach(&clipped);
                radial(near, far)
            }
            Shape::Taper { lo, hi } => {
                // Clipped to the core's stations, the cone's radius runs
                // straight between its values there.
                let (a, b) = if lo.0 <= hi.0 { (lo, hi) } else { (hi, lo) };
                if !self.meets(a.0, b.0) {
                    return None;
                }
                if b.0 <= a.0 {
                    return radial(a.1.min(b.1), a.1.max(b.1));
                }
                let at = |s: f64| a.1 + (b.1 - a.1) * ((s - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
                let (x, y) = (at(a.0.max(self.from.0)), at(b.0.min(self.to.0)));
                radial(x.min(y), x.max(y))
            }
        }
    }
}

/// U11: flags every bore and shaft whose span's core the original's own
/// material reaches. `guard` is the target's own (B1); every face tested
/// counts toward `pairs` (check 39), whose limit ends the search.
pub(crate) fn flag(
    requirements: &mut [Requirement],
    own: &Own<'_>,
    guard: f64,
    pairs: &mut u64,
) -> Result<(), super::rules::TooMany> {
    for requirement in requirements.iter_mut() {
        if requirement.flags.contains(&Flag::CoreNotClear) {
            continue;
        }
        if intruded(requirement, own, guard, pairs)? {
            requirement.flags.push(Flag::CoreNotClear);
            requirement.flags.sort_unstable();
        }
    }
    Ok(())
}

/// The cores of one requirement's spans, as (core, stations) to test.
fn cores(requirement: &Requirement, guard: f64) -> Option<(FeatureId, bool, Frame, Vec<Core>)> {
    let tau = super::CONTACT_MM.max(requirement.floor_mm).max(guard);
    let core = |hole: bool, from: f64, to: f64, rho: f64| Core {
        rho,
        hole,
        from: (from - REACH_MM, false),
        to: (to + REACH_MM, false),
    };
    let (feature, hole, axis, cores) = match &requirement.requires {
        Requires::Bore {
            hole, axis, spans, ..
        } => (
            *hole,
            true,
            axis,
            spans
                .iter()
                .filter_map(|s| {
                    let rho = if s.pinned || s.basis == SpanBasis::Zone {
                        s.hole_diameter_mm / 2.0 - super::CONTACT_MM
                    } else {
                        s.clear_diameter_mm? / 2.0 + tau + guard
                    };
                    Some(core(true, s.from_mm, s.to_mm, rho))
                })
                .collect(),
        ),
        Requires::Shaft {
            shaft, axis, spans, ..
        } => (
            *shaft,
            false,
            axis,
            spans
                .iter()
                .filter_map(|s| {
                    let rho = if s.pinned || s.basis == SpanBasis::Zone {
                        s.shaft_diameter_mm / 2.0 + super::CONTACT_MM
                    } else {
                        s.clear_diameter_mm? / 2.0 - tau - guard
                    };
                    Some(core(false, s.from_mm, s.to_mm, rho))
                })
                .collect(),
        ),
        _ => return None,
    };
    Some((
        feature,
        hole,
        Frame::new(axis.point_mm.to_array(), axis.direction.to_array()),
        cores,
    ))
}

fn intruded(
    requirement: &Requirement,
    own: &Own<'_>,
    guard: f64,
    pairs: &mut u64,
) -> Result<bool, super::rules::TooMany> {
    let Some((feature, hole, frame, cores)) = cores(requirement, guard) else {
        return Ok(false);
    };
    // An undecided cylinder is no stack: its requirement is unverifiable
    // already (U2).
    let Some(stack) = own.stack(feature, &frame, hole) else {
        return Ok(false);
    };
    for mut core in cores {
        // The core stops short of the stack's own root or bottom within
        // reach of its ends, by the guard: the face there is no intrusion,
        // nor a face beside it whose box the vertex offset grows past it.
        for (at, _) in &stack.closed {
            if (at - (core.from.0 + REACH_MM)).abs() <= REACH_MM {
                core.from = (at + guard, true);
            }
            if (at - (core.to.0 - REACH_MM)).abs() <= REACH_MM {
                core.to = (at - guard, true);
            }
        }
        for k in own.candidates(&frame, &core) {
            *pairs += 1;
            if *pairs > super::MAX_RELATIONS {
                return Err(super::rules::TooMany);
            }
            let Some(&(face, _)) = own.faces.get(k) else {
                continue;
            };
            if stack.own.contains(&face) {
                continue;
            }
            let hit = match own.shape(k, &frame) {
                Shape::Square { at, tilt } => {
                    own.square_hit(k, &frame, &core, (at, tilt), (pairs, super::MAX_RELATIONS))?
                }
                shape => core.intruded(&shape),
            };
            if hit.is_some() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

impl Own<'_> {
    /// The faces whose boxes meet the core's box: for a bore, its cylinder;
    /// for a shaft, the part's box between its stations, wherever the
    /// axis runs.
    fn candidates(&self, frame: &Frame, core: &Core) -> Vec<usize> {
        let ends = [
            add(frame.point, scale(frame.dir, core.from.0)),
            add(frame.point, scale(frame.dir, core.to.0)),
        ];
        let query = if core.hole {
            let r = core.rho.max(0.0);
            let reach = |k: usize| r * (1.0 - frame.dir[k] * frame.dir[k]).max(0.0).sqrt();
            let lo = [0, 1, 2].map(|k| ends[0][k].min(ends[1][k]) - reach(k));
            let hi = [0, 1, 2].map(|k| ends[0][k].max(ends[1][k]) + reach(k));
            Some((lo, hi))
        } else {
            // The part's box; between the stations where the axis runs
            // along a coordinate.
            self.part.extent.map(|e| {
                let (mut lo, mut hi) = (e.min_mm.to_array(), e.max_mm.to_array());
                for k in 0..3 {
                    if (frame.dir[k].abs() - 1.0).abs() <= 1e-12 {
                        lo[k] = lo[k].max(ends[0][k].min(ends[1][k]));
                        hi[k] = hi[k].min(ends[0][k].max(ends[1][k]));
                    }
                }
                (lo, hi)
            })
        };
        match query {
            Some(q) => self.tree.meeting(&q, REACH_MM),
            None => (0..self.faces.len()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Core, Shape, arc, cut, polygon_reach, slab};

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9
    }

    /// An arc's least and greatest distance from the origin: its point
    /// toward the origin, and away, when it sweeps over them.
    #[test]
    fn an_arc_s_reach_is_where_it_turns_toward_and_away() {
        // A whole circle of radius 1 about (3, 0): 2 to 4.
        let (low, high) = arc([3.0, 0.0], [4.0, 0.0], 1.0, 2.0 * std::f64::consts::PI);
        assert!(close(low, 2.0) && close(high, 4.0));
        // Its upper half, from (4, 0) counterclockwise: it passes (2, 0)
        // at its end only, and (4, 0) at its start: neither turn inside.
        let (low, high) = arc([3.0, 0.0], [4.0, 0.0], 1.0, std::f64::consts::PI);
        assert!(close(low, 2.0) && close(high, 4.0));
        // A quarter from (3, 1) clockwise to (4, 0): its far point is its
        // end; nothing of it turns toward the origin.
        let (low, high) = arc([3.0, 0.0], [3.0, 1.0], 1.0, -std::f64::consts::FRAC_PI_2);
        assert!(low.is_infinite() && close(high, 4.0));
        // A quarter from (3, 1) counterclockwise to (2, 0): toward it at
        // its end, away from it nowhere.
        let (low, high) = arc([3.0, 0.0], [3.0, 1.0], 1.0, std::f64::consts::FRAC_PI_2);
        assert!(close(low, 2.0) && close(high, 0.0));
        // About the origin: its radius.
        assert_eq!(arc([0.0, 0.0], [2.0, 0.0], 2.0, 1.0), (2.0, 2.0));
    }

    /// A polygon cut to a slab of stations, and its reach from the axis:
    /// zero where the axis passes through it.
    #[test]
    fn a_polygon_is_cut_to_the_core_s_stations() {
        // A square along the axis from station 0 to 10, 2 to 4 across.
        let square = [
            (0.0, [2.0, 0.0]),
            (10.0, [2.0, 0.0]),
            (10.0, [4.0, 0.0]),
            (0.0, [4.0, 0.0]),
        ];
        let cut_square = slab(&square, 3.0, 5.0);
        assert!(cut_square.iter().all(|p| p.0 >= 3.0 && p.0 <= 5.0));
        let (near, far) = polygon_reach(&cut_square);
        assert!(close(near, 2.0) && close(far, 4.0));
        assert_eq!(slab(&square, 11.0, 12.0), Vec::new());
        // A face across the axis, tilted: the axis passes through it.
        let tilted = [
            (0.0, [-1.0, -1.0]),
            (1.0, [1.0, -1.0]),
            (1.0, [1.0, 1.0]),
            (0.0, [-1.0, 1.0]),
        ];
        assert_eq!(polygon_reach(&tilted).0, 0.0);
    }

    /// A plane crossing a box: the box of its section.
    #[test]
    fn a_plane_cuts_a_box_to_its_section() {
        let unit = ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let section = cut(&unit, [0.0, 0.0, 1.0], 0.5);
        assert_eq!(section, Some(([0.0, 0.0, 0.5], [1.0, 1.0, 0.5])));
        assert_eq!(cut(&unit, [0.0, 0.0, 1.0], 2.0), None);
    }

    /// A bore's core is met where a face comes within its radius over its
    /// stations, a shaft's where one reaches out to it; the edge counts,
    /// and a closed end bounds it exclusively.
    #[test]
    fn a_core_is_met_at_its_edge_and_not_past_a_closed_end() {
        let bore = Core {
            rho: 3.0,
            hole: true,
            from: (0.0, true),
            to: (10.0, false),
        };
        let reach = |from: f64, to: f64, near: f64, far: f64| {
            Shape::Reach(super::Reach {
                from,
                to,
                near,
                far,
            })
        };
        assert_eq!(bore.intruded(&reach(2.0, 4.0, 3.0, 5.0)), Some(3.0));
        assert!(bore.intruded(&reach(2.0, 4.0, 3.0 + 1e-9, 5.0)).is_none());
        assert!(bore.intruded(&reach(-2.0, 0.0, 0.0, 5.0)).is_none());
        assert!(bore.intruded(&reach(10.0, 12.0, 0.0, 5.0)).is_some());
        assert!(bore.intruded(&Shape::Anywhere).is_some_and(f64::is_nan));
        let shaft = Core {
            hole: false,
            ..bore
        };
        assert_eq!(shaft.intruded(&reach(2.0, 4.0, 0.0, 3.0)), Some(3.0));
        assert!(shaft.intruded(&reach(2.0, 4.0, 0.0, 2.9)).is_none());
        // A cone on the axis, clipped to the core's stations: from radius
        // 1 at station -10 to 6 at station 10, it is 3.5 at 0.
        let cone = Shape::Taper {
            lo: (-10.0, 1.0),
            hi: (10.0, 6.0),
        };
        assert!(shaft.intruded(&cone).is_some());
        assert!(bore.intruded(&cone).is_none());
    }
}
