//! The fit's status from a kernel's measures (`docs/research/m7-fit.md`,
//! FV1 to FV9). [`decide`] is pure and the only builder of a `verified`
//! fit: every rule below is a branch of it, and a measure core can check
//! against its own reading, it checks (FB3, FB6, FZ7, FZ9, FY4). A helper
//! can make a fit `not_verified`; no reply can make one `verified` but one
//! in which every measure is clear and every cross-check holds.

use std::collections::BTreeSet;

use serde::Serialize;

use super::measures::{
    BodyMeasure, FitBox, FitFailure, FitMeasures, LandmarkMeasure, PairMeasure, PlaceMeasures,
    SceneMeasures, SolidMeasure, ZoneMeasure,
};
use super::plan::{FitPlan, Pair, PairState, PairValues, moved, within};
use super::scene::{Landmark, LandmarkValue};
use super::zones::Built;
use super::{FIT_VOLUME_MM3, FitStatus, Interference};
use crate::geom::Rigid;

/// Landmarks agree within this many mm, and this in direction (FB3).
const LANDMARK_MM: f64 = 1e-6;
const LANDMARK_SINE: f64 = 1e-9;
/// A leaf's location agrees with fitment's transform within these (FB2).
const LOCATION_ROTATION: f64 = 1e-9;
const LOCATION_MM: f64 = 1e-6;
/// A zone agrees with its closed form within these (FZ7).
const ZONE_VOLUME_REL: f64 = 1e-9;
const ZONE_MM: f64 = 1e-6;
/// Volumes are conserved across a cut within this, relative (FY4).
const CONSERVATION_REL: f64 = 1e-9;

/// FV9: every reason a fit is not verified, a closed list.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FitCode {
    /// A pair's stated uncertainty leaves no tolerance (FT7).
    UncertaintyAboveTolerance,
    /// The interface leaves the placement free (FT10, FV6).
    PlacementNotUnique,
    /// The kernel warned or failed, or the helper raised (FV2).
    KernelAlert,
    /// A residual above 0 and under the volume resolution (FV3).
    BelowResolution,
    /// A body within the threshold outside every zone (FT5, FV4).
    Proximity,
    /// A face within reach that is not analytic (FT5, FV4).
    UnsupportedGeometry,
    /// The original fails its own fit check (FT9, FV5).
    OriginalFailsFit,
    /// A body is not a valid solid with volume (FT6).
    InvalidBody,
    /// A body's shape tolerance is above t/10 (FT6).
    ToleranceTooLarge,
    /// The kernel's reading is not fitment's (FB2 to FB4, FB6, FZ9).
    BindingFailed,
    /// A solid no body names lies within reach (FB6).
    UnclaimedBody,
    /// A body the kernel finds within reach was not tested (FT8).
    UntestedBody,
    /// A zone has no closed form, or was built off it (FZ7).
    ZoneUnbuilt,
    /// A cut lost or gained volume (FY4).
    Conservation,
    /// A stored candidate's bytes could not be read (FC6).
    StoredBytesUnreadable,
    /// The check missed its deadline (FV8).
    HelperTimeout,
    /// A resource limit ended the helper (FV8).
    ResourceExhausted,
    /// The job was cancelled (FV8).
    Cancelled,
    /// The kernel crashed twice (FV8).
    KernelCrashed,
    /// The helper broke the protocol (FV8).
    HelperProtocol,
    /// No checked helper can run (FH3, FV8).
    HelperUnavailable,
}

impl FitCode {
    /// Every code, in order.
    pub const ALL: [Self; 21] = [
        Self::UncertaintyAboveTolerance,
        Self::PlacementNotUnique,
        Self::KernelAlert,
        Self::BelowResolution,
        Self::Proximity,
        Self::UnsupportedGeometry,
        Self::OriginalFailsFit,
        Self::InvalidBody,
        Self::ToleranceTooLarge,
        Self::BindingFailed,
        Self::UnclaimedBody,
        Self::UntestedBody,
        Self::ZoneUnbuilt,
        Self::Conservation,
        Self::StoredBytesUnreadable,
        Self::HelperTimeout,
        Self::ResourceExhausted,
        Self::Cancelled,
        Self::KernelCrashed,
        Self::HelperProtocol,
        Self::HelperUnavailable,
    ];

