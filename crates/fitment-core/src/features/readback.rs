//! The rules a part's features read back from JSON must keep: what
//! [`read_part`](super::read_part) guarantees of the features it writes
//! (`docs/research/m2-features.md`, section 10.2). A document that breaks
//! one is not read, so no consumer sees, for example, a known outline
//! whose loops do not close.

use std::collections::BTreeSet;
use std::f64::consts::TAU;

use super::cylinder::CylinderClass;
use super::outline::{Outline, OutlineLoop, Segment};
use super::{FeatureId, FeatureKind, Part, SolidInfo};
use crate::geom::Point3;

/// Relative slack on sums read back: a sum is recomputed in the order it
/// was written, but the check should not depend on that.
const SUM_REL: f64 = 1e-9;

type Checked = Result<(), String>;

/// Checks every feature list of `part`, whose one solid is `solid`. JSON
/// holds no NaN, so every number read is finite.
pub(super) fn check(part: &Part, solid: &SolidInfo) -> Checked {
    let ids: BTreeSet<FeatureId> = part
        .planes
        .iter()
        .map(|p| p.id)
        .chain(part.cylinders.iter().map(|c| c.id))
        .chain(part.cones.iter().map(|c| c.id))
        .chain(part.tori.iter().map(|t| t.id))
        .chain(part.other_faces.iter().map(|o| o.id))
        .collect();
    let owned = part
        .planes
        .iter()
        .map(|p| (p.id, p.solid, p.shell))
        .chain(part.cylinders.iter().map(|c| (c.id, c.solid, c.shell)))
        .chain(part.cones.iter().map(|c| (c.id, c.solid, c.shell)))
        .chain(part.tori.iter().map(|t| (t.id, t.solid, t.shell)))
        .chain(part.other_faces.iter().map(|o| (o.id, o.solid, o.shell)));
    for (id, of, shell) in owned {
        if of != solid.id || shell >= solid.shells {
            return Err(format!(
                "feature '{id}' names solid '{of}' and shell {shell}; the part has '{}' with {} shells",
                solid.id, solid.shells
            ));
        }
    }
    face_counts(part)?;
    for plane in &part.planes {
        if plane.spread_mm < 0.0 {
            return Err(format!("plane '{}' has a negative spread", plane.id));
        }
        let mut sum = Some(0.0);
        for face in &plane.faces {
            let area = outline(&face.outline, &ids)
                .map_err(|problem| format!("face #{} of '{}': {problem}", face.face, plane.id))?;
            if !same(area, face.area_mm2) {
                return Err(format!(
                    "face #{} of '{}' states an area its outline does not give",
                    face.face, plane.id
                ));
            }
            sum = sum.zip(face.area_mm2).map(|(s, a)| s + a);
        }
        if !same(sum, plane.area_mm2) {
            return Err(format!(
                "plane '{}' states an area that is not its faces' sum",
                plane.id
            ));
        }
    }
    let lengths = part
        .cylinders
        .iter()
        .map(|c| (c.id, [c.radius_mm, c.spread_mm]))
        .chain(
            part.cones
                .iter()
                .map(|c| (c.id, [c.semi_angle_rad, c.spread_mm])),
        )
        .chain(part.tori.iter().map(|t| {
            (
                t.id,
                [t.minor_radius_mm.min(t.major_radius_mm), t.spread_mm],
            )
        }));
    for (id, [size, spread]) in lengths {
        if size <= 0.0 || spread < 0.0 {
            return Err(format!(
                "feature '{id}' has a size that is not positive, or a negative spread"
            ));
        }
    }
    cylinders(part, &ids)?;
    if !part
        .notes
        .windows(2)
        .all(|pair| (pair[0].instance, pair[0].code) <= (pair[1].instance, pair[1].code))
    {
        return Err("the notes are not sorted by #id, then code".to_owned());
    }
    Ok(())
}

