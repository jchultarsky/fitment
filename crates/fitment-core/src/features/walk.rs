//! The end walk (`docs/research/m2-features.md`, section 9.1, rules W1 to
//! W8): from a full end station of a cylinder, outward along its axis,
//! across the faces beyond, until a face says how the end ends.

use std::collections::BTreeMap;

use super::group::{self, IDENTITY_MM, LEVER_MIN_MM};
use super::topo::{Curve, Loop, P, Surface, Topo, cross, dot, length, radial, scale, sub};
use super::{FeatureId, FeatureKind};

/// The most transition faces one walk crosses (W7).
pub(crate) const WALK_MAX: u32 = 8;

/// Why an end is unknown.
pub(crate) use super::stack::UnknownReason;

/// A cylinder as the walk sees it.
pub(crate) struct Wall {
    pub(crate) id: FeatureId,
    pub(crate) radius: f64,
    /// True for a shaft wall, false for a hole wall.
    pub(crate) shaft: bool,
    /// Its axis line: the point at its lowest station, the canonical unit
    /// direction.
    pub(crate) point: P,
    pub(crate) z: P,
}

/// What the walk needs of the part: which feature holds each face, and the
/// faces of each curved feature.
pub(crate) struct Part<'a> {
    pub(crate) topo: &'a Topo,
    pub(crate) owner: &'a [FeatureId],
    pub(crate) faces: &'a BTreeMap<FeatureId, Vec<usize>>,
}

/// Where a walk stopped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Terminal {
    /// A perpendicular plane holding only the current circles.
    Disk(FeatureId),
    /// A face that is not crossed: a plane with other edges, or a face no
    /// rule reads after planes only.
    Face(FeatureId),
    /// A cone that closes at its apex, with its semi-angle and the apex's
    /// station.
    Apex(FeatureId, f64, f64),
    /// A coaxial cylinder of the opposite sense, with its radius.
    Post(FeatureId, f64),
    /// A coaxial cylinder of the same sense, after a transition.
    Step(FeatureId),
}

/// A crossed cone: its semi-angle, the radius it reaches, and whether it
/// widens along the walk.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Crossed {
    pub(crate) id: FeatureId,
    pub(crate) semi_angle: f64,
    pub(crate) far_radius: f64,
    pub(crate) widens: bool,
}

/// What one walk found. Stations are along the wall's axis from its point.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Walk {
    /// Whether air lies beyond the end (W3), and where the walk stopped; or
    /// why the end is unknown.
    pub(crate) outcome: Result<(bool, Terminal), UnknownReason>,
    /// The station the walk started from.
    pub(crate) at: f64,
    /// The rim (W8).
    pub(crate) rim: f64,
    /// The station of the last perpendicular plane crossed or stopped at.
    pub(crate) plane: Option<f64>,
    /// The transition faces crossed, with the station each was entered at.
    pub(crate) transitions: Vec<(FeatureId, f64)>,
    /// The cones crossed, in order.
    pub(crate) cones: Vec<Crossed>,
}

/// A contradiction found on the way: a perpendicular annulus whose "air"
/// disagrees with its radii (check 28), at the plane's face `#id`.
pub(crate) struct Contradiction(pub(crate) u64);

/// What the walk found of one face group beyond the current circles.
enum Beyond {
    /// A plane perpendicular to the axis, with its outward unit normal.
    Plane(P),
    /// A coaxial cone: its widening direction, semi-angle, and its apex's
    /// station along the wall's axis.
    Cone(P, f64, f64),
    /// A coaxial torus.
    Torus,
    /// A coaxial cylinder, with its sense and radius.
    Cylinder(bool, f64),
    /// Anything else.
    Other,
}

/// The circles and other edges bounding a face group, apart from the
/// current circles.
struct Boundary {
    /// Far circles: (edge, station, radius).
    far: Vec<(usize, f64, f64)>,
    others: usize,
}

