//! Gate stages 4 to 6: the solid's topology, its units, and its geometry
//! values (`docs/research/m2-features.md`, section 4.2, checks 13 to 26).
//!
//! Each stage runs every check it has, then refuses if any failed, so that
//! a later stage never judges what an earlier one could not read. Within
//! two stages the checks are ordered too, for the same reason: check 16
//! runs only on a solid that passed check 15, and in stage 6 the geometry
//! must read (22) and lie in range (26) before vertices are measured
//! against it (23, 24), and those must pass before faces are oriented (25).

use libm::{atan2, cos, tan};
use stepq::geometry::{Curve, Reader, Surface};
use stepq::model::Graph;
use stepq::topology::{Loop, Solid, TopologyError};
use stepq::units::{ContextUnits, UnitKind};

use super::{Refusal, RefusalCode};
use crate::features::{SolidUnits, StatedUncertainty, StatedUnit};

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

/// What stages 4 to 6 establish about the solid.
pub(crate) struct SolidFacts {
    pub(crate) topology: Solid,
    pub(crate) units: SolidUnits,
    pub(crate) max_uncertainty_mm: f64,
    pub(crate) max_vertex_offset_mm: f64,
}

/// Runs stages 4 to 6 on solid `id`.
pub(crate) fn check(graph: &Graph<'_>, id: u64) -> Result<SolidFacts, Vec<Refusal>> {
    let topology = topology(graph, id)?;
    let (units, max_uncertainty_mm) = units(graph, id)?;
    let max_vertex_offset_mm = geometry(graph, &topology, max_uncertainty_mm)?;
    Ok(SolidFacts {
        topology,
        units,
        max_uncertainty_mm,
        max_vertex_offset_mm,
    })
}

// ------------------------------------------------------------ stage 4

/// Checks 13 to 16: the solid reads, every edge is used twice in opposite
/// directions, and each shell is one connected piece.
fn topology(graph: &Graph<'_>, id: u64) -> Result<Solid, Vec<Refusal>> {
    let solid = Solid::read(graph, id).map_err(|error| {
        let (code, instance) = match &error {
            TopologyError::Unsupported { instance, .. } => {
                (RefusalCode::TopologyUnsupported, *instance)
            }
            TopologyError::Malformed { instance, .. } | TopologyError::Missing { instance } => {
                (RefusalCode::TopologyMalformed, *instance)
            }
            TopologyError::UnresolvedReference { from, .. } => {
                (RefusalCode::TopologyMalformed, *from)
            }
            _ => (RefusalCode::TopologyMalformed, id),
        };
        vec![Refusal::new(
            code,
            Some(instance),
            format!("solid #{id} cannot be read: {error}"),
        )]
    })?;
    let mut refusals = Vec::new();
    for (index, edge) in solid.edges().iter().enumerate() {
        let uses = solid.uses(index);
        let shell = |face: usize| solid.faces().get(face).map(|f| f.shell);
        if !matches!(uses, [a, b] if a.forward != b.forward && shell(a.face) == shell(b.face)) {
            refusals.push(Refusal::new(
                RefusalCode::SolidNotClosed,
                Some(edge.instance),
                format!(
                    "edge #{} is used {} times{}; a closed solid uses each edge twice, in opposite directions, within one shell",
                    edge.instance,
                    uses.len(),
                    match uses {
                        [a, b] if a.forward == b.forward => ", both the same way",
                        [_, _] => ", by two shells",
                        _ => "",
                    }
                ),
            ));
        }
    }
    // Check 16 walks faces through their edges' uses, which is linear only
    // once every edge has two: a solid that fails check 15 stops here.
    if !refusals.is_empty() {
        return Err(refusals);
    }
    for shell in solid.shells() {
        let faces = shell.faces.clone();
        let Some(first) = faces.clone().next() else {
            continue;
        };
        let mut reached = vec![false; faces.len()];
        let mut stack = vec![first];
        if let Some(slot) = reached.first_mut() {
            *slot = true;
        }
        while let Some(face) = stack.pop() {
            for next in solid.neighbours(face) {
                if let Some(slot) = next
                    .checked_sub(faces.start)
                    .and_then(|k| reached.get_mut(k))
                {
                    if !*slot {
                        *slot = true;
                        stack.push(next);
                    }
                }
            }
        }
        let count = reached.iter().filter(|&&r| r).count();
        if count != faces.len() {
            refusals.push(Refusal::new(
                RefusalCode::ShellDisconnected,
                Some(shell.instance),
                format!(
                    "shell #{} is in pieces: {count} of its {} faces join the first",
                    shell.instance,
                    faces.len()
                ),
            ));
        }
    }
    if refusals.is_empty() {
        Ok(solid)
    } else {
        Err(refusals)
    }
}

