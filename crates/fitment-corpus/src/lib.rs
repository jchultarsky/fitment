//! The corpus's scorer (`docs/research/m6-api.md`, section 9.2), shared by
//! the tests of `fitment` and, from M6's PR 6, `fitment-server`: one strict
//! gate, one definition of a false positive and a false negative, one
//! baseline, one report and one document checker (SC1), judging an
//! [`Outcome`] built from a library result or a document's entry (SC2).
//! Never published (SC3): a versionless dev-dependency only.
//!
//! `gate.rs` and `outcome.rs` name nothing but `fitment_core`,
//! `serde_json` and each other, so fitment-core's tests, which may have no
//! dev-dependency, include them by path (SC6).

pub mod baseline;
pub mod checker;
pub mod corpus;
pub mod fit;
pub mod gate;
pub mod outcome;
pub mod parity;
pub mod policy;
pub mod report;
pub mod schema;
pub mod score;

pub use corpus::{CandidateRef, Case, Corpus, SocketKey, Truth};
pub use outcome::{Outcome, SocketOutcome};
pub use policy::Policy;
pub use score::{Run, RunPath, Score};
