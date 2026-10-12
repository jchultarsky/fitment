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

#[test]
fn text_from_the_file_cannot_add_lines_to_stderr() {
    // A schema name holding an encoded line break and escape character.
    let step = "ISO-10303-21;HEADER;FILE_DESCRIPTION((''),'2;1');\
        FILE_NAME('t','',(''),(''),'','','');\
        FILE_SCHEMA(('X\\X\\0A\\X\\1Bfitment: ok.stp: accepted'));ENDSEC;\
        DATA;ENDSEC;END-ISO-10303-21;";
    let dir = std::env::temp_dir().join(format!("fitment-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("forged.stp");
    std::fs::write(&file, step).unwrap();
    let output = fitment().arg("features").arg(&file).output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(!stderr.contains('\u{1b}'), "{stderr:?}");
    assert!(stderr.contains("\\n"), "{stderr}");
}