    /// The code as documents write it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UncertaintyAboveTolerance => "uncertainty_above_tolerance",
            Self::PlacementNotUnique => "placement_not_unique",
            Self::KernelAlert => "kernel_alert",
            Self::BelowResolution => "below_resolution",
            Self::Proximity => "proximity",
            Self::UnsupportedGeometry => "unsupported_geometry",
            Self::OriginalFailsFit => "original_fails_fit",
            Self::InvalidBody => "invalid_body",
            Self::ToleranceTooLarge => "tolerance_too_large",
            Self::BindingFailed => "binding_failed",
            Self::UnclaimedBody => "unclaimed_body",
            Self::UntestedBody => "untested_body",
            Self::ZoneUnbuilt => "zone_unbuilt",
            Self::Conservation => "conservation",
            Self::StoredBytesUnreadable => "stored_bytes_unreadable",
            Self::HelperTimeout => "helper_timeout",
            Self::ResourceExhausted => "resource_exhausted",
            Self::Cancelled => "cancelled",
            Self::KernelCrashed => "kernel_crashed",
            Self::HelperProtocol => "helper_protocol",
            Self::HelperUnavailable => "helper_unavailable",
        }
    }

    /// Its fixed sentence (FV9).
    pub fn sentence(self) -> &'static str {
        match self {
            Self::UncertaintyAboveTolerance => {
                "the uncertainty the files state is at or above the tolerance"
            }
            Self::PlacementNotUnique => "the interface leaves the placement free",
            Self::KernelAlert => "the kernel reported an alert",
            Self::BelowResolution => "an overlap is below the volume resolution",
            Self::Proximity => "a body is within the threshold outside every declared contact",
            Self::UnsupportedGeometry => "a face within reach is not analytic",
            Self::OriginalFailsFit => "the original part fails its own fit check",
            Self::InvalidBody => "a body is not a valid solid",
            Self::ToleranceTooLarge => "a body's shape tolerance is above a tenth of the tolerance",
            Self::BindingFailed => "the kernel's reading of the assembly is not fitment's",
            Self::UnclaimedBody => "a solid no body names lies within reach",
            Self::UntestedBody => "a body within reach was not tested",
            Self::ZoneUnbuilt => "a declared contact's zone could not be built as stated",
            Self::Conservation => "a cut did not conserve volume",
            Self::StoredBytesUnreadable => "the stored file could not be read",
            Self::HelperTimeout => "the fit check timed out",
            Self::ResourceExhausted => "the fit check ran out of a resource",
            Self::Cancelled => "the job was cancelled",
            Self::KernelCrashed => "the kernel crashed",
            Self::HelperProtocol => "the fit helper broke its protocol",
            Self::HelperUnavailable => "no fit helper is available",
        }
    }

    /// FV8: the code of a helper's failure.
    pub fn of(failure: &FitFailure) -> Self {
        match failure {
            FitFailure::Timeout => Self::HelperTimeout,
            FitFailure::ResourceExhausted => Self::ResourceExhausted,
            FitFailure::Cancelled => Self::Cancelled,
            FitFailure::Crashed => Self::KernelCrashed,
            FitFailure::Protocol => Self::HelperProtocol,
            FitFailure::Unavailable(_) => Self::HelperUnavailable,
            FitFailure::StoredBytesUnreadable => Self::StoredBytesUnreadable,
        }
    }

    /// True for a code that says the kernel's measures cannot be trusted
    /// where it applies: an interference measured there is no evidence.
    fn unsound(self) -> bool {
        matches!(
            self,
            Self::KernelAlert
                | Self::InvalidBody
                | Self::ToleranceTooLarge
                | Self::BindingFailed
                | Self::ZoneUnbuilt
                | Self::Conservation
                | Self::HelperProtocol
        )
    }
}

/// Where a finding applies.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum FindingAt {
    /// The whole fit.
    Run,
    /// The candidate's own body.
    Candidate,
    /// A body of the scene, or a solid the kernel transferred, by path
    /// (empty for an unclaimed solid).
    Body {
        /// Its path.
        path: Vec<u64>,
    },
    /// The candidate at one occurrence.
    Placement {
        /// The occurrence.
        occurrence: Vec<u64>,
    },
    /// One pair.
    Pair {
        /// The candidate's occurrence.
        occurrence: Vec<u64>,
        /// The other body's path, or its copy's occurrence.
        body: Vec<u64>,
        /// True for a copy.
        copy: bool,
    },
    /// One socket's placement.
    Socket {
        /// The socket.
        socket: String,
    },
}

/// One reason a fit is not verified (FV9): its code, where, and the
/// values behind it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Finding {
    /// The code.
    pub code: FitCode,
    /// Where it applies.
    pub at: FindingAt,
    /// The values, for people.
    pub detail: String,
}

impl Finding {
    fn new(code: FitCode, at: FindingAt, detail: impl Into<String>) -> Self {
        Self {
            code,
            at,
            detail: detail.into(),
        }
    }

    /// The code's sentence with the values (FV9).
    pub fn sentence(&self) -> String {
        if self.detail.is_empty() {
            self.code.sentence().to_owned()
        } else {
            format!("{}: {}", self.code.sentence(), self.detail)
        }
    }
}

/// One pair whose residual is an interference (FV3).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PairInterference {
    /// The candidate's occurrence.
    pub occurrence: Vec<u64>,
    /// The other body's path, or its copy's occurrence.
    pub body: Vec<u64>,
    /// True for a copy.
    pub copy: bool,
    /// The other body's name.
    pub name: String,
    /// The pair's tolerances.
    pub values: PairValues,
    /// V, the Common's volume.
    pub common_mm3: f64,
    /// R, V less the zones.
    pub residual_mm3: f64,
    /// R's box.
    pub residual_box: Option<FitBox>,
}

/// A fit's outcome: each socket's status and the candidate's (FV7), the
/// findings and interferences behind them, and whether the run that
/// produced it is incomplete (FV8). Built only by [`decide`] and by the
/// constructors below, none of which makes a `verified`.
#[derive(Debug, Clone, PartialEq)]
pub struct FitOutcome {
    candidate: FitStatus,
    sockets: Vec<(String, FitStatus)>,
    findings: Vec<Finding>,
    interferences: Vec<PairInterference>,
    incomplete: bool,
}

impl FitOutcome {
    /// The candidate's fit (VD4 over its sockets).
    pub fn candidate(&self) -> &FitStatus {
        &self.candidate
    }

