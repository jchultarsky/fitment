//! Axial elements and the rules between them (`docs/research/m3-socket.md`,
//! 7.2 to 7.5): X, B, I and J.
// a, b, p, t, n, e and the like are the design's own names for targets,
// neighbours, pairs and stations.
#![allow(clippy::many_single_char_names)]

use std::collections::BTreeMap;

use super::geo::{Motion, V, across, add, cross, dot, norm, scale, sub};
use crate::features::{CylinderClass, Extent, FeatureId, Part, Sense, StackEnd, StackSegment};

/// Whether an element is a hole wall or a shaft wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Hole,
    Shaft,
}

/// The end kinds of a stack that matter here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndKind {
    /// Air beyond: a hole's opening or a shaft's free end.
    Open,
    /// Material beyond: a bottom or a root.
    Closed,
    /// Irregular or unknown.
    Unknown,
}

/// One cylinder of a hole or shaft stack, or an undecided cylinder, with
/// its interval along the stack's axis.
#[derive(Debug, Clone)]
pub(crate) struct Element {
    /// The hole, shaft or undecided cylinder it belongs to.
    pub(crate) feature: FeatureId,
    pub(crate) side: Side,
    /// Its index among its stack's segments.
    pub(crate) segment: usize,
    /// The number of segments in its stack.
    pub(crate) segments: usize,
    pub(crate) point: V,
    pub(crate) dir: V,
    pub(crate) radius: f64,
    /// Stations along `dir` from `point`; infinite on a side whose extent
    /// is unknown, until clipped.
    pub(crate) lo: f64,
    pub(crate) hi: f64,
    /// The cylinder's own stations. `lo` and `hi` reach past them over the
    /// transitions and end zones the element stands for (X7): a hole's
    /// narrower neighbouring segment, or a shaft's wider one, covers a
    /// transition, and an end segment reaches its rim. A chamfer, cone or
    /// fillet there lies inside a hole's or outside a shaft's cylinder.
    pub(crate) own_lo: f64,
    pub(crate) own_hi: f64,
    /// What is known of it: its cylinder and zones, before an unknown side
    /// runs to the part's extent (U3). A fit holds only here.
    pub(crate) known_lo: f64,
    pub(crate) known_hi: f64,
    /// True on a side whose extent is not known (U3).
    pub(crate) lo_unknown: bool,
    pub(crate) hi_unknown: bool,
    pub(crate) undecided: bool,
    /// The largest arc a partial cylinder's stations cover (U1, I7);
    /// `None` for a full cylinder.
    pub(crate) partial: Option<f64>,
    /// The planes the stack's open ends open onto (H1).
    pub(crate) start_into: Option<FeatureId>,
    pub(crate) finish_into: Option<FeatureId>,
    /// The stack's ends, at its lowest and highest station.
    pub(crate) start: EndKind,
    pub(crate) finish: EndKind,
    /// The rims of open ends, as stations.
    pub(crate) start_rim: Option<f64>,
    pub(crate) finish_rim: Option<f64>,
    /// The faces of the whole stack: its cylinders, transitions and end
    /// faces (B8 excludes them when a B relation reads the stack).
    pub(crate) faces: Vec<u64>,
    /// The cylinder's faces.
    pub(crate) wall: Vec<u64>,
}

fn end_kind(end: Option<&StackEnd>) -> (EndKind, Option<f64>) {
    match end {
        Some(StackEnd::Opening { rim_mm, .. } | StackEnd::FreeEnd { rim_mm, .. }) => {
            (EndKind::Open, Some(*rim_mm))
        }
        Some(StackEnd::Bottom { .. } | StackEnd::Root { .. }) => (EndKind::Closed, None),
        _ => (EndKind::Unknown, None),
    }
}

/// The plane a hole's open end opens onto: square to its axis, through
/// its rim. M2 names the face the walk lands on, which for a washer's
/// hole is its outside cylinder across the annulus.
fn opens_into(
    end: Option<&StackEnd>,
    planes: &BTreeMap<FeatureId, &crate::features::Plane>,
    stack: &Stack<'_>,
) -> Option<FeatureId> {
    let Some(StackEnd::Opening { rim_mm, .. }) = end else {
        return None;
    };
    plane_at(planes, stack, *rim_mm)
}

