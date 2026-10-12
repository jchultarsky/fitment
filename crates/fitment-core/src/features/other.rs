//! Cones, tori and the faces no rule reads (`docs/research/m2-features.md`,
//! sections 8.6 and 8.7), and items of the shape that are not solids.

use serde::{Deserialize, Serialize};

use super::FeatureId;
use super::cylinder::{Axis, Sense};
use crate::geom::Point3;

/// Edge-connected faces on one cone with one sense.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Cone {
    /// `cone-<smallest face #id>`.
    pub id: FeatureId,
    /// The solid it bounds.
    pub solid: FeatureId,
    /// Its shell: 0 for the outer shell, k for the k-th void.
    pub shell: u32,
    /// Its faces' `#id`s, sorted.
    pub faces: Vec<u64>,
    /// The reference face's conical surface `#id`.
    pub surface: u64,
    /// Material outside the cone (a countersink) or inside it (a chamfer
    /// on a shaft).
    pub sense: Sense,
    /// The reference surface's axis, through its stated location, along
    /// the direction in which the cone widens.
    pub axis: Axis,
    /// The apex.
    pub apex_mm: Point3,
    /// The semi-angle, between the axis and the surface.
    pub semi_angle_rad: f64,
    /// The smallest and largest distance of its faces' boundaries from the
    /// axis, from their vertices and coaxial circles; `None` if a boundary
    /// edge is not a line or a circle about the axis.
    #[serde(deserialize_with = "Option::deserialize")]
    pub radii_mm: Option<[f64; 2]>,
    /// The largest distance of a member's apex from the reference's.
    pub spread_mm: f64,
}

/// Edge-connected faces on one torus with one sense.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Torus {
    /// `torus-<smallest face #id>`.
    pub id: FeatureId,
    /// The solid it bounds.
    pub solid: FeatureId,
    /// Its shell: 0 for the outer shell, k for the k-th void.
    pub shell: u32,
    /// Its faces' `#id`s, sorted.
    pub faces: Vec<u64>,
    /// The reference face's toroidal surface `#id`.
    pub surface: u64,
    /// True if the material lies inside the tube (an edge round), false if
    /// outside it (a fillet).
    pub convex: bool,
    /// The reference surface's axis, through its centre.
    pub axis: Axis,
    /// The distance from the axis to the tube's centre circle.
    pub major_radius_mm: f64,
    /// The tube's radius.
    pub minor_radius_mm: f64,
    /// The largest distance of a member's centre or radii from the
    /// reference's.
    pub spread_mm: f64,
}

/// Why a face is an [`OtherFace`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OtherReason {
    /// No M2 rule reads its surface: a sphere, or a surface stepq does not
    /// read.
    NoRule,
}

/// A face no rule reads: a sphere or an unread surface. Each is its own
/// feature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct OtherFace {
    /// `other-<face #id>`.
    pub id: FeatureId,
    /// The solid it bounds.
    pub solid: FeatureId,
    /// Its shell: 0 for the outer shell, k for the k-th void.
    pub shell: u32,
    /// The face's `#id`.
    pub face: u64,
    /// Its surface's `#id`.
    pub surface: u64,
    /// The surface's entity type, such as `B_SPLINE_SURFACE_WITH_KNOTS`.
    pub entity: String,
    /// Why no rule reads it.
    pub reason: OtherReason,
}
