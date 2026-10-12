//! Cylinders (`docs/research/m2-features.md`, section 8): faces grouped by
//! rules G1 and G2, with their sense (S1). Whether a group is full or
//! partial (F) arrives with M2's PR 5.

use serde::{Deserialize, Serialize};

use super::FeatureId;
use crate::geom::{Dir3, Point3};

/// An axis line: a point on it and its unit direction.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Axis {
    /// A point on the line.
    pub point_mm: Point3,
    /// Its direction.
    pub direction: Dir3,
}

/// Which side of a cylinder or cone the material lies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
// A wall has material on one side; there is no third.
#[allow(clippy::exhaustive_enums)]
pub enum Sense {
    /// The outward normal points at the axis: material lies outside, as
    /// around a hole.
    Hole,
    /// The outward normal points away from the axis: material lies inside,
    /// as in a shaft.
    Shaft,
}

impl Sense {
    /// The sense of a face whose outward normal is its surface's own
    /// (`sense` true), which for a cylinder or cone points away from the
    /// axis.
    pub(crate) fn of(sense: bool) -> Self {
        if sense { Self::Shaft } else { Self::Hole }
    }
}

/// Edge-connected faces on one cylinder with one sense.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Cylinder {
    /// `cylinder-<smallest face #id>`.
    pub id: FeatureId,
    /// The solid it bounds.
    pub solid: FeatureId,
    /// Its shell: 0 for the outer shell, k for the k-th void.
    pub shell: u32,
    /// Its faces' `#id`s, sorted.
    pub faces: Vec<u64>,
    /// The reference face's cylindrical surface `#id`: that of the group's
    /// first face in the file.
    pub surface: u64,
    /// Hole wall or shaft wall.
    pub sense: Sense,
    /// The reference surface's axis, as stated.
    pub axis: Axis,
    /// The reference surface's radius.
    pub radius_mm: f64,
    /// The largest distance of a member's radius or axis point from the
    /// reference's.
    pub spread_mm: f64,
}
