//! The matcher (`docs/research/m4-match.md`, sections 3 and 12): one
//! socket document, checked once (MF2), judging candidates one at a time.
//! Per socket: the shortlist, alignment, then verification at the first
//! placement that passes every requirement M4 can verify, or at the one
//! passing most (AL10); the candidate's verdict is the worst of its
//! sockets' (VD2, VR17).

use serde::Serialize;

use super::align::{Placement, Placements, align};
use super::evidence::{At, Evidence, Failure, FailureCode, Warning};
use super::shortlist::{Kept, Missing, shortlist};
use super::verify::{Budget, check};
use super::{CandidateView, MatchOptions, MatchPolicy, SocketSignature, SocketView};
use crate::assembly::Source;
use crate::features::{FeaturesDocument, Part, ProductRef, Status};
use crate::fit::{FitRequest, FitStatus, FitVerifier, INTERFACE_REJECTED};
use crate::gate::Refusal;
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
    Stale {
        /// Its extractor version.
        extractor: u32,
        /// Its socket rules version.
        socket_rules: u32,
    },
    /// A review was required and requirements are unreviewed.
    #[error("{count} requirements are unreviewed")]
    Unreviewed {
        /// How many.
        count: usize,
        /// The first few, as `socket-1/bore:hole-88`.
        first: Vec<String>,
    },
}

/// How far one socket's judgment went (MF5).
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
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "what", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NotChecked {
    /// Removed in review, with the reviewer's note.
    Removed {
        /// The requirement.
        requirement: RequirementId,
        /// Why.
        note: String,
    },
    /// A shaft's room before a blind hole's floor (I4): information.
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
}

impl SocketResult {
    /// How many checks failed.
    pub fn failed(&self) -> usize {
        self.checks.iter().filter(|(_, c)| !c.passed()).count()
    }
}

/// Where a candidate came from (MF6's `source`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CandidateSource {
    /// A STEP file: its SHA-256, size and header.
    File(Source),
    /// A leaf of an assembly (CF1): the assembly's SHA-256 and the
    /// definition's `#id`.
    Component {
        /// The assembly's SHA-256, hex.
        assembly_sha256: String,
        /// The definition's `#id`.
        definition: u64,
    },
    /// A part the caller built or read itself.
    Given,
}

/// A flag a candidate's verdict carries (VD12): a closed list whose names
/// never change meaning.
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
}

impl CandidateResult {
    /// Its stage (EV5): its least advanced socket's; `None` if refused.
    pub fn stage(&self) -> Option<Stage> {
        self.sockets.iter().map(|s| s.stage).min()
    }
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
        Ok(Self {
            document,
            sockets,
            policy,
            options,
            fit,
        })
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

    /// The fit verifier's kernel and version, if it names one (MF6).
    pub fn fit_kernel(&self) -> Option<(String, String)> {
        self.fit.kernel()
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
        let source = CandidateSource::File(Source::of(step, read.header().cloned()));
        match read.part() {
            Some(part) => self.judge(part, source, Some(step)),
            None => CandidateResult {
                source,
                product: None,
                status: Status::Refused,
                refusals: read.refusals().to_vec(),
                read: None,
                verdict: Verdict::refused(read.refusals().iter().map(|r| r.code).collect()),
                flags: self.review_flags(vec![CandidateFlag::MaterialNotCompared]),
                warnings: Vec::new(),
                sockets: Vec::new(),
            },
        }
    }

    /// One candidate part, read once (CF1), against every socket; `source`
    /// says where it came from.
    pub fn judge_part(&self, part: &Part, source: CandidateSource) -> CandidateResult {
        self.judge(part, source, None)
    }

    fn judge(&self, part: &Part, source: CandidateSource, step: Option<&[u8]>) -> CandidateResult {
        let view = CandidateView::of(part);
        let sockets: Vec<SocketResult> = self
            .sockets
            .iter()
            .map(|ready| self.judge_socket(ready, &view, step))
            .collect();
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
        Some(self.result(ready, &cand, judged, None))
    }

    /// One socket: the shortlist, alignment, the search and the checks.
    fn judge_socket(
        &self,
        ready: &Ready<'_>,
        cand: &CandidateView<'_>,
        step: Option<&[u8]>,
    ) -> SocketResult {
        let view = &ready.view;
        let kept: Result<Kept, Vec<Missing>> = shortlist(&ready.signature, cand.signature());
        let searching = view.active().iter().any(|r| self.searched(view, r));
        let placements = if searching {
            align(view, cand, kept.as_ref().ok(), &self.policy)
        } else {
            Placements::none()
        };
        let built = (placements.tried, placements.exhausted);
        let dropped = kept.is_err();
        let unplaced = |stage| Judged {
            dropped,
            stage,
            placement: None,
            chosen: Chosen::None,
            checks: self.unplaced(view, cand, &placements),
            placements: built,
        };
        let mut result = if !searching {
            // Nothing to place: what is active fails every candidate.
            self.result(ready, cand, unplaced(Stage::Verification), step)
        } else if placements.ordered.is_empty() {
            let mut r = self.result(ready, cand, unplaced(Stage::Alignment), step);
            r.verdict = r.verdict.rejecting(RejectReason::NoPlacement {
                socket: view.socket().id.clone(),
                unanchored: placements.unanchored.clone(),
            });
            r
        } else {
            let (placement, chosen, checks) = self.search(view, cand, &placements);
            let judged = Judged {
                dropped,
                stage: Stage::Verification,
                placement: Some(placement),
                chosen,
                checks,
                placements: built,
            };
            self.result(ready, cand, judged, step)
        };
        if let Err(missing) = kept {
            result.stage = Stage::Shortlist;
            result.missing = missing;
            result.verdict = result.verdict.rejecting(RejectReason::Shortlist {
                socket: view.socket().id.clone(),
            });
        }
        result
    }

    /// The socket's result from its checks: `decide` over every active
    /// requirement, with the fit the verifier gives where the interface
    /// passed, and none attempted where it failed (VD4, M4-26).
    fn result(
        &self,
        ready: &Ready<'_>,
        cand: &CandidateView<'_>,
        judged: Judged,
        step: Option<&[u8]>,
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
            // one never attempted is not checked (VD4).
            let fit = if placements.len() == view.socket().occurrences.len() {
                self.fit.check(&FitRequest {
                    socket: &view.socket().id,
                    candidate_step: step,
                    assembly_sha256: &self.document.source().sha256,
                    placements,
                })
            } else {
                FitStatus::NotChecked {
                    reason: "an occurrence of the socket could not be placed".to_owned(),
                }
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
