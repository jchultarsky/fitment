//! A plane face's outline, exact and never sampled
//! (`docs/research/m2-features.md`, section 7.3): its loops, each a chain
//! of the stated lines and arcs, with signed areas and the feature across
//! each edge.

use serde::{Deserialize, Serialize};

use super::FeatureId;
use super::group::IDENTITY_MM;
use super::topo::{Curve, Face, Loop, P, Topo, Travel, cross, dot, length, sub};
use crate::geom::Point3;

/// A plane face's outline: known, or unknown with the reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Outline {
    /// Every edge is a line or an arc in the face's plane, and exactly one
    /// loop has positive area.
    #[non_exhaustive]
    Known {
        /// The outer loop first, then the inner loops as the face lists
        /// its bounds.
        loops: Vec<OutlineLoop>,
    },
    /// The outline cannot be stated exactly. The plane, its normal and
    /// offset, and the face stay; M3 counts an unknown outline as
    /// overlapping, and M4 as covering nothing.
    #[non_exhaustive]
    Unknown {
        /// Why.
        problem: OutlineProblem,
        /// The `#id` at fault: an edge, a bound, or the face.
        instance: u64,
    },
}

/// Why an outline is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OutlineProblem {
    /// An edge's curve is neither a line nor a circle.
    CurveNotRead,
    /// A circle does not lie in the face's plane: its axis is not the
    /// normal, or its centre is off the plane.
    CircleNotParallel,
    /// An arc's two distinct vertices lie within the stated uncertainty of
    /// each other, so its sweep is ambiguous.
    AmbiguousArc,
    /// Not exactly one loop has positive area.
    LoopOrientation,
    /// A loop is a single vertex.
    VertexLoop,
}

/// One loop of an outline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct OutlineLoop {
    /// The face bound's `#id`.
    pub bound: u64,
    /// True for the outer loop: the one with positive area.
    pub outer: bool,
    /// Its signed area about the outward normal, in mm²: positive for the
    /// outer loop, negative for a hole in the face.
    pub area_mm2: f64,
    /// Its edges in traversal order.
    pub segments: Vec<Segment>,
}

/// One edge of a loop, in the direction the loop runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Segment {
    /// A straight edge.
    #[non_exhaustive]
    Line {
        /// The edge's `#id`.
        edge: u64,
        /// Its start, as the vertex states it.
        from_mm: Point3,
        /// Its end.
        to_mm: Point3,
        /// The primary feature on the other side of the edge.
        across: FeatureId,
    },
    /// A circular edge.
    #[non_exhaustive]
    Arc {
        /// The edge's `#id`.
        edge: u64,
        /// Its start, as the vertex states it.
        from_mm: Point3,
        /// Its end; equal to the start for a whole circle.
        to_mm: Point3,
        /// The circle's centre.
        center_mm: Point3,
        /// Its radius.
        radius_mm: f64,
        /// The signed angle swept about the outward normal, in
        /// [−2π, 2π]: positive counterclockwise, ±2π for a whole circle.
        sweep_rad: f64,
        /// The primary feature on the other side of the edge.
        across: FeatureId,
    },
}

/// What [`outline`] found: the outline, the face's net area if known, and
/// the stated outer bound if it is not the loop with positive area.
pub(crate) struct Read {
    pub(crate) outline: Outline,
    pub(crate) area_mm2: Option<f64>,
    pub(crate) outer_differs: Option<u64>,
}

/// An outline that cannot be built: why, and the `#id` at fault.
type Problem = (OutlineProblem, u64);

/// Reads the outline of plane face `index` with outward unit `normal`.
/// `owner` gives each face's primary feature; `uncertainty` is the part's
/// largest stated length uncertainty, in mm. `None` only on an internal
/// fault: a value the gate passed that is not finite.
pub(crate) fn outline(
    topo: &Topo,
    index: usize,
    normal: P,
    owner: &[FeatureId],
    uncertainty: f64,
) -> Option<Read> {
    let face = topo.faces.get(index)?;
    match loops(topo, index, face, normal, owner, uncertainty) {
        Err((problem, instance)) => Some(Read {
            outline: Outline::Unknown { problem, instance },
            area_mm2: None,
            outer_differs: None,
        }),
        Ok(None) => None,
        Ok(Some((loops, stated_outer))) => {
            let area: f64 = loops.iter().map(|l| l.area_mm2).sum();
            let outer = loops.first().map(|l| l.bound);
            let outer_differs = stated_outer.filter(|&stated| Some(stated) != outer);
            Some(Read {
                outline: Outline::Known { loops },
                area_mm2: Some(crate::geom::finite(area)?),
                outer_differs,
            })
        }
    }
}

