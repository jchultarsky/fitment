#![doc = include_str!("../README.md")]
// The I/O and determinism bans in clippy.toml are errors here, not warnings,
// and so are explicit panics (`panic!`, `unwrap`, `expect`, `todo!`,
// `unimplemented!`): SECURITY.md treats a panic on crafted input as a
// vulnerability. Implicit ones (indexing, arithmetic, `assert!`) are not
// lints and need review. Workspace lints cannot be extended per member, so
// these live in the crate root.
#![deny(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::dbg_macro,
    clippy::exit,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented
)]
// Public types that will grow must be #[non_exhaustive] so that adding a
// variant or field is not a breaking change after the first publish.
#![warn(clippy::exhaustive_enums, clippy::exhaustive_structs)]

mod assembly;
mod features;
mod fit;
mod gate;
mod geom;
mod matching;
mod properties;
mod sha256;
mod socket;
mod tolerance;
mod verdict;

pub use assembly::{
    AssemblyComponents, AssemblyNote, AssemblyNoteCode, AssemblyRead, BadHex, Candidate,
    ComponentError, ComponentRead, ComponentSummary, DefinitionKind, GateResult, ListedDefinition,
    Listing, MAX_OCCURRENCES, MatchedBy, Occurrence, OccurrencePath, PARTS_FORMAT,
    PARTS_FORMAT_VERSION, PartEntry, PartOccurrence, PartsDocument, PartsFormatError, PathForm,
    SelectError, SelectedComponent, Selection, Source, UsageRef, read_assembly,
};
pub use features::{
    Axis, Bottom, Cone, Countersink, Cylinder, CylinderClass, DefinitionRef, End, Extent,
    Extraction, FEATURES_FORMAT, FEATURES_FORMAT_VERSION, FaceCounts, FeatureId, FeatureIdError,
    FeatureKind, FeaturesDocument, FileHeader, FormatError, Frame, Hole, HoleKind, Note, NoteCode,
    OtherFace, OtherItem, OtherReason, Outline, OutlineLoop, OutlineProblem, Part, Plane,
    PlaneFace, ProductRef, Refused, Segment, Sense, Shaft, ShaftKind, SolidInfo, SolidUnits,
    StackEnd, StackSegment, StatedUncertainty, StatedUnit, Status, Torus, UndecidedReason,
    UnknownReason, read_part,
};
pub use fit::{
    CANDIDATE_REFUSED, FitRequest, FitStatus, FitVerifier, INTERFACE_REJECTED, Interference,
    NO_FIT_VERIFIER, NoFitVerifier,
};
pub use gate::{Refusal, RefusalCode};
pub use geom::{Dir3, Point3, Rigid, Vec3};
pub use matching::{
    Admitted, At, Band, BandWidth, Blocked, CandidateFlag, CandidateResult, CandidateSource,
    CandidateView, Chosen, Clearance, EndSide, Evidence, Failure, FailureCode, GUARD_MM, Grip,
    IndexSource, IndexTokens, Kept, MATCH_FORMAT, MATCH_FORMAT_VERSION, MATCH_RULES_VERSION,
    MAX_ANCHOR_PAIRS, MAX_COVERAGE_STEPS, MAX_PLACEMENTS, MAX_SHORTLIST_CHECKS, MatchDocument,
    MatchOptions, MatchPolicy, Matcher, Measure, MeasureValue, Missing, MissingElement, NotChecked,
    NotCurrent, OriginalCheck, Outcome, PairToken, PartSignature, Placement, Placements, Posting,
    Quantity, ReadFacts, SIGNATURE_VERSION, Seed, ShortlistIndex, SigCone, SigPlane, SigStack,
    SocketRefused, SocketResult, SocketSignature, SocketView, Stage, Summary, Unread, Warning,
    WarningCode, Window, align, level_one, match_candidates, shortlist, verify,
};
pub use properties::{
    Field, MAX_STATED, MAX_STATED_BYTES, Mapped, Meaning, NOT_MAPPED, PROPERTIES_VERSION,
    PropertyMap, StatedProperties, StatedProperty,
};
pub use sha256::{Sha256, sha256, sha256_hex};
pub use socket::{
    Basis, BoreSpan, CarryOver, Cause, CauseFaces, Depth, Envelope, Extracted, Fit, Flag, LeftOut,
    LeftOutReason, Open, PlacedRef, PriorReview, Requirement, Requires, ReviewEntry, ReviewError,
    ReviewStatus, Ring, SOCKET_FORMAT, SOCKET_FORMAT_VERSION, SOCKET_RULES_VERSION, SeatContact,
    SelectionRecord, ShaftSpan, Socket, SocketConstants, SocketDocument, SocketExtraction,
    SocketFormatError, SourceRef, SpanBasis, StaleReason, StaleReview, Target, Toucher,
};
pub use tolerance::{ToleranceError, TolerancePolicy};
pub use verdict::{Check, RejectReason, RequirementId, Verdict, VerdictKind, combine, decide};

/// The version of the rules that turn a STEP file into features. It rises
/// whenever the output for any input could change: a rule, a constant, an
/// order, a field. A features document records it, and the catalog
/// re-reads every file whose document has another (milestone M5).
pub const EXTRACTOR_VERSION: u32 = 6;
