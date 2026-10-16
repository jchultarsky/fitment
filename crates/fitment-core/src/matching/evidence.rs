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
/// apply are set, and only they are written.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[non_exhaustive]
pub struct At {
    /// A span, in the requirement's stations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_mm: Option<[f64; 2]>,
    /// A candidate segment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segment: Option<FeatureId>,
    /// A seat's contact face, by the target's `#id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact_face: Option<u64>,
    /// An end.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<EndSide>,
    /// A station.
    #[serde(skip_serializing_if = "Option::is_none")]
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

    /// Its numbers stored: finite, never −0.0 (EV8).
    fn stored(mut self) -> Self {
        self.span_mm = self.span_mm.map(|s| s.map(finite));
        self.at_mm = self.at_mm.map(finite);
        self
    }
}

/// One comparison and its outcome (EV2).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Measure {
    quantity: Quantity,
    at: At,
    #[serde(flatten)]
    value: MeasureValue,
    passed: bool,
}

/// A band's width (EV2): `band_mm`, or `band_rad` for an angle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[non_exhaustive]
pub enum BandWidth {
    /// In millimetres.
    #[serde(rename = "band_mm")]
    Mm(f64),
    /// In radians.
    #[serde(rename = "band_rad")]
    Rad(f64),
}

/// What a measure found, against what.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum MeasureValue {
    /// A number between limits.
    #[non_exhaustive]
    Number {
        /// The candidate's value.
        found: f64,
        /// The socket's.
        nominal: Option<f64>,
        /// The band the rule used.
        band: Band,
        /// Its width.
        #[serde(flatten)]
        width: BandWidth,
        /// The effective lower limit; `None` when unbounded.
        min: Option<f64>,
        /// The effective upper limit.
        max: Option<f64>,
        /// The distance from `found` to the nearer limit, positive inside.
        margin: f64,
    },
    /// A fact required or not.
    #[non_exhaustive]
    Flag {
        /// What was found.
        found: bool,
        /// What the socket requires.
        required: bool,
    },
}

impl Measure {
    /// A numeric comparison: it passes exactly when `found` lies strictly
    /// inside the limits (an edge rejects). Everything is stored first,
    /// finite and never −0.0, and the margin and the result follow from
    /// what is stored, so the evidence alone recomputes them (EV7): a value
    /// that is not finite is stored as the extreme that violates a limit
    /// given, a limit that is not a number as one no value meets, and a
    /// measure with no limit fails.
    pub fn number(
        quantity: Quantity,
        at: At,
        (found, nominal): (f64, Option<f64>),
        (band, width): (Band, f64),
        (min, max): (Option<f64>, Option<f64>),
    ) -> Self {
        let min = min.map(|m| if m.is_nan() { f64::MAX } else { finite(m) });
        let max = max.map(|m| if m.is_nan() { f64::MIN } else { finite(m) });
        let found = if found.is_finite() {
            finite(found)
        } else if max.is_some() {
            f64::MAX
        } else {
            f64::MIN
        };
        let margin = match (min.map(|m| found - m), max.map(|m| m - found)) {
            (Some(below), Some(above)) => below.min(above),
            (Some(one), None) | (None, Some(one)) => one,
            (None, None) => f64::MIN,
        };
        let margin = finite(margin);
        let width = finite(width);
        Self {
            quantity,
            at: at.stored(),
            value: MeasureValue::Number {
                found,
                nominal: nominal.map(finite),
                band,
                width: if band == Band::Angle {
                    BandWidth::Rad(width)
                } else {
                    BandWidth::Mm(width)
                },
                min,
                max,
                margin,
            },
            passed: margin > 0.0,
        }
    }

    /// A fact: it passes when found, or when not required.
    pub fn flag(quantity: Quantity, at: At, found: bool, required: bool) -> Self {
        Self {
            quantity,
            at: at.stored(),
            value: MeasureValue::Flag { found, required },
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
    pub fn value(&self) -> &MeasureValue {
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
            at: at.stored(),
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
    /// Under D6 functional, a clearance that strict would have rejected
    /// (VD12's `larger_than_original`): not written, the flag says it.
    #[serde(skip)]
    departs: bool,
}

impl Evidence {
    /// The evidence of a check on `feature`.
    pub fn new(feature: Vec<FeatureId>, measures: Vec<Measure>, failures: Vec<Failure>) -> Self {
        Self {
            feature,
            measures,
            failures,
            warnings: Vec::new(),
            departs: false,
        }
    }

    /// This evidence, departing from the original as D6 functional allows.
    pub(crate) fn departing(mut self, departs: bool) -> Self {
        self.departs = departs;
        self
    }

    /// True if a clearance passed that D6 strict would have rejected.
    pub fn departs(&self) -> bool {
        self.departs
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

    /// This evidence with one more failure.
    pub(crate) fn with_failure(mut self, failure: Failure) -> Self {
        self.failures.push(failure);
        self
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
    use super::{At, Band, Measure, MeasureValue, Quantity};

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

    /// EV7: the result recomputes from what is stored, whatever came in:
    /// values and limits that are not finite, and −0.0.
    #[test]
    fn a_measure_recomputes_from_what_it_stores() {
        let cases = [
            (f64::NAN, None, Some(6.01)),
            (f64::NEG_INFINITY, None, Some(6.01)),
            (f64::INFINITY, Some(5.99), None),
            (6.0, Some(f64::NAN), Some(6.01)),
            (6.0, Some(5.99), Some(f64::NAN)),
            (-0.0, Some(-1.0), Some(1.0)),
        ];
        for (found, min, max) in cases {
            let m = Measure::number(
                Quantity::DiameterMm,
                At::whole(),
                (found, None),
                (Band::Epsilon, 0.01),
                (min, max),
            );
            let MeasureValue::Number {
                found,
                min,
                max,
                margin,
                ..
            } = *m.value()
            else {
                return;
            };
            let again = min
                .map_or(f64::INFINITY, |lo| found - lo)
                .min(max.map_or(f64::INFINITY, |hi| hi - found));
            assert!(found.is_finite() && found.to_bits() != (-0.0_f64).to_bits());
            assert_eq!(margin.to_bits(), again.to_bits());
            assert_eq!(m.passed(), again > 0.0);
        }
    }
}
