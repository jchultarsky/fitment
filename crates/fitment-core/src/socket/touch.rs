//! C6: a target face and a neighbour face that may touch in a way no
//! other rule reads (`docs/research/m3-socket.md`, 7.8). Every pair whose
//! sound boxes, grown by τ, meet is a candidate; the pairs C1, Q1, H1 and
//! the axial rules read are explained, with the faces that share an edge
//! with a face C1 reads; closed forms decide what they can (two planes,
//! a plane and a cylinder, two parallel cylinders), and anything else may
//! touch. `other` faces are U4's and U5's.

// a, b, c, d, e, n, p, r, s, t, u, w: points, directions and radii, as the
// design names them.
#![allow(clippy::many_single_char_names)]

use std::collections::{BTreeMap, BTreeSet};

use super::axial::Element;
use super::geo::{Motion, V, across, add, cross, dot, norm, perpendiculars, scale, sub};
use super::overlap::{self, P2};
use super::planar::{ConeSeat, Contact};
use super::rules::{Found, Neighbour, Ring, TooMany};
use super::{CONTACT_MM, MAX_RELATIONS, PARALLEL_SINE_REL};
use crate::features::extent::FaceBox;
use crate::features::{FeatureId, Outline, Part, Segment, Sense};

/// What a face is, for C6, in the target's frame.
#[derive(Debug, Clone)]
enum Surface {
    Plane {
        normal: V,
        offset: f64,
        /// Its outer loop, over-approximated (C2), as 3D points.
        outer: Option<Vec<V>>,
        /// Its holes, under-approximated, as 3D points.
        holes: Vec<Vec<V>>,
    },
    Cylinder {
        point: V,
        dir: V,
        radius: f64,
        sense: Sense,
    },
    Cone {
        apex: V,
        /// The way it widens.
        dir: V,
        semi_angle: f64,
    },
    /// A torus: its box alone.
    Boxed,
}

/// One face of a part, moved into the target's frame.
#[derive(Debug, Clone)]
struct Face {
    id: u64,
    surface: Surface,
    /// The stacks, partial or undecided cylinders it belongs to (a plate's
    /// top face is the end of each of its holes), and its own feature.
    owners: Vec<FeatureId>,
    /// Its own feature.
    feature: FeatureId,
    bounds: Option<(V, V)>,
}

fn moved_bounds(bounds: Option<([f64; 3], [f64; 3])>, motion: &Motion) -> Option<(V, V)> {
    let (low, high) = bounds?;
    let mut out = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for i in 0..8 {
        let p = motion.point([
            if i & 1 == 0 { low[0] } else { high[0] },
            if i & 2 == 0 { low[1] } else { high[1] },
            if i & 4 == 0 { low[2] } else { high[2] },
        ]);
        for ((low, high), x) in out.0.iter_mut().zip(out.1.iter_mut()).zip(p) {
            *low = low.min(x);
            *high = high.max(x);
        }
    }
    Some(out)
}

