//! The test parts, run through the built `fitment` as a user would.
//!
//! - The committed unit set (`crates/fitment-core/tests/data`) stays under
//!   300 KB (decision 6) and holds exactly the files its manifest lists.
//!   fitment-core's tests cannot list a directory, so this test does.
//! - The generated corpus (`tools/gen-corpus.py corpus/out`, or the
//!   directory `FITMENT_CORPUS` names) is read case by case and each part
//!   is accepted or refused as its construction says. Without the corpus
//!   the test passes with a note, unless `FITMENT_REQUIRE_CORPUS=1`, which
//!   CI sets: a gate that did not run must not pass green.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;

/// Decision 6: the committed unit set's budget.
const UNIT_SET_BYTES: u64 = 300_000;

/// The gate stages built so far (`docs/research/m2-features.md`, 4.2).
const BUILT_STAGES: u64 = 3;

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
        let codes: Vec<&str> = document["refusals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|refusal| refusal["code"].as_str().unwrap())
            .collect();
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
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