    /// Each socket's fit, in placement order.
    pub fn sockets(&self) -> &[(String, FitStatus)] {
        &self.sockets
    }

    /// Socket `id`'s fit.
    pub fn socket(&self, id: &str) -> Option<&FitStatus> {
        self.sockets.iter().find(|(s, _)| s == id).map(|(_, f)| f)
    }

    /// Why it is not verified, in the order found.
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// The pairs that interfere.
    pub fn interferences(&self) -> &[PairInterference] {
        &self.interferences
    }

    /// The first finding's code, if one.
    pub fn code(&self) -> Option<FitCode> {
        self.findings.first().map(|f| f.code)
    }

    /// True if the run that produced it depends on the machine (a timeout,
    /// a resource kill, a cancel, a protocol failure or an unreadable
    /// store): it never claims to be complete (FV8).
    pub fn incomplete(&self) -> bool {
        self.incomplete
    }

    /// FV8: no measures, for `failure`, at every one of `sockets`.
    pub fn failed(sockets: &[&str], failure: &FitFailure) -> Self {
        let detail = match failure {
            FitFailure::Unavailable(why) => why.clone(),
            _ => String::new(),
        };
        let finding = Finding::new(FitCode::of(failure), FindingAt::Run, detail);
        let incomplete = !matches!(failure, FitFailure::Crashed | FitFailure::Unavailable(_));
        Self::not_verified(sockets, vec![finding], FitCode::of(failure), incomplete)
    }

    /// FV6: no fit asked, for a placement the interface leaves free at
    /// some of `sockets` (each with its free motions, as
    /// [`super::Freedom::describe`] gives them); every socket is
    /// `placement_not_unique`.
    pub fn not_unique(sockets: &[&str], free: &[(&str, String)]) -> Self {
        let findings = free
            .iter()
            .map(|(socket, motions)| {
                Finding::new(
                    FitCode::PlacementNotUnique,
                    FindingAt::Socket {
                        socket: (*socket).to_owned(),
                    },
                    motions.clone(),
                )
            })
            .collect();
        Self::not_verified(sockets, findings, FitCode::PlacementNotUnique, false)
    }

    /// Every socket `not_verified` for the first of `findings`, or for
    /// `code` alone, whose finding is added, when there is none.
    fn not_verified(
        sockets: &[&str],
        mut findings: Vec<Finding>,
        code: FitCode,
        incomplete: bool,
    ) -> Self {
        if findings.is_empty() {
            findings.push(Finding::new(code, FindingAt::Run, ""));
        }
        let status = FitStatus::NotVerified {
            reason: findings.first().map(Finding::sentence).unwrap_or_default(),
        };
        Self {
            candidate: status.clone(),
            sockets: sockets
                .iter()
                .map(|s| ((*s).to_owned(), status.clone()))
                .collect(),
            findings,
            interferences: Vec::new(),
            incomplete,
        }
    }

    /// FV5: this outcome under `control`, the original's own: if the
    /// control is not verified, every fit here that is `verified` becomes
    /// `not_verified`, `original_fails_fit`, with the control's own code;
    /// an `interferes` stays, as does anything else.
    #[must_use]
    pub fn controlled_by(mut self, control: &Self) -> Self {
        if matches!(control.candidate, FitStatus::Verified { .. }) {
            return self;
        }
        let theirs = match (&control.candidate, control.code()) {
            (_, Some(code)) => code.as_str(),
            (FitStatus::Interferes { .. }, None) => "interferes",
            _ => "not_checked",
        };
        let finding = Finding::new(
            FitCode::OriginalFailsFit,
            FindingAt::Run,
            format!("the original's fit is {theirs}"),
        );
        let downgraded = FitStatus::NotVerified {
            reason: finding.sentence(),
        };
        for (_, status) in &mut self.sockets {
            if matches!(status, FitStatus::Verified { .. }) {
                *status = downgraded.clone();
            }
        }
        if matches!(self.candidate, FitStatus::Verified { .. }) {
            self.candidate = downgraded;
        }
        self.findings.push(finding);
        self
    }
}

/// FV1: the fit's status from the kernel's `measures` of `plan`. Pure;
/// FV2 to FV4 and FV7 are its branches, with the cross-checks of FB2 to
/// FB4, FB6, FT6, FT8, FZ7, FZ9 and FY4. FV5, FV6 and FV8 are
/// [`FitOutcome::controlled_by`], [`FitOutcome::not_unique`] and
/// [`FitOutcome::failed`]. It fails closed: a reply it cannot prove
/// consistent with the plan and with itself is `helper_protocol` or
/// `binding_failed`, never a pass.
pub fn decide(plan: &FitPlan, measures: &FitMeasures) -> FitOutcome {
    let mut findings = Vec::new();
    replies(plan, measures, &mut findings);
    scene_findings(plan, &measures.scene, &mut findings);
    place_findings(plan, &measures.place, &mut findings);
    unclaimed(plan, measures, &mut findings);
    let mut interferences = Vec::new();
    let asked: Vec<&Pair> = plan.asked().collect();
    for pair in plan.pairs() {
        let at = pair_at(pair);
        match pair.state {
            PairState::Uncertain => findings.push(Finding::new(
                FitCode::UncertaintyAboveTolerance,
                at,
                format!(
                    "u_pair {} mm against t {} mm",
                    pair.values.u_pair_mm,
                    plan.linear_mm()
                ),
            )),
            PairState::ZoneUnbuilt => {
                findings.push(Finding::new(FitCode::ZoneUnbuilt, at, unbuilt(pair)));
            }
            PairState::Asked => {}
        }
    }
    if measures.check.pairs.len() == asked.len() {
        for (pair, measure) in asked.iter().zip(&measures.check.pairs) {
            let volume = body_volume(measures, pair);
            if let Some(found) = check_pair(pair, measure, volume, &mut findings) {
                interferences.push(found);
            }
        }
    } else {
        findings.push(Finding::new(
            FitCode::HelperProtocol,
            FindingAt::Run,
            format!(
                "{} pairs measured of {} asked",
                measures.check.pairs.len(),
                asked.len()
            ),
        ));
    }
    reach_findings(plan, measures, &mut findings);
    interferences.retain(|i| sound(&findings, i));
    aggregate(plan, findings, interferences)
}

