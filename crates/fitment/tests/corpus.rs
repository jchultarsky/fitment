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

use std::collections::{BTreeMap, BTreeSet};
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

/// The committed candidates' cap (`docs/research/m4-match.md`, KC15).
const CANDIDATE_SET_BYTES: u64 = 210_000;

/// The gate stages built so far (`docs/research/m2-features.md`, 4.2).
const BUILT_STAGES: u64 = 6;

fn unit_set() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests/data")
}

fn assembly_set() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests/assemblies")
}

fn candidate_set() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests/candidates")
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

/// KH13: the committed candidates fit their cap and list exactly their
/// files.
#[test]
fn the_candidate_set_fits_its_budget_and_its_manifest() {
    fits(&candidate_set(), CANDIDATE_SET_BYTES);
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
    for case in cases.iter().filter(|case| case["kind"] == "part") {
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

/// True for a requirement whose stations and openings the part's box
/// bounds (U3's unknown ends, U1's partial cylinders, I7): they depend on
/// the frame the part is modelled in.
fn framed(r: &Value) -> bool {
    let flags = r["flags"].as_array().cloned().unwrap_or_default();
    let kind = r["requires"]["kind"].as_str().unwrap_or("").to_owned();
    kind.starts_with("partial")
        || flags
            .iter()
            .any(|f| f == "extent_unknown" || f == "target_undecided" || f == "partial_neighbour")
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
            let framed = |r: &Value| variant["kind"] == "tgt" && framed(r);
            // Occurrence by occurrence, in path order (in a `_perm` variant,
            // whose paths are another order, by placement): each with its
            // socket.
            let by_placement = variant["kind"] == "perm";
            let shape = |extracted: &fitment_core::Extracted, tilt: bool| {
                let mut out = serde_json::Map::new();
                let mut occurrences: Vec<_> = extracted.occurrences.iter().collect();
                if by_placement {
                    occurrences.sort_by_key(|o| {
                        common::variants::key(&serde_json::to_value(o.transform).unwrap())
                    });
                }
                for (k, occurrence) in occurrences.into_iter().enumerate() {
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

/// The rules left deferred when M4 closed, all M3's: `4.2#30` (internal, a
/// bug), `F3` (withdrawn), `Z7` (holds by construction) and `V1` (M7).
/// `Y4.original_run` left it with M4's matcher (KH10); M4 closed with none
/// of its own.
const DEFERRED: [&str; 4] = ["4.2#30", "F3", "Z7", "V1"];

/// The rules that yield a requirement every candidate fails (design 8.4,
/// Y4): each needs a case that shows it, not a test alone (13.1).
const UNVERIFIABLE: [&str; 13] = [
    "U1", "U2", "U4", "U5", "U6", "U7", "U8", "U9", "U10", "U11", "C4", "C6", "Q1",
];

/// KH14: M4's rules that yield an unverifiable requirement, a
/// `floor_above_tolerance` or a refusal: each needs a case whose
/// expectation shows that outcome, as `UNVERIFIABLE` does. VR4's no-room
/// branch is not among them: it needs a span floor from another source than
/// the clear diameter's, which no generated assembly states (with one floor
/// on every neighbour, M3's B1 makes the clearance line to line first), so
/// axial.rs's own test shows it.
const M4_UNVERIFIABLE: [&str; 9] = [
    "VR2", "VR6", "VR14", "VD5", "VD6", "VD9", "VD10", "CF2", "MF2",
];

/// KH14: every rule that can fail a candidate needs a bad case
/// (`does_not_fit` or `edge`) that cites it, so no check exists without a
/// candidate it rejects (RS10).
const CAN_FAIL: [&str; 13] = [
    "CF4", "CF5", "CF8", "VR3", "VR4", "VR7", "VR8", "VR9", "VR10", "VR11", "VR12", "VR13", "VR22",
];

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
    let by_id: BTreeMap<&str, &Value> = manifest["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["id"].as_str().unwrap(), c))
        .collect();
    for (id, entry) in rules {
        let cases = entry["cases"].as_array().map_or(0, Vec::len);
        let tests: Vec<&str> = entry["tests"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let oracle: Vec<&str> = entry["oracle"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let deferred = entry["deferred"].as_str().is_some_and(|r| !r.is_empty());
        if cases == 0 && tests.is_empty() && oracle.is_empty() && !deferred {
            failures.push(format!(
                "{id}: no case, no test, no oracle check and no reason"
            ));
        }
        if deferred && (cases > 0 || !tests.is_empty() || !oracle.is_empty()) {
            failures.push(format!("{id}: deferred, yet a case or test covers it"));
        }
        failures.extend(oracle_problems(id, &oracle, &root));
        failures.extend(case_lists(id, entry, &by_id));
        // M2, M3 and M4 are closed: only these may wait, each for a reason
        // outside them (a bug's code, a withdrawn rule, a property of the
        // construction, a later milestone). M4's exit removed the allowance
        // its own rules had to wait for the PR that built them (KH14).
        if deferred && !DEFERRED.contains(&id.as_str()) {
            failures.push(format!(
                "{id}: deferred, and not among the deferrals M4 closed with"
            ));
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
    for design in [
        "docs/research/m2-features.md",
        "docs/research/m3-socket.md",
        "docs/research/m4-match.md",
    ] {
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

/// KH14's and 13.1's lists for one rule: those that need a case, and
/// those that need a bad one.
fn case_lists(id: &str, entry: &Value, by_id: &BTreeMap<&str, &Value>) -> Vec<String> {
    let mut out = Vec::new();
    let cases: Vec<&str> = entry["cases"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let shown = cases
        .iter()
        .filter_map(|c| by_id.get(c))
        .any(|c| shows_blocked(c));
    if M4_UNVERIFIABLE.contains(&id) && !shown {
        out.push(format!(
            "{id}: yields an unverifiable requirement, a floor or a refusal, and no case shows it"
        ));
    }
    let bad = cases
        .iter()
        .filter_map(|c| by_id.get(c))
        .any(|c| matches!(c["truth"].as_str(), Some("does_not_fit" | "edge")));
    if CAN_FAIL.contains(&id) && !bad {
        out.push(format!(
            "{id}: can fail a candidate, and no bad case shows it"
        ));
    }
    if UNVERIFIABLE.contains(&id) && cases.is_empty() {
        out.push(format!(
            "{id}: yields an unverifiable requirement, and no case shows it"
        ));
    }
    out
}

/// True if a match case expects, under some policy, a requirement no
/// candidate can pass, a floor, or a refusal.
fn shows_blocked(case: &Value) -> bool {
    case["expect"]
        .as_object()
        .into_iter()
        .flatten()
        .any(|(_, e)| {
            let codes = e["must_fail"].as_array().into_iter().flatten().any(|m| {
                m["codes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|c| c == "unverifiable" || c == "floor_above_tolerance")
            });
            codes
                || e.get("unverifiable").is_some()
                || e.get("refused").is_some()
                || e["verdict"] == "socket_refused"
        })
}

/// The match oracle's rules (KO): checked by tools/verify-occt.py in CI,
/// each by a function that exists and names the rule.
fn oracle_problems(id: &str, oracle: &[&str], root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for check in oracle {
        let Some((file, name)) = check.split_once("::") else {
            out.push(format!("{id}: '{check}' is not file::function"));
            continue;
        };
        let text = fs::read_to_string(root.join(file)).unwrap_or_default();
        match python_function(&text, name) {
            None => out.push(format!("{id}: no function {name} in {file}")),
            Some(body) if !body.contains(id) => {
                out.push(format!("{id}: {name} in {file} does not name it"));
            }
            Some(_) => {}
        }
    }
    out
}

/// The text of a top-level Python function `name`, from its `def` to the
/// next top-level statement.
fn python_function<'t>(text: &'t str, name: &str) -> Option<&'t str> {
    let start = text.find(&format!("\ndef {name}("))? + 1;
    let rest = &text[start..];
    let end = rest
        .match_indices('\n')
        .map(|(i, _)| i + 1)
        .find(|&i| {
            rest[i..]
                .chars()
                .next()
                .is_some_and(|c| !c.is_whitespace() && c != ')')
        })
        .unwrap_or(rest.len());
    Some(&rest[..end])
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
        // `[A-Z]{1,2}[0-9]+[a-z]?`: M2's and M3's one letter, M4's two.
        let letters = id.chars().take_while(char::is_ascii_uppercase).count();
        let digits = id
            .chars()
            .skip(letters)
            .take_while(char::is_ascii_digit)
            .count();
        let tail = id.len() - letters - digits;
        let shaped = (1..=2).contains(&letters)
            && digits > 0
            && (tail == 0 || (tail == 1 && id.ends_with(|c: char| c.is_ascii_lowercase())))
            && id.len() <= 5;
        if shaped && rest[id.len()..].starts_with('.') && !found.contains(&id) {
            found.push(id);
        }
    }
    found
}