// ------------------------------------------------------------ stage 5

/// Checks 17 to 21: the solid's contexts agree, and their length unit,
/// plane angle unit and a length uncertainty can be read. Returns the
/// units and the largest length uncertainty, in mm.
fn units(graph: &Graph<'_>, id: u64) -> Result<(SolidUnits, f64), Vec<Refusal>> {
    let units = ContextUnits::of_item(graph, id).map_err(|error| {
        vec![Refusal::new(
            RefusalCode::UnitsUnreadable,
            Some(id),
            format!("the units of solid #{id} cannot be read: {error}"),
        )]
    })?;
    let context = units.context;
    let mut refusals = Vec::new();
    let stated = |unit: &stepq::units::Unit| StatedUnit {
        instance: unit.instance,
        name: unit.name.clone(),
        factor: unit.factor,
    };
    let length = units.length().map(stated).map_err(|error| {
        Refusal::new(
            RefusalCode::LengthUnitUnreadable,
            Some(context),
            format!("context #{context} has no readable length unit: {error}"),
        )
    });
    let plane_angle = units.plane_angle().map(stated).map_err(|error| {
        Refusal::new(
            RefusalCode::AngleUnitUnreadable,
            Some(context),
            format!("context #{context} has no readable plane angle unit: {error}"),
        )
    });
    let uncertainty = units.uncertainty().map_err(|error| {
        Refusal::new(
            RefusalCode::UncertaintyUnreadable,
            Some(context),
            format!("an uncertainty of context #{context} cannot be read: {error}"),
        )
    });
    let mut stated_uncertainty = Vec::new();
    let mut largest: Option<f64> = None;
    if let Ok(uncertainties) = &uncertainty {
        for u in *uncertainties {
            if u.unit.kind == UnitKind::Length {
                largest = Some(largest.map_or(u.value, |l| l.max(u.value)));
                stated_uncertainty.push(StatedUncertainty {
                    instance: u.instance,
                    name: u.name.clone(),
                    value_mm: u.value,
                });
            }
        }
        if largest.is_none() {
            refusals.push(Refusal::new(
                RefusalCode::UncertaintyMissing,
                Some(context),
                format!("context #{context} states no length uncertainty"),
            ));
        }
    }
    for result in [&length, &plane_angle] {
        if let Err(refusal) = result {
            refusals.push(refusal.clone());
        }
    }
    if let Err(refusal) = uncertainty {
        refusals.push(refusal);
    }
    match (length, plane_angle, largest) {
        (Ok(length), Ok(plane_angle), Some(largest)) if refusals.is_empty() => Ok((
            SolidUnits {
                context,
                length,
                plane_angle,
                uncertainty: stated_uncertainty,
            },
            largest,
        )),
        _ => Err(refusals),
    }
}

// ------------------------------------------------------------ stage 6

