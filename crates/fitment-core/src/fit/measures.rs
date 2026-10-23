//! What a fit verifier's kernel measures (`docs/research/m7-fit.md`, FH4,
//! FC1): numbers, alert names and booleans, never a status. The helper's
//! `scene`, `place` and `check` replies, decoded by `fitment-fit`, are
//! these; [`super::decide`] alone turns them into a fit's status (FV1).
//! Every list is in the order the request gave.

/// An axis-aligned box in the root frame, in mm.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize)]
// Two corners; a box has nothing else.
#[allow(clippy::exhaustive_structs)]
pub struct FitBox {
    /// The smallest x, y and z.
    pub min_mm: [f64; 3],
    /// The largest.
    pub max_mm: [f64; 3],
}

impl FitBox {
    /// This box grown by `by` on every side.
    #[must_use]
    pub fn grown(&self, by: f64) -> Self {
        Self {
            min_mm: self.min_mm.map(|x| x - by),
            max_mm: self.max_mm.map(|x| x + by),
        }
    }

    /// True if the two boxes share a point (their faces included).
    pub fn meets(&self, other: &Self) -> bool {
        (0..3).all(|i| self.min_mm[i] <= other.max_mm[i] && other.min_mm[i] <= self.max_mm[i])
    }

    /// True if every value is finite and no minimum is above its maximum.
    pub fn sound(&self) -> bool {
        (0..3).all(|i| {
            self.min_mm[i].is_finite()
                && self.max_mm[i].is_finite()
                && self.min_mm[i] <= self.max_mm[i]
        })
    }

    /// The largest distance between a corner of this box and the same
    /// corner of `other`.
    pub(crate) fn apart(&self, other: &Self) -> f64 {
        (0..3)
            .map(|i| {
                (self.min_mm[i] - other.min_mm[i])
                    .abs()
                    .max((self.max_mm[i] - other.max_mm[i]).abs())
            })
            .fold(0.0, f64::max)
    }
}

/// A landmark face as the kernel read it (FB3), in the root frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub enum LandmarkMeasure {
    /// The face was not found, or is not of the kind fitment named.
    #[default]
    Missing,
    /// A plane: its outward normal and offset.
    #[non_exhaustive]
    Plane {
        /// The outward normal.
        normal: [f64; 3],
        /// The offset along it, in mm.
        offset_mm: f64,
    },
    /// A cylinder: a point of its axis, its direction and its radius.
    #[non_exhaustive]
    Cylinder {
        /// A point on the axis.
        point_mm: [f64; 3],
        /// Its direction.
        direction: [f64; 3],
        /// The radius, in mm.
        radius_mm: f64,
    },
}

impl LandmarkMeasure {
    /// A plane's measure.
    pub fn plane(normal: [f64; 3], offset_mm: f64) -> Self {
        Self::Plane { normal, offset_mm }
    }

    /// A cylinder's measure.
    pub fn cylinder(point_mm: [f64; 3], direction: [f64; 3], radius_mm: f64) -> Self {
        Self::Cylinder {
            point_mm,
            direction,
            radius_mm,
        }
    }
}

/// The kernel's reading of one of core's bodies (FB2, FB3, FT6).
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct BodyMeasure {
    /// The body's path, as core named it.
    pub path: Vec<u64>,
    /// True if exactly one leaf binds to its faces and transform.
    pub found: bool,
    /// True if `BRepCheck_Analyzer` finds it valid.
    pub valid: bool,
    /// Its volume, in mm³.
    pub volume_mm3: f64,
    /// Its largest sub-shape tolerance, in mm.
    pub tolerance_mm: f64,
    /// Its box.
    pub bounds: FitBox,
    /// Its landmarks, in core's order.
    pub landmarks: Vec<LandmarkMeasure>,
}

/// One solid the kernel transferred from the assembly, claimed or not
/// (FB6).
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct SolidMeasure {
    /// Its faces' `#id`s, sorted.
    pub faces: Vec<u64>,
    /// Its composed location, as three rows of `[R | t]`.
    pub location: [[f64; 4]; 3],
    /// Its box.
    pub bounds: FitBox,
    /// True for a shape of a leaf that is not a solid (a shell, a face, a
    /// wire, …), reported with no faces so no body claims it: within reach
    /// of a placement it is `unclaimed_body` (FB6).
    pub non_solid: bool,
}

/// The `scene` reply (FH4).
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct SceneMeasures {
    /// The SHA-256 of the request, as the reply echoes it
    /// ([`super::FitScene::request_digest`]).
    pub request_sha256: Option<String>,
    /// One per body core sent, in its order.
    pub bodies: Vec<BodyMeasure>,
    /// Every solid the reader transferred.
    pub solids: Vec<SolidMeasure>,
}

/// The candidate at one placement (FB3, FT8).
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct PlacementMeasure {
    /// The occurrence it is placed at.
    pub occurrence: Vec<u64>,
    /// The placed candidate's box.
    pub bounds: FitBox,
    /// Its landmarks placed, in core's order.
    pub landmarks: Vec<LandmarkMeasure>,
}

