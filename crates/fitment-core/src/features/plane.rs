//! Planes (`docs/research/m2-features.md`, section 7).

use serde::{Deserialize, Serialize};

use super::FeatureId;
use super::outline::Outline;
use crate::geom::Dir3;

/// One infinite plane of the solid: coplanar faces of one shell with one
/// outward normal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Plane {
    /// `plane-<smallest face #id>`.
    pub id: FeatureId,
    /// The solid it bounds.
    pub solid: FeatureId,
    /// Its shell: 0 for the outer shell, k for the k-th void.
    pub shell: u32,
    /// The outward unit normal: that of the plane's direction class, whose
    /// smallest-`#id` face sets it.
    pub normal: Dir3,
    /// The plane is every x with `normal · x = offset_mm`: its smallest-`#id`
    /// face's offset.
    pub offset_mm: f64,
    /// The largest minus the smallest face offset, at most 1e-6 mm.
    pub spread_mm: f64,
    /// The sum of its faces' areas, or `None` if an outline is unknown.
    #[serde(deserialize_with = "Option::deserialize")]
    pub area_mm2: Option<f64>,
    /// Its faces, sorted by `#id`.
    pub faces: Vec<PlaneFace>,
}

/// One face of a plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PlaneFace {
    /// The face's `#id`.
    pub face: u64,
    /// Its plane surface's `#id`.
    pub surface: u64,
    /// Its own offset along the plane's normal, measured on its own plane
    /// at its first vertex.
    pub offset_mm: f64,
    /// Its net area, or `None` if its outline is unknown.
    #[serde(deserialize_with = "Option::deserialize")]
    pub area_mm2: Option<f64>,
    /// Its outline.
    pub outline: Outline,
}