type P = [f64; 3];

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: P, b: P) -> P {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The length of (x, y): `sqrt` is correctly rounded on every platform,
/// where `hypot` is not.
fn norm2(x: f64, y: f64) -> f64 {
    (x * x + y * y).sqrt()
}

fn length(a: P) -> f64 {
    dot(a, a).sqrt()
}

/// The distance from `p` to the axis line through `origin` along unit `z`,
/// and `p`'s station along it.
fn radial(point: P, origin: P, axis: P) -> (f64, f64) {
    let offset = sub(point, origin);
    let station = dot(offset, axis);
    let along = [station * axis[0], station * axis[1], station * axis[2]];
    (length(sub(offset, along)), station)
}

/// How far `p` lies from `surface`, or `None` for a surface not read.
fn off_surface(surface: &Surface, p: P) -> Option<f64> {
    Some(match surface {
        Surface::Plane(s) => dot(sub(p, s.position.location), s.position.z).abs(),
        Surface::Cylindrical(s) => {
            (radial(p, s.position.location, s.position.z).0 - s.radius).abs()
        }
        Surface::Conical(s) => {
            // The cone has radius `radius` at its location and widens along
            // z; the distance is measured perpendicular to its generator.
            let (r, h) = radial(p, s.position.location, s.position.z);
            ((r - (s.radius + h * tan(s.semi_angle))) * cos(s.semi_angle)).abs()
        }
        Surface::Spherical(s) => (length(sub(p, s.position.location)) - s.radius).abs(),
        Surface::Toroidal(s) => {
            let (r, h) = radial(p, s.position.location, s.position.z);
            (norm2(r - s.major_radius, h) - s.minor_radius).abs()
        }
        _ => return None,
    })
}

/// How far `p` lies from a line or circle, or `None` for another curve.
fn off_curve(curve: &Curve, p: P) -> Option<f64> {
    Some(match curve.basis() {
        Curve::Line(c) => radial(p, c.pnt, c.dir.orientation).0,
        Curve::Circle(c) => {
            let (r, h) = radial(p, c.position.location, c.position.z);
            norm2(r - c.radius, h)
        }
        _ => return None,
    })
}

/// Checks 22 to 26 on a solid whose topology and units read. Returns the
/// largest distance of a vertex from its surfaces and curves, in mm.
fn geometry(graph: &Graph<'_>, solid: &Solid, uncertainty: f64) -> Result<f64, Vec<Refusal>> {
    let reader = Reader::for_item(graph, solid.instance).map_err(|error| {
        vec![Refusal::new(
            RefusalCode::UnitsUnreadable,
            Some(solid.instance),
            format!(
                "solid #{} has no units to read its geometry in: {error}",
                solid.instance
            ),
        )]
    })?;
    let mut refusals = Vec::new();
    let unreadable = |error: stepq::geometry::GeometryError| {
        Refusal::new(
            RefusalCode::GeometryUnreadable,
            Some(error.instance),
            format!("{error}"),
        )
    };
    let points: Vec<Option<P>> = solid
        .vertices()
        .iter()
        .map(|v| {
            reader
                .point(v.point)
                .map_err(|e| refusals.push(unreadable(e)))
                .ok()
        })
        .collect();
    let surfaces: Vec<Option<Surface>> = solid
        .faces()
        .iter()
        .map(|f| {
            reader
                .surface(f.surface)
                .map_err(|e| refusals.push(unreadable(e)))
                .ok()
        })
        .collect();
    let curves: Vec<Option<Curve>> = solid
        .edges()
        .iter()
        .map(|e| {
            reader
                .curve(e.curve)
                .map_err(|e| refusals.push(unreadable(e)))
                .ok()
        })
        .collect();
    if !refusals.is_empty() {
        return Err(refusals);
    }
    let points: Vec<P> = points.into_iter().flatten().collect();
    let surfaces: Vec<Surface> = surfaces.into_iter().flatten().collect();
    let curves: Vec<Curve> = curves.into_iter().flatten().collect();
    refusals.extend(not_geometry(&surfaces, &curves));
    if !refusals.is_empty() {
        return Err(refusals);
    }

    // Check 26, before 23 to 25, which need finite arithmetic on what it
    // bounds: a surface placed at 1e308 makes a vertex's distance NaN.
    refusals.extend(out_of_range(solid, &points, &surfaces, &curves));
    if !refusals.is_empty() {
        return Err(refusals);
    }

    // Checks 23 and 24.
    let bound = uncertainty.max(GATE_SLACK_MM) * (1.0 + GATE_SLACK_REL);
    let (largest, off) = vertex_offsets(solid, &surfaces, &curves, &points, bound);
    refusals.extend(off);
    if !refusals.is_empty() {
        return Err(refusals);
    }

    // Check 25.
    refusals.extend(orientation(solid, &surfaces, &curves, &points));
    if refusals.is_empty() {
        Ok(largest)
    } else {
        Err(refusals)
    }
}

/// Check 22, the rest: a surface or curve stepq does not read must still be
/// a surface or curve. stepq reads any other entity as one it does not
/// interpret, so a face whose geometry is a point would otherwise pass as
/// an unread surface.
fn not_geometry(surfaces: &[Surface], curves: &[Curve]) -> Vec<Refusal> {
    let named = |entity: &str, kinds: &[&str]| {
        entity
            .split('+')
            .any(|part| kinds.iter().any(|kind| part.contains(kind)))
    };
    let mut refusals = Vec::new();
    for surface in surfaces {
        if let Surface::Other(other) = surface {
            if !named(&other.entity, &["SURFACE"]) {
                refusals.push(Refusal::new(
                    RefusalCode::GeometryUnreadable,
                    Some(other.instance),
                    format!("#{} is a {}, not a surface", other.instance, other.entity),
                ));
            }
        }
    }
    for curve in curves {
        if let Curve::Other(other) = curve.basis() {
            let kinds = ["CURVE", "ELLIPSE", "HYPERBOLA", "PARABOLA", "POLYLINE"];
            if !named(&other.entity, &kinds) {
                refusals.push(Refusal::new(
                    RefusalCode::GeometryUnreadable,
                    Some(other.instance),
                    format!("#{} is a {}, not a curve", other.instance, other.entity),
                ));
            }
        }
    }
    refusals
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

/// True unless every value is finite and below [`RANGE_MM`] in magnitude.
fn beyond(values: &[f64]) -> bool {
    values.iter().any(|v| !below(v.abs(), RANGE_MM))
}

/// Check 26: every vertex, every surface's and circle's location and
/// radii, and every line's point lie within [`RANGE_MM`].
fn out_of_range(
    solid: &Solid,
    points: &[P],
    surfaces: &[Surface],
    curves: &[Curve],
) -> Vec<Refusal> {
    let refusal = |instance: u64, what: &str| {
        Refusal::new(
            RefusalCode::OutOfRange,
            Some(instance),
            format!("{what} #{instance} lies beyond the {RANGE_MM} mm read"),
        )
    };
    let mut refusals = Vec::new();
    for (vertex, p) in solid.vertices().iter().zip(points) {
        if beyond(p) {
            refusals.push(refusal(vertex.instance, "vertex"));
        }
    }
    for surface in surfaces {
        let far = match surface {
            Surface::Plane(s) => beyond(&s.position.location),
            Surface::Cylindrical(s) => beyond(&s.position.location) || beyond(&[s.radius]),
            Surface::Conical(s) => beyond(&s.position.location) || beyond(&[s.radius]),
            Surface::Spherical(s) => beyond(&s.position.location) || beyond(&[s.radius]),
            Surface::Toroidal(s) => {
                beyond(&s.position.location) || beyond(&[s.major_radius, s.minor_radius])
            }
            _ => false,
        };
        if far {
            refusals.push(refusal(surface.instance(), "surface"));
        }
    }
    for curve in curves {
        let far = match curve.basis() {
            Curve::Line(c) => beyond(&c.pnt),
            Curve::Circle(c) => beyond(&c.position.location) || beyond(&[c.radius]),
            _ => false,
        };
        if far {
            refusals.push(refusal(curve.basis().instance(), "curve"));
        }
    }
    refusals
}

/// Checks 23 and 24: every vertex lies on the surfaces of its faces and the
/// lines and circles of its edges within `bound`. Returns the largest
/// distance measured, and a refusal for each vertex beyond it.
fn vertex_offsets(
    solid: &Solid,
    surfaces: &[Surface],
    curves: &[Curve],
    points: &[P],
    bound: f64,
) -> (f64, Vec<Refusal>) {
    let mut refusals = Vec::new();
    let mut largest: f64 = 0.0;
    for (index, (face, surface)) in solid.faces().iter().zip(surfaces).enumerate() {
        for vertex in solid.face_vertices(index) {
            let (Some(p), Some(v)) = (points.get(vertex), solid.vertices().get(vertex)) else {
                continue;
            };
            let Some(off) = off_surface(surface, *p) else {
                continue;
            };
            largest = largest.max(off);
            if !at_most(off, bound) {
                refusals.push(Refusal::new(
                    RefusalCode::VertexOffSurface,
                    Some(v.instance),
                    format!(
                        "vertex #{} lies {off:.3e} mm off face #{}'s surface #{}; the stated uncertainty allows {bound:.3e} mm",
                        v.instance, face.instance, face.surface
                    ),
                ));
            }
        }
    }
    for (edge, curve) in solid.edges().iter().zip(curves) {
        for vertex in [edge.start, edge.end] {
            let (Some(p), Some(v)) = (points.get(vertex), solid.vertices().get(vertex)) else {
                continue;
            };
            let Some(off) = off_curve(curve, *p) else {
                continue;
            };
            largest = largest.max(off);
            if !at_most(off, bound) {
                refusals.push(Refusal::new(
                    RefusalCode::VertexOffCurve,
                    Some(v.instance),
                    format!(
                        "vertex #{} lies {off:.3e} mm off edge #{}'s curve #{}; the stated uncertainty allows {bound:.3e} mm",
                        v.instance, edge.instance, edge.curve
                    ),
                ));
            }
        }
    }
    (largest, refusals)
}

/// One use of an edge by a face's loop, as the solid holds the face: its
/// curve, and its start and end in the direction of travel.
struct Travel<'c> {
    curve: &'c Curve,
    edge_same_sense: bool,
    forward: bool,
    closed: bool,
    from: P,
    to: P,
}

/// The edge uses of face `index`'s loops, by loop. A vertex loop has none.
fn travels<'c>(
    solid: &Solid,
    index: usize,
    curves: &'c [Curve],
    points: &[P],
) -> Vec<Vec<Travel<'c>>> {
    let Some(face) = solid.faces().get(index) else {
        return Vec::new();
    };
    let as_written = solid
        .shells()
        .get(face.shell)
        .and_then(|shell| shell.orientation)
        .unwrap_or(true);
    let mut loops = Vec::new();
    for bound in &face.bounds {
        let Loop::EdgeLoop { edges, .. } = &bound.bound else {
            continue;
        };
        let mut travel = Vec::new();
        for oriented in edges {
            let (Some(edge), Some(curve)) =
                (solid.edges().get(oriented.edge), curves.get(oriented.edge))
            else {
                continue;
            };
            let forward = (oriented.orientation == bound.orientation) == as_written;
            let (Some(&start), Some(&end)) = (points.get(edge.start), points.get(edge.end)) else {
                continue;
            };
            let (from, to) = if forward { (start, end) } else { (end, start) };
            travel.push(Travel {
                curve,
                edge_same_sense: edge.same_sense,
                forward,
                closed: edge.is_closed(),
                from,
                to,
            });
        }
        loops.push(travel);
    }
    loops
}

