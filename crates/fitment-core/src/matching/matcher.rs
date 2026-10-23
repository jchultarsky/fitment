//! The matcher (`docs/research/m4-match.md`, sections 3 and 12): one
//! socket document, checked once (MF2), judging candidates one at a time.
//! Per socket: the shortlist, alignment, then verification at the first
//! placement that passes every requirement M4 can verify, or at the one
//! passing most (AL10); the candidate's verdict is the worst of its
//! sockets' (VD2, VR17). With a fit verifier that has an identity, the fit
//! is asked once per candidate whose every socket passed
//! (`docs/research/m7-fit.md`, FC2, FT11), in [`fitting`].

mod fitting;

use serde::Serialize;

use super::align::{Placement, Placements, align};
use super::evidence::{At, Evidence, Failure, FailureCode, Warning};
use super::index::{Blocked, IndexSource, level_one};
use super::shortlist::{Kept, Missing, shortlist};
use super::verify::{Budget, check};
use super::{CandidateView, MatchOptions, MatchPolicy, SocketSignature, SocketView};
use crate::assembly::{SelectedComponent, Source};
use crate::features::{FeaturesDocument, Part, ProductRef, Status};
use crate::fit::{
    FitEvidence, FitStatus, FitVerifier, Freedom, INTERFACE_REJECTED, OCCURRENCE_UNPLACED,
};
use crate::gate::{Refusal, RefusalCode};
use crate::geom::Rigid;
use crate::socket::{Flag, Requirement, Requires, ReviewStatus, SocketDocument};
use crate::verdict::{Check, RejectReason, RequirementId, Verdict, VerdictKind, combine, decide};

/// Why a socket document cannot be matched against (MF2).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum SocketRefused {
    /// The assembly gave no socket: its refusals.
    #[error("the socket document is refused: {0:?}")]
    Refused(Vec<Refusal>),
    /// Written by another extractor or other socket rules (M4-22).
    #[error("the socket document is stale: extractor {extractor}, socket rules {socket_rules}")]
    #[non_exhaustive]
    Stale {
        /// Its extractor version.
        extractor: u32,
        /// Its socket rules version.
        socket_rules: u32,
    },
    /// A review was required and requirements are unreviewed.
    #[error("{count} requirements are unreviewed")]
    #[non_exhaustive]
    Unreviewed {
        /// How many.
        count: usize,
        /// The first few, as `socket-1/bore:hole-88`.
        first: Vec<String>,
    },
}

impl SocketRefused {
    /// A document of other versions than this build's: its extractor's
    /// and its socket rules'.
    pub fn stale(extractor: u32, socket_rules: u32) -> Self {
        Self::Stale {
            extractor,
            socket_rules,
        }
    }
}

/// How far one socket's judgment went (MF5).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Stage {
    /// The shortlist dropped it; alignment and verification ran anyway,
    /// for the evidence (M4-17).
    Shortlist,
    /// No placement was found.
    Alignment,
    /// It was placed and verified.
    Verification,
}

/// How the verified placement was chosen (EV4).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Chosen {
    /// The first placement passing every requirement M4 can verify.
    FirstPassing,
    /// None passed: the one passing most, the first of those.
    FewestFailures,
    /// The caller gave it (AL14).
    Given,
    /// Nothing was placed.
    None,
}

/// What M4 does not check in a socket (EV1, M4-11, VR16).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "what", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NotChecked {
    /// Removed in review, with the reviewer's note.
    #[non_exhaustive]
    Removed {
        /// The requirement.
        requirement: RequirementId,
        /// Why.
        note: String,
    },
    /// A shaft's room before a blind hole's floor (I4): information.
    #[non_exhaustive]
    MaxExtraMm {
        /// The requirement.
        requirement: RequirementId,
        /// The room, in mm.
        mm: f64,
    },
}

/// One socket's judgment of a candidate.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SocketResult {
    /// The socket.
    pub socket: String,
    /// How far it went.
    pub stage: Stage,
    /// Its verdict, by `decide` (M0's contract).
    pub verdict: Verdict,
    /// The placement verified, if one was.
    pub placement: Option<Placement>,
    /// How that placement was chosen.
    pub chosen: Chosen,
    /// VR2's u and ε for this candidate in this socket, in mm.
    pub u_mm: f64,
    /// ε = t − u.
    pub epsilon_mm: f64,
    /// What the shortlist found missing, if it dropped the candidate.
    pub missing: Vec<Missing>,
    /// One check per active requirement, in the socket's order.
    pub checks: Vec<(RequirementId, Check)>,
    /// What is not checked here: removed requirements, `max_extra_mm`.
    pub not_checked: Vec<NotChecked>,
    /// How many placements alignment built, and whether a bound stopped it.
    pub placements: (usize, bool),
    /// The placement's free motions (FC4, FM4), worked out when a verifier
    /// with an identity is asked for the candidate's fit; `None` otherwise.
    pub freedom: Option<Freedom>,
    /// Every other placement alignment built at which every requirement
    /// passed, for which no fit is asked (FM3, FT12), in alignment's order;
    /// listed when the verifier has an identity and the interface passed
    /// here, `None` otherwise.
    pub alternatives: Option<Vec<Rigid>>,
}

impl SocketResult {
    /// How many checks failed.
    pub fn failed(&self) -> usize {
        self.checks.iter().filter(|(_, c)| !c.passed()).count()
    }
}

