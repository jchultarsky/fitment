//! The command line, run as a user would.

use assert_cmd::Command;
use predicates::str::contains;

fn fitment() -> Command {
    Command::cargo_bin("fitment").unwrap()
}

#[test]
fn version_matches_the_package() {
    fitment()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("fitment {}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn help_describes_the_tool() {
    fitment()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("matching interfaces"));
}

#[test]
fn unknown_arguments_are_refused() {
    fitment().arg("--frobnicate").assert().failure().code(2);
}
