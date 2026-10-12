//! Gate stages 4 to 6: the solid's topology, its units, and its geometry
//! values (`docs/research/m2-features.md`, section 4.2, checks 13 to 26).
//!
//! Each stage runs every check it has, then refuses if any failed, so that
//! a later stage never judges what an earlier one could not read. Within
//! two stages the checks are ordered too, for the same reason: check 16
//! runs only on a solid that passed check 15, and in stage 6 the geometry
//! must read (22) and lie in range (26) before vertices are measured
//! against it (23, 24), and those must pass before faces are oriented (25).

use stepq::geometry::Reader;
use stepq::model::Graph;
use stepq::topology::{Solid, TopologyError};
use stepq::units::{ContextUnits, UnitKind};

use super::{Refusal, RefusalCode};
use crate::features::topo::{Bound, Curve, Edge, Face, Loop, Surface, Topo, Use, Vertex};
use crate::features::{SolidUnits, StatedUncertainty, StatedUnit};

/// What stages 4 to 6 establish about the solid.
pub(crate) struct SolidFacts {
    pub(crate) topology: Topo,
    pub(crate) units: SolidUnits,
    pub(crate) max_uncertainty_mm: f64,
    pub(crate) max_vertex_offset_mm: f64,
}

/// Runs stages 4 to 6 on solid `id`.
pub(crate) fn check(graph: &Graph<'_>, id: u64) -> Result<SolidFacts, Vec<Refusal>> {
    let solid = topology(graph, id)?;
    let (units, max_uncertainty_mm) = units(graph, id)?;
    let (topology, max_vertex_offset_mm) = geometry(graph, &solid, max_uncertainty_mm)?;
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

/// Check 22: every vertex point, face surface and edge curve reads. The
/// solid is then copied into fitment's own types, on which the rest of
/// stage 6 runs. Returns the copy and the largest distance of a vertex
/// from its surfaces and curves, in mm.
fn geometry(
    graph: &Graph<'_>,
    solid: &Solid,
    uncertainty: f64,
) -> Result<(Topo, f64), Vec<Refusal>> {
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
    let points: Vec<Option<[f64; 3]>> = solid
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
                .map(|s| surface(&s))
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
                .map(|c| curve(&c))
                .map_err(|e| refusals.push(unreadable(e)))
                .ok()
        })
        .collect();
    if !refusals.is_empty() {
        return Err(refusals);
    }
    let topo = copy(
        solid,
        points.into_iter().flatten().collect(),
        surfaces.into_iter().flatten().collect(),
        curves.into_iter().flatten().collect(),
    );
    let largest = super::geometry::check(&topo, uncertainty)?;
    Ok((topo, largest))
}

fn surface(surface: &stepq::geometry::Surface) -> Surface {
    use stepq::geometry::Surface as S;
    match surface {
        S::Plane(s) => Surface::Plane {
            instance: s.instance,
            location: s.position.location,
            z: s.position.z,
        },
        S::Cylindrical(s) => Surface::Cylinder {
            instance: s.instance,
            location: s.position.location,
            z: s.position.z,
            radius: s.radius,
        },
        S::Conical(s) => Surface::Cone {
            instance: s.instance,
            location: s.position.location,
            z: s.position.z,
            radius: s.radius,
            semi_angle: s.semi_angle,
        },
        S::Spherical(s) => Surface::Sphere {
            instance: s.instance,
            location: s.position.location,
            radius: s.radius,
        },
        S::Toroidal(s) => Surface::Torus {
            instance: s.instance,
            location: s.position.location,
            z: s.position.z,
            major: s.major_radius,
            minor: s.minor_radius,
        },
        S::Other(s) => Surface::Other {
            instance: s.instance,
            entity: s.entity.clone(),
        },
        other => Surface::Other {
            instance: other.instance(),
            entity: "UNKNOWN".to_owned(),
        },
    }
}

/// The 3D curve: a surface curve's `curve_3d`.
fn curve(curve: &stepq::geometry::Curve) -> Curve {
    use stepq::geometry::Curve as C;
    match curve.basis() {
        C::Line(c) => Curve::Line {
            instance: c.instance,
            point: c.pnt,
            dir: c.dir.orientation,
        },
        C::Circle(c) => Curve::Circle {
            instance: c.instance,
            centre: c.position.location,
            z: c.position.z,
            radius: c.radius,
        },
        C::Other(c) => Curve::Other {
            instance: c.instance,
            entity: c.entity.clone(),
        },
        other => Curve::Other {
            instance: other.instance(),
            entity: "UNKNOWN".to_owned(),
        },
    }
}

/// The solid in fitment's types. `points`, `surfaces` and `curves` are
/// read for every vertex, face and edge, in stepq's order.
fn copy(solid: &Solid, points: Vec<[f64; 3]>, surfaces: Vec<Surface>, curves: Vec<Curve>) -> Topo {
    let vertices = solid
        .vertices()
        .iter()
        .zip(points)
        .map(|(v, point)| Vertex {
            instance: v.instance,
            point,
        })
        .collect();
    let edges = solid
        .edges()
        .iter()
        .zip(curves)
        .enumerate()
        .map(|(index, (e, curve))| Edge {
            instance: e.instance,
            start: e.start,
            end: e.end,
            curve,
            same_sense: e.same_sense,
            faces: solid.uses(index).iter().map(|u| u.face).collect(),
        })
        .collect();
    let faces = solid
        .faces()
        .iter()
        .zip(surfaces)
        .enumerate()
        .map(|(index, (f, surface))| {
            let as_written = solid
                .shells()
                .get(f.shell)
                .and_then(|shell| shell.orientation)
                .unwrap_or(true);
            Face {
                instance: f.instance,
                shell: f.shell,
                surface,
                sense: solid.face_sense(index),
                same_sense: f.same_sense,
                bounds: f
                    .bounds
                    .iter()
                    .map(|b| Bound {
                        instance: b.instance,
                        stated_outer: b.outer,
                        lp: walk(&b.bound, b.orientation == as_written),
                    })
                    .collect(),
            }
        })
        .collect();
    Topo {
        instance: solid.instance,
        shells: solid.shells().iter().map(|s| s.instance).collect(),
        faces,
        edges,
        vertices,
    }
}

/// A bound's loop in traversal order (design, section 5.3): its oriented
/// edges as written if the bound runs as its shell holds it, else reversed,
/// each from start to end if its orientation agrees.
fn walk(bound: &stepq::topology::Loop, as_written: bool) -> Loop {
    match bound {
        stepq::topology::Loop::EdgeLoop { edges, .. } => {
            let mut uses: Vec<Use> = edges
                .iter()
                .map(|oriented| Use {
                    edge: oriented.edge,
                    forward: oriented.orientation == as_written,
                })
                .collect();
            if !as_written {
                uses.reverse();
            }
            Loop::Edges(uses)
        }
        stepq::topology::Loop::VertexLoop { vertex, .. } => Loop::Vertex(*vertex),
        _ => Loop::Edges(Vec::new()),
    }
}
