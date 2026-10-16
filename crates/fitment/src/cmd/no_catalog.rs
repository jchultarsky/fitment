//! `fitment catalog` in a build without the `catalog` feature
//! (`docs/research/m5-catalog.md`, CL8): it takes any arguments and says
//! the build has none.

use std::ffi::OsString;
use std::process::ExitCode;

use super::text::note;

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
    note("error: this build has no catalog (cargo feature `catalog`)");
    ExitCode::from(2)
}
