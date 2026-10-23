//! The result of checking a candidate's body against its neighbors (D1),
//! and the pure half of the fit verifier (`docs/research/m7-fit.md`, FC3):
//! the scene fitment reads from the assembly ([`FitScene`], FB1), the
//! zones a socket declares ([`zones`], FZ1 to FZ9), each pair's own
//! tolerances and the pairs within the kernel's reach ([`plan`], FT3, FT7,
//! FT8), the status from a kernel's measures ([`decide`], FV1 to FV9), and
//! a placement's free motions ([`freedom`], FC4). Nothing here reads a
//! file or runs a process: the kernel's numbers come in as [`FitMeasures`],
//! and only [`decide`] turns them into a [`FitStatus`].
//!
//! A verifier answers [`FitVerifier::measure`] with measures, never a
//! status (FC1): the matcher asks it once per candidate whose every
//! socket's interface passed (FC2, FT11), and builds the status from the
//! measures with [`plan`] and [`decide`] itself.

mod decide;
mod evidence;
mod freedom;
mod measures;
mod plan;
mod scene;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
mod zones;

use serde::Serialize;

use crate::socket::SocketDocument;

pub use decide::{Finding, FindingAt, FitCode, FitOutcome, PairInterference, decide};
pub use evidence::{FitEvidence, PairEvidence};
pub use freedom::{FREE_SLIDES_MM, FREE_TURNS_DEG, FreeMotion, Freedom, MotionKind, freedom};
pub use measures::{
    BodyMeasure, CheckMeasures, FitBox, FitFailure, FitMeasures, LandmarkMeasure, PairMeasure,
    PlaceMeasures, PlacementMeasure, ReachClaim, SceneMeasures, SolidMeasure, ZoneMeasure,
};
pub use plan::{
    FitCandidate, FitPlan, Pair, PairState, PairValues, PairZone, Placed, PlanInput, plan,
};
pub(crate) use scene::faces_of;
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
/// version, as a [`FitStatus::Verified`] names them, and the helper that
/// runs it, as `fitment.match` v2's `fit_verifier` states it (FM2).
/// [`decide`] builds a `verified` only with the identity its plan was made
/// with.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FitIdentity {
    /// The geometry kernel, such as `open_cascade`.
    pub kernel: String,
    /// Its exact version, such as `8.0.1`.
    pub version: String,
    /// The binding the helper imports, such as
    /// `cadquery-ocp-novtk 8.0.1.1.0`; empty when there is none.
    pub binding: String,
    /// The helper's Python, such as `3.13`; empty when there is none.
    pub python: String,
    /// The helper's protocol (FH6); 0 when there is none.
    pub protocol: u32,
    /// The SHA-256 of the helper script; empty when there is none.
    pub helper_sha256: String,
    /// The SHA-256 of its lock; empty when there is none.
    pub lock_sha256: String,
}

impl FitIdentity {
    /// The identity of `kernel` at `version`, with no helper named.
    pub fn new(kernel: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            kernel: kernel.into(),
            version: version.into(),
            binding: String::new(),
            python: String::new(),
            protocol: 0,
            helper_sha256: String::new(),
            lock_sha256: String::new(),
        }
    }

    /// This identity with the helper that runs the kernel (FH3): its
    /// binding, Python, protocol, and the SHA-256s of its script and lock.
    #[must_use]
    pub fn with_helper(
        mut self,
        binding: impl Into<String>,
        python: impl Into<String>,
        protocol: u32,
        helper_sha256: impl Into<String>,
        lock_sha256: impl Into<String>,
    ) -> Self {
        self.binding = binding.into();
        self.python = python.into();
        self.protocol = protocol;
        self.helper_sha256 = helper_sha256.into();
        self.lock_sha256 = lock_sha256.into();
        self
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

/// The reason a socket whose interface passed gets no fit when another
/// socket's rejected (FT11): the fit is asked once per candidate, after
/// every socket's interface passed.
pub const INTERFACE_REJECTED_ELSEWHERE: &str = "another socket's interface rejected";

/// The reason a socket with an occurrence the matcher could not place gets
/// no fit (FT2, M4's rule).
pub const OCCURRENCE_UNPLACED: &str = "an occurrence of the socket could not be placed";

/// The reason the other sockets of such a candidate get none (FT11): the
/// one request holds every occurrence of every socket.
pub const OCCURRENCE_UNPLACED_ELSEWHERE: &str =
    "an occurrence of another socket could not be placed";

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
    // FC1: built only by `decide`; no other crate can make one (VT51's
    // compile-fail doctest is on `FitVerifier`).
    #[non_exhaustive]
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

/// What a fit verifier is asked about one candidate (FC1, FT11): its body,
/// the socket document, and what [`plan`] reads: core's scene and zones,
/// the candidate as core read it, and its placement at every occurrence of
/// every socket (FT2). Built only by core, once per candidate whose every
/// socket's interface passed; the verifier measures what it names and
/// returns the measures, from which core alone decides (FV1).
#[derive(Debug, Clone)]
pub struct FitRequest<'a> {
    body: FitBody<'a>,
    document: &'a SocketDocument,
    input: PlanInput<'a>,
}

