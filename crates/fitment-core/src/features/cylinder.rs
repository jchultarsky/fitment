//! Cylinders (`docs/research/m2-features.md`, section 8): faces grouped by
//! rules G1 and G2, with their sense (S1), and whether they wrap around
//! their axis (F, in `full.rs`).

use serde::{Deserialize, Serialize};

use super::FeatureId;
use crate::geom::{Dir3, Point3};

/// An axis line: a point on it and its unit direction.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// The reference surface's axis line, with the canonical sign (the
    /// largest component positive), through the lowest station, or the
    /// surface's stated location if there is none.
    pub axis: Axis,
    /// The reference surface's radius.
    pub radius_mm: f64,
    /// The largest distance of a member's radius or axis point from the
    /// reference's.
    pub spread_mm: f64,
    /// Whether the faces wrap around the axis.
    pub class: CylinderClass,
    /// The stations, where end circles bound the faces, as distances along
    /// the axis from its point, ascending.
    pub stations_mm: Vec<f64>,
    /// How many openings cross the wall between the stations, such as a
    /// cross hole. Counted on full cylinders only.
    pub wall_openings: u32,
    /// The cylinders that share an edge with this one and lie on its
    /// surface within the stated uncertainty, but not within 1e-6 mm.
    pub near_misses: Vec<FeatureId>,
}

/// Whether a cylinder's faces wrap all the way around its axis (rule F,
/// section 8.3).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum CylinderClass {
    /// Every station is a full circle and the wall stays between the
    /// lowest and the highest: both ends are known.
    #[non_exhaustive]
    Full {},
    /// Every station is a full circle, but the wall runs past the stations
    /// at one end, where it ends on curves no rule reads.
    #[non_exhaustive]
    FullOneEndIrregular {
        /// The end the wall runs past.
        irregular: End,
    },
    /// The faces do not wrap around the axis: an axial line bounds them,
    /// or a station is not a full circle.
    #[non_exhaustive]
    Partial {
        /// The largest angle one station's arcs cover, or `None` with no
        /// end circle.
        #[serde(deserialize_with = "Option::deserialize")]
        sweep_rad: Option<f64>,
    },
    /// No rule decides.
    #[non_exhaustive]
    Undecided {
        /// Why.
        reason: UndecidedReason,
    },
}

impl CylinderClass {
    /// True if the faces are known to wrap around the axis: full, or with
    /// one end irregular.
    pub fn is_full(&self) -> bool {
        matches!(self, Self::Full {} | Self::FullOneEndIrregular { .. })
    }
}

/// One end of a cylinder along its axis direction.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
// An axis has two ends.
#[allow(clippy::exhaustive_enums)]
pub enum End {
    /// The end at the lowest station.
    Start,
    /// The end at the highest station.
    Finish,
}

/// Why a cylinder is [`CylinderClass::Undecided`].
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UndecidedReason {
    /// No end circle: curves no rule reads, or a seam, which proves
    /// nothing without the curves on the surface (review of PR 5).
    NoEndCircle,
    /// Every station is full, but the wall runs past them at both ends.
    IrregularAtBothEnds,
    /// The lowest station's arcs say the wall lies below it, or the
    /// highest's that it lies above.
    EndsDisagree,
}