/// Where a candidate came from (MF6's `source`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CandidateSource {
    /// A STEP file: its SHA-256, size and header.
    File(Source),
    /// A leaf of an assembly (CF1): the assembly's SHA-256 and the
    /// definition's `#id`.
    #[non_exhaustive]
    Component {
        /// The assembly's SHA-256, hex.
        assembly_sha256: String,
        /// The definition's `#id`.
        definition: u64,
    },
    /// A part the caller built or read itself.
    Given,
}

impl CandidateSource {
    /// A leaf of an assembly: the assembly's SHA-256 (hex) and the
    /// definition's `#id`.
    pub fn component(assembly_sha256: impl Into<String>, definition: u64) -> Self {
        Self::Component {
            assembly_sha256: assembly_sha256.into(),
            definition,
        }
    }
}

/// What level 1 admits for a whole socket document (M5's QR3).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "admitted", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Admitted {
    /// The candidates every socket may keep.
    #[non_exhaustive]
    Parts {
        /// Their ids, ascending.
        candidates: Vec<u64>,
    },
    /// A socket that admits no candidate whatever the catalog holds.
    #[non_exhaustive]
    Blocked {
        /// The socket.
        socket: String,
        /// Why.
        why: Blocked,
    },
}

/// A stored document another extractor version wrote: not judged, for
/// this build's rules would report it as this build's read (M5's QR7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "the features document was written by extractor version {extractor_version}, not this build's"
)]
#[non_exhaustive]
pub struct NotCurrent {
    /// The document's own extractor version.
    pub extractor_version: u32,
}

/// A flag a candidate's verdict carries (VD12): a closed list whose names
/// never change meaning.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CandidateFlag {
    /// A passed requirement is a thread or press fit no rule confirms.
    ThreadNotConfirmed,
    /// No material is compared (section 14): on every verdict.
    MaterialNotCompared,
    /// The candidate's features are not complete (VD11).
    CandidateIncomplete,
    /// A socket has requirements removed in review.
    RequirementsRemoved,
    /// A socket has requirements no one reviewed.
    RequirementsUnreviewed,
    /// Under D6 functional, a clearance passed that strict would not.
    LargerThanOriginal,
}

/// The facts of the read that judged a candidate (EV6).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ReadFacts {
    /// The extractor that read it.
    pub extractor_version: u32,
    /// Its features' `complete`.
    pub complete: bool,
    /// Its largest stated uncertainty, in mm.
    pub max_uncertainty_mm: f64,
    /// Its solids' largest vertex offset, in mm.
    pub max_vertex_offset_mm: f64,
}

/// One candidate's judgment.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CandidateResult {
    /// Where it came from.
    pub source: CandidateSource,
    /// Its product, as the file states it.
    pub product: Option<ProductRef>,
    /// M2's gate: accepted or refused.
    pub status: Status,
    /// Why M2's gate refused the file; empty when read (CF2).
    pub refusals: Vec<Refusal>,
    /// The read's facts; `None` when refused.
    pub read: Option<ReadFacts>,
    /// The worst of its sockets' verdicts, or its refusal's.
    pub verdict: Verdict,
    /// Its flags, sorted (VD12).
    pub flags: Vec<CandidateFlag>,
    /// Its warnings: empty in M4 (VD13).
    pub warnings: Vec<Warning>,
    /// Each socket's, in the document's order; none for a refused file.
    pub sockets: Vec<SocketResult>,
    /// The fit's evidence (FM3), when a verifier with an identity was
    /// asked, or would have been but for a placement not unique (FV6) or a
    /// helper's failure (FV8); `None` otherwise, as in 0.3.0.
    pub fit: Option<Box<FitEvidence>>,
}

impl CandidateResult {
    /// FV8: the fit's run depends on the machine (a timeout, a resource
    /// kill, a cancel, a protocol failure or an unreadable store), so a run
    /// that holds this result never claims to be complete.
    pub fn fit_incomplete(&self) -> bool {
        self.fit.as_ref().is_some_and(|e| e.outcome().incomplete())
    }

    /// Its stage (EV5): its least advanced socket's; `None` if refused.
    pub fn stage(&self) -> Option<Stage> {
        self.sockets.iter().map(|s| s.stage).min()
    }

    /// SL1 broken (M4-17): a socket the shortlist dropped whose every
    /// check verification ran passes. Alignment and verification run
    /// anyway, for the evidence, so the drop is a fitment bug.
    pub fn shortlist_unsound(&self) -> bool {
        self.sockets
            .iter()
            .any(|s| s.stage == Stage::Shortlist && s.checks.iter().all(|(_, c)| c.passed()))
    }

    /// M4-17, VD14 (`docs/research/m6-api.md`, CO2): this result is a
    /// fitment bug, and the run exits 1: an internal refusal, or a pass
    /// after a shortlist drop ([`Self::shortlist_unsound`]). The CLI and
    /// the server both ask this, so their exit codes agree (JB4).
    pub fn is_fitment_bug(&self) -> bool {
        self.refusals
            .iter()
            .any(|r| r.code == RefusalCode::Internal)
            || self.shortlist_unsound()
    }

