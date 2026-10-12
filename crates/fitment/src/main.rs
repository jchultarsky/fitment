//! The `fitment` command line.
//!
//! Commands arrive with the milestones that build them: `features` (M2),
//! `parts` and `socket` (M3), `match` (M4), `catalog` (M5).

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
    }
}
