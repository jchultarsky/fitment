//! The result of checking a candidate's body against its neighbors (D1),
//! and the pure half of the fit verifier (`docs/research/m7-fit.md`, FC3):
//! the scene fitment reads from the assembly ([`FitScene`], FB1), the
//! zones a socket declares ([`zones`], FZ1 to FZ9), each pair's own
//! tolerances and the pairs within the kernel's reach ([`plan`], FT3, FT7,
//! FT8), the status from a kernel's measures ([`decide`], FV1 to FV9), and
//! a placement's free motions ([`freedom`], FC4). Nothing here reads a
//! file or runs a process: the kernel's numbers come in as [`FitMeasures`],
//! and only [`decide`] turns them into a [`FitStatus`].

mod decide;
mod freedom;
mod measures;
mod plan;
mod scene;
mod zones;

use serde::Serialize;

pub use decide::{Finding, FindingAt, FitCode, FitOutcome, PairInterference, decide};
pub use freedom::{FREE_SLIDES_MM, FREE_TURNS_DEG, FreeMotion, Freedom, MotionKind, freedom};
pub use measures::{
    BodyMeasure, CheckMeasures, FitBox, FitFailure, FitMeasures, LandmarkMeasure, PairMeasure,
    PlaceMeasures, PlacementMeasure, ReachClaim, SceneMeasures, SolidMeasure, ZoneMeasure,
};
pub use plan::{
    FitCandidate, FitPlan, Pair, PairState, PairValues, PairZone, Placed, PlanInput, plan,
};
pub use scene::{
    FitScene, FitSceneError, Landmark, LandmarkValue, SceneBody, UnreadLeaf, landmarks,
};
pub use zones::{Built, Unbuilt, Zone, ZoneKind, ZoneShape, Zones, zones};

/// The version of the fit's own rules (FH6, FC8): it rises with any change
/// that could change a fit's status or evidence, such as a zone, a depth,
/// a threshold, a reason or a pair chosen.
pub const FIT_RULES_VERSION: u32 = 1;

/// The least residual volume that is an interference, in mm³ (FV3): a
/// residual under it, and above 0, is `below_resolution`.
pub const FIT_VOLUME_MM3: f64 = 1e-6;

/// What no fit checks (FT12), in the order every fit's evidence lists it.
pub const SCOPE_NOT_CHECKED: [&str; 8] = [
    "motion",
    "tolerance_stack_up",
    "assembly_order",
    "tool_access",
    "unmodelled_items",
    "threads",
    "new_fasteners",
    "alternative_placements",
];

/// The verifier a fit is decided for (FC1): the kernel and its exact
/// version, as a [`FitStatus::Verified`] names them. [`decide`] builds a
/// `verified` only with the identity its plan was made with.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FitIdentity {
    /// The geometry kernel, such as `open_cascade`.
    pub kernel: String,
    /// Its exact version, such as `8.0.1`.
    pub version: String,
}

impl FitIdentity {
    /// The identity of `kernel` at `version`.
    pub fn new(kernel: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            kernel: kernel.into(),
            version: version.into(),
        }
    }
}

/// The candidate's body, as a fit request names it (FC1, FB4).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FitBody<'a> {
    /// A file candidate: the bytes M2's gate accepted, and their SHA-256
    /// (64 lower-case hex digits), which the helper's own hash must equal.
    #[non_exhaustive]
    Step {
        /// The file's bytes.
        bytes: &'a [u8],
        /// Their SHA-256.
        sha256: String,
    },
    /// A component of an assembly: the assembly's SHA-256, the
    /// occurrence's path and the faces' `#id`s of its solid (FC5).
    #[non_exhaustive]
    Component {
        /// The assembly's SHA-256.
        assembly_sha256: String,
        /// The occurrence, by usage `#id`s.
        occurrence: Vec<u64>,
        /// Its solid's faces' `#id`s, sorted.
        faces: Vec<u64>,
    },
}

impl<'a> FitBody<'a> {
    /// A file candidate's body: `bytes` and their SHA-256.
    pub fn step(bytes: &'a [u8]) -> Self {
        Self::Step {
            bytes,
            sha256: crate::sha256::sha256_hex(bytes),
        }
    }

    /// A component's body.
    pub fn component(
        assembly_sha256: impl Into<String>,
        occurrence: Vec<u64>,
        faces: Vec<u64>,
    ) -> Self {
        Self::Component {
            assembly_sha256: assembly_sha256.into(),
            occurrence,
            faces,
        }
    }
}

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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
