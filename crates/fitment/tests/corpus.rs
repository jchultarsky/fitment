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

/// The committed assemblies' budget (`docs/research/m3-socket.md`, 13.5):
/// the set as built in M3 PR 1, 187,691 bytes, with 10% headroom.
const ASSEMBLY_SET_BYTES: u64 = 210_000;

/// The gate stages built so far (`docs/research/m2-features.md`, 4.2).
const BUILT_STAGES: u64 = 6;

fn unit_set() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests/data")
}

fn assembly_set() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests/assemblies")
}

fn manifest(dir: &Path) -> Value {
    let text = fs::read_to_string(dir.join("manifest.json")).unwrap();
    let manifest: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(manifest["format"], "fitment.test-parts");
    manifest
}

#[test]
fn the_unit_set_fits_its_budget_and_its_manifest() {
    fits(&unit_set(), UNIT_SET_BYTES);
}

#[test]
fn the_assembly_set_fits_its_budget_and_its_manifest() {
    fits(&assembly_set(), ASSEMBLY_SET_BYTES);
}

/// `dir` holds exactly the files its manifest lists, within `budget` bytes.
fn fits(dir: &Path, budget: u64) {
    let dir = dir.to_path_buf();
    let mut total = 0;
    let mut files = BTreeSet::new();
    for entry in fs::read_dir(&dir).unwrap() {
        let entry = entry.unwrap();
        total += entry.metadata().unwrap().len();
        files.insert(entry.file_name().to_string_lossy().into_owned());
    }
    assert!(
        total <= budget,
        "{} is {total} bytes, over its budget of {budget}",
        dir.display()
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
    for case in cases.iter().filter(|case| case["kind"] != "assembly") {
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

#[test]
fn every_corpus_assembly_reads_as_its_construction_says() {
    let Some(dir) = corpus() else {
        return;
    };
    let cases = manifest(&dir)["cases"].as_array().unwrap().clone();
    let assemblies: Vec<&Value> = cases.iter().filter(|c| c["kind"] == "assembly").collect();
    assert!(
        !assemblies.is_empty(),
        "the corpus manifest lists no assembly"
    );
    let mut failures = Vec::new();
    for case in assemblies {
        let name = case["id"].as_str().unwrap();
        let bytes = fs::read(dir.join(case["file"].as_str().unwrap())).unwrap();
        failures.extend(
            common::assembly::check(case, &bytes)
                .into_iter()
                .map(|f| format!("{name}: {f}")),
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// P2, P4 and P5: each `_rot` variant's sockets are its base's, each
/// occurrence placed by VARIANT ∘ the base's; each `_tgt` variant's are its
/// base's moved by TILT, the frame its targets are modelled in.
#[test]
fn every_variant_has_its_base_s_sockets() {
    use common::variants::{canonical, differ, moved, sorted, stripped};
    let Some(dir) = corpus() else {
        return;
    };
    let cases = manifest(&dir)["cases"].as_array().unwrap().clone();
    let read = |case: &Value| fs::read(dir.join(case["file"].as_str().unwrap())).unwrap();
    let mut failures = Vec::new();
    let mut compared = 0;
    for case in cases.iter().filter(|c| c["expect"]["variant"].is_object()) {
        let variant = &case["expect"]["variant"];
        let base = cases.iter().find(|c| c["id"] == variant["of"]).unwrap();
        let m: [[f64; 4]; 3] =
            [0, 1, 2].map(|i| [0, 1, 2, 3].map(|j| variant["motion"][i][j].as_f64().unwrap()));
        let (ours, theirs) = (read(base), read(case));
        for target in base["expect"]["sockets"].as_object().unwrap().keys() {
            let doc = |bytes: &[u8]| {
                let document =
                    fitment_core::SocketDocument::extract(bytes, Some(target), None).unwrap();
                // Each document reads back.
                fitment_core::SocketDocument::from_json(&document.to_json()).unwrap();
                document.extracted().unwrap().clone()
            };
            let (base_doc, variant_doc) = (doc(&ours), doc(&theirs));
            // What the part's box bounds (U3's unknown ends, U1's partial
            // cylinders, I7) depends on the frame the part is modelled in:
            // in a `_tgt` variant those requirements' stations and openings
            // are left out; the rest is compared.
            let framed = |r: &Value| {
                let flags = r["flags"].as_array().cloned().unwrap_or_default();
                let kind = r["requires"]["kind"].as_str().unwrap_or("").to_owned();
                variant["kind"] == "tgt"
                    && (kind.starts_with("partial")
                        || flags.iter().any(|f| {
                            f == "extent_unknown"
                                || f == "target_undecided"
                                || f == "partial_neighbour"
                        }))
            };
            // Occurrence by occurrence, in path order: each with its socket.
            let shape = |extracted: &fitment_core::Extracted, tilt: bool| {
                let mut out = serde_json::Map::new();
                for (k, occurrence) in extracted.occurrences.iter().enumerate() {
                    let socket = extracted
                        .sockets
                        .iter()
                        .find(|s| occurrence.socket.as_deref() == Some(s.id.as_str()))
                        .unwrap();
                    let requirements: Vec<Value> = socket
                        .requirements
                        .iter()
                        .map(|r| {
                            let mut value = serde_json::to_value(r).unwrap();
                            if framed(&value) {
                                common::variants::unbounded(&mut value);
                            }
                            value
                        })
                        .collect();
                    out.insert(
                        format!("{k:06}"),
                        serde_json::json!({
                            "requirements": requirements,
                            "left_out": socket.left_out,
                            "occurrences": socket.occurrences.len(),
                        }),
                    );
                }
                let mut value = Value::Object(out);
                if tilt {
                    moved(&mut value, &m);
                }
                canonical(&mut value);
                stripped(&mut value);
                sorted(&mut value);
                value
            };
            let tilt = variant["kind"] == "tgt";
            let where_ = format!("{} {target}", case["id"].as_str().unwrap());
            if let Some(d) = differ(
                &shape(&base_doc, tilt),
                &shape(&variant_doc, false),
                1e-9,
                &where_,
            ) {
                failures.push(d);
            }
            if base_doc.occurrences.len() != variant_doc.occurrences.len() {
                failures.push(format!("{where_}: another number of occurrences"));
            }
            if variant["kind"] == "rot" {
                failures.extend(common::variants::placed_by(
                    &m,
                    &base_doc,
                    &variant_doc,
                    &where_,
                ));
            }
            compared += 1;
        }
    }
    assert!(compared > 0, "no variant was compared");
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
    // Every rule the designs state in bold (`**G1.`, `**W7b.`, `**B8.`)
    // and every gate check is listed, itself or by its parts (`K2.blind`).
    // M2's checks are keyed `4.2#n`, M3's `m3.4.1#n`.
    let listed = |rule: &str| {
        rules.contains_key(rule) || rules.keys().any(|k| k.starts_with(&format!("{rule}.")))
    };
    let mut stated = Vec::new();
    for design in ["docs/research/m2-features.md", "docs/research/m3-socket.md"] {
        stated.extend(design_rules(
            &fs::read_to_string(root.join(design)).unwrap(),
        ));
    }
    let checks = (1..=30)
        .map(|n| format!("4.2#{n}"))
        .chain((31..=39).map(|n| format!("m3.4.1#{n}")));
    for rule in stated.into_iter().chain(checks) {
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