impl Part<'_> {
    fn kind(&self, face: usize, axis: (P, P, f64)) -> Beyond {
        let (origin, z, radius) = axis;
        let Some(face) = self.topo.faces.get(face) else {
            return Beyond::Other;
        };
        match &face.surface {
            Surface::Plane { z: n, .. } => {
                let n = if face.sense { *n } else { super::topo::neg(*n) };
                if length(cross(n, z)) * radius.max(LEVER_MIN_MM) <= IDENTITY_MM {
                    Beyond::Plane(n)
                } else {
                    Beyond::Other
                }
            }
            Surface::Cone {
                location,
                z: w,
                radius: r,
                semi_angle,
                ..
            } => {
                let apex = group::apex(*location, *w, *r, *semi_angle);
                let lever = face.extent.max(LEVER_MIN_MM);
                if radial(apex, origin, z).0 <= IDENTITY_MM
                    && length(cross(*w, z)) * lever <= IDENTITY_MM
                {
                    Beyond::Cone(*w, *semi_angle, dot(sub(apex, origin), z))
                } else {
                    Beyond::Other
                }
            }
            Surface::Torus {
                location,
                z: w,
                major,
                minor,
                ..
            } => {
                if radial(*location, origin, z).0 <= IDENTITY_MM
                    && length(cross(*w, z)) * (major + minor) <= IDENTITY_MM
                {
                    Beyond::Torus
                } else {
                    Beyond::Other
                }
            }
            Surface::Cylinder {
                location,
                z: w,
                radius: r,
                ..
            } => {
                let lever = face.extent.max(LEVER_MIN_MM);
                if radial(*location, origin, z).0 <= IDENTITY_MM
                    && length(cross(*w, z)) * lever <= IDENTITY_MM
                {
                    Beyond::Cylinder(face.sense, *r)
                } else {
                    Beyond::Other
                }
            }
            _ => Beyond::Other,
        }
    }

    /// The boundary of `faces` other than the `current` edges: its coaxial
    /// circles (far circles), and whether it has any other edge. The scan
    /// stops at the first other edge, after which every rule stops the
    /// walk: a face many holes open onto is not read once per hole (found
    /// in review of PR 6: 10,000 holes in one plate took 4 s).
    fn boundary(&self, faces: &[usize], current: &[usize], axis: (P, P)) -> Boundary {
        let (origin, z) = axis;
        let mut boundary = Boundary {
            far: Vec::new(),
            others: 0,
        };
        let uses = faces
            .iter()
            .filter_map(|&i| self.topo.faces.get(i))
            .flat_map(|face| face.bounds.iter())
            .filter_map(|bound| match &bound.lp {
                Loop::Edges(uses) => Some(uses.iter()),
                Loop::Vertex(_) => None,
            })
            .flatten();
        for edge_use in uses {
            let Some(edge) = self.topo.edges.get(edge_use.edge) else {
                continue;
            };
            if edge.faces.iter().all(|f| faces.contains(f)) || current.contains(&edge_use.edge) {
                continue;
            }
            {
                match &edge.curve {
                    Curve::Circle {
                        centre,
                        z: n,
                        radius,
                        ..
                    } if length(cross(*n, z)) * radius <= IDENTITY_MM
                        && radial(*centre, origin, z).0 <= IDENTITY_MM =>
                    {
                        boundary
                            .far
                            .push((edge_use.edge, dot(sub(*centre, origin), z), *radius));
                    }
                    _ => {
                        boundary.others += 1;
                        break;
                    }
                }
            }
        }
        boundary.far.sort_by_key(|f| f.0);
        boundary.far.dedup_by_key(|f| f.0);
        boundary
    }
}

/// The one station and radius all far circles share, if they do.
fn one_circle(far: &[(usize, f64, f64)]) -> Option<(f64, f64)> {
    let &(_, station, radius) = far.first()?;
    far.iter()
        .all(|&(_, s, r)| (s - station).abs() <= IDENTITY_MM && (r - radius).abs() <= IDENTITY_MM)
        .then_some((station, radius))
}

/// Where a walk is: the current circles, the faces behind them, the
/// radius and station reached, and what it has found so far.
struct State {
    found: Walk,
    current: Vec<usize>,
    behind: Vec<usize>,
    radius: f64,
    station: f64,
    /// What the first face said lies beyond (W3).
    air: Option<bool>,
    /// True once a cone or torus has been crossed.
    curved: bool,
    visited: Vec<FeatureId>,
}

/// What one face group does to the walk.
enum Flow {
    /// Cross it: the far circles, at this station and radius, are next.
    Cross(Vec<usize>, f64, f64),
    /// Stop: the end, or why it is unknown.
    Stop(Result<(bool, Terminal), UnknownReason>),
}

