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

mod features;
mod fit;
mod gate;
mod geom;
mod tolerance;
mod verdict;

pub use features::{
    Axis, Cone, Cylinder, CylinderClass, DefinitionRef, End, Extraction, FEATURES_FORMAT,
    FEATURES_FORMAT_VERSION, FaceCounts, FeatureId, FeatureIdError, FeatureKind, FeaturesDocument,
    FileHeader, FormatError, Frame, Note, NoteCode, OtherFace, OtherItem, OtherReason, Outline,
    OutlineLoop, OutlineProblem, Part, Plane, PlaneFace, ProductRef, Refused, Segment, Sense,
    SolidInfo, SolidUnits, StatedUncertainty, StatedUnit, Status, Torus, UndecidedReason,
    read_part,
};
pub use fit::{FitStatus, Interference, NO_FIT_VERIFIER};
pub use gate::{Refusal, RefusalCode};
pub use geom::{Dir3, Point3, Rigid, Vec3};
pub use tolerance::{ToleranceError, TolerancePolicy};
pub use verdict::{Check, RejectReason, RequirementId, Verdict, VerdictKind, decide};

/// The version of the rules that turn a STEP file into features. It rises
/// whenever the output for any input could change: a rule, a constant, an
/// order, a field. A features document records it, and the catalog
/// re-reads every file whose document has another (milestone M5).
pub const EXTRACTOR_VERSION: u32 = 4;
