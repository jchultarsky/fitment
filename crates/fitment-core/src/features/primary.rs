//! The primary features: every face of the solid in exactly one plane,
//! cylinder, cone, torus or other face (`docs/research/m2-features.md`,
//! section 6).

use std::collections::BTreeMap;

use super::cylinder::{Axis, Cylinder, CylinderClass, Sense};
use super::full;
use super::group::{self, PlaneGroup};
use super::other::{Cone, OtherFace, OtherReason, Torus};
use super::outline;
use super::plane::{Plane, PlaneFace};
use super::stack::{self, Hole, HoleKind, Shaft, ShaftKind};
use super::topo::{Surface, Topo, dot, length, radial, sub};
use super::walk;
use super::{FaceCounts, FeatureId, FeatureKind, Note, NoteCode};
use crate::gate::{Refusal, RefusalCode};
use crate::geom::{Dir3, Point3, finite};

/// The primary features of one solid, and the notes made reading them.
pub(crate) struct Primary {
    pub(crate) complete: bool,
    pub(crate) face_counts: FaceCounts,
    pub(crate) planes: Vec<Plane>,
    pub(crate) cylinders: Vec<Cylinder>,
    pub(crate) holes: Vec<Hole>,
    pub(crate) shafts: Vec<Shaft>,
    pub(crate) cones: Vec<Cone>,
    pub(crate) tori: Vec<Torus>,
    pub(crate) other_faces: Vec<OtherFace>,
    pub(crate) notes: Vec<Note>,
}

/// A fitment fault: a step found its own invariant broken (check 30).
fn internal(instance: u64, message: impl Into<String>) -> Refusal {
    Refusal::new(RefusalCode::Internal, Some(instance), message)
}

/// Reads the primary features of `topo`, solid `solid`, whose largest
/// stated length uncertainty is `uncertainty` mm.
///
/// # Errors
///
/// An `internal` refusal if a face lands in no feature or in two, or a
/// value the gate passed is not finite.
pub(crate) fn read(topo: &Topo, solid: FeatureId, uncertainty: f64) -> Result<Primary, Refusal> {
    let planes = group::planes(topo);
    let cylinders = group::connected(
        topo,
        |s| matches!(s, Surface::Cylinder { .. }),
        group::same_cylinder,
    );
    let cones = group::connected(
        topo,
        |s| matches!(s, Surface::Cone { .. }),
        group::same_cone,
    );
    let tori = group::connected(
        topo,
        |s| matches!(s, Surface::Torus { .. }),
        group::same_torus,
    );
    let mut others: Vec<usize> = (0..topo.faces.len())
        .filter(|&i| {
            topo.faces.get(i).is_some_and(|f| {
                matches!(f.surface, Surface::Sphere { .. } | Surface::Other { .. })
            })
        })
        .collect();
    // By `#id`, as every list is: a shell may list its faces in any order.
    others.sort_by_key(|&i| topo.faces.get(i).map(|f| f.instance));

    // The partition: every face in exactly one primary feature.
    let mut owner: Vec<Option<FeatureId>> = vec![None; topo.faces.len()];
    let mut assign = |kind: FeatureKind, faces: &[usize]| -> Result<(), Refusal> {
        let first = faces
            .first()
            .and_then(|&i| topo.faces.get(i))
            .ok_or_else(|| internal(topo.instance, "a primary feature has no face"))?;
        let id = FeatureId::new(kind, first.instance);
        for &face in faces {
            match owner.get_mut(face) {
                Some(slot @ None) => *slot = Some(id),
                _ => {
                    return Err(internal(
                        topo.faces.get(face).map_or(topo.instance, |f| f.instance),
                        format!("a face is in {id} and in another feature"),
                    ));
                }
            }
        }
        Ok(())
    };
    for plane in &planes {
        assign(FeatureKind::Plane, &plane.faces)?;
    }
    for group in &cylinders {
        assign(FeatureKind::Cylinder, group)?;
    }
    for group in &cones {
        assign(FeatureKind::Cone, group)?;
    }
    for group in &tori {
        assign(FeatureKind::Torus, group)?;
    }
    for &face in &others {
        assign(FeatureKind::Other, &[face])?;
    }
    let owner: Vec<FeatureId> = owner
        .into_iter()
        .zip(&topo.faces)
        .map(|(id, face)| {
            id.ok_or_else(|| internal(face.instance, "a face is in no primary feature"))
        })
        .collect::<Result<_, _>>()?;

    let (near, mut notes) = cylinder_pairs(topo, &owner, uncertainty)?;
    let mut read_planes = Vec::new();
    for group in &planes {
        read_planes.push(plane(topo, solid, group, &owner, uncertainty, &mut notes)?);
    }
    notes.sort_by_key(|note| (note.instance, note.code));
    let read_cylinders: Vec<(Cylinder, full::Classified)> = cylinders
        .iter()
        .map(|g| cylinder(topo, solid, g, &owner, &near))
        .collect::<Result<_, _>>()?;
    let curved: Vec<&Vec<usize>> = cylinders.iter().chain(&cones).chain(&tori).collect();
    let (holes, shafts, complete) = compound(topo, &owner, &curved, &read_cylinders)?;
    Ok(Primary {
        face_counts: face_counts(topo),
        planes: read_planes,
        cylinders: read_cylinders.into_iter().map(|(c, _)| c).collect(),
        complete,
        holes,
        shafts,
        cones: cones
            .iter()
            .map(|g| cone(topo, solid, g))
            .collect::<Result<_, _>>()?,
        tori: tori
            .iter()
            .map(|g| torus(topo, solid, g))
            .collect::<Result<_, _>>()?,
        other_faces: others
            .iter()
            .map(|&i| other(topo, solid, i))
            .collect::<Result<_, _>>()?,
        notes,
    })
}

