//! `fitment match`, run as a user would (`docs/research/m4-match.md`,
//! sections 9.4, 11 and 13): its exit codes, its outputs, and every
//! document it writes recomputed from the JSON alone (EV7, KH12).

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::str::contains;
use serde_json::Value;

use fitment_corpus::checker::{digest, problems};

fn fitment() -> Command {
    Command::cargo_bin("fitment").unwrap()
}

fn core(dir: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fitment-core/tests")
        .join(dir)
        .join(name)
}

/// A fresh directory for one test.
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fitment-match-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// s01's bracket socket, written to `dir`.
fn bracket_socket(dir: &Path) -> PathBuf {
    let out = dir.join("bracket.socket.json");
    fitment()
        .args([
            "socket",
            core("assemblies", "s01_bolted.stp").to_str().unwrap(),
        ])
        .args(["--part", "BRACKET", "-o", out.to_str().unwrap()])
        .assert()
        .success();
    out
}

fn run_json(args: &[&str]) -> (i32, Value) {
    let output = fitment()
        .arg("match")
        .args(args)
        .args(["--format", "json"])
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    (output.status.code().unwrap_or(-1), doc)
}

/// VD14: a passing candidate exits 4 (M4 verifies no body); every one
/// rejected exits 5; --require-fit turns 4 into 5.
#[test]
fn the_exit_code_is_the_best_verdict() {
    let dir = scratch("codes");
    let socket = bracket_socket(&dir);
    let socket = socket.to_str().unwrap();
    let good = core("candidates", "m_bracket.stp");
    let bad = core("candidates", "m_bracket_missing_hole.stp");
    let (good, bad) = (good.to_str().unwrap(), bad.to_str().unwrap());
    fitment()
        .args(["match", socket, good, bad])
        .assert()
        .code(4);
    fitment().args(["match", socket, bad]).assert().code(5);
    fitment()
        .args(["match", socket, good, "--require-fit"])
        .assert()
        .code(5)
        .stderr(contains("no candidate can pass without a fit verifier"));
    // An unreadable candidate beside a passing one: exit 1, the document
    // still written.
    let missing = dir.join("nowhere.stp");
    let out = dir.join("result.json");
    fitment()
        .args(["match", socket, good, missing.to_str().unwrap()])
        .args(["-o", out.to_str().unwrap()])
        .assert()
        .code(1);
    assert!(out.is_file());
}

/// VD14's usage and refusal codes: a bad tolerance (an angle of 90
/// degrees or more among them), OUT the socket file,
/// `--original` naming another file (2); a socket that does not read back,
/// a stale one, an unreviewed one under --require-reviewed (3).
#[test]
fn bad_input_is_refused_before_any_candidate() {
    let dir = scratch("refused");
    let socket = bracket_socket(&dir);
    let s = socket.to_str().unwrap();
    let good = core("candidates", "m_bracket.stp");
    let good = good.to_str().unwrap();
    fitment()
        .args(["match", s, good, "--tolerance-mm", "0"])
        .assert()
        .code(2);
    fitment()
        .args(["match", s, good, "--tolerance-deg", "90"])
        .assert()
        .code(2)
        .stderr(contains("less than 90"));
    fitment().args(["match", s, good, "-o", s]).assert().code(2);
    let other = core("assemblies", "s02_cbore.stp");
    fitment()
        .args(["match", s, good, "--original", other.to_str().unwrap()])
        .assert()
        .code(2);
    fitment()
        .args(["match", s, good, "--require-reviewed"])
        .assert()
        .code(3)
        .stdout("")
        .stderr(contains("unreviewed"));
    let text = std::fs::read_to_string(&socket).unwrap();
    let stale = dir.join("stale.json");
    let older = text.replacen(
        &format!(
            "\"socket_rules_version\": {}",
            fitment_core::SOCKET_RULES_VERSION
        ),
        "\"socket_rules_version\": 5",
        1,
    );
    std::fs::write(&stale, older).unwrap();
    fitment()
        .args(["match", stale.to_str().unwrap(), good])
        .assert()
        .code(3)
        .stderr(contains("extract it again"));
    let edited = dir.join("edited.json");
    std::fs::write(&edited, text.replacen("6.6", "6.7", 1)).unwrap();
    fitment()
        .args(["match", edited.to_str().unwrap(), good])
        .assert()
        .code(3);
}

