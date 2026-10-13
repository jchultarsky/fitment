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
        /// Its end circles' axis points, both ends known (a full cylinder).
        ends: Option<(V, V)>,
    },
    Cone {
        apex: V,
        /// The way it widens.
        dir: V,
        semi_angle: f64,
        /// Its end circles, centre and radius, where its radii are known:
        /// the face lies between them.
        circles: Option<[(V, f64); 2]>,
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
                    ends: match (&c.class, c.stations_mm.first(), c.stations_mm.last()) {
                        (crate::features::CylinderClass::Full { .. }, Some(a), Some(b)) => {
                            let (p, d) = (c.axis.point_mm.to_array(), c.axis.direction.to_array());
                            Some((
                                motion.point(add(p, scale(d, *a))),
                                motion.point(add(p, scale(d, *b))),
                            ))
                        }
                        _ => None,
                    },
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
                    circles: c.radii_mm.and_then(|radii| {
                        let tan = libm::tan(c.semi_angle_rad);
                        let (apex, d) = (c.apex_mm.to_array(), c.axis.direction.to_array());
                        (tan > 0.0 && tan.is_finite())
                            .then(|| radii.map(|r| (motion.point(add(apex, scale(d, r / tan))), r)))
                    }),
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

/// The stations a face spans along an axis: a full cylinder's by its end
/// circles about that axis (exact, whatever the frame), else its box's.
fn spans(face: &Face, point: V, dir: V) -> (f64, f64) {
    if let Surface::Cylinder {
        ends: Some((x, y)),
        dir: own,
        radius,
        ..
    } = &face.surface
    {
        // The circles' stations, widened by their tilt to this axis.
        let tilt = radius * norm(cross(*own, dir));
        let (u, v) = (dot(sub(*x, point), dir), dot(sub(*y, point), dir));
        return (u.min(v) - tilt, u.max(v) + tilt);
    }
    if let Surface::Cone {
        circles: Some(circles),
        dir: own,
        ..
    } = &face.surface
    {
        let sine = norm(cross(*own, dir));
        return circles
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (c, r)| {
                let s = dot(sub(*c, point), dir);
                (lo.min(s - r * sine), hi.max(s + r * sine))
            });
    }
    stations(face.bounds, point, dir)
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
    // Wholly inside it, short of it by more than τ: a face's distance to
    // the axis is convex, so its outline's corners bound it (a spacer's end
    // inside a tube).
    if let Surface::Plane {
        outer: Some(outer), ..
    } = &plane.surface
    {
        let inside = outer
            .iter()
            .map(|q| norm(across(sub(*q, *point), *dir)))
            .fold(f64::NEG_INFINITY, f64::max);
        if inside < radius - tau {
            return Ok(false);
        }
    }
    let (lo, hi) = spans(cylinder, *point, *dir);
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
        // Within the slab |h| ≤ τ, its cross-sections lie in an ellipse
        // about where its axis crosses: semi-axes r across and r / cos θ
        // along its lean, each section's centre sliding τ tan θ along the
        // lean. The sliding sections lie in the ellipse grown in both
        // semi-axes by the factor 1 + τ sin θ / r, which holds them all.
        // Through one of the face's holes, it touches nothing, in whatever
        // frame its box is taken.
        if !footprint(plane, cylinder, tau, count)? {
            return Ok(false);
        }
        let crossing = add(*point, scale(*dir, -height / along));
        let lean = sub(*dir, scale(*normal, along));
        let lean = if norm(lean) > 1e-12 {
            scale(lean, 1.0 / norm(lean))
        } else {
            perpendiculars(*normal).0
        };
        let long = (radius + tau * sine) / along.abs();
        let short = radius + tau * sine;
        return in_region(
            plane,
            &overlap::ellipse(crossing, *normal, lean, long, short),
            tau,
            count,
        );
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
            ..
        },
    ) = (&plane.surface, &cone.surface)
    else {
        return Ok(true);
    };
    let along = dot(*dir, *normal);
    // Between its end circles, where known: their heights over the plane,
    // and the hull of their outlines as its footprint; else its box.
    if let Surface::Cone {
        circles: Some(circles),
        ..
    } = &cone.surface
    {
        let sine = (1.0 - along * along).max(0.0).sqrt();
        let (low, high) =
            circles
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (c, r)| {
                    let h = dot(*normal, *c) - offset;
                    (lo.min(h - r * sine), hi.max(h + r * sine))
                });
        if low > tau || high < -tau {
            return Ok(false);
        }
        if along.abs() < 1.0 - PARALLEL_SINE_REL * PARALLEL_SINE_REL / 2.0 {
            let (origin, u, w) = frame(*normal, *offset);
            let points: Vec<V> = circles
                .iter()
                .flat_map(|(c, r)| overlap::disc(*c, *dir, *r))
                .map(|p| sub(p, scale(*normal, dot(*normal, p) - offset)))
                .collect();
            let hull: Vec<V> = hull(&overlap::flatten(&points, origin, u, w))
                .into_iter()
                .map(|q| add(origin, add(scale(u, q[0]), scale(w, q[1]))))
                .collect();
            return in_region(plane, &hull, tau, count);
        }
    }
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

