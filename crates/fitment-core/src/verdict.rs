//! Verdicts, and the one function allowed to make them.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Serialize;

use crate::fit::{CANDIDATE_REFUSED, FitStatus, INTERFACE_REJECTED, Interference};
use crate::gate::RefusalCode;
use crate::matching::{Evidence, FailureCode};

/// Identifies one requirement of a socket.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
#[allow(clippy::exhaustive_structs)] // A newtype; its field is private.
pub struct RequirementId(String);

impl RequirementId {
    /// Wraps an identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RequirementId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for RequirementId {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

impl From<String> for RequirementId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

/// The outcome of checking one requirement against a placed candidate,
/// with its evidence (`docs/research/m4-match.md`, EV1).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Check {
    /// The requirement was positively verified.
    Passed(Evidence),
    /// The requirement was not verified. That includes a check that could
    /// not be evaluated: unknown means reject.
    Failed(Evidence),
}

impl Check {
    /// The check its evidence shows: passed exactly when it records no
    /// failure and every measure passed (EV7).
    pub fn of(evidence: Evidence) -> Self {
        if evidence.passes() {
            Self::Passed(evidence)
        } else {
            Self::Failed(evidence)
        }
    }

    /// Its evidence.
    pub fn evidence(&self) -> &Evidence {
        match self {
            Self::Passed(e) | Self::Failed(e) => e,
        }
    }

    /// True if it passed, as its evidence shows: a `Passed` whose evidence
    /// records a failure, or a measure that failed, did not.
    pub fn passed(&self) -> bool {
        matches!(self, Self::Passed(e) if e.passes())
    }
}

/// What a verdict says about a candidate.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
// The three kinds are fixed by decision D1.
#[allow(clippy::exhaustive_enums)]
pub enum VerdictKind {
    /// Every requirement and the body fit are verified.
    Match,
    /// Every requirement is verified; the body fit was not checked or could
    /// not be verified.
    InterfaceMatch,
    /// The candidate does not fit, or the evidence was incomplete.
    Reject,
}

/// Why a candidate was rejected.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RejectReason {
    /// The socket has no requirements, so nothing could be verified.
    NoRequirements,
    /// The socket lists the same requirement twice.
    #[non_exhaustive]
    DuplicateRequirement {
        /// The repeated requirement.
        requirement: RequirementId,
    },
    /// A requirement was never checked.
    #[non_exhaustive]
    MissingCheck {
        /// The unchecked requirement.
        requirement: RequirementId,
    },
    /// A requirement was checked more than once, so its result is ambiguous.
    #[non_exhaustive]
    DuplicateCheck {
        /// The requirement checked more than once.
        requirement: RequirementId,
    },
    /// A check names a requirement the socket does not have.
    #[non_exhaustive]
    UnknownRequirement {
        /// The unknown requirement.
        requirement: RequirementId,
    },
    /// A requirement was checked and not verified.
    #[non_exhaustive]
    RequirementFailed {
        /// The failed requirement.
        requirement: RequirementId,
        /// What was expected and what was found.
        detail: String,
        /// Its failures' codes, sorted.
        codes: Vec<FailureCode>,
    },
    /// M2's gate refused the candidate's file: no socket was checked
    /// (VD10).
    #[non_exhaustive]
    CandidateRefused {
        /// The gate's refusals.
        codes: Vec<RefusalCode>,
    },
    /// The candidate's body interferes with a neighbor.
    BodyInterferes,
    /// The shortlist found what a socket demands missing (M4-17): the
    /// checks were still made, for the evidence.
    #[non_exhaustive]
    Shortlist {
        /// The socket.
        socket: String,
    },
    /// No placement of the candidate was found in a socket (KH3).
    #[non_exhaustive]
    NoPlacement {
        /// The socket.
        socket: String,
        /// The requirements no candidate feature could anchor.
        unanchored: Vec<RequirementId>,
    },
    /// The socket document has no socket to judge against (VD2).
    NoSockets,
}

/// The judgment on one candidate, with the reasons for a rejection.
///
/// Only [`decide`] builds a verdict, but for a refused candidate's
/// (`docs/research/m4-match.md`, VD1). It serializes for reports but
/// cannot be deserialized, so a stored or edited file can never become a
/// `Match`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize)]
// Fields are private so that every verdict has gone through `decide`, or
// is a refused candidate's `Reject`.
#[allow(clippy::exhaustive_structs)]
pub struct Verdict {
    #[serde(rename = "verdict")]
    kind: VerdictKind,
    fit: FitStatus,
    reasons: Vec<RejectReason>,
}

