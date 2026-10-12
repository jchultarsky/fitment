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
        .code(2)
        .stdout("");
}

#[test]
fn a_selection_error_names_the_candidates() {
    fitment()
        .args(["socket", "--part", "bolt"])
        .arg(unit_assembly("s01_bolted.stp"))
        .assert()
        .code(2)
        .stdout("")
        .stderr(contains("#752 'BOLT' (BOLT), 4 occurrences, first at 3"));
}

#[test]
fn one_occurrence_is_chosen_by_its_path() {
    fitment()
        .args(["socket", "--occurrence", "3", "--format", "table"])
        .arg(unit_assembly("s01_bolted.stp"))
        .assert()
        .success()
        .stdout(contains("shaft:shaft-985"))
        .stderr(contains("BOLT: 1 occurrences"));
    fitment()
        .args(["socket", "--part", "BRACKET", "--occurrence", "3"])
        .arg(unit_assembly("s01_bolted.stp"))
        .assert()
        .code(2)
        .stdout("");
}

#[test]
fn the_table_shows_seats_and_grips() {
    fitment()
        .args(["socket", "--part", "BRACKET", "--format", "table"])
        .arg(unit_assembly("s01_bolted.stp"))
        .assert()
        .success()
        .stdout(contains("8 mm thick, 5 holes through"))
        .stdout(contains("1 faces, 4 contacts"))
        .stdout(contains("left out hole-"));
    // A counterbore's floor carries the head's ring.
    fitment()
        .args(["socket", "--part", "BLOCK", "--format", "table"])
        .arg(unit_assembly("s02_cbore.stp"))
        .assert()
        .success()
        .stdout(contains("ring Ø6.6..Ø10"));
}

#[test]
fn socket_of_a_missing_file_exits_1() {
    fitment()
        .args(["socket", "--part", "BRACKET", "no-such-file.stp"])
        .assert()
        .code(1)
        .stdout("");
}

/// A fresh directory for one test, with s01 copied into it.
fn scratch(test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fitment-cli-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(unit_assembly("s01_bolted.stp"), dir.join("s01.stp")).unwrap();
    dir
}

#[test]
fn the_output_never_replaces_the_assembly() {
    let dir = scratch("input");
    let input = dir.join("s01.stp");
    let before = std::fs::read(&input).unwrap();
    fitment()
        .args(["socket", "--part", "BRACKET", "-o"])
        .arg(&input)
        .arg(&input)
        .assert()
        .code(2)
        .stderr(contains("it is the assembly being read"));
    assert_eq!(std::fs::read(&input).unwrap(), before);
}

#[test]
fn the_output_replaces_only_this_part_s_socket_unless_forced() {
    let dir = scratch("output");
    let input = dir.join("s01.stp");
    let out = dir.join("bracket.socket.json");
    std::fs::write(&out, "notes\n").unwrap();
    let run = |part: &str, force: bool| {
        let mut command = fitment();
        command
            .args(["socket", "--part", part])
            .arg(&input)
            .arg("-o")
            .arg(&out);
        if force {
            command.arg("--force");
        }
        command.assert()
    };
    run("BRACKET", false)
        .code(2)
        .stderr(contains("not a fitment.socket document"));
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "notes\n");
    run("BRACKET", true).success().stdout("");
    let written = std::fs::read_to_string(&out).unwrap();
    assert!(fitment_core::SocketDocument::from_json(&written).is_ok());
    // Its own socket, extracted again, may be replaced; another part's
    // may not.
    run("BRACKET", false).success();
    run("PLATE", false)
        .code(2)
        .stderr(contains("the socket of another part"));
    assert_eq!(std::fs::read_to_string(&out).unwrap(), written);
    // No temporary file is left behind.
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    let _ = std::fs::remove_dir_all(&dir);
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

#[test]
fn review_records_a_decision_and_socket_carries_it_over() {
    let dir = scratch("review");
    let input = dir.join("s01.stp");
    let out = dir.join("bracket.socket.json");
    fitment()
        .args(["socket", "--part", "BRACKET"])
        .arg(&input)
        .arg("-o")
        .arg(&out)
        .assert()
        .success();
    let grip = "grip:plane-467+plane-570";
    // A removal needs a note; an unknown id is a usage error.
    fitment()
        .args(["review", "--remove", grip])
        .arg(&out)
        .assert()
        .code(2)
        .stderr(contains("needs a note"));
    fitment()
        .args(["review", "--keep", "bore:hole-1"])
        .arg(&out)
        .assert()
        .code(2);
    fitment()
        .args(["review", "--remove", grip, "--note", "may be thicker"])
        .arg(&out)
        .assert()
        .success();
    fitment()
        .arg("review")
        .arg(&out)
        .assert()
        .success()
        .stdout(contains("removed  grip:plane-467+plane-570"));
    // Extracted again onto the same file, the decision carries over.
    fitment()
        .args(["socket", "--part", "BRACKET"])
        .arg(&input)
        .arg("-o")
        .arg(&out)
        .assert()
        .success()
        .stderr(contains("1 decisions carried over, 0 stale"));
    // A document edited by hand is refused.
    let text = std::fs::read_to_string(&out).unwrap().replace("\"contact_mm\": 0.01", "\"contact_mm\": 0.02");
    std::fs::write(&out, text).unwrap();
    fitment()
        .arg("review")
        .arg(&out)
        .assert()
        .code(3)
        .stderr(contains("digest"));
    let _ = std::fs::remove_dir_all(&dir);
}