/// The loops, outer first, and the bound the file states as outer, if any.
type Loops = (Vec<OutlineLoop>, Option<u64>);

fn loops(
    topo: &Topo,
    index: usize,
    face: &Face,
    normal: P,
    owner: &[FeatureId],
    uncertainty: f64,
) -> Result<Option<Loops>, Problem> {
    let location = match &face.surface {
        super::topo::Surface::Plane { location, .. } => *location,
        _ => return Ok(None),
    };
    let travels = topo.travels(face);
    // Every loop's area is measured from the face's first vertex, so that a
    // plane placed far from its face loses no digits.
    let Some(origin) = travels.iter().flatten().next().map(|t| t.from) else {
        return Err((OutlineProblem::LoopOrientation, face.instance));
    };
    let mut read = Vec::new();
    for (bound, uses) in face.bounds.iter().zip(&travels) {
        if let Loop::Vertex(_) = bound.lp {
            return Err((OutlineProblem::VertexLoop, bound.instance));
        }
        let mut segments = Vec::new();
        let mut area = 0.0;
        for travel in uses {
            let across = owner.get(topo.across(travel.edge, index)).copied();
            let (Some(edge), Some(across)) = (topo.edges.get(travel.edge), across) else {
                return Ok(None);
            };
            let Some((segment, term)) = segment(
                travel,
                (edge.instance, across),
                location,
                normal,
                uncertainty,
                origin,
            )?
            else {
                return Ok(None);
            };
            area += term;
            segments.push(segment);
        }
        let Some(area_mm2) = crate::geom::finite(area) else {
            return Ok(None);
        };
        read.push(OutlineLoop {
            bound: bound.instance,
            outer: area_mm2 > 0.0,
            area_mm2,
            segments,
        });
    }
    let positive: Vec<usize> = (0..read.len())
        .filter(|&i| read.get(i).is_some_and(|l| l.outer))
        .collect();
    let [outer] = positive[..] else {
        return Err((OutlineProblem::LoopOrientation, face.instance));
    };
    let first = read.remove(outer);
    read.insert(0, first);
    let stated = face
        .bounds
        .iter()
        .find(|b| b.stated_outer)
        .map(|b| b.instance);
    Ok(Some((read, stated)))
}

/// One edge use, with its edge's `#id` and the feature across it, as a
/// segment, and its term of the loop's area about `normal` from `origin`.
fn segment(
    travel: &Travel<'_>,
    (edge, across): (u64, FeatureId),
    location: P,
    normal: P,
    uncertainty: f64,
    origin: P,
) -> Result<Option<(Segment, f64)>, Problem> {
    let (Some(from_mm), Some(to_mm)) = (Point3::new(travel.from), Point3::new(travel.to)) else {
        return Ok(None);
    };
    let (a, b) = (sub(travel.from, origin), sub(travel.to, origin));
    match travel.curve {
        Curve::Line { .. } => {
            let term = dot(cross(a, b), normal) / 2.0;
            Ok(Some((
                Segment::Line {
                    edge,
                    from_mm,
                    to_mm,
                    across,
                },
                term,
            )))
        }
        Curve::Circle {
            centre, z, radius, ..
        } => {
            if length(cross(*z, normal)) * radius > IDENTITY_MM
                || dot(sub(*centre, location), normal).abs() > IDENTITY_MM
            {
                return Err((OutlineProblem::CircleNotParallel, edge));
            }
            if !travel.closed && length(sub(travel.from, travel.to)) <= uncertainty {
                return Err((OutlineProblem::AmbiguousArc, edge));
            }
            let sign = if dot(*z, normal) > 0.0 { 1.0 } else { -1.0 };
            let sweep_rad = travel.sweep(*centre, *z) * sign;
            let c = sub(*centre, origin);
            let term = dot(cross(c, sub(b, a)), normal) / 2.0 + radius * radius * sweep_rad / 2.0;
            let (Some(center_mm), Some(sweep_rad), Some(radius_mm)) = (
                Point3::new(*centre),
                crate::geom::finite(sweep_rad),
                crate::geom::finite(*radius),
            ) else {
                return Ok(None);
            };
            Ok(Some((
                Segment::Arc {
                    edge,
                    from_mm,
                    to_mm,
                    center_mm,
                    radius_mm,
                    sweep_rad,
                    across,
                },
                term,
            )))
        }
        Curve::Other { .. } => Err((OutlineProblem::CurveNotRead, edge)),
    }
}