/// The `place` reply (FH4).
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct PlaceMeasures {
    /// The SHA-256 of the request, as the reply echoes it
    /// ([`super::PlanInput::place_digest`]).
    pub request_sha256: Option<String>,
    /// True if the candidate is a valid solid.
    pub valid: bool,
    /// Its volume, in mm³.
    pub volume_mm3: f64,
    /// Its largest sub-shape tolerance, in mm.
    pub tolerance_mm: f64,
    /// The SHA-256 of the bytes the helper received, for a file
    /// candidate (FB4).
    pub sha256: Option<String>,
    /// One per placement core sent, in its order.
    pub placements: Vec<PlacementMeasure>,
}

/// A zone as the kernel built it (FZ7).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct ZoneMeasure {
    /// Its volume, in mm³.
    pub volume_mm3: f64,
    /// Its centroid.
    pub centroid_mm: [f64; 3],
    /// Its box.
    pub bounds: FitBox,
}

impl ZoneMeasure {
    /// A built zone's measure.
    pub fn new(volume_mm3: f64, centroid_mm: [f64; 3], bounds: FitBox) -> Self {
        Self {
            volume_mm3,
            centroid_mm,
            bounds,
        }
    }
}

/// One pair's measures (FT4, FT5, FY4).
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct PairMeasure {
    /// V: the Common's volume, in mm³.
    pub common_mm3: f64,
    /// R: V less every zone at ε, in mm³.
    pub residual_mm3: f64,
    /// V's volume within the zones, in mm³.
    pub inside_mm3: f64,
    /// R's box, when R has volume.
    pub residual_box: Option<FitBox>,
    /// The distance from the candidate to the body; `None` beyond the
    /// pair's threshold.
    pub distance_mm: Option<f64>,
    /// The distance to the body less its zones at ε + threshold; `None`
    /// when not asked.
    pub zoned_distance_mm: Option<f64>,
    /// The body's volume left by that cut, in mm³.
    pub body_kept_mm3: Option<f64>,
    /// Its volume within those zones, in mm³.
    pub body_inside_mm3: Option<f64>,
    /// True if either body has a face within reach that is not analytic.
    pub non_analytic: bool,
    /// The kernel's alerts, by class name, sorted.
    pub alerts: Vec<String>,
    /// The pair's zones at ε as built, in the plan's order.
    pub zones: Vec<ZoneMeasure>,
    /// Its zones at ε + threshold as built, when the zoned distance was
    /// asked; empty otherwise.
    pub proximity_zones: Vec<ZoneMeasure>,
}

/// The helper's own claim of reach for one scene body at one placement
/// (FT8).
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct ReachClaim {
    /// The scene body.
    pub body: Vec<u64>,
    /// The placement's occurrence.
    pub occurrence: Vec<u64>,
    /// True if the helper's boxes are within the pair's reach.
    pub within: bool,
}

impl ReachClaim {
    /// A claim for `body` at `occurrence`.
    pub fn new(body: Vec<u64>, occurrence: Vec<u64>, within: bool) -> Self {
        Self {
            body,
            occurrence,
            within,
        }
    }
}

/// The `check` reply (FH4).
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct CheckMeasures {
    /// The SHA-256 of the request, as the reply echoes it
    /// ([`super::FitPlan::check_digest`]).
    pub request_sha256: Option<String>,
    /// One per pair the plan asked, in its order.
    pub pairs: Vec<PairMeasure>,
    /// One per scene body that is not selected and placement.
    pub reach: Vec<ReachClaim>,
}

/// Everything the kernel measured for one candidate (FC1).
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct FitMeasures {
    /// The `scene` reply.
    pub scene: SceneMeasures,
    /// The `place` reply.
    pub place: PlaceMeasures,
    /// The `check` reply.
    pub check: CheckMeasures,
}

impl FitMeasures {
    /// The three replies together.
    pub fn new(scene: SceneMeasures, place: PlaceMeasures, check: CheckMeasures) -> Self {
        Self {
            scene,
            place,
            check,
        }
    }
}

/// How a helper failed to measure (FV8), as `fitment-fit` classifies its
/// end. None is ever `verified`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FitFailure {
    /// The check missed its deadline (FL4).
    Timeout,
    /// A resource limit ended the helper: `SIGXCPU`, `SIGXFSZ`, or a
    /// `SIGKILL` fitment did not send.
    ResourceExhausted,
    /// The job was cancelled.
    Cancelled,
    /// The helper crashed twice on this request.
    Crashed,
    /// A malformed frame, a reply that breaks the protocol, or a write
    /// that missed its deadline.
    Protocol,
    /// No checked helper can run, with why.
    Unavailable(String),
    /// A stored candidate's bytes could not be read or did not hash to
    /// their row (FC6).
    StoredBytesUnreadable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_meet_grow_and_compare() {
        let a = FitBox {
            min_mm: [0.0; 3],
            max_mm: [1.0; 3],
        };
        let b = FitBox {
            min_mm: [1.5, 0.0, 0.0],
            max_mm: [2.0, 1.0, 1.0],
        };
        assert!(!a.meets(&b));
        assert!(a.grown(0.25).meets(&b.grown(0.25)));
        assert!(a.meets(&a));
        assert!(a.sound());
        assert!(
            !FitBox {
                min_mm: [1.0, 0.0, 0.0],
                max_mm: [0.0, 1.0, 1.0],
            }
            .sound()
        );
        assert!((a.apart(&a.grown(0.5)) - 0.5).abs() < 1e-15);
    }
}