impl<'a> FitRequest<'a> {
    pub(crate) fn new(
        body: FitBody<'a>,
        document: &'a SocketDocument,
        input: PlanInput<'a>,
    ) -> Self {
        Self {
            body,
            document,
            input,
        }
    }

    /// The candidate's body (FB4).
    pub fn body(&self) -> &FitBody<'a> {
        &self.body
    }

    /// The socket document.
    pub fn socket_document(&self) -> &'a SocketDocument {
        self.document
    }

    /// The scene core read from the socket's assembly (FB1): the bodies the
    /// `scene` request names.
    pub fn scene(&self) -> &'a FitScene {
        self.input.scene
    }

    /// The candidate as core read it: its landmarks, box and SHA-256 (FB3,
    /// FB4).
    pub fn candidate(&self) -> &'a FitCandidate {
        self.input.candidate
    }

    /// `T_k · A` at every occurrence of every socket, in socket and
    /// occurrence order (FT2): the `place` request's placements.
    pub fn placements(&self) -> &'a [Placed] {
        self.input.placements
    }

    /// The verifier the fit is decided for.
    pub fn identity(&self) -> &FitIdentity {
        &self.input.identity
    }

    /// The SHA-256 of the `place` request, which its reply echoes.
    pub fn place_digest(&self) -> String {
        self.input.place_digest()
    }

    /// The plan the kernel's `scene` and `place` replies give (FT3, FT7,
    /// FT8): the pairs the `check` request asks. Core makes the same plan
    /// from the same replies when it decides.
    pub fn plan(&self, scene: &SceneMeasures, place: &PlaceMeasures) -> FitPlan {
        plan(&self.input, scene, place)
    }

    pub(crate) fn input(&self) -> &PlanInput<'a> {
        &self.input
    }
}

/// Measures a placed candidate's body against the assembly's other solids
/// (D1, M7). The verifier returns the kernel's numbers, never a status
/// (FC1): the matcher turns them into one with [`plan`] and [`decide`], so
/// a `verified` names the verifier's [`identity`](Self::identity) and
/// nothing else can make one. [`NoFitVerifier`] has no identity: every fit
/// is `not_checked`, as in 0.3.0.
///
/// A [`FitStatus::Verified`] is built only by [`decide`], from a kernel's
/// measures and with the verifier's [`FitIdentity`] (FC1, M7-36): no
/// verifier, fake or real, can hand the matcher one, and no other crate
/// can make one (VT51):
///
/// ```compile_fail,E0639
/// let forged = fitment_core::FitStatus::Verified {
///     kernel: "open_cascade".to_owned(),
///     version: "8.0.1".to_owned(),
/// };
/// ```
///
/// A status of another kind can be named; only `verified` cannot:
///
/// ```
/// let fine = fitment_core::FitStatus::NotVerified {
///     reason: "the fit check timed out".to_owned(),
/// };
/// assert!(!matches!(fine, fitment_core::FitStatus::Verified { .. }));
/// ```
pub trait FitVerifier: Sync {
    /// The verifier's identity; `None` for no verifier. Only a verifier
    /// with one is asked to measure (FC2).
    fn identity(&self) -> Option<FitIdentity> {
        None
    }

    /// The socket's assembly, the bytes its kernel reads: core reads its
    /// own scene from the same bytes (FB1). `None` for no verifier; a
    /// verifier with an identity and no assembly, or another assembly than
    /// the socket's source, has every fit `not_verified`.
    fn assembly(&self) -> Option<&[u8]> {
        None
    }

    /// The kernel's measures of the candidate as `request` places it
    /// (FH4): the `scene`, `place` and `check` replies, the last for the
    /// pairs of [`FitRequest::plan`]; or how the helper failed (FV8).
    fn measure(&self, request: &FitRequest<'_>) -> Result<FitMeasures, FitFailure>;
}

/// No verifier: no identity, so every fit `not_checked`,
/// [`NO_FIT_VERIFIER`], and no measures are ever asked.
#[derive(Debug, Clone, Copy, Default)]
// A unit with nothing to grow: callers name it.
#[allow(clippy::exhaustive_structs)]
pub struct NoFitVerifier;

impl FitVerifier for NoFitVerifier {
    fn measure(&self, _request: &FitRequest<'_>) -> Result<FitMeasures, FitFailure> {
        Err(FitFailure::Unavailable(NO_FIT_VERIFIER.to_owned()))
    }
}