/// The faces beyond the current circles, as one walk group (W1): its
/// feature and faces, and the first face met; `None` if they are not one
/// group, or the walk has met it before.
fn beyond(part: &Part<'_>, state: &mut State) -> Option<(FeatureId, Vec<usize>, usize)> {
    let mut faces: Vec<usize> = state
        .current
        .iter()
        .filter_map(|&e| part.topo.edges.get(e))
        .flat_map(|e| e.faces.iter().copied())
        .filter(|f| !state.behind.contains(f))
        .collect();
    faces.sort_unstable();
    faces.dedup();
    let mut owners: Vec<FeatureId> = faces
        .iter()
        .filter_map(|&f| part.owner.get(f).copied())
        .collect();
    owners.sort_unstable();
    owners.dedup();
    let (&[h], Some(&first)) = (owners.as_slice(), faces.first()) else {
        return None;
    };
    if state.visited.contains(&h) {
        return None;
    }
    state.visited.push(h);
    // A walk group is local for a plane (W1, G3): only the faces met.
    let group = if h.kind() == FeatureKind::Plane {
        faces
    } else {
        part.faces.get(&h).cloned().unwrap_or(faces)
    };
    Some((h, group, first))
}

/// A perpendicular plane with outward normal `n` (W3, W4, check 28).
fn plane(
    state: &mut State,
    wall: &Wall,
    (h, n, u): (FeatureId, P, P),
    edges: &Boundary,
    instance: u64,
) -> Result<Flow, Contradiction> {
    // The planes met must not lie back along the axis: past a pocket's
    // floor, a chamfer can climb back to the face around the pocket, which
    // is not where a shaft on the floor rises from (found in PR 6 on a gear
    // hub). A round that turns back to a boss's wall after its top is fine:
    // it meets no plane.
    let along = dot(wall.z, u);
    let behind = |at: f64| (state.station - at) * along < -IDENTITY_MM;
    if state.found.plane.is_some_and(behind) || behind(state.found.at) {
        return Ok(Flow::Stop(Err(UnknownReason::TurnsBack)));
    }
    // Air beyond when the outward normal points along u; every plane must
    // agree with the first face.
    let says = dot(n, u) > 0.0;
    if state.air.is_some_and(|a| a != says) {
        return Ok(Flow::Stop(Err(UnknownReason::AirThenMaterial)));
    }
    state.air = Some(says);
    state.found.plane = Some(state.station);
    state.found.rim = state.station;
    if edges.others > 0 {
        return Ok(Flow::Stop(Ok((says, Terminal::Face(h)))));
    }
    if edges.far.is_empty() {
        return Ok(Flow::Stop(Ok((says, Terminal::Disk(h)))));
    }
    let Some((at, far)) = one_circle(&edges.far) else {
        return Ok(Flow::Stop(Err(UnknownReason::Transition)));
    };
    if (at - state.station).abs() > IDENTITY_MM {
        return Ok(Flow::Stop(Err(UnknownReason::Transition)));
    }
    // Check 28: a hole widens into air and narrows into material; a shaft
    // the other way round.
    if says != ((far > state.radius) != wall.shaft) {
        return Err(Contradiction(instance));
    }
    state.found.transitions.push((h, state.station));
    Ok(Flow::Cross(far_edges(edges), state.station, far))
}

fn far_edges(edges: &Boundary) -> Vec<usize> {
    edges.far.iter().map(|f| f.0).collect()
}

/// A coaxial cone widening along `w`, of semi-angle `semi` (W3, W4).
fn cone(
    state: &mut State,
    wall: &Wall,
    (h, w, semi, apex, u): (FeatureId, P, f64, f64, P),
    edges: &Boundary,
) -> Flow {
    let along = dot(wall.z, u);
    let widens = dot(w, u) > 0.0;
    // A narrowing cone closes at its apex when nothing but the current
    // circles bounds it (a vertex loop at the apex is not an edge), the apex
    // lies ahead, and no plane was crossed first: a dimple in a flat floor
    // is not a drill point (review of PR 6). Any other edge, such as a cross
    // bore through the point (L1), leaves the end unknown.
    if !widens && edges.far.is_empty() && edges.others == 0 {
        let says = widens != wall.shaft;
        let air = *state.air.get_or_insert(says);
        state.curved = true;
        if (apex - state.station) * along <= IDENTITY_MM {
            return Flow::Stop(Err(UnknownReason::TurnsBack));
        }
        if state.found.plane.is_some() {
            return Flow::Stop(Err(UnknownReason::Combination));
        }
        return Flow::Stop(Ok((air, Terminal::Apex(h, semi, apex))));
    }
    let Some((at, far)) = (edges.others == 0)
        .then(|| one_circle(&edges.far))
        .flatten()
    else {
        return Flow::Stop(Err(UnknownReason::Transition));
    };
    // W3 reads a first cone by which way it runs; one whose far circle lies
    // back along the axis turns back, as a conical washer's face does from
    // its bore, and says nothing (review of PR 6).
    if state.air.is_none() && (at - state.station) * along < -IDENTITY_MM {
        return Flow::Stop(Err(UnknownReason::TurnsBack));
    }
    let says = widens != wall.shaft;
    state.air.get_or_insert(says);
    state.curved = true;
    state.found.transitions.push((h, state.station));
    state.found.cones.push(Crossed {
        id: h,
        semi_angle: semi,
        far_radius: far,
        widens,
    });
    Flow::Cross(far_edges(edges), at, far)
}