/// The holes and shafts: every full cylinder of the outer shell walks
/// both ends (section 9.1), and mutual steps chain them into stacks (9.2).
/// A void's cylinders never join one (S3).
/// Also returns whether the part is complete (section 9.6): no cylinder
/// undecided, no hole or shaft unknown.
fn compound(
    topo: &Topo,
    owner: &[FeatureId],
    curved: &[&Vec<usize>],
    cylinders: &[(Cylinder, full::Classified)],
) -> Result<(Vec<Hole>, Vec<Shaft>, bool), Refusal> {
    let mut faces_of: BTreeMap<FeatureId, Vec<usize>> = BTreeMap::new();
    for group in curved {
        if let Some(&id) = group.first().and_then(|&f| owner.get(f)) {
            faces_of.insert(id, (*group).clone());
        }
    }
    let walls: Vec<walk::Wall> = cylinders
        .iter()
        .map(|(c, read)| walk::Wall {
            id: c.id,
            radius: c.radius_mm,
            shaft: c.sense == Sense::Shaft,
            point: read.point,
            z: read.z,
        })
        .collect();
    let groups: Vec<stack::Group<'_>> = cylinders
        .iter()
        .zip(&walls)
        .filter(|((c, _), _)| c.class.is_full() && c.shell == 0)
        .map(|((cylinder, read), wall)| stack::Group {
            cylinder,
            wall,
            stations: &read.stations,
            arcs: &read.arcs,
        })
        .collect();
    let part = walk::Part {
        topo,
        owner,
        faces: &faces_of,
    };
    let sides = stack::walk_all(&part, &groups).map_err(|at| {
        Refusal::new(
            RefusalCode::Contradiction,
            Some(at.0),
            format!(
                "plane face #{} is an annulus whose sides contradict its radii: a hole widens into air and narrows into material",
                at.0
            ),
        )
    })?;
    let (holes, shafts) = stack::stacks(&groups, &sides).ok_or_else(|| {
        internal(
            topo.instance,
            "a hole or shaft has a value that is not finite",
        )
    })?;
    let complete = cylinders
        .iter()
        .all(|(c, _)| !matches!(c.class, CylinderClass::Undecided { .. }))
        && holes.iter().all(|h| h.kind != HoleKind::Unknown)
        && shafts.iter().all(|s| s.kind != ShaftKind::Unknown);
    Ok((holes, shafts, complete))
}

/// Pairs of feature ids.
type Pairs = Vec<(FeatureId, FeatureId)>;