    /// VD15 (CO2): the original's result read as its self-check: where it
    /// fails beyond VD5 and VD6, if it does, and before that, where AL11's
    /// bounds excuse its socket.
    pub fn original_check(&self) -> OriginalCheck {
        let mut exhausted = Vec::new();
        for socket in &self.sockets {
            let mut excused = false;
            for (id, check) in &socket.checks {
                if check.passed() {
                    continue;
                }
                let codes: Vec<FailureCode> =
                    check.evidence().failures().iter().map(|f| f.code).collect();
                let at = format!("{}/{}", socket.socket, id.as_str());
                let within = |allowed: &dyn Fn(&FailureCode) -> bool| {
                    !codes.is_empty() && codes.iter().all(allowed)
                };
                if within(&|c| {
                    matches!(
                        c,
                        FailureCode::AlignmentExhausted | FailureCode::CoverageExhausted
                    )
                }) {
                    // AL11's bounds are no bug: the run names them (VR21).
                    exhausted.push(at);
                    excused = true;
                } else if !within(&|c| {
                    matches!(
                        c,
                        FailureCode::Unverifiable | FailureCode::FloorAboveTolerance
                    )
                }) {
                    // A failure with no code is not one VD5 or VD6 explains.
                    return OriginalCheck {
                        exhausted,
                        fails_at: Some(at),
                    };
                }
            }
            if socket.stage == Stage::Shortlist && !excused {
                return OriginalCheck {
                    exhausted,
                    fails_at: Some(format!("{} (the shortlist dropped it)", socket.socket)),
                };
            }
        }
        OriginalCheck {
            exhausted,
            fails_at: None,
        }
    }

    /// VD15 (CO2): the original fails its own socket beyond what VD5 and
    /// VD6 explain, a fitment bug: the run exits 1, its evidence kept.
    pub fn original_fails(&self) -> bool {
        self.original_check().fails_at.is_some()
    }
}

/// What VD15's self-check of the original found (CO2), in the order the
/// CLI notes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct OriginalCheck {
    /// Each `socket/requirement` AL11's bounds stopped, which excuses its
    /// socket: not checked, and no bug.
    pub exhausted: Vec<String>,
    /// Where it fails beyond VD5 and VD6: `socket/requirement`, or a
    /// socket the shortlist dropped; `None` when it does not.
    pub fails_at: Option<String>,
}

/// What a socket's judgment found, before its verdict.
struct Judged {
    /// The shortlist dropped the candidate: no fit is asked (M4-26).
    dropped: bool,
    stage: Stage,
    placement: Option<Placement>,
    chosen: Chosen,
    checks: Vec<(RequirementId, Check)>,
    placements: (usize, bool),
}

/// One socket of the document, ready: its view, signature, unreviewed
/// count and the target's occurrences' transforms (for `T_k · A`).
struct Ready<'a> {
    view: SocketView<'a>,
    signature: SocketSignature,
    unreviewed: usize,
    occurrences: Vec<Rigid>,
}

/// One socket document, ready to judge candidates.
#[non_exhaustive]
pub struct Matcher<'a> {
    document: &'a SocketDocument,
    sockets: Vec<Ready<'a>>,
    policy: MatchPolicy,
    options: MatchOptions,
    fit: &'a dyn FitVerifier,
    /// The fit's scene, zones and control, for a verifier with an
    /// identity (FC2, FT9).
    fitting: Option<fitting::Fitting>,
    /// False to skip the shortlist (KH9's cross-check).
    shortlist: bool,
}