/// VD15, MF1, MF3: the original passes its own socket, beside the
/// candidates and never one of them; a directory's STEP files are each a
/// candidate, labelled `dir/name`; the same file named twice is one entry
/// with both labels.
#[test]
fn the_original_and_directories_are_judged() {
    let dir = scratch("inputs");
    let socket = bracket_socket(&dir);
    let candidates = dir.join("cands");
    std::fs::create_dir_all(&candidates).unwrap();
    for name in ["m_bracket.stp", "m_bracket_blind.stp"] {
        std::fs::copy(core("candidates", name), candidates.join(name)).unwrap();
    }
    std::fs::write(candidates.join("notes.txt"), "not a candidate").unwrap();
    let twice = candidates.join("m_bracket.stp");
    let source = core("assemblies", "s01_bolted.stp");
    let (code, doc) = run_json(&[
        socket.to_str().unwrap(),
        candidates.to_str().unwrap(),
        twice.to_str().unwrap(),
        "--original",
        source.to_str().unwrap(),
    ]);
    assert_eq!(code, 4);
    assert_eq!(doc["summary"]["candidates"], 2);
    assert_eq!(doc["original"]["verdict"], "interface_match");
    assert_eq!(doc["original"]["source"]["kind"], "component");
    let entries = doc["candidates"].as_array().unwrap();
    assert!(entries.iter().all(|c| c["labels"][0] != "original"));
    let bracket = entries
        .iter()
        .find(|c| c["labels"].as_array().unwrap().len() == 2)
        .unwrap();
    assert!(
        bracket["labels"][0]
            .as_str()
            .unwrap()
            .ends_with("cands/m_bracket.stp")
    );
    assert_problems(&doc);
}

