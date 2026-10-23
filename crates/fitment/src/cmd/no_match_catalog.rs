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

pub(super) fn candidates(
    _: &Path,
    _: &Matcher<'_>,
    _: &[String],
    _: bool,
    _: usize,
) -> Result<Candidates, ExitCode> {
    note("error: this build has no catalog (cargo feature `catalog`)");
    Err(ExitCode::from(2))
}
