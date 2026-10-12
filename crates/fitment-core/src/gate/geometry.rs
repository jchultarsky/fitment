//! Gate stage 6 on a solid whose geometry read (check 22's first half):
//! checks 22 (the rest), 26, 23, 24 and 25, in that order, on fitment's own
//! copy of the solid. Each needs what the one before established: a value
//! out of range makes the distances of 23 and 24 meaningless, and a vertex
//! off its curve makes 25's arc sweeps meaningless.

use libm::{cos, tan};

use super::{Refusal, RefusalCode};
use crate::features::topo::{
    Curve, Face, Surface, Topo, Travel, cross, dot, length, neg, norm2, radial, sub,
};

/// Values a vertex may lie off its surface or curve by, beyond the stated
/// uncertainty: the floor when a file states almost none, and a relative
/// slack for rounding.
pub(crate) const GATE_SLACK_MM: f64 = 1e-6;
pub(crate) const GATE_SLACK_REL: f64 = 1e-6;

/// The largest coordinate magnitude read (check 26), in mm.
pub(crate) const RANGE_MM: f64 = 1e6;

/// "The file states the same value twice", for telling whether a circle is
/// coaxial with its face (check 25).
pub(crate) const IDENTITY_MM: f64 = 1e-6;

/// Runs the checks. Returns the largest distance of a vertex from its
/// surfaces and curves, in mm.
pub(crate) fn check(topo: &Topo, uncertainty: f64) -> Result<f64, Vec<Refusal>> {
    let refusals = not_geometry(topo);
    if !refusals.is_empty() {
        return Err(refusals);
    }
    let refusals = out_of_range(topo);
    if !refusals.is_empty() {
        return Err(refusals);
    }
    let bound = uncertainty.max(GATE_SLACK_MM) * (1.0 + GATE_SLACK_REL);
    let (largest, refusals) = vertex_offsets(topo, bound);
    if !refusals.is_empty() {
        return Err(refusals);
    }
    let refusals = orientation(topo);
    if refusals.is_empty() {
        Ok(largest)
    } else {
        Err(refusals)
    }
}

/// `x < limit`, false if either is NaN. Every bound in this stage is
/// tested this way round, so that a NaN fails it.
fn below(x: f64, limit: f64) -> bool {
    x < limit
}

/// `x <= limit`, false if either is NaN.
fn at_most(x: f64, limit: f64) -> bool {
    x <= limit
}

/// Check 22, the rest: a surface or curve stepq does not read must still be
/// a surface or curve. stepq reads any other entity as one it does not
/// interpret, so a face whose geometry is a point would otherwise pass as
/// an unread surface.
fn not_geometry(topo: &Topo) -> Vec<Refusal> {
    let named = |entity: &str, kinds: &[&str]| {
        entity
            .split('+')
            .any(|part| kinds.iter().any(|kind| part.contains(kind)))
    };
    let mut refusals = Vec::new();
    let mut seen = Vec::new();
    for face in &topo.faces {
        if let Surface::Other { instance, entity } = &face.surface {
            if !named(entity, &["SURFACE"]) && !seen.contains(instance) {
                seen.push(*instance);
                refusals.push(Refusal::new(
                    RefusalCode::GeometryUnreadable,
                    Some(*instance),
                    format!("#{instance} is a {entity}, not a surface"),
                ));
            }
        }
    }
    for edge in &topo.edges {
        if let Curve::Other { instance, entity } = &edge.curve {
            let kinds = ["CURVE", "ELLIPSE", "HYPERBOLA", "PARABOLA", "POLYLINE"];
            if !named(entity, &kinds) && !seen.contains(instance) {
                seen.push(*instance);
                refusals.push(Refusal::new(
                    RefusalCode::GeometryUnreadable,
                    Some(*instance),
                    format!("#{instance} is a {entity}, not a curve"),
                ));
            }
        }
    }
    refusals
}

/// True unless every value is finite and below [`RANGE_MM`] in magnitude.
fn beyond(values: &[f64]) -> bool {
    values.iter().any(|v| !below(v.abs(), RANGE_MM))
}

