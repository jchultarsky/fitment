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
fn the_table_lists_each_feature() {
    fitment()
        .args(["features", "--format", "table"])
        .arg(unit_part("p05_cbore.stp"))
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "through Ø6.6/Ø11 length 15   opening@0 → opening@15",
        ))
        .stdout(predicates::str::contains("hole Ø11 full"))
        .stdout(predicates::str::contains("complete true"));
}

#[test]
fn text_from_the_file_cannot_add_lines_to_stderr() {
    // A schema name holding an encoded line break and escape character.
    let step = "ISO-10303-21;HEADER;FILE_DESCRIPTION((''),'2;1');\
        FILE_NAME('t','',(''),(''),'','','');\
        FILE_SCHEMA(('X\\X\\0A\\X\\1B\\X2\\2028202E\\X0\\fitment: ok.stp: accepted'));ENDSEC;\
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
    // Nor a Unicode line separator or a bidi override.
    assert!(!stderr.contains(['\u{2028}', '\u{202e}']), "{stderr:?}");
    assert!(stderr.contains("\\n"), "{stderr}");
}

fn unit_assembly(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fitment-core/tests/assemblies")
        .join(name)
}

#[test]
fn parts_lists_an_assembly_as_a_table_and_exits_0() {
    fitment()
        .arg("parts")
        .arg(unit_assembly("s08x_nested.stp"))
        .assert()
        .success()
        .stdout(
            contains("BRACKET")
                .and(contains("LB_ASSEM"))
                .and(contains("assembly gate: accepted")),
        )
        .stderr("");
}

#[test]
fn parts_prints_a_document_that_reads_back() {
    let output = fitment()
        .args(["parts", "--format", "json"])
        .arg(unit_assembly("s01_bolted.stp"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains(r#""format": "fitment.parts""#), "{text}");
    fitment_core::PartsDocument::from_json(&text).unwrap();
}

#[test]
fn parts_of_a_part_file_lists_it_and_says_it_is_no_assembly() {
    fitment()
        .arg("parts")
        .arg(unit_part("p01_through.stp"))
        .assert()
        .success()
        .stdout(contains("p01_through").and(contains("not_an_assembly")));
}

#[test]
fn parts_of_what_is_not_step_exits_3() {
    let dir = std::env::temp_dir().join(format!("fitment-cli-parts-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("not.stp");
    std::fs::write(&file, "hello").unwrap();
    let output = fitment().arg("parts").arg(&file).output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("refused: unparseable"));
}

#[test]
fn socket_of_a_part_prints_a_document_that_reads_back() {
    let output = fitment()
        .args(["socket", "--part", "BRACKET"])
        .arg(unit_assembly("s01_bolted.stp"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    let document = fitment_core::SocketDocument::from_json(&text).unwrap();
    assert!(document.extracted().is_some());
    assert!(String::from_utf8_lossy(&output.stderr).contains("BRACKET: 1 occurrences"));
}

#[test]
fn socket_of_no_such_part_is_a_usage_error() {
    fitment()
        .args(["socket", "--part", "NOPE"])
        .arg(unit_assembly("s01_bolted.stp"))
        .assert()
        .code(2)
        .stdout("")
        .stderr(contains("part_not_found"));
    fitment()
        .arg("socket")
        .arg(unit_assembly("s01_bolted.stp"))
        .assert()
        .code(2);
}

#[test]
fn socket_of_a_part_file_is_refused() {
    fitment()
        .args(["socket", "--part", "p01_through"])
        .arg(unit_part("p01_through.stp"))
        .assert()
        .code(3)
        .stderr(contains("not_an_assembly"));
}