/// The plane square to `stack`'s axis through its station `at`, if one.
fn plane_at(
    planes: &BTreeMap<FeatureId, &crate::features::Plane>,
    stack: &Stack<'_>,
    at: f64,
) -> Option<FeatureId> {
    let point = add(stack.point, scale(stack.dir, at));
    planes
        .values()
        .find(|plane| {
            let n = plane.normal.to_array();
            dot(n, stack.dir).abs() >= 1.0 - 1e-9
                && (dot(n, point) - plane.offset_mm).abs() <= super::CONTACT_MM
        })
        .map(|plane| plane.id)
}

fn end_face(end: Option<&StackEnd>) -> Option<FeatureId> {
    match end {
        Some(StackEnd::Opening { opens_into, .. } | StackEnd::FreeEnd { opens_into, .. }) => {
            Some(*opens_into)
        }
        Some(StackEnd::Bottom { closed_by, .. }) => Some(*closed_by),
        Some(StackEnd::Root { rises_from, .. }) => Some(*rises_from),
        _ => None,
    }
}

/// Every feature's faces, by id.
pub(crate) fn faces_of(part: &Part) -> BTreeMap<FeatureId, Vec<u64>> {
    let mut map = BTreeMap::new();
    for plane in &part.planes {
        map.insert(plane.id, plane.faces.iter().map(|f| f.face).collect());
    }
    for c in &part.cylinders {
        map.insert(c.id, c.faces.clone());
    }
    for c in &part.cones {
        map.insert(c.id, c.faces.clone());
    }
    for t in &part.tori {
        map.insert(t.id, t.faces.clone());
    }
    for o in &part.other_faces {
        map.insert(o.id, vec![o.face]);
    }
    map
}

/// A station of `point` on the axis through `origin` along `dir`.
fn station(point: V, origin: V, dir: V) -> f64 {
    dot(sub(point, origin), dir)
}

struct Stack<'p> {
    feature: FeatureId,
    side: Side,
    point: V,
    dir: V,
    segments: &'p [StackSegment],
    transitions: &'p [FeatureId],
    ends: &'p [StackEnd],
}

/// The elements of `part`, in its own frame.
pub(crate) fn elements(part: &Part) -> Vec<Element> {
    let faces = faces_of(part);
    let stacks = part
        .holes
        .iter()
        .map(|h| Stack {
            feature: h.id,
            side: Side::Hole,
            point: h.axis.point_mm.to_array(),
            dir: h.axis.direction.to_array(),
            segments: &h.segments,
            transitions: &h.transitions,
            ends: &h.ends,
        })
        .chain(part.shafts.iter().map(|s| Stack {
            feature: s.id,
            side: Side::Shaft,
            point: s.axis.point_mm.to_array(),
            dir: s.axis.direction.to_array(),
            segments: &s.segments,
            transitions: &s.transitions,
            ends: &s.ends,
        }));
    let cylinders: BTreeMap<FeatureId, &crate::features::Cylinder> =
        part.cylinders.iter().map(|c| (c.id, c)).collect();
    let planes: BTreeMap<FeatureId, &crate::features::Plane> =
        part.planes.iter().map(|p| (p.id, p)).collect();
    let mut out = Vec::new();
    for stack in stacks {
        out.extend(stack_elements(part, &cylinders, &planes, &faces, &stack));
    }
    out.extend(loose_elements(part));
    out
}

