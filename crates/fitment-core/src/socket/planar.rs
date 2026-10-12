//! Contacts between plane faces (C1 to C5), conical seats (Q1) and
//! curved faces touching a target plane that no rule reads (C6), for one
//! target occurrence (`docs/research/m3-socket.md`, 7.7 and 7.8).

// a, b, n, p, t: faces, normals, points and indices, as the design names them.
#![allow(clippy::many_single_char_names)]

use std::collections::{BTreeMap, BTreeSet};

use super::geo::{Motion, V, cross, dot, norm, perpendiculars, scale, sub};
use super::model::Flag;
use super::overlap::{self, P2};
use super::rules::{Neighbour, TooMany};
use super::{CONTACT_MM, MAX_RELATIONS, NEAR_MM, PARALLEL_SINE_REL};
use crate::features::{FeatureId, Outline, Part, Sense};

/// One plane face of the target, in its own frame.
pub(crate) struct TargetFace<'p> {
    pub(crate) plane: FeatureId,
    pub(crate) face: u64,
    pub(crate) normal: V,
    pub(crate) offset: f64,
    pub(crate) outline: &'p Outline,
    /// Its region's over-approximating polygon in its plane's 2D frame;
    /// `None` for an unknown outline.
    polygon: Option<Vec<P2>>,
    /// Its holes, each under-approximated, in the same frame.
    holes: Vec<Vec<P2>>,
    origin: V,
    u: V,
    w: V,
}

/// One contact a target face makes with a neighbour face.
#[derive(Debug, Clone)]
pub(crate) struct Contact {
    pub(crate) plane: FeatureId,
    pub(crate) face: u64,
    pub(crate) neighbour: usize,
    pub(crate) neighbour_face: u64,
    pub(crate) gap: f64,
    pub(crate) flags: BTreeSet<Flag>,
    pub(crate) floor: f64,
}

/// A conical seat (Q1).
#[derive(Debug, Clone)]
pub(crate) struct ConeSeat {
    pub(crate) cone: FeatureId,
    pub(crate) neighbour: usize,
    pub(crate) neighbour_cone: FeatureId,
    pub(crate) radii: Option<[f64; 2]>,
    pub(crate) floor: f64,
}

/// What the plane rules found.
#[derive(Debug, Default)]
pub(crate) struct PlaneFound {
    pub(crate) contacts: Vec<Contact>,
    pub(crate) cones: Vec<ConeSeat>,
    /// Target planes curved neighbour faces touch with no rule to read
    /// them (C6), with those faces.
    pub(crate) unread_contacts: BTreeMap<FeatureId, BTreeSet<u64>>,
}

/// The target's plane faces, with their polygons.
pub(crate) fn target_faces(part: &Part) -> Vec<TargetFace<'_>> {
    let mut out = Vec::new();
    for plane in &part.planes {
        let normal = plane.normal.to_array();
        let (u, w) = perpendiculars(normal);
        for face in &plane.faces {
            let origin = scale(normal, face.offset_mm);
            let polygon = overlap::outer_polygon(&face.outline, normal)
                .map(|points| overlap::flatten(&points, origin, u, w));
            let holes = overlap::hole_polygons(&face.outline, normal)
                .iter()
                .map(|points| overlap::flatten(points, origin, u, w))
                .collect();
            out.push(TargetFace {
                plane: plane.id,
                face: face.face,
                normal,
                offset: face.offset_mm,
                outline: &face.outline,
                polygon,
                holes,
                origin,
                u,
                w,
            });
        }
    }
    out
}

/// The polygon of a neighbour face, moved into the target's frame and
/// flattened into `target`'s plane; `None` for an unknown outline.
fn moved_polygon(
    outline: &Outline,
    normal: V,
    motion: &Motion,
    target: &TargetFace<'_>,
) -> Option<Vec<P2>> {
    let points = overlap::outer_polygon(outline, normal)?;
    let moved: Vec<V> = points.iter().map(|p| motion.point(*p)).collect();
    Some(overlap::flatten(&moved, target.origin, target.u, target.w))
}