/// FH2, FH5, FB6, FT2: each reply answers this plan's request (its
/// digest echoed), names each body, placement and claim once, and the
/// kernel transferred one solid per leaf; the plan places the candidate
/// once at every selected occurrence, and at nothing else.
fn replies(plan: &FitPlan, measures: &FitMeasures, out: &mut Vec<Finding>) {
    let run = |code, detail: String| Finding::new(code, FindingAt::Run, detail);
    for (reply, echoed, expected) in [
        ("scene", &measures.scene.request_sha256, plan.scene_digest()),
        ("place", &measures.place.request_sha256, plan.place_digest()),
        ("check", &measures.check.request_sha256, plan.check_digest()),
    ] {
        if echoed.as_deref() != Some(expected) {
            out.push(run(
                FitCode::HelperProtocol,
                format!("the {reply} reply answers another request"),
            ));
        }
    }
    let twice = |keys: Vec<(&[u64], &[u64])>| {
        let unique: BTreeSet<(&[u64], &[u64])> = keys.iter().copied().collect();
        unique.len() != keys.len()
    };
    let bodies = measures.scene.bodies.iter().map(|b| (&b.path[..], &[][..]));
    let placed = (measures.place.placements.iter()).map(|p| (&p.occurrence[..], &[][..]));
    let claims = (measures.check.reach.iter()).map(|c| (&c.body[..], &c.occurrence[..]));
    for (what, keys) in [
        ("body", bodies.collect::<Vec<_>>()),
        ("placement", placed.collect()),
        ("claim of reach", claims.collect()),
    ] {
        if twice(keys) {
            out.push(run(
                FitCode::HelperProtocol,
                format!("a {what} is measured twice"),
            ));
        }
    }
    let leaves = plan.bodies().len() + plan.unread();
    if measures.scene.solids.len() != leaves {
        out.push(run(
            FitCode::BindingFailed,
            format!(
                "the kernel transferred {} solids from {leaves} leaves",
                measures.scene.solids.len()
            ),
        ));
    }
    let selected: BTreeSet<&[u64]> = (plan.bodies().iter())
        .filter(|b| b.selected)
        .map(|b| &b.path[..])
        .collect();
    let at: Vec<&[u64]> = plan
        .placements()
        .iter()
        .map(|p| &p.occurrence[..])
        .collect();
    let once: BTreeSet<&[u64]> = at.iter().copied().collect();
    if at.is_empty() || once.len() != at.len() || once != selected {
        out.push(run(
            FitCode::BindingFailed,
            "the candidate is not placed once at each selected occurrence".to_owned(),
        ));
    }
}

fn pair_at(pair: &Pair) -> FindingAt {
    FindingAt::Pair {
        occurrence: pair.occurrence.clone(),
        body: pair.body.clone(),
        copy: pair.copy,
    }
}

fn unbuilt(pair: &Pair) -> String {
    let named: Vec<String> = pair
        .zones
        .iter()
        .filter(|z| z.interference.is_none() || z.proximity.is_none())
        .map(|z| format!("{} of {}", label(z.zone.kind), z.zone.requirement))
        .collect();
    named.join(", ")
}

fn label(kind: super::ZoneKind) -> &'static str {
    match kind {
        super::ZoneKind::Slab => "slab",
        super::ZoneKind::Annulus => "annulus",
        super::ZoneKind::Shell => "shell",
    }
}

/// The volume of the pair's other body: a neighbour's as the kernel read
/// it, or the candidate's for a copy.
fn body_volume(measures: &FitMeasures, pair: &Pair) -> f64 {
    if pair.copy {
        measures.place.volume_mm3
    } else {
        measures
            .scene
            .bodies
            .iter()
            .find(|b| b.path == pair.body)
            .map_or(f64::NAN, |b| b.volume_mm3)
    }
}