/// The convex hull of `points` (Andrew's monotone chain), counterclockwise.
fn hull(points: &[P2]) -> Vec<P2> {
    let mut sorted: Vec<P2> = points.to_vec();
    sorted.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    let turn = |o: P2, a: P2, b: P2| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut out: Vec<P2> = Vec::new();
    for pass in [false, true] {
        let start = out.len();
        let iter: Box<dyn Iterator<Item = &P2>> = if pass {
            Box::new(sorted.iter().rev())
        } else {
            Box::new(sorted.iter())
        };
        for p in iter {
            while out.len() >= start + 2 && turn(out[out.len() - 2], out[out.len() - 1], *p) <= 0.0
            {
                out.pop();
            }
            out.push(*p);
        }
        out.pop();
    }
    out
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
            ..
        },
        Surface::Cylinder {
            point: q,
            dir: e,
            radius: s,
            sense: theirs,
            ..
        },
    ) = (&t.surface, &f.surface)
    else {
        return true;
    };
    if norm(cross(*d, *e)) > PARALLEL_SINE_REL {
        return false;
    }
    let (a, b) = (spans(t, *p, *d), spans(f, *p, *d));
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
    let (a, b) = (spans(cylinder, *point, *dir), spans(cone, *point, *dir));
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

/// What B8's closed forms decide about a face and a hole's core.
pub(crate) enum Core {
    /// It cannot meet it.
    Apart,
    /// It may, over these stations of the hole's axis.
    Meets((f64, f64)),
}

/// A part's faces as B8 sees them, in the target's frame.
pub(crate) struct Shapes(Vec<Face>);

impl Shapes {
    pub(crate) fn of(
        part: &Part,
        boxes: &[FaceBox],
        elements: &[Element],
        motion: &Motion,
    ) -> Self {
        Self(faces(part, boxes, elements, motion))
    }