/// The holes of a neighbour face, moved and flattened into `target`'s
/// plane.
fn moved_holes(
    outline: &Outline,
    normal: V,
    motion: &Motion,
    target: &TargetFace<'_>,
) -> Vec<Vec<P2>> {
    overlap::hole_polygons(outline, normal)
        .iter()
        .map(|points| {
            let moved: Vec<V> = points.iter().map(|p| motion.point(*p)).collect();
            overlap::flatten(&moved, target.origin, target.u, target.w)
        })
        .collect()
}

/// Points of the target face to measure a gap at: its outline's vertices,
/// or its anchor when the outline is unknown.
fn gap_points(face: &TargetFace<'_>) -> Vec<V> {
    match face.outline {
        Outline::Known { loops } => loops
            .iter()
            .flat_map(|l| l.segments.iter())
            .map(|s| match s {
                crate::features::Segment::Line { from_mm, .. }
                | crate::features::Segment::Arc { from_mm, .. } => from_mm.to_array(),
            })
            .collect(),
        Outline::Unknown { .. } => vec![face.origin],
    }
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

/// C1 to C5, Q1 and C6 between the target and every neighbour in reach.
/// `explained` holds, per neighbour, the faces the axial rules read (C6
/// leaves them out); `pairs` is check 39's count, across the extraction.
pub(crate) fn relate(
    target: &Part,
    faces: &[TargetFace<'_>],
    neighbours: &[Neighbour<'_>],
    explained: &BTreeMap<usize, BTreeSet<u64>>,
    pairs: &mut u64,
) -> Result<PlaneFound, TooMany> {
    let mut found = PlaneFound::default();
    let mut count = Count(*pairs);
    let cos_limit = (1.0 - PARALLEL_SINE_REL * PARALLEL_SINE_REL).sqrt();
    for (n, neighbour) in neighbours.iter().enumerate() {
        if !neighbour.near {
            continue;
        }
        let tau = CONTACT_MM.max(neighbour.floor);
        let band = NEAR_MM.max(tau);
        for plane in &neighbour.part.planes {
            let normal = neighbour.motion.dir(plane.normal.to_array());
            for target in faces {
                count.add(plane.faces.len() as u64)?;
                if dot(normal, target.normal) > -cos_limit {
                    continue;
                }
                for face in &plane.faces {
                    let offset = face.offset_mm + dot(normal, neighbour.motion.point([0.0; 3]));
                    // The gap along the target's normal, at its points: the
                    // smallest in size, the side with more contacts.
                    let denominator = dot(normal, target.normal);
                    let gap = gap_points(target)
                        .into_iter()
                        .map(|p| (offset - dot(normal, p)) / denominator)
                        .min_by(|a, b| a.abs().total_cmp(&b.abs()))
                        .unwrap_or(f64::INFINITY);
                    if gap.is_nan() || gap.abs() > band {
                        continue;
                    }
                    let theirs = moved_polygon(
                        &face.outline,
                        plane.normal.to_array(),
                        &neighbour.motion,
                        target,
                    );
                    let may = match (&target.polygon, &theirs) {
                        (Some(a), Some(b)) => {
                            let mut pairs = 0;
                            // One region inside the other's hole touches
                            // nothing: a pin's end in a plate's hole.
                            let overlaps = overlap::may_overlap(a, b, tau, &mut pairs)
                                && !target
                                    .holes
                                    .iter()
                                    .any(|h| overlap::contained(b, h, tau, &mut pairs))
                                && !moved_holes(
                                    &face.outline,
                                    plane.normal.to_array(),
                                    &neighbour.motion,
                                    target,
                                )
                                .iter()
                                .any(|h| overlap::contained(a, h, tau, &mut pairs));
                            count.add(pairs)?;
                            overlaps
                        }
                        _ => true,
                    };
                    if !may {
                        continue;
                    }
                    let mut flags = BTreeSet::new();
                    if gap > tau {
                        flags.insert(Flag::NearGap);
                    } else if gap < -tau {
                        flags.insert(Flag::Interference);
                    }
                    if target.polygon.is_none() || theirs.is_none() {
                        flags.insert(Flag::UnknownOutline);
                    }
                    if neighbour.selected {
                        flags.insert(Flag::SelfPaired);
                    }
                    found.contacts.push(Contact {
                        plane: target.plane,
                        face: target.face,
                        neighbour: n,
                        neighbour_face: face.face,
                        gap,
                        flags,
                        floor: neighbour.floor,
                    });
                }
            }
        }
        cones(target, neighbour, n, tau, &mut found, &mut count)?;
        unread_contacts(
            faces,
            neighbour,
            n,
            tau,
            explained,
            &found.contacts,
            &found.cones,
            &mut found.unread_contacts,
            &mut count,
        )?;
    }
    *pairs = count.0;
    Ok(found)
}

/// Q1: a target cone and a neighbour cone of opposite sense, coaxial, with
/// one apex and one angle.
fn cones(
    target: &Part,
    neighbour: &Neighbour<'_>,
    n: usize,
    tau: f64,
    found: &mut PlaneFound,
    count: &mut Count,
) -> Result<(), TooMany> {
    for ours in &target.cones {
        for theirs in &neighbour.part.cones {
            count.add(1)?;
            if ours.sense == theirs.sense {
                continue;
            }
            let apex = neighbour.motion.point(theirs.apex_mm.to_array());
            let axis = neighbour.motion.dir(theirs.axis.direction.to_array());
            let sine = norm(cross(axis, ours.axis.direction.to_array()));
            let angle = libm::sin((ours.semi_angle_rad - theirs.semi_angle_rad).abs());
            if norm(sub(apex, ours.apex_mm.to_array())) > tau
                || sine > PARALLEL_SINE_REL
                || angle > PARALLEL_SINE_REL
            {
                continue;
            }
            let radii = match (ours.radii_mm, theirs.radii_mm) {
                (Some([a, b]), Some([c, d])) if a.max(c) <= b.min(d) => Some([a.max(c), b.min(d)]),
                _ => None,
            };
            found.cones.push(ConeSeat {
                cone: ours.id,
                neighbour: n,
                neighbour_cone: theirs.id,
                radii,
                floor: neighbour.floor,
            });
        }
    }
    Ok(())
}

/// C6: a convex curved neighbour face (a shaft, a convex cone or torus)
/// whose box reaches a target plane face within `tau` and may lie over its
/// region, where no rule reads it: neither an axial rule (`explained`) nor
/// a plane contact of the same neighbour with that face. Hole-sense faces
/// press nothing: their material lies beyond them. Plane neighbour faces
/// are C1's; `other` faces are U4's.
#[allow(clippy::too_many_arguments)]
fn unread_contacts(
    faces: &[TargetFace<'_>],
    neighbour: &Neighbour<'_>,
    n: usize,
    tau: f64,
    explained: &BTreeMap<usize, BTreeSet<u64>>,
    contacts: &[Contact],
    cones: &[ConeSeat],
    out: &mut BTreeMap<FeatureId, BTreeSet<u64>>,
    count: &mut Count,
) -> Result<(), TooMany> {
    let mut convex: BTreeSet<u64> = BTreeSet::new();
    // A shaft's cylinder or cone, by face: its axis and largest radius.
    let mut round: BTreeMap<u64, (V, V, f64)> = BTreeMap::new();
    for c in &neighbour.part.cylinders {
        if c.sense == Sense::Shaft {
            convex.extend(c.faces.iter().copied());
            for f in &c.faces {
                round.insert(
                    *f,
                    (
                        c.axis.point_mm.to_array(),
                        c.axis.direction.to_array(),
                        c.radius_mm,
                    ),
                );
            }
        }
    }
    for c in &neighbour.part.cones {
        if c.sense == Sense::Shaft {
            convex.extend(c.faces.iter().copied());
            if let Some([_, r]) = c.radii_mm {
                for f in &c.faces {
                    round.insert(
                        *f,
                        (c.axis.point_mm.to_array(), c.axis.direction.to_array(), r),
                    );
                }
            }
        }
    }
    for t in &neighbour.part.tori {
        if t.convex {
            convex.extend(t.faces.iter().copied());
        }
    }
    let read = explained.get(&n);
    // Q1 reads the cones it seats.
    let seated: BTreeSet<u64> = cones
        .iter()
        .filter(|c| c.neighbour == n)
        .filter_map(|c| {
            neighbour
                .part
                .cones
                .iter()
                .find(|x| x.id == c.neighbour_cone)
        })
        .flat_map(|c| c.faces.iter().copied())
        .collect();
    for face in neighbour.faces {
        if !convex.contains(&face.face)
            || read.is_some_and(|r| r.contains(&face.face))
            || seated.contains(&face.face)
        {
            continue;
        }
        let Some((low, high)) = face.bounds else {
            continue;
        };
        for target in faces {
            count.add(1)?;
            if contacts
                .iter()
                .any(|c| c.neighbour == n && c.face == target.face)
            {
                continue;
            }
            if let Some(axis) = round.get(&face.face) {
                if passes_through(target, neighbour, *axis, tau, count)? {
                    continue;
                }
            }
            if box_reaches(target, neighbour, (low, high), tau) {
                out.entry(target.plane).or_default().insert(face.face);
            }
        }
    }
    Ok(())
}

/// C6: true if a shaft's cylinder or cone, square to the target face,
/// has its whole cross-section in one of the face's holes: it passes
/// through without touching the face.
fn passes_through(
    target: &TargetFace<'_>,
    neighbour: &Neighbour<'_>,
    (point, dir, radius): (V, V, f64),
    tau: f64,
    count: &mut Count,
) -> Result<bool, TooMany> {
    let (p, d) = (neighbour.motion.point(point), neighbour.motion.dir(dir));
    let square = dot(d, target.normal).abs();
    if square < 1.0 - PARALLEL_SINE_REL * PARALLEL_SINE_REL / 2.0 {
        return Ok(false);
    }
    let t = (target.offset - dot(p, target.normal)) / dot(d, target.normal);
    let crossing = super::geo::add(p, scale(d, t));
    let footprint = overlap::flatten(
        &overlap::disc(crossing, target.normal, radius),
        target.origin,
        target.u,
        target.w,
    );
    let mut pairs = 0;
    let through = target
        .holes
        .iter()
        .any(|h| overlap::contained(&footprint, h, tau, &mut pairs));
    count.add(pairs)?;
    Ok(through)
}

/// C6: true if a neighbour face's box, moved into the target's frame,
/// reaches the target face's plane within `tau` and may lie over its
/// region.
fn box_reaches(
    target: &TargetFace<'_>,
    neighbour: &Neighbour<'_>,
    (low, high): (V, V),
    tau: f64,
) -> bool {
    let (mut s, mut uv) = (
        (f64::INFINITY, f64::NEG_INFINITY),
        [[f64::INFINITY, f64::NEG_INFINITY]; 2],
    );
    for i in 0..8 {
        let corner: V = [
            if i & 1 == 0 { low[0] } else { high[0] },
            if i & 2 == 0 { low[1] } else { high[1] },
            if i & 4 == 0 { low[2] } else { high[2] },
        ];
        let p = neighbour.motion.point(corner);
        let along = dot(p, target.normal);
        s = (s.0.min(along), s.1.max(along));
        let d = sub(p, target.origin);
        for (k, axis) in [target.u, target.w].into_iter().enumerate() {
            let x = dot(d, axis);
            uv[k] = [uv[k][0].min(x), uv[k][1].max(x)];
        }
    }
    // Outside the face (beyond its outward normal) and reaching it.
    if s.0 > target.offset + tau || s.1 < target.offset - tau {
        return false;
    }
    target.polygon.as_ref().is_none_or(|polygon| {
        let (lo, hi) = polygon.iter().fold(
            ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]),
            |(lo, hi), p| {
                (
                    [lo[0].min(p[0]), lo[1].min(p[1])],
                    [hi[0].max(p[0]), hi[1].max(p[1])],
                )
            },
        );
        (0..2).all(|k| uv[k][0] <= hi[k] + tau && lo[k] - tau <= uv[k][1])
    })
}

/// True if two target faces' regions may overlap when one is projected
/// along the other's normal (L1); an unknown outline may.
pub(crate) fn faces_may_overlap(a: &TargetFace<'_>, b: &TargetFace<'_>, tau: f64) -> bool {
    let (Some(ours), Some(points)) = (&a.polygon, overlap::outer_polygon(b.outline, b.normal))
    else {
        return true;
    };
    let theirs = overlap::flatten(&points, a.origin, a.u, a.w);
    let mut pairs = 0;
    overlap::may_overlap(ours, &theirs, tau, &mut pairs)
}