/// The angle from `u` to `v` counterclockwise about unit `z` around
/// `centre`, in (0, 2π].
fn ccw_angle(centre: P, from: P, to: P, axis: P) -> f64 {
    let (start, end) = (sub(from, centre), sub(to, centre));
    let angle = atan2(dot(cross(start, end), axis), dot(start, end));
    if angle <= 0.0 {
        angle + std::f64::consts::TAU
    } else {
        angle
    }
}

/// The signed angle a circular edge use sweeps about its circle's axis.
fn sweep(travel: &Travel<'_>, circle: &stepq::geometry::Circle) -> f64 {
    let z = circle.position.z;
    let centre = circle.position.location;
    let along = travel.forward == travel.edge_same_sense;
    if travel.closed {
        if along {
            std::f64::consts::TAU
        } else {
            -std::f64::consts::TAU
        }
    } else if along {
        ccw_angle(centre, travel.from, travel.to, z)
    } else {
        -ccw_angle(centre, travel.to, travel.from, z)
    }
}

/// Check 25: each face's stated sense agrees with the direction its loops
/// run. A plane face whose outline is lines and circles must have positive
/// net area about its outward normal; a cylinder or cone face's coaxial
/// circles at its lowest and highest stations must run as its sense says.
fn orientation(
    solid: &Solid,
    surfaces: &[Surface],
    curves: &[Curve],
    points: &[P],
) -> Vec<Refusal> {
    let mut refusals = Vec::new();
    for (index, (face, surface)) in solid.faces().iter().zip(surfaces).enumerate() {
        let sense = solid.face_sense(index);
        let disagrees = match surface {
            Surface::Plane(plane) => {
                let z = plane.position.z;
                let normal = if sense { z } else { [-z[0], -z[1], -z[2]] };
                net_area(&travels(solid, index, curves, points), normal)
                    .is_some_and(|area| !below(0.0, area))
            }
            Surface::Cylindrical(s) => circles_disagree(
                &travels(solid, index, curves, points),
                s.position.location,
                s.position.z,
                sense,
            ),
            Surface::Conical(s) => circles_disagree(
                &travels(solid, index, curves, points),
                s.position.location,
                s.position.z,
                sense,
            ),
            _ => false,
        };
        if disagrees {
            refusals.push(Refusal::new(
                RefusalCode::FaceOrientation,
                Some(face.instance),
                format!(
                    "face #{} states same_sense {} but its loops run the other way",
                    face.instance,
                    if face.same_sense { ".T." } else { ".F." }
                ),
            ));
        }
    }
    refusals
}

