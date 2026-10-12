//! The test parts, run through the built `fitment` as a user would.
//!
//! - The committed unit set (`crates/fitment-core/tests/data`) stays under
//!   300 KB (decision 6) and holds exactly the files its manifest lists.
//!   fitment-core's tests cannot list a directory, so this test does.
//! - The generated corpus (`tools/gen-corpus.py corpus/out`, or the
//!   directory `FITMENT_CORPUS` names) is read case by case and each part
//!   is accepted or refused as its construction says. An accepted part's
//!   features are checked against the construction too, as fitment-core's
//!   `tests/support/expect.rs` does. Without the corpus
//!   the test passes with a note, unless `FITMENT_REQUIRE_CORPUS=1`, which
//!   CI sets: a gate that did not run must not pass green.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;

mod common;
use serde_json::Value;

/// Decision 6: the committed unit set's budget.
const UNIT_SET_BYTES: u64 = 300_000;

/// The gate stages built so far (`docs/research/m2-features.md`, 4.2).
const BUILT_STAGES: u64 = 6;

fn unit_set() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests/data")
}

fn manifest(dir: &Path) -> Value {
    let text = fs::read_to_string(dir.join("manifest.json")).unwrap();
    let manifest: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(manifest["format"], "fitment.test-parts");
    manifest
}

#[test]
fn the_unit_set_fits_its_budget_and_its_manifest() {
    let dir = unit_set();
    let mut total = 0;
    let mut files = BTreeSet::new();
    for entry in fs::read_dir(&dir).unwrap() {
        let entry = entry.unwrap();
        total += entry.metadata().unwrap().len();
        files.insert(entry.file_name().to_string_lossy().into_owned());
    }
    assert!(
        total <= UNIT_SET_BYTES,
        "the unit set is {total} bytes, over the {UNIT_SET_BYTES} of decision 6"
    );
    let mut listed: BTreeSet<String> = manifest(&dir)["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case["file"].as_str().unwrap().to_owned())
        .collect();
    listed.insert("manifest.json".to_owned());
    assert_eq!(
        files,
        listed,
        "the files in {} and its manifest",
        dir.display()
    );
}

/// The corpus directory, or `None` (with a note) if it is not there and
/// not required.
fn corpus() -> Option<PathBuf> {
    let dir = std::env::var_os("FITMENT_CORPUS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/out"),
        PathBuf::from,
    );
    let required = std::env::var_os("FITMENT_REQUIRE_CORPUS").is_some_and(|v| v == "1");
    if dir.join("manifest.json").is_file() {
        return Some(dir);
    }
    assert!(
        !required,
        "FITMENT_REQUIRE_CORPUS=1 but there is no corpus in {}",
        dir.display()
    );
    eprintln!(
        "skipped: no corpus in {}; run tools/gen-corpus.py corpus/out",
        dir.display()
    );
    None
}