/// A coaxial torus (W3, W4).
fn torus(state: &mut State, wall: &Wall, (h, u): (FeatureId, P), edges: &Boundary) -> Flow {
    let Some((at, far)) = (edges.others == 0)
        .then(|| one_circle(&edges.far))
        .flatten()
    else {
        return Flow::Stop(Err(UnknownReason::Transition));
    };
    if state.air.is_none() && (at - state.station) * dot(wall.z, u) < -IDENTITY_MM {
        return Flow::Stop(Err(UnknownReason::TurnsBack));
    }
    state.air.get_or_insert((far > state.radius) != wall.shaft);
    state.curved = true;
    state.found.transitions.push((h, state.station));
    Flow::Cross(far_edges(edges), at, far)
}

/// A coaxial cylinder of sense `sense` and radius `r`, or a face no rule
/// crosses (W4).
fn stop(state: &mut State, wall: &Wall, h: FeatureId, cylinder: Option<(bool, f64)>) -> Flow {
    state.found.rim = state.found.plane.unwrap_or(state.station);
    let Some(says) = state.air else {
        return Flow::Stop(Err(UnknownReason::FirstFace));
    };
    Flow::Stop(match cylinder {
        Some((sense, _)) if sense == wall.shaft => {
            if state.found.transitions.is_empty() {
                Err(UnknownReason::FirstFace)
            } else {
                Ok((says, Terminal::Step(h)))
            }
        }
        Some((_, r)) => Ok((says, Terminal::Post(h, r))),
        None if state.curved => Err(UnknownReason::AfterCurve),
        None => Ok((says, Terminal::Face(h))),
    })
}

