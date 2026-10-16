//! What one check records (`docs/research/m4-match.md`, section 10): the
//! comparisons it made, every failure it found, and the candidate features
//! it judged. A check's result follows from its evidence alone (EV7).

use serde::Serialize;

use crate::features::FeatureId;

/// What a measure compares (EV2): a closed list whose names never change
/// meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Quantity {
    /// A candidate axis's distance from the requirement's.
    AxisOffsetMm,
    /// The sine of the angle between the two axes.
    AxisSine,
    /// A diameter against a pinned value or a strict maximum.
    DiameterMm,
    /// A clearance span's radial margin beyond the clear diameter.
    ClearMarginMm,
    /// A zone's profile against the original's.
    ProfileOffsetMm,
    /// A face's least distance from a required core (VR22).
    CoreDistanceMm,
    /// Where a usable wall starts, along the requirement's axis.
    WallFromMm,
    /// Where it ends.
    WallToMm,
    /// A plane's offset from a seat's.
    PlaneOffsetMm,
    /// The sine of the angle between a plane's normal and a seat's.
    PlaneSine,
    /// A grip's separation.
    GripLengthMm,
    /// A cone's apex from the conical seat's.
    ApexOffsetMm,
    /// A cone's included angle.
    ConeAngleRad,
    /// A cone's radial range against the seat's.
    ConeRangeMm,
    /// Whether a seat's region is covered.
    Covers,
    /// Whether the start of a hole is open.
    OpenStart,
    /// Whether its finish is.
    OpenFinish,
    /// Whether a span crosses a wall opening that is no cross hole.
    WallOpening,
}

/// Which band a rule compared within (EV2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Band {
    /// VR3's ε: the tolerance less the parts' own uncertainty.
    Epsilon,
    /// M3's τ.
    Tau,
    /// VR4's τ + u.
    TauGuard,
    /// The angular tolerance less its guard.
    Angle,
}

/// Which end of a stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EndSide {
    /// The end at the lowest station.
    Start,
    /// The end at the highest.
    Finish,
}

/// Where in a requirement a measure or failure applies; whichever fields
/// apply are set.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[non_exhaustive]
pub struct At {
    /// A span, in the requirement's stations.
    pub span_mm: Option<[f64; 2]>,
    /// A candidate segment.
    pub segment: Option<FeatureId>,
    /// A seat's contact face, by the target's `#id`.
    pub contact_face: Option<u64>,
    /// An end.
    pub end: Option<EndSide>,
    /// A station.
    pub at_mm: Option<f64>,
}

impl At {
    /// Nowhere in particular: the requirement as a whole.
    pub fn whole() -> Self {
        Self::default()
    }

    /// A span of the requirement.
    pub fn span(from: f64, to: f64) -> Self {
        Self {
            span_mm: Some([from, to]),
            ..Self::default()
        }
    }
}

/// One comparison and its outcome (EV2).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Measure {
    quantity: Quantity,
    at: At,
    #[serde(flatten)]
    value: Value,
    passed: bool,
}

/// What a measure found, against what.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum Value {
    /// A number between limits.
    Number {
        /// The candidate's value.
        found: f64,
        /// The socket's.
        nominal: Option<f64>,
        /// The band the rule used.
        band: Band,
        /// Its width: mm, or radians for an angle.
        band_value: f64,
        /// The effective lower limit; `None` when unbounded.
        min: Option<f64>,
        /// The effective upper limit.
        max: Option<f64>,
        /// The distance from `found` to the nearer limit, positive inside.
        margin: f64,
    },
    /// A fact required or not.
    Flag {
        /// What was found.
        found: bool,
        /// What the socket requires.
        required: bool,
    },
}