/// The faces of `part` C6 examines, moved by `motion`; `other` faces are
/// left out.
fn faces(part: &Part, boxes: &[FaceBox], elements: &[Element], motion: &Motion) -> Vec<Face> {
    let bounds: BTreeMap<u64, Option<(V, V)>> = boxes
        .iter()
        .map(|b| (b.face, moved_bounds(b.bounds, motion)))
        .collect();
    let mut owners: BTreeMap<u64, BTreeSet<FeatureId>> = BTreeMap::new();
    for e in elements {
        for f in &e.faces {
            owners.entry(*f).or_default().insert(e.feature);
        }
    }
    let mut out = Vec::new();
    let mut push = |face: u64, surface: Surface, feature: FeatureId| {
        let mut all: Vec<FeatureId> = owners.get(&face).into_iter().flatten().copied().collect();
        all.push(feature);
        out.push(Face {
            id: face,
            surface,
            owners: all,
            feature,
            bounds: bounds.get(&face).copied().flatten(),
        });
    };
    for plane in &part.planes {
        let n = plane.normal.to_array();
        let normal = motion.dir(n);
        for f in &plane.faces {
            let moved = |points: Vec<V>| points.into_iter().map(|p| motion.point(p)).collect();
            push(
                f.face,
                Surface::Plane {
                    normal,
                    offset: dot(normal, motion.point(scale(n, f.offset_mm))),
                    outer: overlap::outer_polygon(&f.outline, n).map(moved),
                    holes: overlap::hole_polygons(&f.outline, n)
                        .into_iter()
                        .map(moved)
                        .collect(),
                },
                plane.id,
            );
        }
    }
    for c in &part.cylinders {
        for f in &c.faces {
            push(
                *f,
                Surface::Cylinder {
                    point: motion.point(c.axis.point_mm.to_array()),
                    dir: motion.dir(c.axis.direction.to_array()),
                    radius: c.radius_mm,
                    sense: c.sense,
                },
                c.id,
            );
        }
    }
    for c in &part.cones {
        for f in &c.faces {
            push(
                *f,
                Surface::Cone {
                    apex: motion.point(c.apex_mm.to_array()),
                    dir: motion.dir(c.axis.direction.to_array()),
                    semi_angle: c.semi_angle_rad,
                },
                c.id,
            );
        }
    }
    for t in &part.tori {
        for f in &t.faces {
            push(*f, Surface::Boxed, t.id);
        }
    }
    out
}

/// The faces that share an edge with each face, as plane outlines state
/// them (both ways).
fn adjacent(part: &Part) -> BTreeMap<u64, BTreeSet<u64>> {
    let by_feature = super::axial::faces_of(part);
    let mut out: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    for plane in &part.planes {
        for f in &plane.faces {
            let Outline::Known { loops } = &f.outline else {
                continue;
            };
            for segment in loops.iter().flat_map(|l| l.segments.iter()) {
                let (Segment::Line { across, .. } | Segment::Arc { across, .. }) = segment;
                for other in by_feature.get(across).into_iter().flatten() {
                    out.entry(f.face).or_default().insert(*other);
                    out.entry(*other).or_default().insert(f.face);
                }
            }
        }
    }
    out
}

/// The pairs other rules read.
#[derive(Default)]
struct Explained {
    /// (target face, neighbour, neighbour face).
    faces: BTreeSet<(u64, usize, u64)>,
    /// (target owner, neighbour face): B8's and U7's, which pin or flag
    /// the whole target feature.
    read: BTreeSet<(FeatureId, u64)>,
}

impl Explained {
    fn holds(&self, t: &Face, n: usize, f: &Face) -> bool {
        self.faces.contains(&(t.id, n, f.id))
            || t.owners.iter().any(|a| self.read.contains(&(*a, f.id)))
    }
}

/// What the other rules found, for C6.
pub(crate) struct Read<'a> {
    pub(crate) targets: &'a [Element],
    pub(crate) axial: &'a Found,
    pub(crate) contacts: &'a [Contact],
    pub(crate) cones: &'a [ConeSeat],
    pub(crate) rings: &'a [Ring],
}