/// Walks outward from wall `wall`'s station `at` (along its axis), whose
/// end arcs are `arcs`, in direction `u` (+1 or −1 along the axis).
pub(crate) fn walk(
    part: &Part<'_>,
    wall: &Wall,
    (at, arcs): (f64, &[usize]),
    u: f64,
) -> Result<Walk, Contradiction> {
    let mut state = State {
        found: Walk {
            outcome: Err(UnknownReason::TooLong),
            at,
            rim: at,
            plane: None,
            transitions: Vec::new(),
            cones: Vec::new(),
        },
        current: arcs.to_vec(),
        behind: part.faces.get(&wall.id).cloned().unwrap_or_default(),
        radius: wall.radius,
        station: at,
        air: None,
        curved: false,
        visited: vec![wall.id],
    };
    let axis = (wall.point, wall.z, wall.radius);
    let u_vec = scale(wall.z, u);
    for _ in 0..=WALK_MAX {
        let Some((h, faces, first)) = beyond(part, &mut state) else {
            state.found.outcome = Err(UnknownReason::GroupsBeyond);
            return Ok(state.found);
        };
        let edges = part.boundary(&faces, &state.current, (wall.point, wall.z));
        let instance = part.topo.faces.get(first).map_or(0, |f| f.instance);
        let flow = match part.kind(first, axis) {
            Beyond::Plane(n) => plane(&mut state, wall, (h, n, u_vec), &edges, instance)?,
            Beyond::Cone(w, semi, apex) => {
                cone(&mut state, wall, (h, w, semi, apex, u_vec), &edges)
            }
            Beyond::Torus => torus(&mut state, wall, (h, u_vec), &edges),
            Beyond::Cylinder(sense, r) => stop(&mut state, wall, h, Some((sense, r))),
            Beyond::Other => stop(&mut state, wall, h, None),
        };
        match flow {
            Flow::Stop(outcome) => {
                state.found.outcome = outcome;
                return Ok(state.found);
            }
            Flow::Cross(far, station, radius) => {
                state.current = far;
                state.behind = faces;
                state.station = station;
                state.radius = radius;
                state.found.rim = state.found.plane.unwrap_or(station);
            }
        }
    }
    Ok(state.found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::topo::{Bound, Edge, Face, Loop, Use, Vertex};

    /// A hole wall of radius 3 (face 0) ending at z = 0 on a plane face
    /// (face 1) whose outward normal is `normal_z` along z, an annulus out
    /// to radius 5, where a wider cylinder (face 2) begins.
    fn annulus(normal_z: f64) -> Topo {
        let circle = |instance: u64, radius: f64, vertex: usize| Edge {
            instance,
            start: vertex,
            end: vertex,
            curve: Curve::Circle {
                instance: instance + 1,
                centre: [0.0; 3],
                z: [0.0, 0.0, 1.0],
                radius,
            },
            same_sense: true,
            faces: Vec::new(),
        };
        let mut inner = circle(10, 3.0, 0);
        inner.faces = vec![0, 1];
        let mut outer = circle(20, 5.0, 1);
        outer.faces = vec![1, 2];
        let bound = |instance: u64, edges: &[usize]| Bound {
            instance,
            stated_outer: false,
            lp: Loop::Edges(
                edges
                    .iter()
                    .map(|&edge| Use {
                        edge,
                        forward: true,
                    })
                    .collect(),
            ),
        };
        let face = |instance: u64, surface: Surface, bounds: Vec<Bound>| Face {
            instance,
            shell: 0,
            surface,
            sense: false,
            same_sense: false,
            bounds,
            extent: 5.0,
        };
        let cylinder = |instance: u64, radius: f64| Surface::Cylinder {
            instance,
            location: [0.0; 3],
            z: [0.0, 0.0, 1.0],
            radius,
        };
        let mut plane = face(
            200,
            Surface::Plane {
                instance: 201,
                location: [0.0; 3],
                z: [0.0, 0.0, 1.0],
            },
            vec![bound(202, &[0]), bound(203, &[1])],
        );
        plane.sense = normal_z > 0.0;
        Topo {
            instance: 1,
            shells: vec![2],
            faces: vec![
                face(100, cylinder(101, 3.0), vec![bound(102, &[0])]),
                plane,
                face(300, cylinder(301, 5.0), vec![bound(302, &[1])]),
            ],
            edges: vec![inner, outer],
            vertices: vec![
                Vertex {
                    instance: 40,
                    point: [3.0, 0.0, 0.0],
                },
                Vertex {
                    instance: 41,
                    point: [5.0, 0.0, 0.0],
                },
            ],
        }
    }

    fn walked(normal_z: f64) -> Result<Walk, Contradiction> {
        let topo = annulus(normal_z);
        let owner = [
            FeatureId::new(FeatureKind::Cylinder, 100),
            FeatureId::new(FeatureKind::Plane, 200),
            FeatureId::new(FeatureKind::Cylinder, 300),
        ];
        let faces: BTreeMap<FeatureId, Vec<usize>> =
            [(owner[0], vec![0]), (owner[2], vec![2])].into();
        let part = Part {
            topo: &topo,
            owner: &owner,
            faces: &faces,
        };
        let wall = Wall {
            id: owner[0],
            radius: 3.0,
            shaft: false,
            point: [0.0, 0.0, -5.0],
            z: [0.0, 0.0, 1.0],
        };
        walk(&part, &wall, (5.0, &[0]), 1.0)
    }

    #[test]
    fn a_hole_widening_through_an_annulus_into_air_steps_to_the_wider_bore() {
        let found = walked(1.0).ok().unwrap();
        assert_eq!(
            found.outcome,
            Ok((
                true,
                Terminal::Step(FeatureId::new(FeatureKind::Cylinder, 300))
            ))
        );
        assert_eq!(found.rim, 5.0);
    }

    #[test]
    fn a_shaft_widening_into_air_through_an_annulus_is_a_contradiction() {
        // Check 28, the shaft's side: a shaft narrows into air and widens
        // into material, so widening with air beyond contradicts it.
        let topo = annulus(1.0);
        let owner = [
            FeatureId::new(FeatureKind::Cylinder, 100),
            FeatureId::new(FeatureKind::Plane, 200),
            FeatureId::new(FeatureKind::Cylinder, 300),
        ];
        let faces: BTreeMap<FeatureId, Vec<usize>> =
            [(owner[0], vec![0]), (owner[2], vec![2])].into();
        let part = Part {
            topo: &topo,
            owner: &owner,
            faces: &faces,
        };
        let wall = Wall {
            id: owner[0],
            radius: 3.0,
            shaft: true,
            point: [0.0, 0.0, -5.0],
            z: [0.0, 0.0, 1.0],
        };
        assert!(matches!(
            walk(&part, &wall, (5.0, &[0]), 1.0),
            Err(Contradiction(200))
        ));
    }

    #[test]
    fn an_annulus_whose_side_contradicts_its_radii_is_a_contradiction() {
        // Check 28: the hole widens, but the plane says material lies
        // beyond, so the material would lie inside the hole's own wall.
        assert!(matches!(walked(-1.0), Err(Contradiction(200))));
    }
}