/// FB2, FB3, FB6, FT6: each body core named is found once, where fitment
/// places it, its box sound and within fitment's own and its landmarks
/// fitment's; each body of an asked pair is valid with a shape tolerance
/// at most t/10.
fn scene_findings(plan: &FitPlan, scene: &SceneMeasures, out: &mut Vec<Finding>) {
    let tested: BTreeSet<&[u64]> = plan
        .asked()
        .filter(|p| !p.copy)
        .map(|p| p.body.as_slice())
        .collect();
    for body in plan.bodies() {
        let at = || FindingAt::Body {
            path: body.path.clone(),
        };
        let measure = scene.bodies.iter().find(|m| m.path == body.path);
        let claims = scene
            .solids
            .iter()
            .filter(|s| s.faces == body.faces && placed_at(s, &body.transform))
            .count();
        let Some(measure) = measure.filter(|m| m.found && claims == 1) else {
            out.push(Finding::new(
                FitCode::BindingFailed,
                at(),
                format!("{} found by {claims} solids of the kernel's", body.name),
            ));
            continue;
        };
        let ours = body.extent.as_ref().and_then(|e| moved(e, &body.transform));
        if !box_holds(&measure.bounds, measure.tolerance_mm, ours) {
            out.push(Finding::new(
                FitCode::BindingFailed,
                at(),
                format!("{}'s box is not within fitment's", body.name),
            ));
        }
        if !landmarks_agree(&body.landmarks, &measure.landmarks) {
            out.push(Finding::new(
                FitCode::BindingFailed,
                at(),
                format!("{}'s landmarks are not fitment's", body.name),
            ));
        }
        if tested.contains(body.path.as_slice()) {
            fit_to_judge(plan, measure_facts(measure), at(), out);
        }
    }
}

/// FB3: a kernel box is sound and, where fitment has a sound box of the
/// body, lies within it moved where fitment places the body, widened by
/// the kernel's tolerance and 1e-6 mm. It need not hold fitment's box: a
/// box moved off the axes is larger than the body's own.
fn box_holds(kernel: &FitBox, tolerance: f64, ours: Option<FitBox>) -> bool {
    if !(kernel.sound() && tolerance.is_finite() && tolerance >= 0.0) {
        return false;
    }
    ours.is_none_or(|ours| {
        let ours = ours.grown(tolerance + LOCATION_MM);
        (0..3).all(|i| kernel.min_mm[i] >= ours.min_mm[i] && kernel.max_mm[i] <= ours.max_mm[i])
    })
}

fn measure_facts(measure: &BodyMeasure) -> (bool, f64, f64) {
    (measure.valid, measure.volume_mm3, measure.tolerance_mm)
}

/// FT6: valid, with volume, and a shape tolerance at most t/10.
fn fit_to_judge(
    plan: &FitPlan,
    (valid, volume, tolerance): (bool, f64, f64),
    at: FindingAt,
    out: &mut Vec<Finding>,
) {
    if !valid || volume.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        out.push(Finding::new(
            FitCode::InvalidBody,
            at.clone(),
            format!("valid {valid}, volume {volume} mm³"),
        ));
    }
    let cap = plan.linear_mm() / 10.0;
    if !matches!(
        tolerance.partial_cmp(&cap),
        Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
    ) {
        out.push(Finding::new(
            FitCode::ToleranceTooLarge,
            at,
            format!("tolerance {tolerance} mm above {cap} mm"),
        ));
    }
}

/// FB2: a solid's location agrees with fitment's transform.
fn placed_at(solid: &SolidMeasure, transform: &Rigid) -> bool {
    let (r, t) = (transform.rotation(), transform.translation());
    (0..3).all(|i| {
        (0..3).all(|j| (solid.location[i][j] - r[i][j]).abs() <= LOCATION_ROTATION)
            && (solid.location[i][3] - t[i]).abs() <= LOCATION_MM
    })
}

/// FB3: every landmark fitment named agrees with the kernel's reading.
pub(crate) fn landmarks_agree(ours: &[Landmark], theirs: &[LandmarkMeasure]) -> bool {
    ours.len() == theirs.len() && ours.iter().zip(theirs).all(|(o, t)| landmark_agrees(o, t))
}

fn landmark_agrees(ours: &Landmark, theirs: &LandmarkMeasure) -> bool {
    let unit = |v: [f64; 3]| crate::geom::Dir3::normalize(v).map(crate::geom::Dir3::to_array);
    let sine = |a: [f64; 3], b: [f64; 3]| {
        let c = crate::socket::geo::cross(a, b);
        crate::socket::geo::norm(c)
    };
    match (ours.value, *theirs) {
        (
            LandmarkValue::Plane { normal, offset_mm },
            LandmarkMeasure::Plane {
                normal: n,
                offset_mm: o,
            },
        ) => unit(n).is_some_and(|n| {
            let ours = normal.to_array();
            sine(ours, n) <= LANDMARK_SINE
                && crate::socket::geo::dot(ours, n) > 0.0
                && (offset_mm - o).abs() <= LANDMARK_MM
        }),
        (
            LandmarkValue::Cylinder { axis, radius_mm },
            LandmarkMeasure::Cylinder {
                point_mm,
                direction,
                radius_mm: r,
            },
        ) => unit(direction).is_some_and(|d| {
            let ours = axis.direction.to_array();
            let off = crate::socket::geo::sub(point_mm, axis.point_mm.to_array());
            let across = crate::socket::geo::across(off, ours);
            sine(ours, d) <= LANDMARK_SINE
                && crate::socket::geo::norm(across) <= LANDMARK_MM
                && (radius_mm - r).abs() <= LANDMARK_MM
        }),
        _ => false,
    }
}

