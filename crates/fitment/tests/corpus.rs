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
//! - Coverage is checked, not claimed: every rule the designs state has a
//!   case, a test or a reason (KH14, `docs/research/m6-api.md` section
//!   11.1), and the corpus scorer is the one scorer, never published (SC1,
//!   SC3).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use fitment_corpus::Corpus;

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
    fits_budget(&unit_set(), UNIT_SET_BYTES);
}

#[test]
fn the_assembly_set_fits_its_budget_and_its_manifest() {
    fits_budget(&assembly_set(), ASSEMBLY_SET_BYTES);
}

/// KH13: the committed candidates fit their cap and list exactly their
/// files.
#[test]
fn the_candidate_set_fits_its_budget_and_its_manifest() {
    fits_budget(&candidate_set(), CANDIDATE_SET_BYTES);
}

/// `dir` holds exactly the files its manifest lists, within `budget` bytes.
fn fits_budget(dir: &Path, budget: u64) {
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
/// not required (SC7).
fn corpus() -> Option<PathBuf> {
    Corpus::locate().map(|c| c.dir().to_path_buf())
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
/// of its own. M5 closed with these, each for its reason (CT19): CC3 and
/// GT8 cannot be shown by a test, CL6, CL10 and ST8 hold by construction,
/// SZ2 is a guard no input reaches, QR13 is M7's and VF5 the README's.
const DEFERRED: [&str; 12] = [
    "4.2#30", "F3", "Z7", "V1", "CC3", "CL6", "CL10", "GT8", "QR13", "ST8", "SZ2", "VF5",
];

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

/// The designs whose rules the registry lists: each rule stated in bold
/// in one of them is in `rule_ids`, and in one of them only.
const DESIGNS: [&str; 5] = [
    "docs/research/m2-features.md",
    "docs/research/m3-socket.md",
    "docs/research/m4-match.md",
    "docs/research/m5-catalog.md",
    "docs/research/m6-api.md",
];

/// M6's design, open: its rules may wait for the PR of its section 12
/// that builds them (`"M6 PR n"`, section 11.1) until PR 9, M6's exit,
/// removes this allowance.
const M6: &str = "docs/research/m6-api.md";

/// M6's rules that bound an input (a size, a count, a time, a depth): each
/// test of theirs lives where a bound is exceeded (section 11.1).
const M6_LIMITS: [&str; 17] = [
    "UP2", "UP4", "UP6", "HX1", "HX2", "HX3", "HX8", "JS5", "JS10", "JS12", "JB7", "JB11", "AU1",
    "AU7", "AU10", "RA8", "VN3",
];

/// M6's rules that say the API does what a CLI command does: each test of
/// theirs is a parity or exit test.
const M6_PARITY: [&str; 13] = [
    "DC1", "DC3", "DC7", "JB2", "JB4", "JB10", "RA2", "SN2", "SN10", "UP6", "CU1", "CU2", "CA1",
];

/// M6's rules on durability, recovery or job state: each test of theirs is
/// a fault, soak or concurrency test.
const M6_FAULT: [&str; 16] = [
    "JB5", "JB6", "JS8", "JS11", "JS13", "OB2", "OB4", "OB5", "SD1", "SD2", "SD4", "SD5", "SD6",
    "CX3", "CX4", "CX6",
];

/// Where the tests of each of M6's lists live (section 9.4).
const M6_LISTS: [(&[&str], &[&str]); 3] = [
    (
        &M6_LIMITS,
        &[
            "crates/fitment-server/tests/hostile.rs",
            "crates/fitment-server/tests/slow.rs",
        ],
    ),
    (
        &M6_PARITY,
        &[
            "crates/fitment-server/tests/parity.rs",
            "crates/fitment-server/tests/exit.rs",
        ],
    ),
    (
        &M6_FAULT,
        &[
            "crates/fitment-server/tests/fault.rs",
            "crates/fitment-server/tests/soak.rs",
            "crates/fitment-server/tests/concurrency.rs",
        ],
    ),
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
    let by_id: BTreeMap<&str, &Value> = manifest["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["id"].as_str().unwrap(), c))
        .collect();
    let m6 = design_rules(&fs::read_to_string(root.join(M6)).unwrap());
    let mut failures: Vec<String> = M6_LISTS
        .iter()
        .flat_map(|(list, _)| list.iter())
        .filter(|id| !m6.iter().any(|r| r == *id))
        .map(|id| format!("{id}: in M6's lists, and not stated by {M6}"))
        .collect();
    for (id, entry) in rules {
        failures.extend(rule_problems(id, entry, &root, &m6));
        failures.extend(case_lists(id, entry, &by_id));
    }
    // Every rule the designs state in bold (`**G1.`, `**W7b.`, `**B8.`)
    // and every gate check is listed, itself or by its parts (`K2.blind`).
    // M2's checks are keyed `4.2#n`, M3's `m3.4.1#n`.
    let listed = |rule: &str| {
        rules.contains_key(rule) || rules.keys().any(|k| k.starts_with(&format!("{rule}.")))
    };
    let mut stated = Vec::new();
    for design in DESIGNS {
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

/// The M6 PR this tree builds: a deferral names a later one.
const M6_PR_NOW: u32 = 4;

/// The last M6 PR a rule may wait for: PR 9, M6's exit, removes the
/// allowance (section 12; PR 10 is the release).
const M6_PR_LAST: u32 = 9;

/// True for a deferral M6's open design allows: one of its own rules,
/// waiting for a later PR that builds it (`M6 PR n`, after this one and
/// at most the exit's).
fn m6_deferral(id: &str, reason: &str, m6: &[String]) -> bool {
    m6.iter().any(|r| r == id)
        && reason
            .strip_prefix("M6 PR ")
            .and_then(|n| n.parse::<u32>().ok())
            .is_some_and(|n| n > M6_PR_NOW && n <= M6_PR_LAST)
}

/// M6's lists (section 11.1): a rule of one that is not deferred has a
/// test in the files its list names; cases alone, or tests elsewhere, do
/// not show a bound exceeded, a CLI's parity or a fault survived.
fn m6_list_problems(id: &str, tests: &[&str], deferred: bool) -> Vec<String> {
    M6_LISTS
        .iter()
        .filter(|(list, _)| list.contains(&id) && !deferred)
        .filter(|(_, files)| {
            !tests
                .iter()
                .any(|t| files.iter().any(|f| t.starts_with(&format!("{f}::"))))
        })
        .map(|(_, files)| format!("{id}: no test of it in {files:?}"))
        .collect()
}

/// VT30's controls for M6's deferrals and lists: a deferral to this PR, an
/// earlier one or past the exit, or of a rule not M6's, is refused; a
/// listed rule shown by cases alone, or by a test elsewhere, is refused.
#[test]
fn m6_deferrals_and_lists_hold() {
    let m6 = vec!["UP2".to_owned()];
    assert!(m6_deferral("UP2", "M6 PR 5", &m6));
    assert!(m6_deferral("UP2", "M6 PR 9", &m6));
    assert!(!m6_deferral("UP2", "M6 PR 4", &m6), "a deferral to this PR");
    assert!(
        !m6_deferral("UP2", "M6 PR 3", &m6),
        "a deferral to an earlier PR"
    );
    for reason in ["M6 PR 1", "M6 PR 0", "M6 PR 10", "M6 PR", "M5 PR 4"] {
        assert!(!m6_deferral("UP2", reason, &m6), "{reason}");
    }
    assert!(!m6_deferral("ST1", "M6 PR 5", &m6));
    let hostile = "crates/fitment-server/tests/hostile.rs::a_body_past_the_cap";
    let elsewhere = "crates/fitment-server/tests/api.rs::a_body_past_the_cap";
    assert_eq!(m6_list_problems("UP2", &[], false).len(), 1);
    assert_eq!(m6_list_problems("UP2", &[elsewhere], false).len(), 1);
    assert_eq!(
        m6_list_problems("UP2", &[hostile, elsewhere], false),
        Vec::<String>::new()
    );
    assert_eq!(m6_list_problems("UP2", &[], true), Vec::<String>::new());
    assert_eq!(
        m6_list_problems("SC1", &[elsewhere], false),
        Vec::<String>::new()
    );
}

/// One rule's entry: a case, a test that exists, an oracle check or a
/// reason, and never a reason beside a case or test; a deferral only for
/// a reason the closed milestones left or M6's open one allows; and each
/// test where M6's lists want it.
fn rule_problems(id: &str, entry: &Value, root: &Path, m6: &[String]) -> Vec<String> {
    let mut failures = Vec::new();
    let cases = entry["cases"].as_array().map_or(0, Vec::len);
    let strings = |key: &str| -> Vec<&str> {
        entry[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect()
    };
    let (tests, oracle) = (strings("tests"), strings("oracle"));
    let reason = entry["deferred"].as_str().unwrap_or("");
    let deferred = !reason.is_empty();
    if cases == 0 && tests.is_empty() && oracle.is_empty() && !deferred {
        failures.push(format!(
            "{id}: no case, no test, no oracle check and no reason"
        ));
    }
    // While M6 is open, a rule waiting for its PR may be cited by cases
    // that support it (XH1 by PR 1's review cases); a test still ends its
    // wait.
    let supporting = cases > 0 && m6_deferral(id, reason, m6);
    if deferred && ((cases > 0 && !supporting) || !tests.is_empty() || !oracle.is_empty()) {
        failures.push(format!("{id}: deferred, yet a case or test covers it"));
    }
    failures.extend(oracle_problems(id, &oracle, root));
    // M2 to M5 are closed: only these may wait, each for a reason outside
    // them (a bug's code, a withdrawn rule, a property of the construction,
    // a later milestone). M5's exit removed the allowance its rules had to
    // wait for the PR that built them (CT19); M6's rules have it while M6
    // is open.
    if deferred && !DEFERRED.contains(&id) && !m6_deferral(id, reason, m6) {
        failures.push(format!(
            "{id}: deferred, and not among the deferrals M4 and M5 closed with, nor M6's to its PR"
        ));
    }
    for test in &tests {
        let Some((file, name)) = test.split_once("::") else {
            failures.push(format!("{id}: '{test}' is not file::function"));
            continue;
        };
        if !is_test(&root.join(file), name) {
            failures.push(format!("{id}: no test {name} in {file}"));
        }
    }
    failures.extend(m6_list_problems(id, &tests, deferred));
    failures
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

/// True if `file` has a test function `name`.
fn is_test(file: &Path, name: &str) -> bool {
    is_test_in(&fs::read_to_string(file).unwrap_or_default(), name)
}

/// True if `text` has a test function `name`: `fn` or `async fn` under
/// `#[test]`, `#[tokio::test]` or `#[tokio::test(...)]` among the
/// attribute and doc-comment lines right above it (M6-41).
fn is_test_in(text: &str, name: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    let attribute = |line: &&str| {
        let line = line.trim();
        line == "#[test]"
            || line == "#[tokio::test]"
            || (line.starts_with("#[tokio::test(") && line.ends_with(")]"))
    };
    lines.iter().enumerate().any(|(i, line)| {
        let line = line.trim_start();
        let declared = line.starts_with(&format!("fn {name}("))
            || line.starts_with(&format!("async fn {name}("));
        // Upward over its own attributes and doc comments only: a test
        // above another function is not that function's.
        declared
            && lines[..i]
                .iter()
                .rev()
                .take_while(|l| {
                    let l = l.trim();
                    l.starts_with("#[") || l.starts_with("///")
                })
                .any(attribute)
    })
}

/// VT30's control for M6-41: a test is found as written, `#[test]` or
/// tokio's, sync or async, and a function under no test attribute is not.
#[test]
fn a_test_is_found_as_written() {
    let found = [
        ("#[test]\nfn a() {}", "a"),
        ("#[tokio::test]\nasync fn b() {}", "b"),
        (
            "#[tokio::test(flavor = \"multi_thread\")]\n/// Docs.\nasync fn c() {}",
            "c",
        ),
    ];
    for (text, name) in found {
        assert!(is_test_in(text, name), "{text}");
    }
    let not_found = [
        ("async fn d() {}", "d"),
        ("/// Docs.\nasync fn e() {}", "e"),
        ("fn f() {}", "f"),
        ("#[test]\nfn g_more() {}", "g"),
        ("#[tokio::main]\nasync fn h() {}", "h"),
        ("#[test]\nfn a() {}\nfn helper() {}", "helper"),
        (
            "#[tokio::test]\nasync fn a() {}\nasync fn helper() {}",
            "helper",
        ),
        ("#[test]\nfn a() {}\n\n/// Docs.\nfn helper() {}", "helper"),
    ];
    for (text, name) in not_found {
        assert!(!is_test_in(text, name), "{text}");
    }
}

/// The ids stated in bold by more than one of `designs` (name, text): one
/// id, one rule (section 11.1).
fn stated_twice(designs: &[(&str, String)]) -> Vec<String> {
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    let mut out = Vec::new();
    for (name, text) in designs {
        for id in design_rules(text) {
            if let Some(first) = seen.insert(id.clone(), name) {
                out.push(format!("{id}: stated in bold by {first} and {name}"));
            }
        }
    }
    out
}

/// VT30: no rule id is stated in bold by two designs, and the check finds
/// one that is (its control).
#[test]
fn no_rule_id_is_stated_twice() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let designs: Vec<(&str, String)> = DESIGNS
        .iter()
        .map(|d| (*d, fs::read_to_string(root.join(d)).unwrap()))
        .collect();
    assert_eq!(stated_twice(&designs), Vec::<String>::new());
    let control = [
        ("a.md", "- **AB1. One.** Text.".to_owned()),
        ("b.md", "- **AB1. Another.** Text.".to_owned()),
    ];
    assert_eq!(stated_twice(&control).len(), 1);
}

/// SC1: no test keeps a scorer of its own. Outside `fitment-corpus`, no
/// source names a scorer's function, the baseline's file or the corpus's
/// variables, and only fitment-core's candidates include the scorer's
/// files by path (SC6).
#[test]
fn no_test_keeps_a_scorer_of_its_own() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // Built here, so that this file does not name them itself.
    let owned: Vec<String> = ["fits", "false_positive", "false_negatives", "expectation"]
        .iter()
        .map(|f| format!("fn {f}("))
        .chain(["FITMENT_CORPUS", "FITMENT_REQUIRE_CORPUS"].map(|v| format!("var_os(\"{v}\")")))
        .chain([
            format!("false-{}.json", "negatives"),
            format!("{}/gate.rs", "support"),
        ])
        .chain([format!("{}/checker.rs", "common")])
        .collect();
    let mut files = Vec::new();
    for entry in fs::read_dir(root.join("crates")).unwrap() {
        let krate = entry.unwrap().path();
        if krate.ends_with("fitment-corpus") {
            continue;
        }
        files.extend(sources(&krate.join("src")));
        files.extend(sources(&krate.join("tests")));
    }
    assert!(files.len() > 20, "{} sources", files.len());
    let mut failures = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).unwrap_or_default();
        for pattern in owned.iter().filter(|p| text.contains(p.as_str())) {
            failures.push(format!("{}: {pattern}", file.display()));
        }
        let includes = text.contains(&format!("{}/src/", "fitment-corpus"));
        if includes && !file.ends_with("fitment-core/tests/candidates.rs") {
            failures.push(format!("{}: includes the scorer by path", file.display()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every `.rs` file under `dir`.
fn sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(sources(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

/// SC3: `fitment-corpus` is never published: `publish = false` in its
/// manifest, a workspace dependency without a version (which `cargo
/// publish` strips), and only ever a dev-dependency.
#[test]
fn the_scorer_is_never_published() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let own = fs::read_to_string(root.join("crates/fitment-corpus/Cargo.toml")).unwrap();
    assert!(own.lines().any(|l| l.trim() == "publish = false"));
    let workspace = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let line = workspace
        .lines()
        .find(|l| l.starts_with("fitment-corpus ="))
        .expect("fitment-corpus in [workspace.dependencies]");
    assert!(!line.contains("version"), "{line}");
    let mut uses = 0;
    for entry in fs::read_dir(root.join("crates")).unwrap() {
        let manifest = entry.unwrap().path().join("Cargo.toml");
        let text = fs::read_to_string(&manifest).unwrap_or_default();
        let mut section = "";
        for line in text.lines() {
            if line.starts_with('[') {
                section = line;
            } else if line.starts_with("fitment-corpus") {
                uses += 1;
                assert_eq!(section, "[dev-dependencies]", "{}", manifest.display());
            }
        }
    }
    assert!(uses > 0, "no crate uses the scorer");
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
        // `[A-Z]{1,2}[0-9]+[a-z]?`: M2's and M3's one letter, M4's and M5's two.
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