/// Check 27 and the near-miss notes, over every edge between faces of two
/// cylinder features: on one cylinder within ε with opposite senses is a
/// wall of no thickness, a contradiction; with one sense, within the stated
/// uncertainty but not ε, a near miss. Returns the near misses as pairs of
/// feature ids, both ways round, and their notes.
fn cylinder_pairs(
    topo: &Topo,
    owner: &[FeatureId],
    uncertainty: f64,
) -> Result<(Pairs, Vec<Note>), Refusal> {
    let mut near = Vec::new();
    let mut notes = Vec::new();
    for edge in &topo.edges {
        let [a, b] = edge.faces[..] else {
            continue;
        };
        let (Some(&ia), Some(&ib)) = (owner.get(a), owner.get(b)) else {
            continue;
        };
        if ia == ib || ia.kind() != FeatureKind::Cylinder || ib.kind() != FeatureKind::Cylinder {
            continue;
        }
        let senses = (
            topo.faces.get(a).map(|f| f.sense),
            topo.faces.get(b).map(|f| f.sense),
        );
        if senses.0 != senses.1 {
            if group::one_cylinder(topo, a, b, group::IDENTITY_MM) {
                return Err(Refusal::new(
                    RefusalCode::Contradiction,
                    Some(edge.instance),
                    format!(
                        "the faces of {ia} and {ib} meet at edge #{} on one cylinder with opposite senses: a wall of no thickness",
                        edge.instance
                    ),
                ));
            }
        } else if group::one_cylinder(topo, a, b, uncertainty) {
            near.extend([(ia, ib), (ib, ia)]);
            notes.push(Note {
                code: NoteCode::NearMiss,
                instance: edge.instance,
                message: format!(
                    "{ia} and {ib} meet at edge #{} on one cylinder within the stated uncertainty, {uncertainty} mm, but not within 1e-6 mm",
                    edge.instance
                ),
            });
        }
    }
    near.sort_unstable();
    near.dedup();
    Ok((near, notes))
}

fn face_counts(topo: &Topo) -> FaceCounts {
    let count = |kind: fn(&Surface) -> bool| {
        let n = topo.faces.iter().filter(|f| kind(&f.surface)).count();
        u32::try_from(n).unwrap_or(u32::MAX)
    };
    FaceCounts {
        plane: count(|s| matches!(s, Surface::Plane { .. })),
        cylindrical: count(|s| matches!(s, Surface::Cylinder { .. })),
        conical: count(|s| matches!(s, Surface::Cone { .. })),
        spherical: count(|s| matches!(s, Surface::Sphere { .. })),
        toroidal: count(|s| matches!(s, Surface::Torus { .. })),
        other: count(|s| matches!(s, Surface::Other { .. })),
        total: u32::try_from(topo.faces.len()).unwrap_or(u32::MAX),
    }
}

/// `x`, finite and never `-0.0`, or an internal refusal at `instance`.
fn num(x: f64, instance: u64) -> Result<f64, Refusal> {
    finite(x).ok_or_else(|| internal(instance, format!("a value of #{instance} is not finite")))
}

fn point(p: [f64; 3], instance: u64) -> Result<Point3, Refusal> {
    Point3::new(p)
        .ok_or_else(|| internal(instance, format!("a point of #{instance} is not finite")))
}

fn direction(d: [f64; 3], instance: u64) -> Result<Dir3, Refusal> {
    Dir3::from_unit(d)
        .or_else(|| Dir3::normalize(d))
        .ok_or_else(|| {
            internal(
                instance,
                format!("a direction of #{instance} is not a unit vector"),
            )
        })
}

fn shell(topo: &Topo, face: usize) -> u32 {
    topo.faces
        .get(face)
        .and_then(|f| u32::try_from(f.shell).ok())
        .unwrap_or(u32::MAX)
}

/// The `#id`s of a group's faces, sorted, and its reference face (the
/// lowest index, the file's first).
fn members(topo: &Topo, group: &[usize]) -> Result<(Vec<u64>, usize), Refusal> {
    let ids: Vec<u64> = group
        .iter()
        .filter_map(|&i| topo.faces.get(i).map(|f| f.instance))
        .collect();
    let reference = group
        .iter()
        .copied()
        .min()
        .ok_or_else(|| internal(topo.instance, "a group has no face"))?;
    Ok((ids, reference))
}