/// FT6, FB3, FB4: the candidate is a valid solid of small tolerance, its
/// bytes are the ones fitment read, and at each placement its landmarks
/// are fitment's placed and its box sound and within fitment's own moved
/// by the same placement.
fn place_findings(plan: &FitPlan, place: &PlaceMeasures, out: &mut Vec<Finding>) {
    let candidate = plan.candidate();
    fit_to_judge(
        plan,
        (place.valid, place.volume_mm3, place.tolerance_mm),
        FindingAt::Candidate,
        out,
    );
    if candidate.sha256().is_some() && candidate.sha256() != place.sha256.as_deref() {
        out.push(Finding::new(
            FitCode::BindingFailed,
            FindingAt::Candidate,
            "the bytes the helper read are not the candidate's",
        ));
    }
    for placed in plan.placements() {
        let at = || FindingAt::Placement {
            occurrence: placed.occurrence.clone(),
        };
        let Some(measure) = place
            .placements
            .iter()
            .find(|m| m.occurrence == placed.occurrence)
        else {
            out.push(Finding::new(
                FitCode::BindingFailed,
                at(),
                "not placed by the kernel",
            ));
            continue;
        };
        let expected: Option<Vec<Landmark>> = candidate
            .landmarks()
            .iter()
            .map(|l| l.placed(&placed.transform))
            .collect();
        if !expected.is_some_and(|e| landmarks_agree(&e, &measure.landmarks)) {
            out.push(Finding::new(
                FitCode::BindingFailed,
                at(),
                "the placed candidate's landmarks are not fitment's",
            ));
        }
        let ours = candidate.extent().and_then(|e| moved(e, &placed.transform));
        if !box_holds(&measure.bounds, place.tolerance_mm, ours) {
            out.push(Finding::new(
                FitCode::BindingFailed,
                at(),
                "the placed candidate's box is not within fitment's",
            ));
        }
    }
}

/// FB6: a solid the kernel transferred that no body claims, within reach
/// of a placement, is `unclaimed_body`; one outside reach is counted. Its
/// reach uses the candidate's and the occurrence's floors and the guard:
/// it states none of its own, and no shape tolerance.
fn unclaimed(plan: &FitPlan, measures: &FitMeasures, out: &mut Vec<Finding>) {
    for solid in &measures.scene.solids {
        let claimed = plan
            .bodies()
            .iter()
            .any(|b| b.faces == solid.faces && placed_at(solid, &b.transform));
        if claimed {
            continue;
        }
        let theirs = solid.bounds.sound().then_some((solid.bounds, 0.0));
        let near = plan.placements().iter().any(|placed| {
            let values = plan.values(&placed.occurrence, &[]);
            let ours = plan.candidate_box(&measures.place, &placed.occurrence);
            within(&values, ours, theirs)
        });
        if near {
            out.push(Finding::new(
                FitCode::UnclaimedBody,
                FindingAt::Body { path: Vec::new() },
                format!("a solid of faces {:?}", first_faces(&solid.faces)),
            ));
        }
    }
}

fn first_faces(faces: &[u64]) -> Vec<u64> {
    faces.iter().take(4).copied().collect()
}

/// FT8: every scene body but the selected occurrences, at every placement,
/// has the helper's claim of reach; a body it finds within reach that the
/// plan does not pair is `untested_body`.
fn reach_findings(plan: &FitPlan, measures: &FitMeasures, out: &mut Vec<Finding>) {
    let paired: BTreeSet<(&[u64], &[u64])> = plan
        .pairs()
        .iter()
        .filter(|p| !p.copy)
        .map(|p| (p.occurrence.as_slice(), p.body.as_slice()))
        .collect();
    for placed in plan.placements() {
        for body in plan.bodies().iter().filter(|b| !b.selected) {
            let claim = measures
                .check
                .reach
                .iter()
                .find(|c| c.body == body.path && c.occurrence == placed.occurrence);
            let at = || FindingAt::Pair {
                occurrence: placed.occurrence.clone(),
                body: body.path.clone(),
                copy: false,
            };
            match claim {
                None => out.push(Finding::new(
                    FitCode::HelperProtocol,
                    at(),
                    format!("no claim of reach for {}", body.name),
                )),
                Some(c)
                    if c.within && !paired.contains(&(&placed.occurrence[..], &body.path[..])) =>
                {
                    out.push(Finding::new(FitCode::UntestedBody, at(), body.name.clone()));
                }
                Some(_) => {}
            }
        }
    }
}

