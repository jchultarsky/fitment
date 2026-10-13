//! Contacts between plane faces (C1 to C5) and conical seats (Q1), for
//! one target occurrence (`docs/research/m3-socket.md`, 7.7 and 7.8).

// a, b, n, p, t: faces, normals, points and indices, as the design names them.
#![allow(clippy::many_single_char_names)]

use std::collections::{BTreeMap, BTreeSet};

use super::geo::{Motion, V, cross, dot, norm, perpendiculars, scale, sub};
use super::model::Flag;
use super::overlap::{self, P2};
use super::rules::{Neighbour, TooMany};
use super::{CONTACT_MM, MAX_RELATIONS, NEAR_MM, PARALLEL_SINE_REL};
use crate::features::{FeatureId, Outline, Part};

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
    /// Target features neighbour faces may touch with no rule to read
    /// them (C6, `touch.rs`), with those faces.
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

/// The holes of a neighbour face near the target (N4), moved and
/// flattened into `target`'s plane.
fn moved_holes(
    neighbour: &Neighbour<'_>,
    face: &crate::features::PlaneFace,
    normal: V,
    target: &TargetFace<'_>,
) -> Vec<Vec<P2>> {
    let flat = |points: &Vec<V>| {
        let moved: Vec<V> = points.iter().map(|p| neighbour.motion.point(*p)).collect();
        overlap::flatten(&moved, target.origin, target.u, target.w)
    };
    match neighbour.index {
        Some(index) => index
            .holes_near(face.face, neighbour.region.as_ref())
            .into_iter()
            .map(flat)
            .collect(),
        None => overlap::hole_polygons(&face.outline, normal)
            .iter()
            .map(flat)
            .collect(),
    }
}

/// C1's gap of neighbour plane face `face` (outward `normal` and `offset`
/// in the target's frame) from `target`, along the target's normal: at
/// the target's points and the neighbour's vertices, the smallest in size;
/// 0 where the planes cross between them (a block tilted within the
/// window, resting on one edge).
fn gap_of(
    target: &TargetFace<'_>,
    face: &crate::features::PlaneFace,
    normal: V,
    offset: f64,
    neighbour: &Neighbour<'_>,
) -> f64 {
    let motion = &neighbour.motion;
    let denominator = dot(normal, target.normal);
    let mut gaps: Vec<f64> = gap_points(target)
        .into_iter()
        .map(|p| (offset - dot(normal, p)) / denominator)
        .collect();
    if let Outline::Known { loops } = &face.outline {
        let from = |s: &crate::features::Segment| match s {
            crate::features::Segment::Line { from_mm, .. }
            | crate::features::Segment::Arc { from_mm, .. } => from_mm.to_array(),
        };
        let at = |p: V| dot(motion.point(p), target.normal) - target.offset;
        match neighbour.index {
            // The outer loop's vertices, and the points of the hole loops
            // near the target (N4): a hole far from it says nothing of the
            // gap there.
            Some(index) => {
                let outer = loops.iter().find(|l| l.outer).into_iter();
                gaps.extend(outer.flat_map(|l| l.segments.iter()).map(|s| at(from(s))));
                for hole in index.holes_near(face.face, neighbour.region.as_ref()) {
                    gaps.extend(hole.iter().map(|p| at(*p)));
                }
            }
            None => gaps.extend(
                loops
                    .iter()
                    .flat_map(|l| l.segments.iter())
                    .map(|s| at(from(s))),
            ),
        }
    }
    if gaps.iter().any(|g| *g < 0.0) && gaps.iter().any(|g| *g > 0.0) {
        return 0.0;
    }
    gaps.into_iter()
        .min_by(|a, b| a.abs().total_cmp(&b.abs()))
        .unwrap_or(f64::INFINITY)
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

/// C1 to C5 and Q1 between the target and every neighbour in reach;
/// `pairs` is check 39's count, across the extraction.
pub(crate) fn relate(
    target: &Part,
    faces: &[TargetFace<'_>],
    neighbours: &[Neighbour<'_>],
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
        // N4: the neighbour's faces near the target.
        let kept: BTreeSet<u64> = neighbour.faces.iter().map(|b| b.face).collect();
        for plane in &neighbour.part.planes {
            let near: Vec<&crate::features::PlaneFace> = plane
                .faces
                .iter()
                .filter(|f| kept.contains(&f.face))
                .collect();
            if near.is_empty() {
                continue;
            }
            let normal = neighbour.motion.dir(plane.normal.to_array());
            for target in faces {
                count.add(near.len() as u64)?;
                if dot(normal, target.normal) > -cos_limit {
                    continue;
                }
                for face in near.iter().copied() {
                    let offset = face.offset_mm + dot(normal, neighbour.motion.point([0.0; 3]));
                    let gap = gap_of(target, face, normal, offset, neighbour);
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
                                && !moved_holes(neighbour, face, plane.normal.to_array(), target)
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
            // Apexes apart along the axis move the surfaces by that times
            // the sine of the semi-angle: a shallow taper sits deep.
            let offset = sub(apex, ours.apex_mm.to_array());
            let axis_ours = ours.axis.direction.to_array();
            let along = dot(offset, axis_ours);
            let lateral = norm(sub(offset, scale(axis_ours, along)));
            if lateral > tau
                || (along * libm::sin(ours.semi_angle_rad)).abs() > tau
                || sine > PARALLEL_SINE_REL
                || angle > PARALLEL_SINE_REL
            {
                continue;
            }
            // The range both cover; with theirs unknown, the target's own,
            // which holds wherever they touch.
            let radii = match (ours.radii_mm, theirs.radii_mm) {
                (Some([a, b]), Some([c, d])) if a.max(c) <= b.min(d) => Some([a.max(c), b.min(d)]),
                (Some(own), _) => Some(own),
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