impl Verdict {
    /// A candidate M2's gate refused: a `Reject` with its codes, no fit
    /// checked (VD1, VD10). Only the matcher builds it.
    pub(crate) fn refused(mut codes: Vec<RefusalCode>) -> Self {
        codes.sort_unstable();
        codes.dedup();
        Self {
            kind: VerdictKind::Reject,
            fit: FitStatus::NotChecked {
                reason: CANDIDATE_REFUSED.to_owned(),
            },
            reasons: vec![RejectReason::CandidateRefused { codes }],
        }
    }

    /// This verdict, rejecting for `reason` first.
    pub(crate) fn rejecting(mut self, reason: RejectReason) -> Self {
        self.kind = VerdictKind::Reject;
        self.reasons.insert(0, reason);
        self
    }

    /// What the verdict says.
    pub fn kind(&self) -> VerdictKind {
        self.kind
    }

    /// The result of the body-fit check.
    pub fn fit(&self) -> &FitStatus {
        &self.fit
    }

    /// Why the candidate was rejected; empty unless the kind is `Reject`.
    pub fn reasons(&self) -> &[RejectReason] {
        &self.reasons
    }
}

/// Judges a candidate from the socket's requirements, the checks made
/// against them and the body-fit result.
///
/// The interface passes only when the socket has at least one requirement
/// and every requirement has exactly one check, which passed. Anything else
/// rejects: a missing, repeated or unknown check, or a repeated requirement.
/// Given a passing interface, only [`FitStatus::Verified`] makes a `Match`;
/// [`FitStatus::Interferes`] rejects in every case.
///
/// Reasons are listed in a fixed order: socket problems and per-requirement
/// failures in the socket's order, then checks for unknown requirements in
/// sorted order, then body interference.
pub fn decide(
    requirements: &[RequirementId],
    checks: impl IntoIterator<Item = (RequirementId, Check)>,
    fit: FitStatus,
) -> Verdict {
    let mut reasons = Vec::new();
    if requirements.is_empty() {
        reasons.push(RejectReason::NoRequirements);
    }

    let mut by_requirement: BTreeMap<RequirementId, Vec<Check>> = BTreeMap::new();
    for (requirement, check) in checks {
        by_requirement.entry(requirement).or_default().push(check);
    }

    let mut seen = BTreeSet::new();
    let mut repeated = BTreeSet::new();
    for requirement in requirements {
        if !seen.insert(requirement) {
            // Report a repeat once, however many times it appears.
            if repeated.insert(requirement) {
                reasons.push(RejectReason::DuplicateRequirement {
                    requirement: requirement.clone(),
                });
            }
            continue;
        }
        match by_requirement.get(requirement).map(Vec::as_slice) {
            None | Some([]) => reasons.push(RejectReason::MissingCheck {
                requirement: requirement.clone(),
            }),
            Some([check]) if check.passed() => {}
            Some([check]) => reasons.push(failed(requirement, check.evidence())),
            Some(_) => reasons.push(RejectReason::DuplicateCheck {
                requirement: requirement.clone(),
            }),
        }
    }

    for requirement in by_requirement.keys() {
        if !seen.contains(requirement) {
            reasons.push(RejectReason::UnknownRequirement {
                requirement: requirement.clone(),
            });
        }
    }

    if matches!(fit, FitStatus::Interferes { .. }) {
        reasons.push(RejectReason::BodyInterferes);
    }

    // Interferes always added a reason, so it can only reach the first arm.
    let kind = if !reasons.is_empty() {
        VerdictKind::Reject
    } else if matches!(fit, FitStatus::Verified { .. }) {
        VerdictKind::Match
    } else {
        VerdictKind::InterfaceMatch
    };

    Verdict { kind, fit, reasons }
}

