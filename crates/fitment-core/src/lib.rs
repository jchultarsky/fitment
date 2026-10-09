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

mod fit;
mod tolerance;
mod verdict;

pub use fit::{FitStatus, Interference, NO_FIT_VERIFIER};
pub use tolerance::{ToleranceError, TolerancePolicy};
pub use verdict::{Check, RejectReason, RequirementId, Verdict, VerdictKind, decide};