/// The cylinders in no stack: partial ones (U1) and undecided ones (U2),
/// bounded by the part's extent along their axes, or not at all.
fn loose_elements(part: &Part) -> Vec<Element> {
    let mut out = Vec::new();
    for cylinder in &part.cylinders {
        let CylinderClass::Partial { sweep_rad } = cylinder.class else {
            continue;
        };
        if cylinder.shell != 0 {
            continue;
        }
        let point = cylinder.axis.point_mm.to_array();
        let dir = cylinder.axis.direction.to_array();
        // Its stations do not bound it (an axial edge or an unread curve
        // may run past them): the part's extent along its axis does.
        let (lo, hi) = match part.extent.as_ref() {
            Some(extent) => along(extent, point, dir),
            None => (f64::NEG_INFINITY, f64::INFINITY),
        };
        out.push(Element {
            feature: cylinder.id,
            side: if cylinder.sense == Sense::Hole {
                Side::Hole
            } else {
                Side::Shaft
            },
            segment: 0,
            segments: 1,
            point,
            dir,
            radius: cylinder.radius_mm,
            lo,
            hi,
            own_lo: lo,
            own_hi: hi,
            known_lo: lo,
            known_hi: hi,
            lo_unknown: true,
            hi_unknown: true,
            undecided: false,
            partial: Some(sweep_rad.unwrap_or(0.0)),
            start_into: None,
            finish_into: None,
            start: EndKind::Unknown,
            finish: EndKind::Unknown,
            start_rim: None,
            finish_rim: None,
            faces: cylinder.faces.clone(),
            wall: cylinder.faces.clone(),
        });
    }
    for cylinder in &part.cylinders {
        if !matches!(cylinder.class, CylinderClass::Undecided { .. }) {
            continue;
        }
        let point = cylinder.axis.point_mm.to_array();
        let dir = cylinder.axis.direction.to_array();
        // An undecided cylinder's wall may run anywhere its part does: the
        // part's extent along its axis, or with none, its stations.
        let (lo, hi) = match part.extent.as_ref() {
            Some(extent) => along(extent, point, dir),
            None => (f64::NEG_INFINITY, f64::INFINITY),
        };
        out.push(Element {
            feature: cylinder.id,
            side: if cylinder.sense == Sense::Hole {
                Side::Hole
            } else {
                Side::Shaft
            },
            segment: 0,
            segments: 1,
            point,
            dir,
            radius: cylinder.radius_mm,
            lo,
            hi,
            own_lo: lo,
            own_hi: hi,
            known_lo: lo,
            known_hi: hi,
            lo_unknown: true,
            hi_unknown: true,
            undecided: true,
            partial: None,
            start_into: None,
            finish_into: None,
            start: EndKind::Unknown,
            finish: EndKind::Unknown,
            start_rim: None,
            finish_rim: None,
            faces: cylinder.faces.clone(),
            wall: cylinder.faces.clone(),
        });
    }
    out
}

/// The stations `extent`'s corners project to on the axis through `point`
/// along `dir`.
fn along(extent: &Extent, point: V, dir: V) -> (f64, f64) {
    let (low, high) = (extent.min_mm.to_array(), extent.max_mm.to_array());
    let mut out = (f64::INFINITY, f64::NEG_INFINITY);
    for i in 0..8 {
        let corner = [
            if i & 1 == 0 { low[0] } else { high[0] },
            if i & 2 == 0 { low[1] } else { high[1] },
            if i & 4 == 0 { low[2] } else { high[2] },
        ];
        let s = station(corner, point, dir);
        out = (out.0.min(s), out.1.max(s));
    }
    out
}

fn range(stations: &[f64]) -> (f64, f64) {
    let lo = stations.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = stations.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if lo <= hi { (lo, hi) } else { (0.0, 0.0) }
}

