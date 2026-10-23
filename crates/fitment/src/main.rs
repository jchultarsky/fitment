//! The `fitment` command line.
//!
//! Commands arrive with the milestones that build them: `features` (M2),
//! `parts`, `socket` and `review` (M3), `match` (M4), `catalog` (M5, behind
//! the default `catalog` feature), `fit` and `match --fit` (M7, behind the
//! default `fit` feature).

mod cmd;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Find catalog parts that can replace a part in a STEP assembly, by
/// matching interfaces.
#[derive(Parser)]
#[command(version, about, long_about = None, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read one part and print its features, or why it was refused.
    ///
    /// Exit codes: 0 accepted, 1 error, 2 usage, 3 refused.
    Features(cmd::features::Args),
    /// List the parts of an assembly with their quantities and
    /// occurrences, and whether a socket can be read from it.
    ///
    /// Exit codes: 0 listed, 1 error, 2 usage, 3 refused.
    Parts(cmd::parts::Args),
    /// Read what the neighbours of a part require of any part that
    /// replaces it: the socket, as a reviewable JSON file.
    ///
    /// Exit codes: 0 extracted, 1 error, 2 usage (and a --part or
    /// --occurrence that names no part), 3 refused.
    Socket(cmd::socket::Args),
    /// Check a socket document and show its review, or record one
    /// decision in it: keep, remove (with a note) or unreview a
    /// requirement.
    ///
    /// Exit codes: 0 valid (and updated), 1 error, 2 usage (an unknown id,
    /// a removal without a note), 3 the document does not read back.
    Review(cmd::review::Args),
    /// Judge each candidate part against a socket document: match,
    /// interface match or reject, with the evidence.
    ///
    /// Exit codes: 0 some candidate matches (its body fit verified, with
    /// --fit), 4 some passes the interface (its body fit not verified),
    /// 5 every candidate rejected, 1 error (also: the fit helper cannot be
    /// used, or a fit timed out or ran out of a resource), 2 usage, 3 the
    /// socket document refused (also: more interface passes than
    /// --max-fits).
    // A build without the fit verifier says what 0.3.0 said.
    #[cfg_attr(
        not(feature = "fit"),
        command(
            long_about = "Judge each candidate part against a socket document: match, \
            interface match or reject, with the evidence.\n\nExit codes: 0 some candidate \
            matches, 4 some passes the interface (its body fit not checked), 5 every candidate \
            rejected, 1 error, 2 usage, 3 the socket document refused."
        )
    )]
    Match(cmd::match_::Args),
    /// Store STEP parts by content, indexed for matching: init, ingest,
    /// list, get, replace, delete, prune, gc and verify, in the catalog
    /// named by -C DIR.
    ///
    /// Exit codes: 0 done, 1 error, 2 usage (and not a catalog), 3 refused
    /// input.
    Catalog(cmd::catalog::Args),
    /// Prepare the fit verifier's helper, the Open CASCADE process that
    /// `fitment match --fit` measures bodies with: `fitment fit setup`.
    ///
    /// Exit codes: 0 ready, 1 error, 2 usage (and a build without the
    /// fit verifier).
    #[command(hide = cfg!(not(feature = "fit")))]
    Fit(cmd::fit::Args),
}

/// Exit codes shared by every command.
mod exit {
    /// The input was read and is not a canonical part (or assembly).
    pub const REFUSED: u8 = 3;
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Features(args) => cmd::features::run(&args),
        Command::Parts(args) => cmd::parts::run(&args),
        Command::Socket(args) => cmd::socket::run(&args),
        Command::Review(args) => cmd::review::run(&args),
        Command::Match(args) => cmd::match_::run(&args),
        Command::Catalog(args) => cmd::catalog::run(&args),
        Command::Fit(args) => cmd::fit::run(&args),
    }
}