fn plane(
    topo: &Topo,
    solid: FeatureId,
    group: &PlaneGroup,
    owner: &[FeatureId],
    uncertainty: f64,
    notes: &mut Vec<Note>,
) -> Result<Plane, Refusal> {
    let Some(first) = group.faces.first().and_then(|&i| topo.faces.get(i)) else {
        return Err(internal(topo.instance, "a plane has no face"));
    };
    let id = FeatureId::new(FeatureKind::Plane, first.instance);
    let mut faces = Vec::new();
    let mut area = Some(0.0);
    for &index in &group.faces {
        let Some(face) = topo.faces.get(index) else {
            return Err(internal(topo.instance, "a plane face is missing"));
        };
        let (Some(normal), Some((anchor, _))) = (group::outward(face), group::anchor(topo, face))
        else {
            return Err(internal(face.instance, "a plane face is not on a plane"));
        };
        let read = outline::outline(
            topo,
            index,
            normal,
            owner,
            uncertainty.max(group::IDENTITY_MM),
        )
        .ok_or_else(|| internal(face.instance, "the outline of a plane face did not read"))?;
        if let Some(stated) = read.outer_differs {
            notes.push(Note {
                code: NoteCode::StatedOuterBoundDiffers,
                instance: face.instance,
                message: format!(
                    "face #{} states #{stated} as its outer bound, but another loop has the positive area",
                    face.instance
                ),
            });
        }
        area = area.zip(read.area_mm2).map(|(sum, a)| sum + a);
        faces.push(PlaneFace {
            face: face.instance,
            surface: face.surface.instance(),
            offset_mm: num(dot(group.normal, anchor), face.instance)?,
            area_mm2: read.area_mm2,
            outline: read.outline,
        });
    }
    let offsets: Vec<f64> = faces.iter().map(|f| f.offset_mm).collect();
    let low = offsets.iter().copied().fold(f64::INFINITY, f64::min);
    let high = offsets.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Ok(Plane {
        id,
        solid,
        shell: shell(topo, group.faces.first().copied().unwrap_or(0)),
        normal: direction(group.normal, first.instance)?,
        offset_mm: offsets.first().copied().unwrap_or(0.0),
        spread_mm: num(high - low, first.instance)?,
        area_mm2: area.map(|a| num(a, first.instance)).transpose()?,
        faces,
    })
}

fn cylinder(
    topo: &Topo,
    solid: FeatureId,
    group: &[usize],
    owner: &[FeatureId],
    near: &[(FeatureId, FeatureId)],
) -> Result<(Cylinder, full::Classified), Refusal> {
    let (faces, reference) = members(topo, group)?;
    let Some(face) = topo.faces.get(reference) else {
        return Err(internal(topo.instance, "a cylinder's face is missing"));
    };
    let Surface::Cylinder {
        instance,
        location,
        z,
        radius,
    } = &face.surface
    else {
        return Err(internal(
            face.instance,
            "a cylinder face is not on a cylinder",
        ));
    };
    let mut spread: f64 = 0.0;
    for member in group.iter().filter_map(|&i| topo.faces.get(i)) {
        if let Surface::Cylinder {
            location: p,
            radius: r,
            ..
        } = &member.surface
        {
            spread = spread
                .max((r - radius).abs())
                .max(radial(*p, *location, *z).0);
        }
    }
    let id = FeatureId::new(FeatureKind::Cylinder, faces.first().copied().unwrap_or(0));
    let read = full::classify(
        topo,
        &full::Group {
            id,
            faces: group,
            owner,
            origin: *location,
            axis: *z,
            radius: *radius,
            shaft: face.sense,
        },
    )
    .map_err(|at| {
        Refusal::new(
            RefusalCode::Contradiction,
            Some(at.0),
            format!(
                "the end arcs of {id} through edge #{} chain around its axis more than once",
                at.0
            ),
        )
    })?;
    let mut class = read.class.clone();
    if let CylinderClass::Partial {
        sweep_rad: Some(sweep),
    } = &mut class
    {
        *sweep = num(*sweep, *instance)?;
    }
    // `near` is sorted: this cylinder's pairs are one run of it.
    let start = near.partition_point(|(a, _)| *a < id);
    let end = near.partition_point(|(a, _)| *a <= id);
    let cylinder = Cylinder {
        id,
        solid,
        shell: shell(topo, reference),
        faces,
        surface: *instance,
        sense: Sense::of(face.sense),
        axis: Axis {
            point_mm: point(read.point, *instance)?,
            direction: direction(read.z, *instance)?,
        },
        radius_mm: num(*radius, *instance)?,
        spread_mm: num(spread, *instance)?,
        class,
        stations_mm: read
            .stations
            .iter()
            .map(|&s| num(s, *instance))
            .collect::<Result<_, _>>()?,
        wall_openings: read.wall_openings,
        near_misses: near
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .map(|&(_, b)| b)
            .collect(),
    };
    Ok((cylinder, read))
}