#[test]
fn every_corpus_part_is_accepted_or_refused_as_its_construction_says() {
    let Some(dir) = corpus() else {
        return;
    };
    let cases = manifest(&dir)["cases"].as_array().unwrap().clone();
    assert!(!cases.is_empty(), "the corpus manifest lists no case");
    let mut failures = Vec::new();
    for case in &cases {
        let name = case["id"].as_str().unwrap();
        let output = Command::cargo_bin("fitment")
            .unwrap()
            .arg("features")
            .arg(dir.join(case["file"].as_str().unwrap()))
            .output()
            .unwrap();
        let document: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{name}: not JSON: {e}"));
        // The distinct codes, in order: a code repeats once per instance
        // at fault, and the manifest names each code once.
        let mut codes: Vec<&str> = document["refusals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|refusal| refusal["code"].as_str().unwrap())
            .collect();
        codes.dedup();
        let expect = &case["expect"];
        let refused =
            expect["result"] == "refused" && expect["stage"].as_u64().unwrap() <= BUILT_STAGES;
        let (status, wanted, code): (&str, Vec<&str>, i32) = if refused {
            let wanted = expect["refusals"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c.as_str().unwrap())
                .collect();
            ("refused", wanted, 3)
        } else {
            ("accepted", Vec::new(), 0)
        };
        if document["status"] != status || codes != wanted || output.status.code() != Some(code) {
            failures.push(format!(
                "{name}: {} {codes:?} (exit {:?}), expected {status} {wanted:?}",
                document["status"],
                output.status.code()
            ));
        } else if let Some(problem) = common::reads_back(&output.stdout) {
            failures.push(format!("{name}: {problem}"));
        } else if status == "accepted" {
            failures.extend(
                common::expect::features(&document["part"], &expect["features"])
                    .into_iter()
                    .map(|f| format!("{name}: {f}")),
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Coverage is checked, not claimed (design, section 13.1): every rule
/// and gate check the corpus manifest lists has a case that cites it, a
/// Rust test that exists, or a reason it is deferred; and every rule a case
/// cites is listed.
#[test]
fn every_rule_has_a_case_a_test_or_a_reason() {
    let Some(dir) = corpus() else {
        return;
    };
    let manifest = manifest(&dir);
    let rules = manifest["rule_ids"]
        .as_object()
        .expect("the corpus manifest lists its rule_ids");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut failures = Vec::new();
    for (id, entry) in rules {
        let cases = entry["cases"].as_array().map_or(0, Vec::len);
        let tests: Vec<&str> = entry["tests"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let deferred = entry["deferred"].as_str().is_some_and(|r| !r.is_empty());
        if cases == 0 && tests.is_empty() && !deferred {
            failures.push(format!("{id}: no case, no test and no reason"));
        }
        if deferred && (cases > 0 || !tests.is_empty()) {
            failures.push(format!("{id}: deferred, yet a case or test covers it"));
        }
        for test in tests {
            let Some((file, name)) = test.split_once("::") else {
                failures.push(format!("{id}: '{test}' is not file::function"));
                continue;
            };
            if !is_test(&root.join(file), name) {
                failures.push(format!("{id}: no test {name} in {file}"));
            }
        }
    }
    // Every rule the design states in bold (`**G1.`, `**W7b.`) and every
    // gate check is listed, itself or by its parts (`K2.blind`).
    let design = fs::read_to_string(root.join("docs/research/m2-features.md")).unwrap();
    let listed = |rule: &str| {
        rules.contains_key(rule) || rules.keys().any(|k| k.starts_with(&format!("{rule}.")))
    };
    let checks = (1..=30).map(|n| format!("4.2#{n}"));
    for rule in design_rules(&design).into_iter().chain(checks) {
        if !listed(&rule) {
            failures.push(format!(
                "{rule}: the design states it, but rule_ids does not list it"
            ));
        }
    }
    for case in manifest["cases"].as_array().unwrap() {
        for rule in case["rules"].as_array().unwrap() {
            let rule = rule.as_str().unwrap();
            if !rules.contains_key(rule) {
                failures.push(format!(
                    "{}: cites {rule}, which rule_ids does not list",
                    case["id"]
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// True if `file` has a `#[test]` function `name`.
fn is_test(file: &Path, name: &str) -> bool {
    let text = fs::read_to_string(file).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines.iter().enumerate().any(|(i, line)| {
        line.trim_start().starts_with(&format!("fn {name}("))
            && lines[i.saturating_sub(2)..i]
                .iter()
                .any(|l| l.trim() == "#[test]")
    })
}

/// The rule ids the design states in bold at the start of a rule, such as
/// `**G1.` or `**W7b.`.
fn design_rules(design: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (at, _) in design.match_indices("**") {
        let rest = &design[at + 2..];
        let id: String = rest
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        let mut chars = id.chars();
        let shaped = chars.next().is_some_and(|c| c.is_ascii_uppercase())
            && chars.clone().next().is_some_and(|c| c.is_ascii_digit())
            && id.len() <= 4;
        if shaped && rest[id.len()..].starts_with('.') && !found.contains(&id) {
            found.push(id);
        }
    }
    found
}