/// FT4, FT5, FV2 to FV4, FZ7, FZ9, FY4 for one asked pair: its findings,
/// and its interference if its residual is one.
fn check_pair(
    pair: &Pair,
    m: &PairMeasure,
    body_volume: f64,
    out: &mut Vec<Finding>,
) -> Option<PairInterference> {
    let at = || pair_at(pair);
    let v = &pair.values;
    if let Some(why) = inconsistent(pair, m) {
        out.push(Finding::new(FitCode::HelperProtocol, at(), why));
        return None;
    }
    if !m.alerts.is_empty() {
        out.push(Finding::new(
            FitCode::KernelAlert,
            at(),
            m.alerts.join(", "),
        ));
    }
    let built: Vec<Option<Built>> = pair.zones.iter().map(|z| z.interference).collect();
    if !zones_agree(&built, &m.zones) {
        out.push(Finding::new(
            FitCode::ZoneUnbuilt,
            at(),
            "a zone was built off its closed form",
        ));
    }
    if !conserved(m.residual_mm3 + m.inside_mm3, m.common_mm3) {
        out.push(Finding::new(
            FitCode::Conservation,
            at(),
            format!(
                "R {} + inside {} against V {} mm³",
                m.residual_mm3, m.inside_mm3, m.common_mm3
            ),
        ));
    }
    if m.non_analytic {
        out.push(Finding::new(
            FitCode::UnsupportedGeometry,
            at(),
            pair.name.clone(),
        ));
    }
    if m.residual_mm3 > 0.0 && m.residual_mm3 < FIT_VOLUME_MM3 {
        out.push(Finding::new(
            FitCode::BelowResolution,
            at(),
            format!("R {} mm³", m.residual_mm3),
        ));
    }
    proximity(pair, m, body_volume, out);
    (m.residual_mm3 >= FIT_VOLUME_MM3).then(|| PairInterference {
        occurrence: pair.occurrence.clone(),
        body: pair.body.clone(),
        copy: pair.copy,
        name: pair.name.clone(),
        values: *v,
        common_mm3: m.common_mm3,
        residual_mm3: m.residual_mm3,
        residual_box: m.residual_box,
    })
}

/// True if the pair's zoned distance was asked: its body within the
/// threshold, and zones to cut it by (FT5).
fn zoned_asked(pair: &Pair, m: &PairMeasure) -> bool {
    !pair.zones.is_empty() && m.distance_mm.is_some_and(|d| d <= pair.values.threshold_mm)
}

/// FH5, FY4: what makes one pair's measures inconsistent with its request
/// or with themselves, if anything.
fn inconsistent(pair: &Pair, m: &PairMeasure) -> Option<&'static str> {
    if !numbers_sound(m) {
        return Some("the pair's measures are out of range");
    }
    if m.zones.len() != pair.zones.len() {
        return Some("a zone's measure is missing or extra");
    }
    let held = |zones: &mut dyn Iterator<Item = Option<Built>>| {
        zones
            .map(|b| b.map_or(f64::NAN, |b| b.volume_mm3))
            .sum::<f64>()
    };
    let within_zones = held(&mut pair.zones.iter().map(|z| z.interference));
    if pair.zones.is_empty()
        && (m.inside_mm3 != 0.0 || (m.residual_mm3 - m.common_mm3).abs() > slack(m.common_mm3))
    {
        return Some("a pair with no zone has volume within one");
    }
    if m.inside_mm3 > within_zones + slack(within_zones) {
        return Some("more of the Common lies within the zones than they hold");
    }
    if m.common_mm3 >= FIT_VOLUME_MM3 && !m.distance_mm.is_some_and(|d| d <= LOCATION_MM) {
        return Some("bodies with a Common volume are apart");
    }
    if !zoned_asked(pair, m) {
        let answered = m.zoned_distance_mm.is_some()
            || m.body_kept_mm3.is_some()
            || m.body_inside_mm3.is_some()
            || !m.proximity_zones.is_empty();
        return answered.then_some("a zoned distance was measured that was not asked");
    }
    let (Some(zoned), Some(_), Some(inside)) =
        (m.zoned_distance_mm, m.body_kept_mm3, m.body_inside_mm3)
    else {
        return Some("the zoned distance was not measured");
    };
    if m.proximity_zones.len() != pair.zones.len() {
        return Some("a proximity zone's measure is missing or extra");
    }
    if m.distance_mm.is_some_and(|d| zoned < d) {
        return Some("the body less its zones is nearer than the body");
    }
    let wide = held(&mut pair.zones.iter().map(|z| z.proximity));
    (inside > wide + slack(wide)).then_some("more of the body lies within the zones than they hold")
}

/// FT5, FZ9: a pair within its threshold is in proximity if the body less
/// its zones is too, or with no zone, the body itself; every zone's body
/// must come within the threshold.
fn proximity(pair: &Pair, m: &PairMeasure, body_volume: f64, out: &mut Vec<Finding>) {
    let at = || pair_at(pair);
    let threshold = pair.values.threshold_mm;
    let Some(distance) = m.distance_mm.filter(|d| *d <= threshold) else {
        if !pair.zones.is_empty() {
            out.push(Finding::new(
                FitCode::BindingFailed,
                at(),
                format!("a zone's body is farther than {threshold} mm"),
            ));
        }
        return;
    };
    let (Some(zoned), Some(kept), Some(inside)) =
        (m.zoned_distance_mm, m.body_kept_mm3, m.body_inside_mm3)
    else {
        // No zone: the distance alone (`inconsistent` refused any other).
        out.push(Finding::new(
            FitCode::Proximity,
            at(),
            format!("{distance} mm"),
        ));
        return;
    };
    let built: Vec<Option<Built>> = pair.zones.iter().map(|z| z.proximity).collect();
    if !zones_agree(&built, &m.proximity_zones) {
        out.push(Finding::new(
            FitCode::ZoneUnbuilt,
            at(),
            "a proximity zone was built off its closed form",
        ));
    }
    if !conserved(kept + inside, body_volume) {
        out.push(Finding::new(
            FitCode::Conservation,
            at(),
            format!("the body's kept {kept} + inside {inside} against {body_volume} mm³"),
        ));
    }
    if zoned <= threshold {
        out.push(Finding::new(
            FitCode::Proximity,
            at(),
            format!("{zoned} mm"),
        ));
    }
}