/// Each cylinder's stations ascend, its class is in range, its near misses
/// are other cylinders of the part, and only a full one has wall openings.
fn cylinders(part: &Part, ids: &BTreeSet<FeatureId>) -> Checked {
    for cylinder in &part.cylinders {
        let id = cylinder.id;
        let sorted = cylinder.stations_mm.windows(2).all(|w| w[0] < w[1]);
        let class = match &cylinder.class {
            CylinderClass::FullBySeam { proven_extent_mm } => {
                proven_extent_mm[0] == 0.0 && proven_extent_mm[1] > 0.0
            }
            CylinderClass::Partial {
                sweep_rad: Some(sweep),
            } => *sweep >= 0.0 && *sweep <= TAU,
            _ => true,
        };
        let near = cylinder
            .near_misses
            .iter()
            .all(|n| *n != id && n.kind() == FeatureKind::Cylinder && ids.contains(n));
        let openings = cylinder.wall_openings == 0
            || matches!(
                cylinder.class,
                CylinderClass::Full { .. } | CylinderClass::FullOneEndIrregular { .. }
            );
        if !(sorted && class && near && openings) {
            return Err(format!(
                "cylinder '{id}' has unsorted stations, a class out of range, a near miss that is not another cylinder of the part, or wall openings while not full"
            ));
        }
    }
    Ok(())
}

/// True if two optional values are both absent, or agree within
/// [`SUM_REL`].
fn same(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => (a - b).abs() <= SUM_REL * a.abs().max(b.abs()).max(1.0),
        _ => false,
    }
}

/// The face counts by surface kind agree with the feature lists.
fn face_counts(part: &Part) -> Checked {
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let counts = &part.face_counts;
    let listed = [
        (
            "plane",
            counts.plane,
            count(part.planes.iter().map(|p| p.faces.len()).sum()),
        ),
        (
            "cylindrical",
            counts.cylindrical,
            count(part.cylinders.iter().map(|c| c.faces.len()).sum()),
        ),
        (
            "conical",
            counts.conical,
            count(part.cones.iter().map(|c| c.faces.len()).sum()),
        ),
        (
            "toroidal",
            counts.toroidal,
            count(part.tori.iter().map(|t| t.faces.len()).sum()),
        ),
        (
            "spherical and other",
            counts.spherical.saturating_add(counts.other),
            count(part.other_faces.len()),
        ),
    ];
    for (kind, stated, found) in listed {
        if stated != found {
            return Err(format!(
                "face_counts states {stated} {kind} faces, the features hold {found}"
            ));
        }
    }
    Ok(())
}

/// A face's outline is consistent, and gives this area: `None` if it is
/// unknown, else the sum of its loops'.
fn outline(outline: &Outline, ids: &BTreeSet<FeatureId>) -> Result<Option<f64>, String> {
    let Outline::Known { loops } = outline else {
        return Ok(None);
    };
    let Some((outer, inner)) = loops.split_first() else {
        return Err("a known outline has no loop".to_owned());
    };
    if !outer.outer || outer.area_mm2 <= 0.0 {
        return Err("the first loop is not an outer loop of positive area".to_owned());
    }
    if inner.iter().any(|l| l.outer || l.area_mm2 > 0.0) {
        return Err("a loop after the first is outer, or has positive area".to_owned());
    }
    for lp in loops {
        segments(lp, ids)?;
    }
    Ok(Some(loops.iter().map(|l| l.area_mm2).sum()))
}

/// A loop's segments chain end to start and close, each arc has a positive
/// radius and a sweep in [−2π, 2π], and each names a feature of the part
/// across it.
fn segments(lp: &OutlineLoop, ids: &BTreeSet<FeatureId>) -> Checked {
    let ends = |segment: &Segment| -> (Point3, Point3, FeatureId) {
        match segment {
            Segment::Line {
                from_mm,
                to_mm,
                across,
                ..
            }
            | Segment::Arc {
                from_mm,
                to_mm,
                across,
                ..
            } => (*from_mm, *to_mm, *across),
        }
    };
    let bound = lp.bound;
    let Some(last) = lp.segments.last() else {
        return Err(format!("loop #{bound} has no segment"));
    };
    let mut previous = ends(last).1;
    for segment in &lp.segments {
        let (from, to, across) = ends(segment);
        if from != previous {
            return Err(format!("loop #{bound}'s segments do not chain"));
        }
        if !ids.contains(&across) {
            return Err(format!(
                "loop #{bound} names '{across}' across an edge, which the part does not have"
            ));
        }
        if let Segment::Arc {
            radius_mm,
            sweep_rad,
            ..
        } = segment
        {
            if *radius_mm <= 0.0 || sweep_rad.abs() > TAU {
                return Err(format!(
                    "loop #{bound} has an arc of radius {radius_mm} sweeping {sweep_rad} rad"
                ));
            }
        }
        previous = to;
    }
    Ok(())
}