impl Measure {
    /// A numeric comparison: it passes exactly when `found` lies strictly
    /// inside the limits (an edge rejects); at least one limit is given,
    /// and a value that is not finite fails.
    pub fn number(
        quantity: Quantity,
        at: At,
        (found, nominal): (f64, Option<f64>),
        (band, band_value): (Band, f64),
        (min, max): (Option<f64>, Option<f64>),
    ) -> Self {
        let below = min.map_or(f64::INFINITY, |m| found - m);
        let above = max.map_or(f64::INFINITY, |m| m - found);
        let margin = below.min(above);
        let margin = if margin.is_finite() {
            margin
        } else {
            f64::NEG_INFINITY
        };
        let passed = margin > 0.0;
        Self {
            quantity,
            at,
            value: Value::Number {
                found: finite(found),
                nominal: nominal.map(finite),
                band,
                band_value: finite(band_value),
                min: min.map(finite),
                max: max.map(finite),
                margin: finite(margin),
            },
            passed,
        }
    }

    /// A fact: it passes when found, or when not required.
    pub fn flag(quantity: Quantity, at: At, found: bool, required: bool) -> Self {
        Self {
            quantity,
            at,
            value: Value::Flag { found, required },
            passed: found || !required,
        }
    }

    /// What it compares.
    pub fn quantity(&self) -> Quantity {
        self.quantity
    }

    /// Where.
    pub fn at(&self) -> &At {
        &self.at
    }

    /// Its values.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Whether it passed.
    pub fn passed(&self) -> bool {
        self.passed
    }
}

/// A stored number: finite, never −0.0 (a non-finite one stated as the
/// largest finite of its sign, which fails any limit).
fn finite(x: f64) -> f64 {
    if x.is_nan() {
        f64::MIN
    } else if x.is_infinite() {
        if x > 0.0 { f64::MAX } else { f64::MIN }
    } else {
        x + 0.0
    }
}

/// Why a check failed (section 8.5): a closed list whose names never
/// change meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[non_exhaustive]
pub enum FailureCode {
    /// No candidate can pass it (VR14, VD5); the detail gives the reason.
    #[serde(rename = "unverifiable")]
    Unverifiable,
    /// The tolerance leaves no room above the parts' uncertainty (VD6).
    #[serde(rename = "floor_above_tolerance")]
    FloorAboveTolerance,
    /// No anchor matched (KH3).
    #[serde(rename = "no_placement")]
    NoPlacement,
    /// The placements' bound was reached (AL11).
    #[serde(rename = "alignment_exhausted")]
    AlignmentExhausted,
    /// The coverage steps' bound was reached (AL11).
    #[serde(rename = "coverage_exhausted")]
    CoverageExhausted,
    /// No candidate feature on the axis or plane.
    #[serde(rename = "no_feature")]
    NoFeature,
    /// A feature off the axis beyond tolerance.
    #[serde(rename = "axis_offset")]
    AxisOffset,
    /// A feature at an angle beyond tolerance.
    #[serde(rename = "axis_angle")]
    AxisAngle,
    /// A diameter below its least.
    #[serde(rename = "diameter_out_of_limits:min")]
    DiameterMin,
    /// A diameter above its greatest.
    #[serde(rename = "diameter_out_of_limits:max")]
    DiameterMax,
    /// A zone's profile off the original's.
    #[serde(rename = "diameter_out_of_limits:profile")]
    DiameterProfile,
    /// No usable wall over part of a span.
    #[serde(rename = "wall_missing")]
    WallMissing,
    /// An end required open is not.
    #[serde(rename = "end_not_open")]
    EndNotOpen,
    /// A span crosses a wall opening that is no cross hole (CF8).
    #[serde(rename = "wall_opening")]
    WallOpening,
    /// A face may meet a required core (VR22).
    #[serde(rename = "core_intruded")]
    CoreIntruded,
    /// A seat's region is not covered.
    #[serde(rename = "contact_not_covered")]
    ContactNotCovered,
    /// The candidate face there has an unknown outline.
    #[serde(rename = "unknown_outline")]
    UnknownOutline,
    /// The only candidate feature there is one M2 left unknown.
    #[serde(rename = "candidate_feature_unknown")]
    CandidateFeatureUnknown,
    /// A grip's seat has no plane.
    #[serde(rename = "seat_missing")]
    SeatMissing,
    /// A grip's separation differs.
    #[serde(rename = "grip_length")]
    GripLength,
    /// A cone's apex.
    #[serde(rename = "cone_mismatch:apex")]
    ConeApex,
    /// A cone's angle.
    #[serde(rename = "cone_mismatch:angle")]
    ConeAngle,
    /// A cone's radial range.
    #[serde(rename = "cone_mismatch:range")]
    ConeRange,
    /// A cone's sense.
    #[serde(rename = "cone_mismatch:sense")]
    ConeSense,
}