fn explained(target: &Part, neighbours: &[Neighbour<'_>], read: &Read<'_>) -> Explained {
    let mut out = Explained::default();
    let feature_faces = super::axial::faces_of(target);
    // The pairs C1, Q1 and H1 read; Q1's and H1's also over the other faces
    // of both stacks, which meet them only along their shared rims.
    let mut pairs: BTreeSet<(u64, usize, u64)> = BTreeSet::new();
    let mut seated: BTreeSet<(u64, usize, u64)> = BTreeSet::new();
    for c in read.contacts {
        pairs.insert((c.face, c.neighbour, c.neighbour_face));
    }
    for c in read.cones {
        let theirs = neighbours
            .get(c.neighbour)
            .and_then(|n| n.part.cones.iter().find(|x| x.id == c.neighbour_cone));
        for t in feature_faces.get(&c.cone).into_iter().flatten() {
            for f in theirs.iter().flat_map(|x| x.faces.iter()) {
                seated.insert((*t, c.neighbour, *f));
            }
        }
    }
    for r in read.rings {
        for feature in [r.plane, r.hole] {
            for t in feature_faces.get(&feature).into_iter().flatten() {
                for f in &r.faces {
                    seated.insert((*t, r.neighbour, *f));
                }
            }
        }
    }
    let mates = |elements: &[Element], face: u64| -> BTreeSet<u64> {
        elements
            .iter()
            .filter(|e| e.faces.contains(&face))
            .flat_map(|e| e.faces.iter().copied())
            .collect()
    };
    for &(t, n, f) in &seated {
        pairs.insert((t, n, f));
        let theirs = neighbours
            .get(n)
            .map(|x| mates(&x.elements, f))
            .unwrap_or_default();
        for u in mates(read.targets, t).into_iter().chain([t]) {
            for g in theirs.iter().copied().chain([f]) {
                pairs.insert((u, n, g));
            }
        }
    }
    // An axial relation reads its target segment's wall against the
    // neighbour faces it names.
    for r in &read.axial.relations {
        let Some(t) = read.targets.get(r.target) else {
            continue;
        };
        for tw in &t.wall {
            for f in &r.faces {
                pairs.insert((*tw, r.neighbour, *f));
            }
        }
    }
    // And the faces sharing an edge with either face of such a pair: a
    // bolt head's hex sides on the face it seats on, a countersink's rim
    // under a flush head's top.
    let ours = adjacent(target);
    let theirs: Vec<BTreeMap<u64, BTreeSet<u64>>> =
        neighbours.iter().map(|n| adjacent(n.part)).collect();
    for &(t, n, f) in &pairs {
        out.faces.insert((t, n, f));
        for g in theirs.get(n).and_then(|a| a.get(&f)).into_iter().flatten() {
            out.faces.insert((t, n, *g));
        }
        for u in ours.get(&t).into_iter().flatten() {
            out.faces.insert((*u, n, f));
        }
    }
    for r in &read.axial.relations {
        let Some(t) = read.targets.get(r.target) else {
            continue;
        };
        if r.rule == "B8" {
            for f in &r.faces {
                out.read.insert((t.feature, *f));
            }
        }
    }
    for (feature, faces) in &read.axial.skew {
        for f in faces {
            out.read.insert((*feature, *f));
        }
    }
    out
}

struct Count(u64);

impl Count {
    fn add(&mut self, n: u64) -> Result<(), TooMany> {
        self.0 = self.0.saturating_add(n);
        if self.0 > MAX_RELATIONS {
            Err(TooMany)
        } else {
            Ok(())
        }
    }
}

/// C6 for one target occurrence: the target features (planes by plane,
/// curved faces by their stack) with the neighbour faces that may touch
/// them unread. `pairs` is check 39's count.
pub(crate) fn relate(
    target: &Part,
    boxes: &[FaceBox],
    neighbours: &[Neighbour<'_>],
    read: &Read<'_>,
    pairs: &mut u64,
) -> Result<BTreeMap<FeatureId, BTreeSet<u64>>, TooMany> {
    let mut count = Count(*pairs);
    let ours = faces(target, boxes, read.targets, &Motion::IDENTITY);
    let why = explained(target, neighbours, read);
    let mut out: BTreeMap<FeatureId, BTreeSet<u64>> = BTreeMap::new();
    for (n, neighbour) in neighbours.iter().enumerate() {
        if !neighbour.near {
            continue;
        }
        let tau = CONTACT_MM.max(neighbour.floor);
        let theirs = faces(
            neighbour.part,
            neighbour.faces,
            &neighbour.elements,
            &neighbour.motion,
        );
        for f in &theirs {
            for t in &ours {
                count.add(1)?;
                if !boxes_meet(t.bounds, f.bounds, tau) || why.holds(t, n, f) {
                    continue;
                }
                if may_touch(t, f, tau, &mut count)? {
                    let anchor = match t.surface {
                        Surface::Plane { .. } => t.feature,
                        _ => t.owners.first().copied().unwrap_or(t.feature),
                    };
                    out.entry(anchor).or_default().insert(f.id);
                }
            }
        }
    }
    *pairs = count.0;
    Ok(out)
}

fn boxes_meet(a: Option<(V, V)>, b: Option<(V, V)>, tau: f64) -> bool {
    match (a, b) {
        (Some((al, ah)), Some((bl, bh))) => {
            (0..3).all(|i| al[i] <= bh[i] + tau && bl[i] <= ah[i] + tau)
        }
        _ => true,
    }
}

/// A plane's 2D frame: origin, u and w.
fn frame(normal: V, offset: f64) -> (V, V, V) {
    let (u, w) = perpendiculars(normal);
    (scale(normal, offset), u, w)
}

/// True if `region`, a polygon in the plane of `face`, may overlap the
/// face's region and is not wholly in one of its holes.
fn in_region(face: &Face, region: &[V], tau: f64, count: &mut Count) -> Result<bool, TooMany> {
    let Surface::Plane {
        normal,
        offset,
        outer,
        holes,
    } = &face.surface
    else {
        return Ok(true);
    };
    let (origin, u, w) = frame(*normal, *offset);
    let flat = |points: &[V]| -> Vec<P2> { overlap::flatten(points, origin, u, w) };
    let ours = flat(region);
    let Some(outer) = outer else {
        return Ok(true);
    };
    let mut pairs = 0;
    let meets = overlap::may_overlap(&flat(outer), &ours, tau, &mut pairs)
        && !holes
            .iter()
            .any(|h| overlap::contained(&ours, &flat(h), tau, &mut pairs));
    count.add(pairs)?;
    Ok(meets)
}

fn may_touch(t: &Face, f: &Face, tau: f64, count: &mut Count) -> Result<bool, TooMany> {
    match (&t.surface, &f.surface) {
        (Surface::Plane { .. }, Surface::Plane { .. }) => {
            // A common point lies on both planes: a face wholly off the
            // other's plane touches nothing of it, whatever the other's
            // outline.
            if off_plane(t, f, tau) || off_plane(f, t, tau) {
                return Ok(false);
            }
            Ok(edge_on_face(f, t, tau, count)? || edge_on_face(t, f, tau, count)?)
        }
        (Surface::Plane { .. }, Surface::Cylinder { .. }) => plane_cylinder(t, f, tau, count),
        (Surface::Cylinder { .. }, Surface::Plane { .. }) => plane_cylinder(f, t, tau, count),
        (Surface::Cylinder { .. }, Surface::Cylinder { .. }) => Ok(cylinders(t, f, tau)),
        (Surface::Plane { .. }, Surface::Cone { .. }) => plane_cone(t, f, tau, count),
        (Surface::Cone { .. }, Surface::Plane { .. }) => plane_cone(f, t, tau, count),
        (Surface::Cylinder { .. }, Surface::Cone { .. }) => Ok(cylinder_cone(t, f, tau)),
        (Surface::Cone { .. }, Surface::Cylinder { .. }) => Ok(cylinder_cone(f, t, tau)),
        _ => Ok(true),
    }
}

/// Plane faces `edge` and `face`: true if a part of `edge` within τ of
/// `face`'s plane may lie over its region, short of its boundary.
/// Opposed planes are C1's, and planes facing the same way press nothing;
/// a part sunk past τ into the region is a clash of the originals (M7's);
/// a touch along the region's boundary also touches the face across that
/// edge, which is examined in its own right.
fn edge_on_face(edge: &Face, face: &Face, tau: f64, count: &mut Count) -> Result<bool, TooMany> {
    let (
        Surface::Plane {
            normal: m,
            outer: theirs,
            ..
        },
        Surface::Plane {
            normal,
            offset,
            outer,
            holes,
        },
    ) = (&edge.surface, &face.surface)
    else {
        return Ok(true);
    };
    if dot(*m, *normal).abs() >= (1.0 - PARALLEL_SINE_REL * PARALLEL_SINE_REL).sqrt() {
        return Ok(false);
    }
    let Some(points) = theirs else {
        return Ok(true);
    };
    let height = |p: &V| dot(*normal, *p) - offset;
    if points.iter().all(|p| height(p) > tau) || points.iter().all(|p| height(p) < -tau) {
        return Ok(false);
    }
    let onto = |part: Vec<V>| -> Vec<V> {
        part.iter()
            .map(|p| sub(*p, scale(*normal, height(p))))
            .collect()
    };
    let sunk = onto(clip_below(points, |p| height(p) + tau));
    if sunk.len() >= 3 && in_region(face, &sunk, -tau, count)? {
        return Ok(false);
    }
    let slab = clip_below(&clip_below(points, |p| height(p) - tau), |p| {
        -height(p) - tau
    });
    let slab = onto(slab);
    if slab.is_empty() {
        return Ok(false);
    }
    let Some(outer) = outer else {
        return Ok(true);
    };
    let (origin, u, w) = frame(*normal, *offset);
    let ring = overlap::flatten(outer, origin, u, w);
    let flat = overlap::flatten(&slab, origin, u, w);
    let mut pairs = 0;
    let reaches = overlap::reaches_interior(&flat, &ring, tau, &mut pairs)
        && !holes.iter().any(|h| {
            overlap::contained(&flat, &overlap::flatten(h, origin, u, w), tau, &mut pairs)
        });
    count.add(pairs)?;
    Ok(reaches)
}

/// True if plane face `a` lies wholly more than τ to one side of plane
/// face `b`'s plane, by its outline or else its box.
fn off_plane(a: &Face, b: &Face, tau: f64) -> bool {
    let Surface::Plane { normal, offset, .. } = &b.surface else {
        return false;
    };
    let points: Vec<V> = match (&a.surface, a.bounds) {
        (
            Surface::Plane {
                outer: Some(points),
                ..
            },
            _,
        ) => points.clone(),
        (_, Some((low, high))) => (0..8)
            .map(|i| {
                [
                    if i & 1 == 0 { low[0] } else { high[0] },
                    if i & 2 == 0 { low[1] } else { high[1] },
                    if i & 4 == 0 { low[2] } else { high[2] },
                ]
            })
            .collect(),
        _ => return false,
    };
    let height = |p: &V| dot(*normal, *p) - offset;
    points.iter().all(|p| height(p) > tau) || points.iter().all(|p| height(p) < -tau)
}

/// The polygon `points` where `f` ≤ 0 (Sutherland and Hodgman).
fn clip_below(points: &[V], f: impl Fn(&V) -> f64) -> Vec<V> {
    let mut out = Vec::new();
    for (i, p) in points.iter().enumerate() {
        let q = points[(i + 1) % points.len()];
        let (fp, fq) = (f(p), f(&q));
        if fp <= 0.0 {
            out.push(*p);
        }
        if (fp < 0.0 && fq > 0.0) || (fp > 0.0 && fq < 0.0) {
            let t = fp / (fp - fq);
            out.push(add(*p, scale(sub(q, *p), t)));
        }
    }
    out
}

/// The stations a face's box spans along an axis.
fn stations(bounds: Option<(V, V)>, point: V, dir: V) -> (f64, f64) {
    let Some((low, high)) = bounds else {
        return (f64::NEG_INFINITY, f64::INFINITY);
    };
    let mut out = (f64::INFINITY, f64::NEG_INFINITY);
    for i in 0..8 {
        let c = [
            if i & 1 == 0 { low[0] } else { high[0] },
            if i & 2 == 0 { low[1] } else { high[1] },
            if i & 4 == 0 { low[2] } else { high[2] },
        ];
        let s = dot(sub(c, point), dir);
        out = (out.0.min(s), out.1.max(s));
    }
    out
}

/// A plane face and a cylinder face: true if the cylinder may reach the
/// plane within τ over the face's region.
fn plane_cylinder(
    plane: &Face,
    cylinder: &Face,
    tau: f64,
    count: &mut Count,
) -> Result<bool, TooMany> {
    let (
        Surface::Plane { normal, offset, .. },
        Surface::Cylinder {
            point, dir, radius, ..
        },
    ) = (&plane.surface, &cylinder.surface)
    else {
        return Ok(true);
    };
    let (lo, hi) = stations(cylinder.bounds, *point, *dir);
    if !(lo.is_finite() && hi.is_finite()) {
        return Ok(true);
    }
    let along = dot(*dir, *normal);
    let height = dot(*normal, *point) - offset;
    if along.abs() > PARALLEL_SINE_REL {
        // At an angle: its lowest point over its stations, then the box's
        // footprint (a square shaft through a hole of the face is C2's).
        let sine = (1.0 - along * along).max(0.0).sqrt();
        let lowest = (height + along * lo).min(height + along * hi) - radius * sine;
        let highest = (height + along * lo).max(height + along * hi) + radius * sine;
        if lowest > tau || highest < -tau {
            return Ok(false);
        }
        // Square to the plane, its cross-section is a disc where its axis
        // crosses: through one of the face's holes, it touches nothing.
        if along.abs() >= 1.0 - PARALLEL_SINE_REL * PARALLEL_SINE_REL / 2.0 {
            let crossing = add(*point, scale(*dir, -height / along));
            return in_region(
                plane,
                &overlap::disc(crossing, *normal, *radius),
                tau,
                count,
            );
        }
        return footprint(plane, cylinder, tau, count);
    }
    // Parallel: the cylinder meets the slab |h| ≤ τ only where
    // height + r cos φ is within τ: two bands at ±r sin φ beside the axis.
    let low = ((-tau - height) / radius).clamp(-1.0, 1.0);
    let high = ((tau - height) / radius).clamp(-1.0, 1.0);
    if (-tau - height) / radius > 1.0 || (tau - height) / radius < -1.0 {
        return Ok(false);
    }
    let sides = |c: f64| (1.0 - c * c).max(0.0).sqrt() * radius;
    let (near, far) = if low <= 0.0 && high >= 0.0 {
        (sides(low).min(sides(high)), *radius)
    } else {
        let (a, b) = (sides(low), sides(high));
        (a.min(b), a.max(b))
    };
    let foot = sub(*point, scale(*normal, height));
    let lateral = cross(*normal, *dir);
    for sign in [1.0, -1.0] {
        let band = [
            add(add(foot, scale(*dir, lo)), scale(lateral, sign * near)),
            add(add(foot, scale(*dir, hi)), scale(lateral, sign * near)),
            add(add(foot, scale(*dir, hi)), scale(lateral, sign * far)),
            add(add(foot, scale(*dir, lo)), scale(lateral, sign * far)),
        ];
        if in_region(plane, &band, tau, count)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A plane face and a cone face: wholly off the plane by its box, nothing;
/// square to it, its cross-section there (a disc, through one of the
/// face's holes or not); else its box's footprint.
fn plane_cone(plane: &Face, cone: &Face, tau: f64, count: &mut Count) -> Result<bool, TooMany> {
    let (
        Surface::Plane { normal, offset, .. },
        Surface::Cone {
            apex,
            dir,
            semi_angle,
        },
    ) = (&plane.surface, &cone.surface)
    else {
        return Ok(true);
    };
    let Some((low, high)) = cone.bounds else {
        return Ok(true);
    };
    let heights: Vec<f64> = (0..8)
        .map(|i| {
            let c = [
                if i & 1 == 0 { low[0] } else { high[0] },
                if i & 2 == 0 { low[1] } else { high[1] },
                if i & 4 == 0 { low[2] } else { high[2] },
            ];
            dot(*normal, c) - offset
        })
        .collect();
    if heights.iter().all(|h| *h > tau) || heights.iter().all(|h| *h < -tau) {
        return Ok(false);
    }
    let along = dot(*dir, *normal);
    if along.abs() >= 1.0 - PARALLEL_SINE_REL * PARALLEL_SINE_REL / 2.0 {
        // Where its axis crosses the plane, and its widest within τ of it.
        let s = (offset - dot(*normal, *apex)) / along;
        let crossing = add(*apex, scale(*dir, s));
        let tan = libm::tan(*semi_angle);
        let widest = (s.abs() + tau / along.abs()) * tan;
        return in_region(plane, &overlap::disc(crossing, *normal, widest), tau, count);
    }
    footprint(plane, cone, tau, count)
}

/// The footprint of `other`'s box on `plane`'s plane, against its region.
fn footprint(plane: &Face, other: &Face, tau: f64, count: &mut Count) -> Result<bool, TooMany> {
    let (Surface::Plane { normal, offset, .. }, Some((low, high))) = (&plane.surface, other.bounds)
    else {
        return Ok(true);
    };
    let (origin, u, w) = frame(*normal, *offset);
    let mut uv = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    for i in 0..8 {
        let c = [
            if i & 1 == 0 { low[0] } else { high[0] },
            if i & 2 == 0 { low[1] } else { high[1] },
            if i & 4 == 0 { low[2] } else { high[2] },
        ];
        let d = sub(c, origin);
        for (k, axis) in [u, w].into_iter().enumerate() {
            let x = dot(d, axis);
            uv[k] = [uv[k][0].min(x), uv[k][1].max(x)];
        }
    }
    let corner = |a: f64, b: f64| add(origin, add(scale(u, a), scale(w, b)));
    let square = [
        corner(uv[0][0], uv[1][0]),
        corner(uv[0][1], uv[1][0]),
        corner(uv[0][1], uv[1][1]),
        corner(uv[0][0], uv[1][1]),
    ];
    in_region(plane, &square, tau, count)
}

/// Two cylinder faces: parallel, they touch where tangent (outside each
/// other, or one inside the other's hole) over common stations; at an
/// angle, U7 reads a shaft across a hole or shaft, and nothing else
/// presses.
fn cylinders(t: &Face, f: &Face, tau: f64) -> bool {
    let (
        Surface::Cylinder {
            point: p,
            dir: d,
            radius: r,
            sense: ours,
        },
        Surface::Cylinder {
            point: q,
            dir: e,
            radius: s,
            sense: theirs,
        },
    ) = (&t.surface, &f.surface)
    else {
        return true;
    };
    if norm(cross(*d, *e)) > PARALLEL_SINE_REL {
        return false;
    }
    let (a, b) = (stations(t.bounds, *p, *d), stations(f.bounds, *p, *d));
    let common = a.1.min(b.1) - a.0.max(b.0);
    if common < -tau {
        return false;
    }
    let apart = norm(across(sub(*q, *p), *d));
    // Two circles of radii r and s, `apart` between centres: crossing when
    // the distance lies between |r − s| and r + s, which over common
    // stations is a contact (or an interference B reads). Two shafts touch
    // outside each other (r + s); a shaft touches a hole's wall from
    // inside it (|r − s|). A shaft inside another's material is a clash.
    let crossing = apart < r + s - tau && apart > (r - s).abs() + tau && common > tau;
    match (ours, theirs) {
        (Sense::Hole, Sense::Hole) => false,
        (Sense::Shaft, Sense::Shaft) => (apart - (r + s)).abs() <= tau || crossing,
        _ => (apart - (r - s).abs()).abs() <= tau || crossing,
    }
}

/// A cylinder face and a cone face, axes parallel: true if the cone's
/// radius over their common stations may reach the cylinder's surface.
/// At an angle, the boxes decide.
fn cylinder_cone(cylinder: &Face, cone: &Face, tau: f64) -> bool {
    let (
        Surface::Cylinder {
            point, dir, radius, ..
        },
        Surface::Cone {
            apex, semi_angle, ..
        },
    ) = (&cylinder.surface, &cone.surface)
    else {
        return true;
    };
    let Surface::Cone { dir: e, .. } = &cone.surface else {
        return true;
    };
    if norm(cross(*dir, *e)) > PARALLEL_SINE_REL {
        return true;
    }
    let (a, b) = (
        stations(cylinder.bounds, *point, *dir),
        stations(cone.bounds, *point, *dir),
    );
    let (lo, hi) = (a.0.max(b.0) - tau, a.1.min(b.1) + tau);
    if lo > hi || !(lo.is_finite() && hi.is_finite()) {
        return lo <= hi;
    }
    let at = dot(sub(*apex, *point), *dir);
    let tan = libm::tan(*semi_angle);
    let rho = |s: f64| (s - at).abs() * tan;
    let near = if lo <= at && at <= hi {
        0.0
    } else {
        rho(lo).min(rho(hi))
    };
    let far = rho(lo).max(rho(hi));
    let apart = norm(across(sub(*apex, *point), *dir));
    // The cylinder's surface lies between |r − e| and r + e of the cone's
    // axis.
    near <= radius + apart + tau && far >= (radius - apart).abs() - tau
}