fn cone(topo: &Topo, solid: FeatureId, group: &[usize]) -> Result<Cone, Refusal> {
    let (faces, reference) = members(topo, group)?;
    let Some(face) = topo.faces.get(reference) else {
        return Err(internal(topo.instance, "a cone's face is missing"));
    };
    let Surface::Cone {
        instance,
        location,
        z,
        radius,
        semi_angle,
    } = &face.surface
    else {
        return Err(internal(face.instance, "a cone face is not on a cone"));
    };
    let apex = group::apex(*location, *z, *radius, *semi_angle);
    let mut spread: f64 = 0.0;
    for member in group.iter().filter_map(|&i| topo.faces.get(i)) {
        if let Surface::Cone {
            location: p,
            z: n,
            radius: r,
            semi_angle: s,
            ..
        } = &member.surface
        {
            spread = spread.max(length(sub(group::apex(*p, *n, *r, *s), apex)));
        }
    }
    Ok(Cone {
        id: FeatureId::new(FeatureKind::Cone, faces.first().copied().unwrap_or(0)),
        solid,
        shell: shell(topo, reference),
        faces,
        surface: *instance,
        sense: Sense::of(face.sense),
        axis: Axis {
            point_mm: point(*location, *instance)?,
            direction: direction(*z, *instance)?,
        },
        apex_mm: point(apex, *instance)?,
        semi_angle_rad: num(*semi_angle, *instance)?,
        radii_mm: cone_radii(topo, group, *location, *z),
        spread_mm: num(spread, *instance)?,
    })
}

/// The smallest and largest distance from the axis through `location`
/// along unit `z` of the boundaries of the faces in `group`: their
/// vertices, and the radius of each circle edge about the axis. A line on
/// a cone face is a ruling, whose distance from the axis is extreme at its
/// ends. `None` if an edge is a circle about another axis, or a curve not
/// read.
fn cone_radii(topo: &Topo, group: &[usize], location: [f64; 3], z: [f64; 3]) -> Option<[f64; 2]> {
    let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut take = |r: f64| {
        low = low.min(r);
        high = high.max(r);
    };
    for face in group.iter().filter_map(|&i| topo.faces.get(i)) {
        if super::extent::holds_apex(topo, face, location, z) {
            take(0.0);
        }
        for vertex in topo.face_vertices(face) {
            take(radial(topo.vertices.get(vertex)?.point, location, z).0);
        }
        for travels in topo.travels(face) {
            for travel in travels {
                match travel.curve {
                    super::topo::Curve::Line { .. } => {}
                    super::topo::Curve::Circle {
                        centre,
                        z: n,
                        radius,
                        ..
                    } => {
                        let (off_axis, _) = radial(*centre, location, z);
                        let tilt = length(super::topo::cross(*n, z));
                        if off_axis > group::IDENTITY_MM || tilt > group::PARALLEL_SINE {
                            return None;
                        }
                        take(*radius);
                    }
                    super::topo::Curve::Other { .. } => return None,
                }
            }
        }
    }
    let low = finite(low)?;
    let high = finite(high)?;
    (low <= high).then_some([low, high])
}

