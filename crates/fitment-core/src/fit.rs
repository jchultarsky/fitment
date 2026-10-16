//! The result of checking a candidate's body against its neighbors (D1).

use serde::Serialize;

/// The reason given when no fit verifier is configured.
pub const NO_FIT_VERIFIER: &str = "no fit verifier in this build";

/// The reason a socket whose interface failed gets no fit check
/// (`docs/research/m4-match.md`, VD4).
pub const INTERFACE_REJECTED: &str = "interface rejected";

/// The reason a candidate M2's gate refused gets none (VD10).
pub const CANDIDATE_REFUSED: &str = "candidate refused";

/// Whether the candidate's body fits among the assembly's other solids.
///
/// Only [`FitStatus::Verified`] can make a [`Match`](crate::VerdictKind::Match),
/// and [`FitStatus::Interferes`] always makes a
/// [`Reject`](crate::VerdictKind::Reject). The other two leave an interface
/// that passed as an [`InterfaceMatch`](crate::VerdictKind::InterfaceMatch).
///
/// It serializes with a `status` tag, for example
/// `{"status":"not_checked","reason":"no fit verifier in this build"}`.
/// It is not deserializable: a fit result comes from a verifier, never from
/// a file.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
// The four outcomes are fixed by decision D1; a fifth would change what a
// verdict means, which is a breaking change anyway.
#[allow(clippy::exhaustive_enums)]
pub enum FitStatus {
    /// The body was checked and touches its neighbors only where the socket
    /// declares contact.
    Verified {
        /// The geometry kernel that performed the check.
        kernel: String,
        /// The exact kernel version, so the result can be reproduced.
        version: String,
    },
    /// The body occupies space that a neighbor occupies.
    Interferes {
        /// Each neighbor the body interferes with.
        interferences: Vec<Interference>,
    },
    /// A check was attempted and did not complete: a crash, a timeout, a
    /// kernel warning, or geometry the verifier does not support.
    NotVerified {
        /// What went wrong.
        reason: String,
    },
    /// No check was attempted.
    NotChecked {
        /// Why not, usually [`NO_FIT_VERIFIER`].
        reason: String,
    },
}

impl FitStatus {
    /// The status when no fit verifier is configured.
    pub fn no_verifier() -> Self {
        Self::NotChecked {
            reason: NO_FIT_VERIFIER.to_owned(),
        }
    }
}

/// One neighbor that a candidate's body interferes with.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Interference {
    /// The neighbor, as named in the assembly.
    pub neighbor: String,
    /// The volume both bodies occupy, in cubic millimetres.
    pub volume_mm3: f64,
}

impl Interference {
    /// Records an interference with `neighbor` over `volume_mm3`.
    pub fn new(neighbor: impl Into<String>, volume_mm3: f64) -> Self {
        Self {
            neighbor: neighbor.into(),
            volume_mm3,
        }
    }
}

/// What a fit verifier is asked about one socket (M7): where the candidate
/// stands at each occurrence, in the assembly's root frame.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FitRequest<'a> {
    /// The socket, as `socket-1`.
    pub socket: &'a str,
    /// The candidate's STEP bytes, when the run has them.
    pub candidate_step: Option<&'a [u8]>,
    /// The socket's source assembly, by its SHA-256 (hex).
    pub assembly_sha256: &'a str,
    /// `T_k · A`: the candidate's placement at each occurrence.
    pub placements: Vec<crate::geom::Rigid>,
}

/// Checks a placed candidate's body against the assembly's other solids
/// (D1, M7). M4 has none: [`NoFitVerifier`] answers `not_checked`.
pub trait FitVerifier: Sync {
    /// The fit of the candidate as `request` places it.
    fn check(&self, request: &FitRequest<'_>) -> FitStatus;

    /// The kernel and version that check, for the document (MF6); `None`
    /// for no verifier.
    fn kernel(&self) -> Option<(String, String)> {
        None
    }
}

/// No verifier: every fit `not_checked`, [`NO_FIT_VERIFIER`].
#[derive(Debug, Clone, Copy, Default)]
// A unit with nothing to grow: callers name it.
#[allow(clippy::exhaustive_structs)]
pub struct NoFitVerifier;

impl FitVerifier for NoFitVerifier {
    fn check(&self, _request: &FitRequest<'_>) -> FitStatus {
        FitStatus::no_verifier()
    }
}