fn stack_elements(
    part: &Part,
    cylinders: &BTreeMap<FeatureId, &crate::features::Cylinder>,
    planes: &BTreeMap<FeatureId, &crate::features::Plane>,
    faces: &BTreeMap<FeatureId, Vec<u64>>,
    stack: &Stack<'_>,
) -> Vec<Element> {
    let (start, start_rim) = end_kind(stack.ends.first());
    let (finish, finish_rim) = end_kind(stack.ends.get(1));
    let mut all_faces: Vec<u64> = Vec::new();
    let mut take = |id: &FeatureId| {
        if let Some(f) = faces.get(id) {
            all_faces.extend(f);
        }
    };
    for segment in stack.segments {
        take(&segment.cylinder);
    }
    for transition in stack.transitions {
        take(transition);
    }
    for end in stack.ends {
        if let Some(face) = end_face(Some(end)) {
            take(&face);
        }
    }
    all_faces.sort_unstable();
    all_faces.dedup();
    let count = stack.segments.len();
    let mut out: Vec<Element> = stack
        .segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let wall = faces.get(&segment.cylinder).cloned().unwrap_or_default();
            // A side with no stated end runs past its end circles onto
            // curves no rule reads: the cylinder's stations bound what is
            // known of it.
            let known = cylinders.get(&segment.cylinder).map_or((0.0, 0.0), |c| {
                let along = |s: f64| {
                    station(
                        add(
                            c.axis.point_mm.to_array(),
                            scale(c.axis.direction.to_array(), s),
                        ),
                        stack.point,
                        stack.dir,
                    )
                };
                let (a, b) = range(&c.stations_mm);
                (along(a).min(along(b)), along(a).max(along(b)))
            });
            let first = index == 0;
            let last = index + 1 == count;
            let lo_unknown = segment.from_mm.is_none() || (first && start == EndKind::Unknown);
            let hi_unknown = segment.to_mm.is_none() || (last && finish == EndKind::Unknown);
            let lo = segment.from_mm.unwrap_or(known.0);
            let hi = segment.to_mm.unwrap_or(known.1);
            Element {
                feature: stack.feature,
                side: stack.side,
                segment: index,
                segments: count,
                point: stack.point,
                dir: stack.dir,
                radius: segment.diameter_mm / 2.0,
                lo,
                hi,
                own_lo: lo,
                own_hi: hi,
                known_lo: lo,
                known_hi: hi,
                lo_unknown,
                hi_unknown,
                undecided: false,
                partial: None,
                start_into: if first {
                    opens_into(stack.ends.first(), planes, stack)
                } else {
                    None
                },
                finish_into: if last {
                    opens_into(stack.ends.get(1), planes, stack)
                } else {
                    None
                },
                start,
                finish,
                start_rim,
                finish_rim,
                faces: all_faces.clone(),
                wall,
            }
        })
        .collect();
    steps(&mut out, planes, stack);
    let zones = Zones(transition_ranges(part, stack));
    stretch(&mut out, stack.side, &zones);
    for element in &mut out {
        element.known_lo = element.lo;
        element.known_hi = element.hi;
    }
    out
}

/// A transition's stations along its stack's axis and its radii, where
/// known.
struct Range {
    lo: f64,
    hi: f64,
    radii: Option<(f64, f64)>,
}

/// The cones and tori of a stack, as ranges (planes are steps between
/// segments, of no length).
fn transition_ranges(part: &Part, stack: &Stack<'_>) -> Vec<Range> {
    let mut out = Vec::new();
    for id in stack.transitions {
        if let Some(cone) = part.cones.iter().find(|c| c.id == *id) {
            let apex = station(cone.apex_mm.to_array(), stack.point, stack.dir);
            let widens = dot(cone.axis.direction.to_array(), stack.dir).signum();
            let tan = libm::tan(cone.semi_angle_rad);
            out.push(match cone.radii_mm {
                Some([a, b]) if tan > 0.0 && tan.is_finite() => {
                    let (x, y) = (apex + widens * a / tan, apex + widens * b / tan);
                    Range {
                        lo: x.min(y),
                        hi: x.max(y),
                        radii: Some((a.min(b), a.max(b))),
                    }
                }
                _ => Range {
                    lo: f64::NEG_INFINITY,
                    hi: f64::INFINITY,
                    radii: None,
                },
            });
        } else if let Some(torus) = part.tori.iter().find(|t| t.id == *id) {
            let centre = station(torus.axis.point_mm.to_array(), stack.point, stack.dir);
            let (major, minor) = (torus.major_radius_mm, torus.minor_radius_mm);
            out.push(Range {
                lo: centre - minor,
                hi: centre + minor,
                radii: Some(((major - minor).max(0.0), major + minor)),
            });
        }
    }
    out
}

/// What lies between a stack's segments and past its ends.
struct Zones(Vec<Range>);

impl Zones {
    /// True if every cone or torus that may reach into the stations `lo`
    /// to `hi` lies outside a hole's cylinder of `radius`, or inside a
    /// shaft's: the element may stand for that zone.
    fn covered(&self, lo: f64, hi: f64, side: Side, radius: f64) -> bool {
        let slack = 1e-6 * radius.max(1.0);
        self.0
            .iter()
            .filter(|r| r.hi > lo + slack && r.lo < hi - slack)
            .all(|r| match (r.radii, side) {
                (Some((low, _)), Side::Hole) => low >= radius - slack,
                (Some((_, high)), Side::Shaft) => high <= radius + slack,
                (None, _) => false,
            })
    }
}