/// VD14, VD15: the original passing never makes a run pass: with every
/// candidate rejected the run exits 5. A directory with no STEP file and
/// nothing else is a usage error; a STEP name that leads nowhere, an
/// error.
#[test]
fn only_candidates_make_a_run_pass() {
    let dir = scratch("only");
    let socket = bracket_socket(&dir);
    let source = core("assemblies", "s01_bolted.stp");
    let missing = core("candidates", "m_bracket_missing_hole.stp");
    let (code, doc) = run_json(&[
        socket.to_str().unwrap(),
        missing.to_str().unwrap(),
        "--original",
        source.to_str().unwrap(),
    ]);
    assert_eq!(code, 5);
    assert_eq!(doc["summary"]["candidates"], 1);
    assert_eq!(doc["original"]["verdict"], "interface_match");
    assert_problems(&doc);
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    fitment()
        .args(["match", socket.to_str().unwrap(), empty.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(contains("no candidate"));
    #[cfg(unix)]
    {
        let links = dir.join("links");
        std::fs::create_dir_all(&links).unwrap();
        let _ = std::fs::remove_file(links.join("gone.stp"));
        std::os::unix::fs::symlink(dir.join("nowhere.stp"), links.join("gone.stp")).unwrap();
        fitment()
            .args(["match", socket.to_str().unwrap(), links.to_str().unwrap()])
            .assert()
            .code(1)
            .stderr(contains("gone.stp"));
    }
}

/// MF5, EV8: argument order changes the labels' order, never the digest.
#[test]
fn the_document_does_not_depend_on_argument_order() {
    let dir = scratch("order");
    let socket = bracket_socket(&dir);
    let names = ["m_bracket.stp", "m_bracket_thick.stp", "m_bracket_lug.stp"];
    let paths: Vec<String> = names
        .iter()
        .map(|n| core("candidates", n).to_str().unwrap().to_owned())
        .collect();
    let mut forward = vec![socket.to_str().unwrap()];
    forward.extend(paths.iter().map(String::as_str));
    let mut backward = vec![socket.to_str().unwrap()];
    backward.extend(paths.iter().rev().map(String::as_str));
    let (_, a) = run_json(&forward);
    let (_, b) = run_json(&backward);
    assert_eq!(a["digest"], b["digest"]);
    assert_eq!(a["summary"], b["summary"]);
    let shas = |d: &Value| -> Vec<Value> {
        d["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["source"]["sha256"].clone())
            .collect()
    };
    assert_eq!(shas(&a), shas(&b));
    assert_problems(&a);
}

/// MF4: a label is escaped wherever it is printed.
#[test]
fn labels_are_escaped() {
    let dir = scratch("labels");
    let socket = bracket_socket(&dir);
    // U+202E reorders text in a terminal, and every OS allows it in a name.
    let odd = dir.join("odd\u{202e}name.stp");
    std::fs::copy(core("candidates", "m_bracket.stp"), &odd).unwrap();
    let output = fitment()
        .args(["match", socket.to_str().unwrap(), odd.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains('\u{202e}'), "{stdout}");
    assert!(stdout.contains("\\u{202e}"));
}

/// The review-then-match path: a requirement removed in review is listed
/// as not checked, and the candidate missing its hole then passes.
#[test]
fn a_removed_requirement_is_not_checked() {
    let dir = scratch("review");
    let socket = bracket_socket(&dir);
    let s = socket.to_str().unwrap();
    let bad = core("candidates", "m_bracket_missing_hole.stp");
    let (_, before) = run_json(&[s, bad.to_str().unwrap()]);
    let failed = before["candidates"][0]["sockets"][0]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["result"] == "failed")
        .unwrap()["requirement"]
        .as_str()
        .unwrap()
        .to_owned();
    fitment()
        .args([
            "review",
            s,
            "--remove",
            &failed,
            "--note",
            "the plate's own",
        ])
        .assert()
        .success();
    let (code, after) = run_json(&[s, bad.to_str().unwrap()]);
    let entry = &after["candidates"][0];
    // The removed hole is not checked; the other bores and seats hold it.
    let removed = entry["sockets"][0]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["requirement"] == failed.as_str())
        .unwrap();
    assert_eq!(removed["result"], "not_checked");
    assert_eq!(removed["note"], "the plate's own");
    assert!(
        entry["flags"]
            .as_array()
            .unwrap()
            .contains(&Value::from("requirements_removed"))
    );
    assert!(code == 4 || code == 5);
    assert_problems(&after);
}

/// The checker (KH12, EV7) finds nothing in `doc`.
fn assert_problems(doc: &Value) {
    let problems = problems(doc);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The checker is no formality: each tampered copy carries a fresh digest,
/// so only the check named can object: a candidate's verdict turned, a
/// measure's result flipped, a margin moved, the summary changed, a match
/// with no verified fit, a flag dropped, a check dropped, a failure
/// count changed, an entry twice, the order reversed. A label
/// changed is no problem: labels are outside the digest.
#[test]
fn the_checker_finds_a_tampered_document() {
    let dir = scratch("tamper");
    let socket = bracket_socket(&dir);
    let names = ["m_bracket.stp", "m_bracket_missing_hole.stp"];
    let paths: Vec<String> = names
        .iter()
        .map(|n| core("candidates", n).to_str().unwrap().to_owned())
        .collect();
    let (_, doc) = run_json(&[socket.to_str().unwrap(), &paths[0], &paths[1]]);
    assert_eq!(problems(&doc).len(), 0);
    let tampered = |edit: &dyn Fn(&mut Value)| -> Vec<String> {
        let mut copy = doc.clone();
        edit(&mut copy);
        copy["digest"] = Value::from(digest(&copy));
        problems(&copy)
    };
    let said = |found: &[String], what: &str| found.iter().any(|p| p.contains(what));
    let turned = tampered(&|d| d["candidates"][1]["verdict"] = Value::from("interface_match"));
    assert!(said(&turned, "its sockets say"), "{turned:?}");
    let flipped = tampered(&|d| {
        d["candidates"][0]["sockets"][0]["checks"][3]["measures"][0]["passed"] = Value::from(false);
    });
    assert!(said(&flipped, "recomputes"), "{flipped:?}");
    let moved = tampered(&|d| {
        d["candidates"][0]["sockets"][0]["checks"][3]["measures"][0]["margin"] = Value::from(1.0);
    });
    assert!(said(&moved, "is not"), "{moved:?}");
    let counted = tampered(&|d| d["summary"]["reject"] = Value::from(0));
    assert!(said(&counted, "the summary"), "{counted:?}");
    let matched = tampered(&|d| {
        d["candidates"][0]["verdict"] = Value::from("match");
        d["candidates"][0]["sockets"][0]["verdict"] = Value::from("match");
    });
    assert!(
        said(&matched, "says match, recomputes interface_match"),
        "{matched:?}"
    );
    assert!(
        said(&matched, "its sockets say interface_match"),
        "{matched:?}"
    );
    let unflagged = tampered(&|d| d["candidates"][0]["flags"] = Value::from(Vec::<String>::new()));
    assert!(
        said(&unflagged, "flag requirements_unreviewed"),
        "{unflagged:?}"
    );
    let dropped = tampered(&|d| {
        d["candidates"][0]["sockets"][0]["checks"]
            .as_array_mut()
            .unwrap()
            .pop();
    });
    assert!(said(&dropped, "active"), "{dropped:?}");
    let miscounted = tampered(&|d| d["candidates"][1]["sockets"][0]["failed"] = Value::from(0));
    assert!(said(&miscounted, "counts"), "{miscounted:?}");
    let twice = tampered(&|d| {
        let first = d["candidates"][1].clone();
        d["candidates"].as_array_mut().unwrap().push(first);
    });
    assert!(said(&twice, "a second entry"), "{twice:?}");
    let reordered = tampered(&|d| d["candidates"].as_array_mut().unwrap().reverse());
    assert!(said(&reordered, "order"), "{reordered:?}");
    let relabelled =
        tampered(&|d| d["candidates"][0]["labels"] = Value::from(vec!["elsewhere.stp"]));
    assert_eq!(relabelled.len(), 0, "{relabelled:?}");
    let mut stale = doc.clone();
    stale["candidates"][0]["labels"] = Value::from(vec!["elsewhere.stp"]);
    assert_eq!(problems(&stale).len(), 0, "a label is outside the digest");
}
