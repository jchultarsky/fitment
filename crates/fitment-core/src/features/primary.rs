//! The primary features: every face of the solid in exactly one plane,
//! cylinder, cone, torus or other face (`docs/research/m2-features.md`,
//! section 6).

use super::cylinder::{Axis, Cylinder, Sense};
use super::group::{self, PlaneGroup};
use super::other::{Cone, OtherFace, OtherReason, Torus};
use super::outline;
use super::plane::{Plane, PlaneFace};
use super::topo::{Surface, Topo, dot, length, radial, sub};
use super::{FaceCounts, FeatureId, FeatureKind, Note, NoteCode};
use crate::gate::{Refusal, RefusalCode};
use crate::geom::{Dir3, Point3, finite};

/// The primary features of one solid, and the notes made reading them.
pub(crate) struct Primary {
    pub(crate) face_counts: FaceCounts,
    pub(crate) planes: Vec<Plane>,
    pub(crate) cylinders: Vec<Cylinder>,
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

    let mut notes = Vec::new();
    let mut read_planes = Vec::new();
    for group in &planes {
        read_planes.push(plane(topo, solid, group, &owner, uncertainty, &mut notes)?);
    }
    notes.sort_by_key(|note| (note.instance, note.code));
    Ok(Primary {
        face_counts: face_counts(topo),
        planes: read_planes,
        cylinders: cylinders
            .iter()
            .map(|g| cylinder(topo, solid, g))
            .collect::<Result<_, _>>()?,
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

fn cylinder(topo: &Topo, solid: FeatureId, group: &[usize]) -> Result<Cylinder, Refusal> {
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
    Ok(Cylinder {
        id: FeatureId::new(FeatureKind::Cylinder, faces.first().copied().unwrap_or(0)),
        solid,
        shell: shell(topo, reference),
        faces,
        surface: *instance,
        sense: Sense::of(face.sense),
        axis: Axis {
            point_mm: point(*location, *instance)?,
            direction: direction(*z, *instance)?,
        },
        radius_mm: num(*radius, *instance)?,
        spread_mm: num(spread, *instance)?,
    })
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
        spread_mm: num(spread, *instance)?,
    })
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