impl std::fmt::Debug for Matcher<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Matcher")
            .field("policy", &self.policy)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl<'a> Matcher<'a> {
    /// The matcher of `document` under `policy`: refused unless the
    /// document is accepted and current, and, with `require_reviewed`,
    /// reviewed throughout (MF2, M4-22).
    ///
    /// # Errors
    ///
    /// [`SocketRefused`] says why.
    pub fn new(
        document: &'a SocketDocument,
        policy: MatchPolicy,
        options: MatchOptions,
        fit: &'a dyn FitVerifier,
    ) -> Result<Self, SocketRefused> {
        let Some(extracted) = document
            .extracted()
            .filter(|_| document.status() == Status::Accepted)
        else {
            return Err(SocketRefused::Refused(document.refusals().to_vec()));
        };
        if !document.is_current() {
            return Err(SocketRefused::Stale {
                extractor: document.extractor_version(),
                socket_rules: document.socket_rules_version(),
            });
        }
        let mut sockets = Vec::new();
        let mut unreviewed = Vec::new();
        for socket in &extracted.sockets {
            let Some(view) = SocketView::new(document, &socket.id) else {
                continue;
            };
            let before = unreviewed.len();
            for r in view.active() {
                if document.decision(&socket.id, &r.id) == ReviewStatus::Unreviewed {
                    unreviewed.push(format!("{}/{}", socket.id, r.id));
                }
            }
            let signature = SocketSignature::of(&view, &policy);
            let occurrences = socket
                .occurrences
                .iter()
                .filter_map(|path| {
                    extracted
                        .occurrences
                        .iter()
                        .find(|o| &o.path == path)
                        .map(|o| o.transform)
                })
                .collect();
            sockets.push(Ready {
                view,
                signature,
                unreviewed: unreviewed.len() - before,
                occurrences,
            });
        }
        if options.require_reviewed() && !unreviewed.is_empty() {
            let count = unreviewed.len();
            unreviewed.truncate(5);
            return Err(SocketRefused::Unreviewed {
                count,
                first: unreviewed,
            });
        }
        let fitting = fit
            .identity()
            .map(|identity| fitting::Fitting::new(identity, fit.assembly(), document));
        Ok(Self {
            document,
            sockets,
            policy,
            options,
            fit,
            fitting,
            shortlist: true,
        })
    }

    /// This matcher with the shortlist off: every candidate is aligned over
    /// all its stacks (KH9: the verdict must not change). For the harness.
    #[doc(hidden)]
    #[must_use]
    pub fn without_shortlist(mut self) -> Self {
        self.shortlist = false;
        self
    }

    /// The socket document.
    pub fn document(&self) -> &'a SocketDocument {
        self.document
    }

    /// The policy.
    pub fn policy(&self) -> &MatchPolicy {
        &self.policy
    }

    /// The options.
    pub fn options(&self) -> MatchOptions {
        self.options
    }

    /// The fit verifier's kernel and version, if it has an identity (MF6).
    pub fn fit_kernel(&self) -> Option<(String, String)> {
        self.fit_identity()
            .map(|i| (i.kernel.clone(), i.version.clone()))
    }

    /// The fit verifier's identity, if it has one (FC1): a run with one
    /// writes `fitment.match` v2 (FM1).
    pub fn fit_identity(&self) -> Option<&crate::fit::FitIdentity> {
        self.fitting.as_ref().map(fitting::Fitting::identity)
    }

    /// The original's own fit, the control (FT9): its complete run, else
    /// its last incomplete one (FV8); `None` with no verifier that has an
    /// identity, or before a candidate's interface passed at every socket.
    pub fn fit_control(&self) -> Option<FitEvidence> {
        self.fitting.as_ref().and_then(fitting::Fitting::control)
    }

    /// FV8: the run that judged `results` with this matcher depends on the
    /// machine, so it never claims to be complete: a run of the control
    /// was incomplete, or a result's fit was ([`CandidateResult::fit_incomplete`]).
    pub fn fit_incomplete<'r>(
        &self,
        results: impl IntoIterator<Item = &'r CandidateResult>,
    ) -> bool {
        self.fitting
            .as_ref()
            .is_some_and(fitting::Fitting::control_incomplete)
            || results.into_iter().any(CandidateResult::fit_incomplete)
    }

    /// Level 1 for every socket of the document under this matcher's own
    /// signatures, intersected (M5's QR3): the candidates of `index` level 2
    /// may keep in every socket, ascending.
    ///
    /// # Errors
    ///
    /// The index's own error, if it cannot be read.
    pub fn admitted<S: IndexSource>(&self, index: &S) -> Result<Admitted, S::Error> {
        let mut found: Option<Vec<u64>> = None;
        for ready in &self.sockets {
            let ids = match level_one(&ready.signature, index, found.as_deref())? {
                Ok(ids) => ids,
                Err(why) => {
                    return Ok(Admitted::Blocked {
                        socket: ready.view.socket().id.clone(),
                        why,
                    });
                }
            };
            // Within the sockets before: the intersection so far.
            found = Some(ids);
        }
        Ok(Admitted::Parts {
            candidates: found.unwrap_or_default(),
        })
    }

    /// Each socket's id with the active requirements that fail every
    /// candidate (MF10): not M4-verifiable (VD5), or blocked by the
    /// target's own uncertainty under the policy (VD6), in order.
    pub fn failing_all(&self) -> Vec<(&str, Vec<&str>)> {
        self.sockets
            .iter()
            .map(|r| {
                let failing = r
                    .view
                    .active()
                    .iter()
                    .filter(|q| !q.verifiable() || r.view.blocked(q, &self.policy))
                    .map(|q| q.id.as_str())
                    .collect();
                (r.view.socket().id.as_str(), failing)
            })
            .collect()
    }

    /// Each socket's id with its unreviewed count, in order.
    pub fn unreviewed(&self) -> Vec<(&str, usize)> {
        self.sockets
            .iter()
            .map(|r| (r.view.socket().id.as_str(), r.unreviewed))
            .collect()
    }

    /// CF1, CF2: one candidate file, through M2's gate, then judged; a
    /// refused file is a `Reject` with the gate's codes.
    pub fn judge_step(&self, step: &[u8]) -> CandidateResult {
        let read = FeaturesDocument::read(step);
        let source = Source::of(step, read.header().cloned());
        // This build just read it, so it is current.
        self.judge_read(&read, source, Some(step))
    }

    /// CF1 for a stored document: an accepted one judged as
    /// [`Matcher::judge_step`] judges the file it was read from, a refused
    /// one the same `Reject`. `source` names the content the document was
    /// checked against: a store builds it from its own checked SHA-256.
    /// The fit verifier gets no STEP bytes.
    ///
    /// # Errors
    ///
    /// [`NotCurrent`] for a document another [`crate::EXTRACTOR_VERSION`]
    /// wrote: judging it with this build's rules would report it as read
    /// by this build.
    pub fn judge_document(
        &self,
        document: &FeaturesDocument,
        source: Source,
    ) -> Result<CandidateResult, NotCurrent> {
        if !document.is_current() {
            return Err(NotCurrent {
                extractor_version: document.extractor_version(),
            });
        }
        Ok(self.judge_read(document, source, None))
    }

    /// The one path both share, for a current document: the fit request
    /// gets `step` where there is one (a file's bytes), so a file run keeps
    /// its fit check.
    fn judge_read(
        &self,
        document: &FeaturesDocument,
        source: Source,
        step: Option<&[u8]>,
    ) -> CandidateResult {
        let body = match step {
            Some(bytes) => fitting::Body::Step {
                bytes,
                sha256: source.sha256.clone(),
            },
            None => fitting::Body::None,
        };
        let source = CandidateSource::File(source);
        match document.part() {
            Some(part) => self.judge(part, source, &body),
            None => CandidateResult {
                source,
                product: None,
                status: Status::Refused,
                refusals: document.refusals().to_vec(),
                read: None,
                verdict: Verdict::refused(document.refusals().iter().map(|r| r.code).collect()),
                flags: self.review_flags(vec![CandidateFlag::MaterialNotCompared]),
                warnings: Vec::new(),
                sockets: Vec::new(),
                fit: None,
            },
        }
    }

    /// One candidate part, read once (CF1), against every socket; `source`
    /// says where it came from.
    pub fn judge_part(&self, part: &Part, source: CandidateSource) -> CandidateResult {
        self.judge(part, source, &fitting::Body::None)
    }

    /// CO5 (`docs/research/m6-api.md`): a component of an assembly, read
    /// and chosen by core ([`SelectedComponent::read`]), judged as
    /// [`Matcher::judge_part`] judges its part. A `source` that is not
    /// [`CandidateSource::Component`] of that same assembly and definition
    /// is an internal refusal, a fitment bug (M4-17): a caller never names
    /// a component as another.
    pub fn judge_component(
        &self,
        component: &SelectedComponent,
        source: CandidateSource,
    ) -> CandidateResult {
        let theirs = matches!(
            &source,
            CandidateSource::Component { assembly_sha256, definition }
                if assembly_sha256 == component.assembly_sha256()
                    && *definition == component.definition()
        );
        if theirs {
            return self.judge(
                component.part(),
                source,
                &fitting::Body::Component(component),
            );
        }
        let refusals = vec![Refusal::new(
            RefusalCode::Internal,
            Some(component.definition()),
            "a component was named as another candidate than itself",
        )];
        CandidateResult {
            source,
            product: component.part().definition.product.clone(),
            status: Status::Refused,
            verdict: Verdict::refused(refusals.iter().map(|r| r.code).collect()),
            refusals,
            read: None,
            flags: self.review_flags(vec![CandidateFlag::MaterialNotCompared]),
            warnings: Vec::new(),
            sockets: Vec::new(),
            fit: None,
        }
    }

    fn judge(
        &self,
        part: &Part,
        source: CandidateSource,
        body: &fitting::Body<'_>,
    ) -> CandidateResult {
        let view = CandidateView::of(part);
        let mut sockets: Vec<SocketResult> = self
            .sockets
            .iter()
            .map(|ready| self.judge_socket(ready, &view))
            .collect();
        let fit = self
            .fitting
            .as_ref()
            .and_then(|fitting| self.fit_candidate(fitting, part, &view, body, &mut sockets))
            .map(Box::new);
        let named: Vec<(String, Verdict)> = sockets
            .iter()
            .map(|s| (s.socket.clone(), s.verdict.clone()))
            .collect();
        CandidateResult {
            source,
            product: part.definition.product.clone(),
            status: Status::Accepted,
            refusals: Vec::new(),
            read: Some(ReadFacts {
                extractor_version: crate::EXTRACTOR_VERSION,
                complete: part.complete,
                max_uncertainty_mm: part.max_uncertainty_mm,
                max_vertex_offset_mm: part
                    .solids
                    .iter()
                    .map(|s| s.max_vertex_offset_mm)
                    .fold(0.0, f64::max),
            }),
            verdict: combine(&named),
            flags: self.flags(part, &sockets),
            warnings: Vec::new(),
            sockets,
            fit,
        }
    }

    /// VD12's flags the socket document gives every candidate, refused
    /// ones too, added to `flags`, sorted.
    fn review_flags(&self, mut flags: Vec<CandidateFlag>) -> Vec<CandidateFlag> {
        if self.sockets.iter().any(|r| !r.view.removed().is_empty()) {
            flags.push(CandidateFlag::RequirementsRemoved);
        }
        if self.sockets.iter().any(|r| r.unreviewed > 0) {
            flags.push(CandidateFlag::RequirementsUnreviewed);
        }
        flags.sort_unstable();
        flags
    }

    /// VD12's flags of a candidate from its sockets.
    fn flags(&self, part: &Part, sockets: &[SocketResult]) -> Vec<CandidateFlag> {
        let mut flags = vec![CandidateFlag::MaterialNotCompared];
        let threaded =
            sockets.iter().zip(&self.sockets).any(|(result, ready)| {
                result.checks.iter().any(|(id, c)| {
                    c.passed()
                        && ready.view.active().iter().any(|r| {
                            r.id == id.as_str() && r.flags.contains(&Flag::ThreadNotConfirmed)
                        })
                })
            });
        if threaded {
            flags.push(CandidateFlag::ThreadNotConfirmed);
        }
        if !part.complete {
            flags.push(CandidateFlag::CandidateIncomplete);
        }
        let mut flags = self.review_flags(flags);
        let departs = sockets
            .iter()
            .flat_map(|s| s.checks.iter())
            .any(|(_, c)| c.passed() && c.evidence().departs());
        if departs {
            flags.push(CandidateFlag::LargerThanOriginal);
        }
        flags.sort_unstable();
        flags
    }

    /// AL14: socket `socket` checked at a placement the caller gives;
    /// `None` if the document has no such socket.
    pub fn verify_at(&self, part: &Part, socket: &str, at: &Rigid) -> Option<SocketResult> {
        let ready = self.sockets.iter().find(|r| r.view.socket().id == socket)?;
        let view = &ready.view;
        let cand = CandidateView::of(part);
        let placement = Placement::given(*at);
        let mut budget = Budget::new();
        let checks = view
            .active()
            .iter()
            .map(|r| {
                let c = check(view, r, &cand, &placement, &self.policy, &mut budget);
                (RequirementId::new(r.id.clone()), c)
            })
            .collect();
        let judged = Judged {
            dropped: false,
            stage: Stage::Verification,
            placement: Some(placement),
            chosen: Chosen::Given,
            checks,
            placements: (1, false),
        };
        Some(self.result(ready, &cand, judged, fitting::ALONE))
    }

    /// One socket: the shortlist, alignment, the search and the checks.
    fn judge_socket(&self, ready: &Ready<'_>, cand: &CandidateView<'_>) -> SocketResult {
        let view = &ready.view;
        let kept: Option<Result<Kept, Vec<Missing>>> = self
            .shortlist
            .then(|| shortlist(&ready.signature, cand.signature()));
        let searching = view.active().iter().any(|r| self.searched(view, r));
        let placements = if searching {
            align(
                view,
                cand,
                kept.as_ref().and_then(|k| k.as_ref().ok()),
                &self.policy,
            )
        } else {
            Placements::none()
        };
        let built = (placements.tried, placements.exhausted);
        let dropped = matches!(kept, Some(Err(_)));
        let unplaced = |stage| Judged {
            dropped,
            stage,
            placement: None,
            chosen: Chosen::None,
            checks: self.unplaced(view, cand, &placements),
            placements: built,
        };
        let pending = crate::fit::INTERFACE_REJECTED_ELSEWHERE;
        let mut result = if !searching {
            // Nothing to place: what is active fails every candidate.
            self.result(ready, cand, unplaced(Stage::Verification), pending)
        } else if placements.ordered.is_empty() {
            let mut r = self.result(ready, cand, unplaced(Stage::Alignment), pending);
            r.verdict = r.verdict.rejecting(RejectReason::NoPlacement {
                socket: view.socket().id.clone(),
                unanchored: placements.unanchored.clone(),
            });
            r
        } else {
            let (placement, chosen, checks) = self.search(view, cand, &placements);
            let passed = !checks.is_empty() && checks.iter().all(|(_, c)| c.passed());
            let alternatives = (self.fitting.is_some() && passed)
                .then(|| self.alternatives(view, cand, &placements, &placement));
            let judged = Judged {
                dropped,
                stage: Stage::Verification,
                placement: Some(placement),
                chosen,
                checks,
                placements: built,
            };
            let mut r = self.result(ready, cand, judged, pending);
            r.alternatives = alternatives;
            r
        };
        if let Some(Err(missing)) = kept {
            result.stage = Stage::Shortlist;
            result.missing = missing;
            result.verdict = result.verdict.rejecting(RejectReason::Shortlist {
                socket: view.socket().id.clone(),
            });
        }
        result
    }

    /// The socket's result from its checks: `decide` over every active
    /// requirement, with no fit attempted where the interface failed (VD4,
    /// M4-26). Where it passed: with no verifier that has an identity, M4's
    /// `not_checked`, unchanged from 0.3.0; with one, `pending` until the
    /// candidate's one fit is asked (FC2, FT11).
    fn result(
        &self,
        ready: &Ready<'_>,
        cand: &CandidateView<'_>,
        judged: Judged,
        pending: &str,
    ) -> SocketResult {
        let view = &ready.view;
        let ids: Vec<RequirementId> = view
            .active()
            .iter()
            .map(|r| RequirementId::new(r.id.clone()))
            .collect();
        let Judged {
            dropped,
            stage,
            placement,
            chosen,
            checks,
            placements,
        } = judged;
        let rejected = FitStatus::NotChecked {
            reason: INTERFACE_REJECTED.to_owned(),
        };
        let mut verdict = decide(&ids, checks.clone(), rejected);
        if verdict.kind() != VerdictKind::Reject && !dropped {
            let at = placement.as_ref().map_or(Rigid::IDENTITY, |p| p.transform);
            let placements: Vec<Rigid> = ready
                .occurrences
                .iter()
                .filter_map(|occurrence| at.then(occurrence))
                .collect();
            // Every occurrence placed, or no verifier is asked: a fit seen
            // at fewer occurrences than the socket has proves nothing, and
            // one never attempted is not checked (VD4, FT2).
            let fit = if placements.len() != view.socket().occurrences.len() {
                FitStatus::NotChecked {
                    reason: OCCURRENCE_UNPLACED.to_owned(),
                }
            } else if self.fitting.is_some() {
                FitStatus::NotChecked {
                    reason: pending.to_owned(),
                }
            } else {
                FitStatus::no_verifier()
            };
            verdict = decide(&ids, checks.clone(), fit);
        }
        let u = view.target_u().max(cand.uncertainty_mm());
        let mut not_checked: Vec<NotChecked> = view
            .removed()
            .iter()
            .zip(view.removal_notes())
            .map(|(r, note)| NotChecked::Removed {
                requirement: RequirementId::new(r.id.clone()),
                note: (*note).to_owned(),
            })
            .collect();
        not_checked.extend(view.active().iter().filter_map(|r| match &r.requires {
            Requires::Shaft {
                max_extra_mm: Some(mm),
                ..
            } => Some(NotChecked::MaxExtraMm {
                requirement: RequirementId::new(r.id.clone()),
                mm: *mm,
            }),
            _ => None,
        }));
        SocketResult {
            socket: view.socket().id.clone(),
            stage,
            verdict,
            placement,
            chosen,
            u_mm: u,
            epsilon_mm: self.policy.linear_mm() - u,
            missing: Vec::new(),
            checks,
            not_checked,
            placements,
            freedom: None,
            alternatives: None,
        }
    }

    /// True for a requirement the search must pass: one M4 can verify and
    /// the floor does not fail (AL10).
    fn searched(&self, view: &SocketView<'_>, r: &Requirement) -> bool {
        r.verifiable() && !view.blocked(r, &self.policy)
    }

    /// AL10: placements in order, bores and shafts first, a placement that
    /// fails one not taken further; the first passing every searched
    /// requirement, else the one passing most (ties to the first). At it,
    /// every active requirement is checked in full. Past a bound, the
    /// failed checks say so.
    fn search(
        &self,
        view: &SocketView<'_>,
        cand: &CandidateView<'_>,
        placements: &Placements,
    ) -> (Placement, Chosen, Vec<(RequirementId, Check)>) {
        let mut budget = Budget::new();
        let axial = |r: &&&Requirement| {
            matches!(r.requires, Requires::Bore { .. } | Requires::Shaft { .. })
        };
        let searched: Vec<&Requirement> = view
            .active()
            .iter()
            .filter(|r| self.searched(view, r))
            .copied()
            .collect();
        let (first, rest): (Vec<&Requirement>, Vec<&Requirement>) =
            searched.iter().partition(|r| axial(r));
        let mut best: Option<(usize, &Placement)> = None;
        let mut chosen = None;
        // The chosen placement's checks, kept: checked again, they would be
        // charged again, and could fail on the bound where they passed.
        let mut kept: Vec<(String, Check)> = Vec::new();
        for placement in &placements.ordered {
            let mut passed = 0;
            let mut these = Vec::new();
            for (k, r) in first.iter().chain(&rest).enumerate() {
                let c = check(view, r, cand, placement, &self.policy, &mut budget);
                let ok = c.passed();
                these.push((r.id.clone(), c));
                if ok {
                    passed += 1;
                } else if k < first.len() {
                    break;
                }
            }
            if passed == searched.len() {
                chosen = Some(placement);
                kept = these;
                break;
            }
            if best.is_none_or(|(n, _)| passed > n) {
                best = Some((passed, placement));
            }
            if budget.used > budget.limit {
                break;
            }
        }
        let exhausted = chosen.is_none() && placements.exhausted;
        let how = if chosen.is_some() {
            Chosen::FirstPassing
        } else {
            Chosen::FewestFailures
        };
        let Some(at) = chosen.or(best.map(|b| b.1)).or(placements.ordered.first()) else {
            return (Placement::given(Rigid::IDENTITY), Chosen::None, Vec::new());
        };
        let checks = view
            .active()
            .iter()
            .map(|r| {
                let c = match kept.iter().position(|(id, _)| *id == r.id) {
                    Some(k) => kept.swap_remove(k).1,
                    None => check(view, r, cand, at, &self.policy, &mut budget),
                };
                let c = if exhausted && !c.passed() {
                    Check::of(c.evidence().clone().with_failure(Failure::new(
                        FailureCode::AlignmentExhausted,
                        At::whole(),
                        "alignment reached its bound before a placement passed",
                    )))
                } else {
                    c
                };
                (RequirementId::new(r.id.clone()), c)
            })
            .collect();
        (at.clone(), how, checks)
    }

    /// FM3: every placement of `placements` other than `chosen` at which
    /// every active requirement passes, each transform once, in
    /// alignment's order: the placements the interface also passes at, for
    /// which no fit is asked (FT12). Each is checked under its own bound,
    /// so listing them never changes the chosen placement's checks.
    fn alternatives(
        &self,
        view: &SocketView<'_>,
        cand: &CandidateView<'_>,
        placements: &Placements,
        chosen: &Placement,
    ) -> Vec<Rigid> {
        let mut out: Vec<Rigid> = Vec::new();
        for placement in &placements.ordered {
            let at = placement.transform;
            if at == chosen.transform || out.contains(&at) {
                continue;
            }
            let mut budget = Budget::new();
            let passes = view
                .active()
                .iter()
                .all(|r| check(view, r, cand, placement, &self.policy, &mut budget).passed());
            if passes {
                out.push(at);
            }
        }
        out
    }

    /// KH3: no placement. A requirement no candidate feature could anchor,
    /// or any other M4 can verify, fails with `no_placement`; the rest fail
    /// as every candidate does (verified at the identity, which they fail
    /// whatever the placement).
    fn unplaced(
        &self,
        view: &SocketView<'_>,
        cand: &CandidateView<'_>,
        placements: &Placements,
    ) -> Vec<(RequirementId, Check)> {
        let mut budget = Budget::new();
        let identity = Placement::given(Rigid::IDENTITY);
        view.active()
            .iter()
            .map(|r| {
                let id = RequirementId::new(r.id.clone());
                let c = if self.searched(view, r) {
                    let why = if placements.unanchored.contains(&id) {
                        "no candidate feature can anchor it"
                    } else if placements.exhausted {
                        "alignment reached its bound"
                    } else {
                        "no placement of the candidate was found"
                    };
                    let code = if placements.exhausted {
                        FailureCode::AlignmentExhausted
                    } else {
                        FailureCode::NoPlacement
                    };
                    Check::of(Evidence::failed(code, why))
                } else {
                    check(view, r, cand, &identity, &self.policy, &mut budget)
                };
                (id, c)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(codes: &[FailureCode]) -> Check {
        let failures = codes
            .iter()
            .map(|code| Failure::new(*code, At::default(), "planted"))
            .collect();
        Check::of(Evidence::new(Vec::new(), Vec::new(), failures))
    }

    fn socket(name: &str, stage: Stage, checks: Vec<(&str, Check)>) -> SocketResult {
        SocketResult {
            socket: name.to_owned(),
            stage,
            verdict: Verdict::refused(Vec::new()),
            placement: None,
            chosen: Chosen::None,
            u_mm: 0.0,
            epsilon_mm: 0.0,
            missing: Vec::new(),
            checks: checks
                .into_iter()
                .map(|(id, c)| (RequirementId::new(id), c))
                .collect(),
            not_checked: Vec::new(),
            placements: (0, false),
            freedom: None,
            alternatives: None,
        }
    }

    fn result(sockets: Vec<SocketResult>) -> CandidateResult {
        CandidateResult {
            source: CandidateSource::component("ab".repeat(32), 7),
            product: None,
            status: Status::Accepted,
            refusals: Vec::new(),
            read: None,
            verdict: Verdict::refused(Vec::new()),
            flags: Vec::new(),
            warnings: Vec::new(),
            sockets,
            fit: None,
        }
    }

    /// VD15 (CO2): a constructed original failing a requirement beyond VD5
    /// and VD6 fails there; one VD5 or VD6 explains does not; a failure
    /// with no code, or a socket the shortlist dropped, does.
    #[test]
    fn a_failing_original_names_where_it_fails() {
        let v = Stage::Verification;
        let fails = result(vec![socket(
            "s1",
            v,
            vec![
                ("bore:a", check(&[])),
                ("bore:b", check(&[FailureCode::Unverifiable])),
                (
                    "bore:c",
                    check(&[FailureCode::DiameterMin, FailureCode::Unverifiable]),
                ),
            ],
        )]);
        assert_eq!(
            fails.original_check(),
            OriginalCheck {
                exhausted: Vec::new(),
                fails_at: Some("s1/bore:c".into()),
            }
        );
        assert!(fails.original_fails());
        let explained = result(vec![socket(
            "s1",
            v,
            vec![
                ("bore:a", check(&[FailureCode::FloorAboveTolerance])),
                ("bore:b", check(&[FailureCode::Unverifiable])),
            ],
        )]);
        assert_eq!(explained.original_check(), OriginalCheck::default());
        assert!(!explained.original_fails());
        let uncoded = Check::Failed(Evidence::new(Vec::new(), Vec::new(), Vec::new()));
        let bare = result(vec![socket("s1", v, vec![("seat:a", uncoded)])]);
        assert_eq!(bare.original_check().fails_at.as_deref(), Some("s1/seat:a"));
        let dropped = result(vec![socket(
            "s2",
            Stage::Shortlist,
            vec![("bore:a", check(&[]))],
        )]);
        assert_eq!(
            dropped.original_check().fails_at.as_deref(),
            Some("s2 (the shortlist dropped it)")
        );
    }

    /// VD15, AL11 (CO2): a constructed original whose search a bound
    /// exhausted is not checked there and excuses its socket, a drop
    /// included; a later socket's failure is still found, after the
    /// exhausted ones in the order the CLI notes them.
    #[test]
    fn an_exhausted_original_is_excused_where_it_is_exhausted() {
        let exhausted = result(vec![
            socket(
                "s1",
                Stage::Shortlist,
                vec![("bore:a", check(&[FailureCode::AlignmentExhausted]))],
            ),
            socket(
                "s2",
                Stage::Verification,
                vec![(
                    "bore:b",
                    check(&[
                        FailureCode::CoverageExhausted,
                        FailureCode::AlignmentExhausted,
                    ]),
                )],
            ),
        ]);
        assert_eq!(
            exhausted.original_check(),
            OriginalCheck {
                exhausted: vec!["s1/bore:a".into(), "s2/bore:b".into()],
                fails_at: None,
            }
        );
        assert!(!exhausted.original_fails());
        let then_fails = result(vec![
            socket(
                "s1",
                Stage::Verification,
                vec![("bore:a", check(&[FailureCode::AlignmentExhausted]))],
            ),
            socket(
                "s2",
                Stage::Verification,
                vec![(
                    "bore:b",
                    check(&[FailureCode::AlignmentExhausted, FailureCode::AxisOffset]),
                )],
            ),
        ]);
        assert_eq!(
            then_fails.original_check(),
            OriginalCheck {
                exhausted: vec!["s1/bore:a".into()],
                fails_at: Some("s2/bore:b".into()),
            }
        );
    }
}