/// Check 26: every vertex, every surface's and circle's location and
/// radii, and every line's point lie within [`RANGE_MM`].
pub(crate) fn out_of_range(topo: &Topo) -> Vec<Refusal> {
    let refusal = |instance: u64, what: &str| {
        Refusal::new(
            RefusalCode::OutOfRange,
            Some(instance),
            format!("{what} #{instance} lies beyond the {RANGE_MM} mm read"),
        )
    };
    let mut refusals = Vec::new();
    for vertex in &topo.vertices {
        if beyond(&vertex.point) {
            refusals.push(refusal(vertex.instance, "vertex"));
        }
    }
    for face in &topo.faces {
        let far = match &face.surface {
            Surface::Plane { location, .. } => beyond(location),
            Surface::Cylinder {
                location, radius, ..
            }
            | Surface::Cone {
                location, radius, ..
            }
            | Surface::Sphere {
                location, radius, ..
            } => beyond(location) || beyond(&[*radius]),
            Surface::Torus {
                location,
                major,
                minor,
                ..
            } => beyond(location) || beyond(&[*major, *minor]),
            Surface::Other { .. } => false,
        };
        if far {
            refusals.push(refusal(face.surface.instance(), "surface"));
        }
    }
    for edge in &topo.edges {
        let far = match &edge.curve {
            Curve::Line { point, .. } => beyond(point),
            Curve::Circle { centre, radius, .. } => beyond(centre) || beyond(&[*radius]),
            Curve::Other { .. } => false,
        };
        if far {
            refusals.push(refusal(edge.curve.instance(), "curve"));
        }
    }
    refusals.sort_by_key(|r| r.instance);
    refusals.dedup();
    refusals
}

/// How far `p` lies from `surface`, or `None` for a surface not read.
fn off_surface(surface: &Surface, p: [f64; 3]) -> Option<f64> {
    Some(match surface {
        Surface::Plane { location, z, .. } => dot(sub(p, *location), *z).abs(),
        Surface::Cylinder {
            location,
            z,
            radius,
            ..
        } => (radial(p, *location, *z).0 - radius).abs(),
        Surface::Cone {
            location,
            z,
            radius,
            semi_angle,
            ..
        } => {
            // The cone has radius `radius` at its location and widens along
            // z; the distance is measured perpendicular to its generator.
            let (r, h) = radial(p, *location, *z);
            ((r - (radius + h * tan(*semi_angle))) * cos(*semi_angle)).abs()
        }
        Surface::Sphere {
            location, radius, ..
        } => (length(sub(p, *location)) - radius).abs(),
        Surface::Torus {
            location,
            z,
            major,
            minor,
            ..
        } => {
            let (r, h) = radial(p, *location, *z);
            (norm2(r - major, h) - minor).abs()
        }
        Surface::Other { .. } => return None,
    })
}

/// How far `p` lies from a line or circle, or `None` for another curve.
fn off_curve(curve: &Curve, p: [f64; 3]) -> Option<f64> {
    Some(match curve {
        Curve::Line { point, dir, .. } => radial(p, *point, *dir).0,
        Curve::Circle {
            centre, z, radius, ..
        } => {
            let (r, h) = radial(p, *centre, *z);
            norm2(r - radius, h)
        }
        Curve::Other { .. } => return None,
    })
}

/// Checks 23 and 24: every vertex lies on the surfaces of its faces and the
/// lines and circles of its edges within `bound`. Returns the largest
/// distance measured, and a refusal for each vertex beyond it.
fn vertex_offsets(topo: &Topo, bound: f64) -> (f64, Vec<Refusal>) {
    let mut refusals = Vec::new();
    let mut largest: f64 = 0.0;
    for face in &topo.faces {
        for vertex in topo.face_vertices(face) {
            let Some(v) = topo.vertices.get(vertex) else {
                continue;
            };
            let Some(off) = off_surface(&face.surface, v.point) else {
                continue;
            };
            largest = largest.max(off);
            if !at_most(off, bound) {
                refusals.push(Refusal::new(
                    RefusalCode::VertexOffSurface,
                    Some(v.instance),
                    format!(
                        "vertex #{} lies {off:.3e} mm off face #{}'s surface #{}; the stated uncertainty allows {bound:.3e} mm",
                        v.instance,
                        face.instance,
                        face.surface.instance()
                    ),
                ));
            }
        }
    }
    for edge in &topo.edges {
        for vertex in [edge.start, edge.end] {
            let Some(v) = topo.vertices.get(vertex) else {
                continue;
            };
            let Some(off) = off_curve(&edge.curve, v.point) else {
                continue;
            };
            largest = largest.max(off);
            if !at_most(off, bound) {
                refusals.push(Refusal::new(
                    RefusalCode::VertexOffCurve,
                    Some(v.instance),
                    format!(
                        "vertex #{} lies {off:.3e} mm off edge #{}'s curve #{}; the stated uncertainty allows {bound:.3e} mm",
                        v.instance,
                        edge.instance,
                        edge.curve.instance()
                    ),
                ));
            }
        }
    }
    (largest, refusals)
}