/// The net signed area of a plane face's loops projected along `normal`,
/// or `None` if an edge is not a line or circle, or the face has no edge.
///
/// Areas are measured from the face's first vertex, so that a plane
/// placed far from its face loses no digits. A circle's term is its exact
/// projection, scaled by the cosine between its axis and the normal: a
/// circle standing across the plane, which no valid face has, then counts
/// as its chord and cannot turn the sign.
fn net_area(loops: &[Vec<Travel<'_>>], normal: P) -> Option<f64> {
    let origin = loops.iter().flatten().next()?.from;
    let mut area = 0.0;
    for travel in loops.iter().flatten() {
        let (a, b) = (sub(travel.from, origin), sub(travel.to, origin));
        area += match travel.curve.basis() {
            Curve::Line(_) => dot(cross(a, b), normal) / 2.0,
            Curve::Circle(circle) => {
                let c = sub(circle.position.location, origin);
                let theta = sweep(travel, circle) * dot(circle.position.z, normal);
                dot(cross(c, sub(b, a)), normal) / 2.0 + circle.radius * circle.radius * theta / 2.0
            }
            _ => return None,
        };
    }
    Some(area)
}

/// True if a coaxial circle at the lowest or highest station of a cylinder
/// or cone face runs against the face's sense. Along the axis z, the face
/// lies above its lowest circle; its boundary runs counterclockwise about z
/// there exactly when the outward normal points away from the axis (a
/// shaft, `sense` true), and the other way at its highest circle.
///
/// The circles bound the face's extent only if every other edge is a line
/// between their stations; a face with any other edge is not judged.
fn circles_disagree(loops: &[Vec<Travel<'_>>], origin: P, z: P, sense: bool) -> bool {
    let mut uses = Vec::new();
    let mut lines = Vec::new();
    for travel in loops.iter().flatten() {
        match travel.curve.basis() {
            Curve::Circle(circle) => {
                let n = circle.position.z;
                let lever = circle.radius.max(1.0);
                let (offset, station) = radial(circle.position.location, origin, z);
                if !(at_most(length(cross(n, z)) * lever, IDENTITY_MM)
                    && at_most(offset, IDENTITY_MM))
                {
                    return false;
                }
                let along = travel.forward == travel.edge_same_sense;
                let ccw = along == (dot(n, z) > 0.0);
                uses.push((station, ccw));
            }
            Curve::Line(_) => {
                lines.extend([travel.from, travel.to].map(|p| radial(p, origin, z).1));
            }
            _ => return false,
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
