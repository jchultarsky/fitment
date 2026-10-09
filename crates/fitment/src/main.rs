//! The `fitment` command line.
//!
//! Commands arrive with the milestones that build them: `features` (M2),
//! `parts` and `socket` (M3), `match` (M4), `catalog` (M5).

use clap::Parser;

/// Find catalog parts that can replace a part in a STEP assembly, by
/// matching interfaces.
#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {}

fn main() {
    let Cli {} = Cli::parse();
    println!("fitment has no commands yet; see https://github.com/jchultarsky/fitment");
}
