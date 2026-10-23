//! `fitment match --catalog` in a build without the `catalog` feature
//! (`docs/research/m5-catalog.md`, CL8): exit 2, saying why.

use std::path::Path;
use std::process::ExitCode;

use fitment_core::{CandidateResult, Matcher};

use super::text::note;

pub(super) struct Candidates {
    pub results: Vec<(CandidateResult, Vec<String>)>,
    pub searched: String,
    pub failed: bool,
    pub fit_reads: Option<usize>,
}

/// No catalog can be opened in this build.
pub(super) enum Opened {}

pub(super) fn open(_: &Path, _: &[String], _: bool) -> Result<Opened, ExitCode> {
    note("error: this build has no catalog (cargo feature `catalog`)");
    Err(ExitCode::from(2))
}

pub(super) fn refused(opened: &mut Opened, _: &Matcher<'_>) -> Result<(), ExitCode> {
    match *opened {}
}

// The catalog build's signature, which consumes the catalog.
#[allow(clippy::needless_pass_by_value)]
pub(super) fn candidates(
    opened: Opened,
    _: &Matcher<'_>,
    _: usize,
) -> Result<Candidates, ExitCode> {
    match opened {}
}
