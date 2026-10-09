//! Verdicts, and the one function allowed to make them.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Serialize;

use crate::fit::FitStatus;

/// Identifies one requirement of a socket.
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

/// The outcome of checking one requirement against a placed candidate.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Check {
    /// The requirement was positively verified.
    Passed,
    /// The requirement was not verified. That includes a check that could
    /// not be evaluated: unknown means reject.
    Failed {
        /// What was expected and what was found.
        reason: String,
    },
}

/// What a verdict says about a candidate.
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
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RejectReason {
    /// The socket has no requirements, so nothing could be verified.
    NoRequirements,
    /// The socket lists the same requirement twice.
    DuplicateRequirement {
        /// The repeated requirement.
        requirement: RequirementId,
    },
    /// A requirement was never checked.
    MissingCheck {
        /// The unchecked requirement.
        requirement: RequirementId,
    },
    /// A requirement was checked more than once, so its result is ambiguous.
    DuplicateCheck {
        /// The requirement checked more than once.
        requirement: RequirementId,
    },
    /// A check names a requirement the socket does not have.
    UnknownRequirement {
        /// The unknown requirement.
        requirement: RequirementId,
    },
    /// A requirement was checked and not verified.
    RequirementFailed {
        /// The failed requirement.
        requirement: RequirementId,
        /// What was expected and what was found.
        detail: String,
    },
    /// The candidate's body interferes with a neighbor.
    BodyInterferes,
}

/// The judgment on one candidate, with the reasons for a rejection.
///
/// Only [`decide`] builds a verdict. It serializes for reports but cannot be
/// deserialized, so a stored or edited file can never become a `Match`.
#[derive(Debug, Clone, PartialEq, Serialize)]
// Fields are private so that every verdict has gone through `decide`.
#[allow(clippy::exhaustive_structs)]
pub struct Verdict {
    #[serde(rename = "verdict")]
    kind: VerdictKind,
    fit: FitStatus,
    reasons: Vec<RejectReason>,
}

impl Verdict {
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
            Some([Check::Passed]) => {}
            Some([Check::Failed { reason }]) => reasons.push(RejectReason::RequirementFailed {
                requirement: requirement.clone(),
                detail: reason.clone(),
            }),
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
