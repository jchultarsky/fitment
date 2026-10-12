//! The command line, run as a user would.

use assert_cmd::Command;
use predicates::prelude::*;
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

fn unit_part(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fitment-core/tests/data")
        .join(name)
}

#[test]
fn no_command_prints_help_and_exits_2() {
    fitment()
        .assert()
        .failure()
        .code(2)
        .stderr(contains("features"));
}

#[test]
fn features_of_a_canonical_part_exit_0() {
    fitment()
        .arg("features")
        .arg(unit_part("p01_through.stp"))
        .assert()
        .success()
        .stdout(contains(r#""status": "accepted""#))
        .stderr("");
}

#[test]
fn a_refused_part_prints_its_reasons_and_exits_3() {
    fitment()
        .arg("features")
        .arg(unit_part("r04_assembly.stp"))
        .assert()
        .code(3)
        .stdout(contains(r#""code": "assembly""#))
        .stderr(contains("refused: assembly: "));
}

#[test]
fn a_missing_file_is_an_error_and_exits_1() {
    fitment()
        .args(["features", "no-such-file.stp"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(contains("error: cannot read no-such-file.stp"));
}

#[test]
fn the_table_names_the_solid() {
    fitment()
        .args(["features", "--format", "table"])
        .arg(unit_part("p01_through.stp"))
        .assert()
        .success()
        .stdout(contains("solid-").and(contains("MANIFOLD_SOLID_BREP")));
}