    /// B8 in closed form: `Some(Core::Apart)` if face `id` cannot meet hole
    /// element `a`'s open core (radius r − τ, stations shrunk by τ),
    /// `Some(Core::Meets(stations))` if it may, with the stations its region spans
    /// along the axis; `None` where only its box can tell (a cone, a torus,
    /// an unknown outline, a cylinder at an angle).
    pub(crate) fn meets_core(&self, id: u64, a: &Element, tau: f64) -> Option<Core> {
        let face = self.0.iter().find(|f| f.id == id)?;
        let core = a.radius - tau;
        let (lo, hi) = (a.lo + tau, a.hi - tau);
        let station = |p: V| dot(sub(p, a.point), a.dir);
        match &face.surface {
            Surface::Plane {
                normal,
                outer: Some(outer),
                holes,
                offset,
            } => {
                let span = outer
                    .iter()
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), p| {
                        let s = station(*p);
                        (l.min(s), h.max(s))
                    });
                let clipped = clip_below(&clip_below(outer, |p| lo - station(*p)), |p| {
                    station(*p) - hi
                });
                if clipped.len() < 3 {
                    return Some(Core::Apart);
                }
                let crossing = dot(*normal, a.dir);
                // Where the axis crosses the face's plane inside the
                // clipped region, the core's section there is a disc
                // (an ellipse within radius core / cos): in one of the
                // face's holes, it meets nothing there.
                if crossing.abs() > 1e-12 {
                    let s = (offset - dot(*normal, a.point)) / crossing;
                    let x = add(a.point, scale(a.dir, s));
                    let (origin, u, w) = frame(*normal, *offset);
                    let flat = |points: &[V]| overlap::flatten(points, origin, u, w);
                    let disc = flat(&overlap::disc(x, *normal, core / crossing.abs()));
                    let mut pairs = 0;
                    if holes
                        .iter()
                        .any(|h| overlap::contained(&disc, &flat(h), tau, &mut pairs))
                    {
                        return Some(Core::Apart);
                    }
                }
                let nearest = nearest_to_line(&clipped, a.point, a.dir, *normal);
                Some(if nearest < core {
                    Core::Meets(span)
                } else {
                    Core::Apart
                })
            }
            // B8 is about material: a shaft's lies inside its surface, and
            // meets the core where their discs overlap (a D-shaft filling a
            // hole with no face inside it); a hole's lies outside, and meets
            // it unless the core is wholly in the hole.
            Surface::Cylinder {
                point,
                dir,
                radius,
                sense,
                ends,
            } if norm(cross(*dir, a.dir)) <= 1e-9 => {
                let apart = norm(across(sub(*point, a.point), a.dir));
                // A full cylinder's stations are its end circles'; else
                // its box's.
                let _ = ends;
                let (l, h) = spans(face, a.point, a.dir);
                if h <= lo || l >= hi {
                    return Some(Core::Apart);
                }
                let meets = match sense {
                    Sense::Shaft => apart < radius + core,
                    // Its material lies just outside its surface: in the
                    // core where the surface reaches it, unless the core
                    // lies wholly in the hole.
                    Sense::Hole => apart > radius - core && apart < radius + core,
                };
                Some(if meets {
                    Core::Meets((l, h))
                } else {
                    Core::Apart
                })
            }
            _ => None,
        }
    }
}

/// The least distance from the line through `point` along unit `dir` to
/// the planar polygon `polygon` (in the plane of unit `normal`): zero if
/// the line crosses it inside.
fn nearest_to_line(polygon: &[V], point: V, dir: V, normal: V) -> f64 {
    let crossing = dot(normal, dir);
    if crossing.abs() > 1e-12 {
        let s = dot(sub(polygon[0], point), normal) / crossing;
        let x = add(point, scale(dir, s));
        let (u, w) = perpendiculars(normal);
        let flat = overlap::flatten(polygon, x, u, w);
        if overlap::inside([0.0, 0.0], &flat) {
            return 0.0;
        }
    }
    let mut best = f64::INFINITY;
    for (i, a) in polygon.iter().enumerate() {
        let b = polygon[(i + 1) % polygon.len()];
        best = best.min(segment_to_line(*a, b, point, dir));
    }
    best
}

/// The least distance from segment `a b` to the line through `p` along
/// unit `d`.
fn segment_to_line(a: V, b: V, p: V, d: V) -> f64 {
    let at = |t: f64| {
        let x = add(a, scale(sub(b, a), t));
        norm(across(sub(x, p), d))
    };
    // The distance is convex along the segment: a ternary search would
    // do, but its square is a quadratic with its least point in closed form.
    let (u, v) = (across(sub(a, p), d), across(sub(b, a), d));
    let vv = dot(v, v);
    let t = if vv > 0.0 {
        (-dot(u, v) / vv).clamp(0.0, 1.0)
    } else {
        0.0
    };
    at(t).min(at(0.0)).min(at(1.0))
}