/// One failure of a check (EV3).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Failure {
    /// What failed.
    pub code: FailureCode,
    /// Where.
    pub at: At,
    /// A sentence for people, from the values.
    pub detail: String,
}

impl Failure {
    /// A failure of `code` at `at`.
    pub fn new(code: FailureCode, at: At, detail: impl Into<String>) -> Self {
        Self {
            code,
            at,
            detail: detail.into(),
        }
    }
}

/// A warning a passing check carries (VD12, VD13): reserved in M4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WarningCode {
    /// M7's D7 warning: a grip's separation differs.
    GripDiffers,
}

/// One warning, with the measure behind it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Warning {
    /// Which.
    pub code: WarningCode,
    /// What was measured.
    pub measure: Measure,
}

/// What one check records (EV1): the candidate features it judged, every
/// comparison and every failure.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[non_exhaustive]
pub struct Evidence {
    feature: Vec<FeatureId>,
    measures: Vec<Measure>,
    failures: Vec<Failure>,
    warnings: Vec<Warning>,
}

impl Evidence {
    /// The evidence of a check on `feature`.
    pub fn new(feature: Vec<FeatureId>, measures: Vec<Measure>, failures: Vec<Failure>) -> Self {
        Self {
            feature,
            measures,
            failures,
            warnings: Vec::new(),
        }
    }

    /// A check that failed for `code` alone, on no feature: an
    /// unverifiable requirement, or no placement.
    pub fn failed(code: FailureCode, detail: impl Into<String>) -> Self {
        Self::new(
            Vec::new(),
            Vec::new(),
            vec![Failure::new(code, At::whole(), detail)],
        )
    }

    /// The candidate features judged.
    pub fn feature(&self) -> &[FeatureId] {
        &self.feature
    }

    /// The comparisons.
    pub fn measures(&self) -> &[Measure] {
        &self.measures
    }

    /// The failures.
    pub fn failures(&self) -> &[Failure] {
        &self.failures
    }

    /// The warnings; empty in M4.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// True if it shows a pass: no failure, and every measure passed.
    pub fn passes(&self) -> bool {
        self.failures.is_empty() && self.measures.iter().all(Measure::passed)
    }
}

#[cfg(test)]
mod tests {
    use super::{At, Band, Measure, Quantity};

    /// EV2: a measure passes exactly when its margin is positive; the edge
    /// rejects; a value with no limit or not finite fails.
    #[test]
    fn a_measure_passes_strictly_inside_its_limits() {
        let at = || At::span(0.0, 5.0);
        let band = (Band::Epsilon, 0.01);
        let inside = Measure::number(
            Quantity::DiameterMm,
            at(),
            (6.0, Some(6.0)),
            band,
            (Some(5.99), Some(6.01)),
        );
        assert!(inside.passed());
        let edge = Measure::number(
            Quantity::DiameterMm,
            at(),
            (6.01, Some(6.0)),
            band,
            (Some(5.99), Some(6.01)),
        );
        assert!(!edge.passed());
        let one_sided = Measure::number(
            Quantity::ClearMarginMm,
            at(),
            (0.3, None),
            band,
            (Some(0.01), None),
        );
        assert!(one_sided.passed());
        let unbounded =
            Measure::number(Quantity::DiameterMm, at(), (6.0, None), band, (None, None));
        assert!(!unbounded.passed());
        let nan = Measure::number(
            Quantity::DiameterMm,
            at(),
            (f64::NAN, None),
            band,
            (Some(0.0), None),
        );
        assert!(!nan.passed());
        assert!(Measure::flag(Quantity::OpenStart, at(), false, false).passed());
        assert!(!Measure::flag(Quantity::OpenStart, at(), false, true).passed());
    }
}
