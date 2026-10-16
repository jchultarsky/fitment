//! The matcher (`docs/research/m4-match.md`, sections 3 and 12): one
//! socket document, checked once (MF2), judging candidates one at a time.
//! Per socket: the shortlist, alignment, then verification at the first
//! placement that passes every requirement M4 can verify, or at the one
//! passing most (AL10); the candidate's verdict is the worst of its
//! sockets' (VD2, VR17).

use super::align::{Placement, Placements, align};
use super::evidence::{At, Evidence, Failure, FailureCode};
use super::shortlist::{Kept, Missing, shortlist};
use super::verify::{Budget, check};
use super::{CandidateView, MatchOptions, MatchPolicy, SocketSignature, SocketView};
use crate::features::{Part, Status, read_part};
use crate::fit::{FitStatus, INTERFACE_REJECTED};
use crate::gate::Refusal;
use crate::geom::Rigid;
use crate::socket::{Requirement, Requires, ReviewStatus, SocketDocument};
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
    /// What the shortlist found missing, if it dropped the candidate.
    pub missing: Vec<Missing>,
    /// One check per active requirement, in the socket's order.
    pub checks: Vec<(RequirementId, Check)>,
    /// The requirements removed in review: not checked (VR16).
    pub not_checked: Vec<RequirementId>,
    /// How many placements alignment built, and whether a bound stopped it.
    pub placements: (usize, bool),
}

/// One candidate's judgment.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CandidateResult {
    /// The worst of its sockets' verdicts, or its refusal's.
    pub verdict: Verdict,
    /// Each socket's, in the document's order; none for a refused file.
    pub sockets: Vec<SocketResult>,
    /// Why M2's gate refused the file; empty when read (CF2).
    pub refusals: Vec<Refusal>,
}

/// One socket document, ready to judge candidates.
#[derive(Debug)]
#[non_exhaustive]
pub struct Matcher<'a> {
    sockets: Vec<(SocketView<'a>, SocketSignature)>,
    policy: MatchPolicy,
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
            for r in view.active() {
                if document.status_of(&socket.id, &r.id).0 == ReviewStatus::Unreviewed {
                    unreviewed.push(format!("{}/{}", socket.id, r.id));
                }
            }
            let signature = SocketSignature::of(&view, &policy);
            sockets.push((view, signature));
        }
        if options.require_reviewed() && !unreviewed.is_empty() {
            let count = unreviewed.len();
            unreviewed.truncate(5);
            return Err(SocketRefused::Unreviewed {
                count,
                first: unreviewed,
            });
        }
        Ok(Self { sockets, policy })
    }

    /// CF1, CF2: one candidate file, through M2's gate, then judged; a
    /// refused file is a `Reject` with the gate's codes.
    pub fn judge_step(&self, step: &[u8]) -> CandidateResult {
        match read_part(step) {
            Ok(part) => self.judge_part(&part),
            Err(refused) => CandidateResult {
                verdict: Verdict::refused(refused.refusals().iter().map(|r| r.code).collect()),
                sockets: Vec::new(),
                refusals: refused.refusals().to_vec(),
            },
        }
    }

    /// One candidate part, read once (CF1), against every socket.
    pub fn judge_part(&self, part: &Part) -> CandidateResult {
        let view = CandidateView::of(part);
        let sockets: Vec<SocketResult> = self
            .sockets
            .iter()
            .map(|(socket, signature)| self.judge(socket, signature, &view))
            .collect();
        let named: Vec<(String, Verdict)> = sockets
            .iter()
            .map(|s| (s.socket.clone(), s.verdict.clone()))
            .collect();
        CandidateResult {
            verdict: combine(&named),
            sockets,
            refusals: Vec::new(),
        }
    }

    /// AL14: socket `socket` checked at a placement the caller gives;
    /// `None` if the document has no such socket.
    pub fn verify_at(&self, part: &Part, socket: &str, at: &Rigid) -> Option<SocketResult> {
        let (view, _) = self.sockets.iter().find(|(v, _)| v.socket().id == socket)?;
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
        Some(Self::result(
            view,
            Stage::Verification,
            Some(placement),
            checks,
            (1, false),
        ))
    }

    /// One socket: the shortlist, alignment, the search and the checks.
    fn judge(
        &self,
        view: &SocketView<'_>,
        signature: &SocketSignature,
        cand: &CandidateView<'_>,
    ) -> SocketResult {
        let kept: Result<Kept, Vec<Missing>> = shortlist(signature, cand.signature());
        if !view.active().iter().any(|r| self.searched(view, r)) {
            // Nothing to place: what is active fails every candidate.
            let none = Placements::none();
            let checks = self.unplaced(view, cand, &none);
            return Self::result(view, Stage::Verification, None, checks, (0, false));
        }
        let placements = align(view, cand, kept.as_ref().ok(), &self.policy);
        let built = (placements.tried, placements.exhausted);
        let mut result = if placements.ordered.is_empty() {
            let checks = self.unplaced(view, cand, &placements);
            let mut r = Self::result(view, Stage::Alignment, None, checks, built);
            r.verdict = r.verdict.rejecting(RejectReason::NoPlacement {
                socket: view.socket().id.clone(),
                unanchored: placements.unanchored.clone(),
            });
            r
        } else {
            let (placement, checks) = self.search(view, cand, &placements);
            Self::result(view, Stage::Verification, Some(placement), checks, built)
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
    /// requirement, the fit not checked (M4 has no verifier), or not
    /// attempted where the interface failed (VD4).
    fn result(
        view: &SocketView<'_>,
        stage: Stage,
        placement: Option<Placement>,
        checks: Vec<(RequirementId, Check)>,
        placements: (usize, bool),
    ) -> SocketResult {
        let ids: Vec<RequirementId> = view
            .active()
            .iter()
            .map(|r| RequirementId::new(r.id.clone()))
            .collect();
        let mut verdict = decide(&ids, checks.clone(), FitStatus::no_verifier());
        if verdict.kind() == VerdictKind::Reject {
            verdict = decide(
                &ids,
                checks.clone(),
                FitStatus::NotChecked {
                    reason: INTERFACE_REJECTED.to_owned(),
                },
            );
        }
        SocketResult {
            socket: view.socket().id.clone(),
            stage,
            verdict,
            placement,
            missing: Vec::new(),
            checks,
            not_checked: view
                .removed()
                .iter()
                .map(|r| RequirementId::new(r.id.clone()))
                .collect(),
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
    ) -> (Placement, Vec<(RequirementId, Check)>) {
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
        for placement in &placements.ordered {
            let mut passed = 0;
            for (k, r) in first.iter().chain(&rest).enumerate() {
                if check(view, r, cand, placement, &self.policy, &mut budget).passed() {
                    passed += 1;
                } else if k < first.len() {
                    break;
                }
            }
            if passed == searched.len() {
                chosen = Some(placement);
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
        let Some(at) = chosen.or(best.map(|b| b.1)).or(placements.ordered.first()) else {
            return (Placement::given(Rigid::IDENTITY), Vec::new());
        };
        let checks = view
            .active()
            .iter()
            .map(|r| {
                let c = check(view, r, cand, at, &self.policy, &mut budget);
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
        (at.clone(), checks)
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