/// Check 25: each face's stated sense agrees with the direction its loops
/// run. A plane face whose outline is lines and circles must have positive
/// net area about its outward normal; a cylinder or cone face's coaxial
/// circles at its lowest and highest stations must run as its sense says.
fn orientation(topo: &Topo) -> Vec<Refusal> {
    let mut refusals = Vec::new();
    for face in &topo.faces {
        let travels = topo.travels(face);
        let disagrees = match &face.surface {
            Surface::Plane { z, .. } => {
                let normal = if face.sense { *z } else { neg(*z) };
                net_area(&travels, normal).is_some_and(|area| !below(0.0, area))
            }
            Surface::Cylinder { location, z, .. } | Surface::Cone { location, z, .. } => {
                circles_disagree(&travels, *location, *z, face.sense)
            }
            _ => false,
        };
        if disagrees {
            refusals.push(orientation_refusal(face));
        }
    }
    refusals
}

fn orientation_refusal(face: &Face) -> Refusal {
    Refusal::new(
        RefusalCode::FaceOrientation,
        Some(face.instance),
        format!(
            "face #{} states same_sense {} but its loops run the other way",
            face.instance,
            if face.same_sense { ".T." } else { ".F." }
        ),
    )
}

/// The net signed area of a plane face's loops projected along `normal`,
/// or `None` if an edge is not a line or circle, or the face has no edge.
///
/// Areas are measured from the face's first vertex, so that a plane
/// placed far from its face loses no digits. A circle's term is its exact
/// projection: a circle standing across the plane, which no valid face
/// has, counts as its chord and cannot turn the sign.
fn net_area(loops: &[Vec<Travel<'_>>], normal: [f64; 3]) -> Option<f64> {
    let origin = loops.iter().flatten().next()?.from;
    loops
        .iter()
        .flatten()
        .map(|travel| travel.area(origin, normal))
        .sum()
}

/// True if a coaxial circle at the lowest or highest station of a cylinder
/// or cone face runs against the face's sense. Along the axis z, the face
/// lies above its lowest circle; its boundary runs counterclockwise about z
/// there exactly when the outward normal points away from the axis (a
/// shaft, `sense` true), and the other way at its highest circle.
///
/// The circles bound the face's extent only if every other edge is a line
/// between their stations; a face with any other edge is not judged.
fn circles_disagree(loops: &[Vec<Travel<'_>>], origin: [f64; 3], z: [f64; 3], sense: bool) -> bool {
    let mut uses = Vec::new();
    let mut lines = Vec::new();
    for travel in loops.iter().flatten() {
        match travel.curve {
            Curve::Circle {
                centre,
                z: n,
                radius,
                ..
            } => {
                let lever = radius.max(1.0);
                let (offset, station) = radial(*centre, origin, z);
                if !(at_most(length(cross(*n, z)) * lever, IDENTITY_MM)
                    && at_most(offset, IDENTITY_MM))
                {
                    return false;
                }
                let ccw = travel.along == (dot(*n, z) > 0.0);
                uses.push((station, ccw));
            }
            Curve::Line { .. } => {
                lines.extend([travel.from, travel.to].map(|p| radial(p, origin, z).1));
            }
            Curve::Other { .. } => return false,
        }
    }
    let lowest = uses.iter().map(|u| u.0).fold(f64::INFINITY, f64::min);
    let highest = uses.iter().map(|u| u.0).fold(f64::NEG_INFINITY, f64::max);
    if uses.is_empty()
        || !below(IDENTITY_MM, highest - lowest)
        || lines
            .iter()
            .any(|&t| !(at_most(lowest - IDENTITY_MM, t) && at_most(t, highest + IDENTITY_MM)))
    {
        return false;
    }
    uses.iter().any(|&(station, ccw)| {
        (station - lowest <= IDENTITY_MM && ccw != sense)
            || (highest - station <= IDENTITY_MM && ccw == sense)
    })
}
