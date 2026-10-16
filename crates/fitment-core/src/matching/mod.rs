//! Matching (`docs/research/m4-match.md`): a candidate part judged against
//! a socket, requirement by requirement, with the evidence. This module
//! holds what PR 2 builds: the policy, the evidence a check records, the
//! candidate's and the socket's signatures, the shortlist and its index.

mod align;
mod axial;
mod candidate;
mod document;
mod evidence;
mod index;
mod matcher;
mod policy;
mod region;
mod shortlist;
mod signature;
mod verify;

pub use align::{MAX_ANCHOR_PAIRS, MAX_PLACEMENTS, Placement, Placements, Seed, align};
pub use candidate::CandidateView;
pub use document::{
    MATCH_FORMAT, MATCH_FORMAT_VERSION, MatchDocument, Outcome, Summary, match_candidates,
};
pub use evidence::{
    At, Band, BandWidth, EndSide, Evidence, Failure, FailureCode, Measure, MeasureValue, Quantity,
    Warning, WarningCode,
};
pub use index::{Blocked, IndexSource, IndexTokens, PairToken, Posting, ShortlistIndex, level_one};
pub use matcher::{
    Admitted, CandidateFlag, CandidateResult, CandidateSource, Chosen, Matcher, NotChecked,
    NotCurrent, ReadFacts, SocketRefused, SocketResult, Stage,
};
pub use policy::{Clearance, Grip, MatchOptions, MatchPolicy};
pub use shortlist::{Kept, Missing, MissingElement, shortlist};
pub use signature::{PartSignature, SigCone, SigPlane, SigStack, SocketSignature, Unread, Window};
pub use verify::verify;

use crate::socket::{Requirement, Requires, Socket, SocketDocument};

/// The version of the signature's content (CF10): it rises with any change
/// to what `PartSignature::of` computes, or to the level-1 tokens a catalog
/// stores; a catalog rebuilds stored tokens of another. 2: pair tokens name
/// their stacks by index (M5's B4).
pub const SIGNATURE_VERSION: u32 = 2;

/// The version of the matching rules (M4-27): it rises with any change to
/// the shortlist, alignment, verification or the verdicts they give.
pub const MATCH_RULES_VERSION: u32 = 1;

/// The most work of coverage marching (VR11) and core face tests (VR22)
/// for one candidate and socket (AL11), counted in edges read: a coverage
/// step costs the edges of the region (and of the cover, where it asks
/// containment), a core test one per face and one per edge of a plane
/// face's polygon. Past it the requirement fails, never passes.
pub const MAX_COVERAGE_STEPS: u64 = 20_000_000;

/// The least uncertainty a part is taken to have (VR2): M3's B1 guard.
pub const GUARD_MM: f64 = crate::socket::GUARD_MM;

/// The most comparisons the shortlist makes for one socket and candidate
/// (SL4); past them it keeps the candidate.
pub const MAX_SHORTLIST_CHECKS: u64 = 1_000_000;

/// One socket of a document, ready to judge: its active and removed
/// requirements, its contact tolerance and the target's own uncertainty.
#[derive(Debug, Clone)]
pub struct SocketView<'a> {
    socket: &'a Socket,
    active: Vec<&'a Requirement>,
    removed: Vec<&'a Requirement>,
    notes: Vec<&'a str>,
    contact_mm: f64,
    target_u: f64,
}

impl<'a> SocketView<'a> {
    /// Socket `id` of an accepted `document`; `None` for none.
    pub fn new(document: &'a SocketDocument, id: &str) -> Option<Self> {
        let extracted = document.extracted()?;
        let socket = extracted.sockets.iter().find(|s| s.id == id)?;
        let target = &extracted.target;
        Some(Self {
            socket,
            active: document.active(&socket.id).collect(),
            removed: document.removed(&socket.id).map(|(r, _)| r).collect(),
            notes: document.removed(&socket.id).map(|(_, n)| n).collect(),
            contact_mm: document.contact_mm(),
            target_u: target
                .part_uncertainty_mm
                .max(target.vertex_offset_mm)
                .max(GUARD_MM),
        })
    }

    /// The socket.
    pub fn socket(&self) -> &Socket {
        self.socket
    }

    /// Its requirements review has not removed.
    pub fn active(&self) -> &[&'a Requirement] {
        &self.active
    }

    /// Those it has.
    pub fn removed(&self) -> &[&'a Requirement] {
        &self.removed
    }

    /// The notes review gave the removed requirements, in their order.
    pub fn removal_notes(&self) -> &[&'a str] {
        &self.notes
    }

    /// The contact tolerance the socket was read with (M3's Z3).
    pub fn contact_mm(&self) -> f64 {
        self.contact_mm
    }

    /// `u_T`: the target's own uncertainty, never below the guard.
    pub fn target_u(&self) -> f64 {
        self.target_u
    }

    /// SG7, VD6 for the target's own uncertainty: true if `requirement`
    /// fails every candidate under `policy`, for the tolerance leaves no
    /// room above `u_T` (t − `u_T` ≤ 0), or a clearance span's limits from its
    /// neighbours leave the original none (c + 2(τ + `u_T`) ≥ d for a bore,
    /// d + 2(τ + `u_T`) ≥ c for a shaft, τ = max(contact, span floor, `u_T`)).
    pub fn blocked(&self, requirement: &Requirement, policy: &MatchPolicy) -> bool {
        let u = self.target_u;
        if policy.linear_mm() - u <= 0.0 {
            return true;
        }
        let room = |from: f64, to: f64, inner: f64, outer: f64| {
            let tau = self
                .contact_mm
                .max(span_floor(requirement, from, to))
                .max(u);
            inner + 2.0 * (tau + u) >= outer
        };
        match &requirement.requires {
            Requires::Bore { spans, .. } => spans.iter().any(|s| {
                !s.pinned
                    && s.clear_diameter_mm
                        .is_some_and(|c| room(s.from_mm, s.to_mm, c, s.hole_diameter_mm))
            }),
            Requires::Shaft { spans, .. } => spans.iter().any(|s| {
                !s.pinned
                    && s.clear_diameter_mm
                        .is_some_and(|c| room(s.from_mm, s.to_mm, s.shaft_diameter_mm, c))
            }),
            _ => false,
        }
    }
}

/// A span's floor: the largest of its sources' whose stations overlap it,
/// or the requirement's with none (VR4, VR5).
pub(crate) fn span_floor(requirement: &Requirement, from: f64, to: f64) -> f64 {
    let e = 1e-9 * (1.0 + from.abs().max(to.abs()));
    requirement
        .sources
        .iter()
        .filter(|s| {
            s.span_mm
                .is_some_and(|[a, b]| a.min(b) <= to + e && a.max(b) >= from - e)
        })
        .map(|s| s.floor_mm)
        .reduce(f64::max)
        .unwrap_or(requirement.floor_mm)
}