/// A candidate's verdict from its sockets' (VD2): the worst of them, a
/// candidate that fits one occurrence and not another being a `Reject`;
/// with none, a `Reject`. Each reason's requirement is named with its
/// socket (`socket-1/bore:hole-88`). The body fits only if it fits in
/// every socket; it interferes if it does in any.
pub fn combine(sockets: &[(String, Verdict)]) -> Verdict {
    if sockets.is_empty() {
        return Verdict {
            kind: VerdictKind::Reject,
            fit: FitStatus::no_verifier(),
            reasons: vec![RejectReason::NoSockets],
        };
    }
    let rank = |k: VerdictKind| match k {
        VerdictKind::Match => 0,
        VerdictKind::InterfaceMatch => 1,
        VerdictKind::Reject => 2,
    };
    let kind = sockets
        .iter()
        .map(|(_, v)| v.kind)
        .max_by_key(|k| rank(*k))
        .unwrap_or(VerdictKind::Reject);
    let interferences: Vec<Interference> = sockets
        .iter()
        .filter_map(|(_, v)| match &v.fit {
            FitStatus::Interferes { interferences } => Some(interferences.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    let fit = combined_fit(sockets.iter().map(|(_, v)| &v.fit), interferences);
    // Only a verified body makes a Match (D1): sockets verified by
    // verifiers that disagree are an interface match.
    let kind = if kind == VerdictKind::Match && !matches!(fit, FitStatus::Verified { .. }) {
        VerdictKind::InterfaceMatch
    } else {
        kind
    };
    let named =
        |socket: &str, r: &RequirementId| RequirementId::new(format!("{socket}/{}", r.as_str()));
    let reasons = sockets
        .iter()
        .flat_map(|(socket, v)| {
            v.reasons.iter().map(move |reason| match reason {
                RejectReason::RequirementFailed {
                    requirement,
                    detail,
                    codes,
                } => RejectReason::RequirementFailed {
                    requirement: named(socket, requirement),
                    detail: detail.clone(),
                    codes: codes.clone(),
                },
                RejectReason::MissingCheck { requirement } => RejectReason::MissingCheck {
                    requirement: named(socket, requirement),
                },
                other => other.clone(),
            })
        })
        .collect();
    Verdict { kind, fit, reasons }
}

/// VD4: the candidate's fit from its sockets': `interferes` if any socket
/// interferes, with every interference; else `verified` only if every
/// socket is, by one kernel and version; else `not_verified` if any is,
/// with every reason; else `not_checked`, the verifier's reason before
/// `interface rejected`.
fn combined_fit<'f>(
    fits: impl Iterator<Item = &'f FitStatus> + Clone,
    interferences: Vec<Interference>,
) -> FitStatus {
    if fits
        .clone()
        .any(|f| matches!(f, FitStatus::Interferes { .. }))
    {
        return FitStatus::Interferes { interferences };
    }
    let verified: Vec<(&String, &String)> = fits
        .clone()
        .filter_map(|f| match f {
            FitStatus::Verified { kernel, version } => Some((kernel, version)),
            _ => None,
        })
        .collect();
    let count = fits.clone().count();
    if let Some(&(kernel, version)) = verified.first().filter(|_| verified.len() == count) {
        if verified.iter().all(|v| *v == (kernel, version)) {
            return FitStatus::Verified {
                kernel: kernel.clone(),
                version: version.clone(),
            };
        }
        return FitStatus::NotVerified {
            reason: "the sockets' verifiers disagree".to_owned(),
        };
    }
    let unverified: Vec<&str> = fits
        .clone()
        .filter_map(|f| match f {
            FitStatus::NotVerified { reason } => Some(reason.as_str()),
            _ => None,
        })
        .collect();
    if !unverified.is_empty() {
        return FitStatus::NotVerified {
            reason: unverified.join("; "),
        };
    }
    let reasons: Vec<&str> = fits
        .filter_map(|f| match f {
            FitStatus::NotChecked { reason } => Some(reason.as_str()),
            _ => None,
        })
        .collect();
    let reason = reasons
        .iter()
        .find(|r| **r != INTERFACE_REJECTED)
        .or(reasons.first())
        .map_or_else(
            || crate::fit::NO_FIT_VERIFIER.to_owned(),
            |r| (*r).to_owned(),
        );
    FitStatus::NotChecked { reason }
}

/// The reason a failed check gives: its failures' details and codes.
fn failed(requirement: &RequirementId, evidence: &Evidence) -> RejectReason {
    let mut codes: Vec<FailureCode> = evidence.failures().iter().map(|f| f.code).collect();
    codes.sort_unstable();
    codes.dedup();
    let detail = if evidence.failures().is_empty() {
        let failed = evidence.measures().iter().any(|m| !m.passed());
        if failed {
            "a measure failed"
        } else {
            "failed with no failure recorded"
        }
        .to_owned()
    } else {
        evidence
            .failures()
            .iter()
            .map(|f| f.detail.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    };
    RejectReason::RequirementFailed {
        requirement: requirement.clone(),
        detail,
        codes,
    }
}