fn torus(topo: &Topo, solid: FeatureId, group: &[usize]) -> Result<Torus, Refusal> {
    let (faces, reference) = members(topo, group)?;
    let Some(face) = topo.faces.get(reference) else {
        return Err(internal(topo.instance, "a torus's face is missing"));
    };
    let Surface::Torus {
        instance,
        location,
        z,
        major,
        minor,
    } = &face.surface
    else {
        return Err(internal(face.instance, "a torus face is not on a torus"));
    };
    let mut spread: f64 = 0.0;
    for member in group.iter().filter_map(|&i| topo.faces.get(i)) {
        if let Surface::Torus {
            location: p,
            major: big,
            minor: small,
            ..
        } = &member.surface
        {
            spread = spread
                .max(length(sub(*p, *location)))
                .max((big - major).abs())
                .max((small - minor).abs());
        }
    }
    Ok(Torus {
        id: FeatureId::new(FeatureKind::Torus, faces.first().copied().unwrap_or(0)),
        solid,
        shell: shell(topo, reference),
        faces,
        surface: *instance,
        convex: face.sense,
        axis: Axis {
            point_mm: point(*location, *instance)?,
            direction: direction(*z, *instance)?,
        },
        major_radius_mm: num(*major, *instance)?,
        minor_radius_mm: num(*minor, *instance)?,
        spread_mm: num(spread, *instance)?,
    })
}

fn other(topo: &Topo, solid: FeatureId, index: usize) -> Result<OtherFace, Refusal> {
    let Some(face) = topo.faces.get(index) else {
        return Err(internal(topo.instance, "an other face is missing"));
    };
    let entity = match &face.surface {
        Surface::Sphere { .. } => "SPHERICAL_SURFACE".to_owned(),
        Surface::Other { entity, .. } => entity.clone(),
        _ => {
            return Err(internal(
                face.instance,
                "a face no rule reads has a read surface",
            ));
        }
    };
    Ok(OtherFace {
        id: FeatureId::new(FeatureKind::Other, face.instance),
        solid,
        shell: shell(topo, index),
        face: face.instance,
        surface: face.surface.instance(),
        entity,
        reason: OtherReason::NoRule,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::topo::{Curve, Edge, Face, Vertex};

    /// Two faces on one cylinder of radius 3 about the z axis, sharing one
    /// line edge, with the given senses.
    fn two_faces(senses: [bool; 2]) -> Topo {
        let face = |instance: u64, sense: bool| Face {
            instance,
            shell: 0,
            surface: Surface::Cylinder {
                instance: instance + 100,
                location: [0.0; 3],
                z: [0.0, 0.0, 1.0],
                radius: 3.0,
            },
            sense,
            same_sense: sense,
            bounds: Vec::new(),
            extent: 5.0,
        };
        Topo {
            instance: 1,
            shells: vec![2],
            faces: vec![face(10, senses[0]), face(20, senses[1])],
            edges: vec![Edge {
                instance: 30,
                start: 0,
                end: 1,
                curve: Curve::Line {
                    instance: 31,
                    point: [3.0, 0.0, 0.0],
                    dir: [0.0, 0.0, 1.0],
                },
                same_sense: true,
                faces: vec![0, 1],
            }],
            vertices: vec![
                Vertex {
                    instance: 40,
                    point: [3.0, 0.0, 0.0],
                },
                Vertex {
                    instance: 41,
                    point: [3.0, 0.0, 5.0],
                },
            ],
        }
    }

    fn owners() -> Vec<FeatureId> {
        vec![
            FeatureId::new(FeatureKind::Cylinder, 10),
            FeatureId::new(FeatureKind::Cylinder, 20),
        ]
    }

    #[test]
    fn faces_on_one_cylinder_with_opposite_senses_are_a_contradiction() {
        // Check 27: a wall of no thickness.
        let refusal = cylinder_pairs(&two_faces([true, false]), &owners(), 1e-7).unwrap_err();
        assert_eq!(refusal.code, RefusalCode::Contradiction);
        assert_eq!(refusal.instance, Some(30));
    }

    #[test]
    fn faces_on_one_cylinder_with_one_sense_are_one_group() {
        let topo = two_faces([true, true]);
        let groups = group::connected(
            &topo,
            |s| matches!(s, Surface::Cylinder { .. }),
            group::same_cylinder,
        );
        assert_eq!(groups, [vec![0, 1]]);
    }
}
