//! `fitment fit` and `fitment match --fit` in a build without the `fit`
//! feature (`docs/research/m7-fit.md`, FG3): exit 2, saying the build has
//! no fit verifier, as a build without the catalog says it has none.

use std::ffi::OsString;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::Path;
use std::process::ExitCode;

use fitment_core::{FitVerifier, NoFitVerifier, SocketDocument};

use super::text::note;

/// What a build without the feature says.
const NONE: &str = "error: this build has no fit verifier (cargo feature `fit`)";

#[derive(clap::Args)]
// `--help` too is an argument it does not have: it exits 2, never 0.
#[command(disable_help_flag = true)]
pub struct Args {
    /// Whatever was given.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    rest: Vec<OsString>,
}

pub fn run(args: &Args) -> ExitCode {
    let _ = &args.rest;
    note(NONE);
    ExitCode::from(2)
}

/// What `--fit` and its flags ask; only `--fit` is read here.
pub(super) struct Flags<'a> {
    pub assembly: Option<&'a Path>,
    pub jobs: Option<NonZeroUsize>,
    pub timeout: Option<NonZeroU64>,
    pub dir: Option<&'a Path>,
}

/// No helper can be had in this build.
pub(super) enum Helper {}

/// No session either.
pub(super) enum Session {}

/// `--fit` is a usage error here (FG3, exit 2); without it, nothing.
pub(super) fn helper(
    flags: &Flags<'_>,
    _: &SocketDocument,
    _: usize,
) -> Result<Option<Helper>, ExitCode> {
    let _ = (flags.jobs, flags.timeout, flags.dir);
    if flags.assembly.is_some() {
        note(NONE);
        return Err(ExitCode::from(2));
    }
    Ok(None)
}

// The fit build's signature, whose session can fail to open.
#[allow(clippy::unnecessary_wraps)]
pub(super) fn session(
    helper: Option<&Helper>,
    _: &SocketDocument,
) -> Result<Option<Session>, ExitCode> {
    match helper {
        Some(never) => match *never {},
        None => Ok(None),
    }
}

pub(super) fn verifier(session: Option<&Session>) -> &dyn FitVerifier {
    match session {
        Some(never) => match *never {},
        None => &NoFitVerifier,
    }
}

pub(super) fn report(session: Option<&Session>) {
    if let Some(never) = session {
        match *never {}
    }
}