/// FY4's ranges: every number finite and never negative, a residual or
/// the volume inside the zones never above the Common.
fn numbers_sound(m: &PairMeasure) -> bool {
    let finite = |x: f64| x.is_finite() && x >= 0.0;
    let optional = |x: Option<f64>| x.is_none_or(finite);
    let slack = slack(m.common_mm3);
    finite(m.common_mm3)
        && finite(m.residual_mm3)
        && finite(m.inside_mm3)
        && m.residual_mm3 <= m.common_mm3 + slack
        && m.inside_mm3 <= m.common_mm3 + slack
        && optional(m.distance_mm)
        && optional(m.zoned_distance_mm)
        && optional(m.body_kept_mm3)
        && optional(m.body_inside_mm3)
        && m.residual_box.is_none_or(|b| b.sound())
}

/// FY4's slack for a volume `whole`: 1e-9 of it, but never a tenth of the
/// volume resolution or more, so a residual at the resolution's scale
/// cannot hide in the rounding of a large body.
fn slack(whole: f64) -> f64 {
    (CONSERVATION_REL * whole.abs()).min(FIT_VOLUME_MM3 / 10.0)
}

/// FY4: `parts` is `whole` within [`slack`].
fn conserved(parts: f64, whole: f64) -> bool {
    (parts - whole).abs() <= slack(whole)
}

/// FZ7: each zone the kernel built agrees with its closed form: volume
/// within 1e-9 relative, centroid and box within 1e-6 mm.
fn zones_agree(ours: &[Option<Built>], theirs: &[ZoneMeasure]) -> bool {
    ours.len() == theirs.len()
        && ours.iter().zip(theirs).all(|(o, t)| {
            o.is_some_and(|o| {
                (o.volume_mm3 - t.volume_mm3).abs() <= ZONE_VOLUME_REL * o.volume_mm3
                    && (0..3).all(|i| (o.centroid_mm[i] - t.centroid_mm[i]).abs() <= ZONE_MM)
                    && t.bounds.sound()
                    && o.bounds.apart(&t.bounds) <= ZONE_MM
            })
        })
}

/// An interference counts only where the kernel's measures can be
/// trusted: no unsound finding at its pair, its other body, the
/// candidate, a placement or the whole run.
fn sound(findings: &[Finding], interference: &PairInterference) -> bool {
    !findings.iter().any(|f| {
        f.code.unsound()
            && match &f.at {
                FindingAt::Run | FindingAt::Candidate | FindingAt::Placement { .. } => true,
                FindingAt::Body { path } => !path.is_empty() && *path == interference.body,
                FindingAt::Pair {
                    occurrence,
                    body,
                    copy,
                } => {
                    *occurrence == interference.occurrence
                        && *body == interference.body
                        && *copy == interference.copy
                }
                FindingAt::Socket { .. } => false,
            }
    })
}

/// FV7: per socket, `interferes` if a pair of its occurrences (or a copy
/// pair that reaches one of them) interferes; else `not_verified` if a
/// finding applies to it; else `verified` with the plan's identity. The
/// candidate's by VD4: `interferes` over `not_verified` over `verified`.
fn aggregate(
    plan: &FitPlan,
    findings: Vec<Finding>,
    interferences: Vec<PairInterference>,
) -> FitOutcome {
    let identity = plan.identity();
    let verified = FitStatus::Verified {
        kernel: identity.kernel.clone(),
        version: identity.version.clone(),
    };
    let touches = |socket: &str, occurrence: &[u64], body: &[u64], copy: bool| {
        plan.socket_of(occurrence) == Some(socket) || (copy && plan.socket_of(body) == Some(socket))
    };
    let applies = |socket: &str, at: &FindingAt| match at {
        FindingAt::Run | FindingAt::Candidate | FindingAt::Body { .. } => true,
        FindingAt::Placement { occurrence } => plan.socket_of(occurrence) == Some(socket),
        FindingAt::Pair {
            occurrence,
            body,
            copy,
        } => touches(socket, occurrence, body, *copy),
        FindingAt::Socket { socket: s } => s == socket,
    };
    let sockets: Vec<(String, FitStatus)> = plan
        .sockets()
        .into_iter()
        .map(|socket| {
            let hit: Vec<Interference> = interferences
                .iter()
                .filter(|i| touches(socket, &i.occurrence, &i.body, i.copy))
                .map(|i| Interference::new(i.name.clone(), i.residual_mm3))
                .collect();
            let status = if !hit.is_empty() {
                FitStatus::Interferes { interferences: hit }
            } else if let Some(f) = findings.iter().find(|f| applies(socket, &f.at)) {
                FitStatus::NotVerified {
                    reason: f.sentence(),
                }
            } else {
                verified.clone()
            };
            (socket.to_owned(), status)
        })
        .collect();
    let candidate = if !interferences.is_empty() {
        FitStatus::Interferes {
            interferences: interferences
                .iter()
                .map(|i| Interference::new(i.name.clone(), i.residual_mm3))
                .collect(),
        }
    } else if let Some(f) = findings.first() {
        FitStatus::NotVerified {
            reason: f.sentence(),
        }
    } else {
        verified
    };
    FitOutcome {
        candidate,
        sockets,
        findings,
        interferences,
        incomplete: false,
    }
}

#[cfg(test)]
mod tests;