/// H1 inside a stack: where one segment ends and the next begins at one
/// station, an annulus plane among the transitions there is the plane
/// each opens onto (a counterbore's floor).
fn steps(
    elements: &mut [Element],
    planes: &BTreeMap<FeatureId, &crate::features::Plane>,
    stack: &Stack<'_>,
) {
    for i in 1..elements.len() {
        let (before, after) = elements.split_at_mut(i);
        let (Some(a), Some(b)) = (before.last_mut(), after.first_mut()) else {
            continue;
        };
        if a.hi_unknown || b.lo_unknown || (b.lo - a.hi).abs() > super::CONTACT_MM {
            continue;
        }
        let at = add(stack.point, scale(stack.dir, a.hi));
        let step = stack.transitions.iter().find(|id| {
            planes.get(id).is_some_and(|plane| {
                let n = plane.normal.to_array();
                dot(n, stack.dir).abs() >= 1.0 - 1e-9
                    && (dot(n, at) - plane.offset_mm).abs() <= super::CONTACT_MM
            })
        });
        if let Some(step) = step {
            a.finish_into = Some(*step);
            b.start_into = Some(*step);
        }
    }
}

/// X7: each transition between two segments goes to a hole's narrower
/// segment or a shaft's wider one, and each end segment reaches its rim,
/// when every cone and torus there lies on the safe side of the
/// element's cylinder: a chamfer or countersink does, a countersunk
/// screw's head or a bead does not, and its zone stays uncovered.
fn stretch(elements: &mut [Element], side: Side, zones: &Zones) {
    for i in 1..elements.len() {
        let (before, after) = elements.split_at_mut(i);
        let (Some(a), Some(b)) = (before.last_mut(), after.first_mut()) else {
            continue;
        };
        if a.hi_unknown || b.lo_unknown || b.lo <= a.hi {
            continue;
        }
        let a_takes = match side {
            Side::Hole => a.radius <= b.radius,
            Side::Shaft => a.radius >= b.radius,
        };
        let radius = if a_takes { a.radius } else { b.radius };
        if !zones.covered(a.hi, b.lo, side, radius) {
            continue;
        }
        if a_takes {
            a.hi = b.lo;
        } else {
            b.lo = a.hi;
        }
    }
    if let Some(first) = elements.first_mut() {
        if let (false, Some(rim)) = (first.lo_unknown, first.start_rim) {
            if rim < first.lo && zones.covered(rim, first.lo, side, first.radius) {
                first.lo = rim;
            }
        }
    }
    if let Some(last) = elements.last_mut() {
        if let (false, Some(rim)) = (last.hi_unknown, last.finish_rim) {
            if rim > last.hi && zones.covered(last.hi, rim, side, last.radius) {
                last.hi = rim;
            }
        }
    }
}

/// `element` moved by `motion`, its unknown sides opened to infinity and
/// then closed again by `extent` (moved too), projected on its axis.
pub(crate) fn placed(element: &Element, motion: &Motion, extent: Option<&Extent>) -> Element {
    let mut moved = element.clone();
    moved.point = motion.point(element.point);
    moved.dir = motion.dir(element.dir);
    let bound = extent.map(|e| {
        let (low, high) = (e.min_mm.to_array(), e.max_mm.to_array());
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for i in 0..8 {
            let corner = [
                if i & 1 == 0 { low[0] } else { high[0] },
                if i & 2 == 0 { low[1] } else { high[1] },
                if i & 4 == 0 { low[2] } else { high[2] },
            ];
            let s = station(motion.point(corner), moved.point, moved.dir);
            lo = lo.min(s);
            hi = hi.max(s);
        }
        (lo, hi)
    });
    if element.lo_unknown {
        moved.lo = bound.map_or(f64::NEG_INFINITY, |b| b.0.min(element.lo));
    }
    if element.hi_unknown {
        moved.hi = bound.map_or(f64::INFINITY, |b| b.1.max(element.hi));
    }
    moved
}

/// How two axial elements lie (X1 to X5): `b` seen on `a`'s axis.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pair {
    pub(crate) sine: f64,
    pub(crate) cosine: f64,
    /// `b`'s interval on `a`'s axis, widened or shrunk by its tilt (X2).
    pub(crate) lo: f64,
    pub(crate) hi: f64,
    /// The overlap's ends (crossed for a gap) and its length.
    pub(crate) from: f64,
    pub(crate) to: f64,
    pub(crate) length: f64,
    /// The largest and smallest offset of `b`'s axis from `a`'s over the
    /// overlap, or over the facing ends of a gap (X3).
    pub(crate) e_max: f64,
    pub(crate) e_min: f64,
}

/// `b` on `a`'s axis, or `None` if the axes are not parallel within
/// `sine_limit` (X1). `grow` is +1 to widen `b`'s interval by its tilt
/// (material inside a hole), -1 to shrink it (a hole around a shaft).
pub(crate) fn pair(a: &Element, b: &Element, sine_limit: f64, grow: f64) -> Option<Pair> {
    let c = dot(a.dir, b.dir);
    let sine = norm(cross(a.dir, b.dir));
    if sine > sine_limit || c == 0.0 {
        return None;
    }
    let q0 = station(b.point, a.point, a.dir);
    let at = |t: f64| {
        if t.is_infinite() {
            t * c.signum()
        } else {
            q0 + t * c
        }
    };
    let (s1, s2) = (at(b.lo), at(b.hi));
    let tilt = grow * b.radius * sine;
    let lo = s1.min(s2) - tilt;
    let hi = s1.max(s2) + tilt;
    let from = a.lo.max(lo);
    let to = a.hi.min(hi);
    // The offset of b's axis from a's at a's station s is |u + s v|.
    let qp = sub(b.point, a.point);
    let b_across = across(b.dir, a.dir);
    let u = sub(across(qp, a.dir), scale(b_across, q0 / c));
    let v = scale(b_across, 1.0 / c);
    let offset = |s: f64| norm(add(u, scale(v, s)));
    let (low, high) = (from.min(to), from.max(to));
    let tilted = norm(v) > 1e-12;
    let e_max = if tilted && (low.is_infinite() || high.is_infinite()) {
        f64::INFINITY
    } else if tilted {
        offset(low).max(offset(high))
    } else {
        norm(u)
    };
    let e_min = if tilted {
        let vv = dot(v, v);
        let s = (-dot(u, v) / vv).clamp(low, high);
        offset(if s.is_finite() { s } else { 0.0 })
    } else {
        norm(u)
    };
    Some(Pair {
        sine,
        cosine: c.abs(),
        lo,
        hi,
        from,
        to,
        length: to - from,
        e_max,
        e_min,
    })
}

/// The shortest distance between the two axis segments (for skew
/// relations, U7), treating infinite ends as rays.
pub(crate) fn segment_distance(a: &Element, b: &Element) -> f64 {
    // Sample-free: the closest points of two segments, clamped.
    let clamp_len = |x: f64| x.clamp(-1e12, 1e12);
    let (p0, p1) = (
        add(a.point, scale(a.dir, clamp_len(a.lo))),
        add(a.point, scale(a.dir, clamp_len(a.hi))),
    );
    let (q0, q1) = (
        add(b.point, scale(b.dir, clamp_len(b.lo))),
        add(b.point, scale(b.dir, clamp_len(b.hi))),
    );
    closest(p0, p1, q0, q1)
}

/// The distance between segments `p0 p1` and `q0 q1`.
fn closest(p0: V, p1: V, q0: V, q1: V) -> f64 {
    let d1 = sub(p1, p0);
    let d2 = sub(q1, q0);
    let r = sub(p0, q0);
    let a = dot(d1, d1);
    let e = dot(d2, d2);
    let f = dot(d2, r);
    let (s, t) = if a <= 1e-24 && e <= 1e-24 {
        (0.0, 0.0)
    } else if a <= 1e-24 {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let c = dot(d1, r);
        if e <= 1e-24 {
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = dot(d1, d2);
            let denom = a * e - b * b;
            let mut s = if denom > 1e-24 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            }
            (s, t)
        }
    };
    norm(sub(add(p0, scale(d1, s)), add(q0, scale(d2, t))))
}
