//! Third-party STEP files, run through the built `fitment` as a user would.
//!
//! `tools/fetch-fixtures.sh core` downloads them into `fixtures/`; they are
//! never committed. `fixtures/manifest.json` pins what fitment's gate makes
//! of each one. Each file must give exactly its pinned result and exit
//! code, and never a panic or an `internal` refusal; an accepted one must
//! have its pinned numbers of planes, of unknown plane outlines, of full
//! cylinders, and of holes and shafts by kind, and
//! every face in exactly one primary
//! feature (design, section 6). Every STEP file
//! fetched outside the manual `ocp` set must be pinned. Without the files
//! the test passes with a note, unless `FITMENT_REQUIRE_FIXTURES=1`, which
//! CI sets.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;

mod common;

#[test]
fn every_fixture_gives_its_pinned_gate_result() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let fixtures = manifest["fixtures"].as_array().unwrap();
    let required = std::env::var_os("FITMENT_REQUIRE_FIXTURES").is_some_and(|v| v == "1");
    let missing: Vec<&str> = fixtures
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .filter(|path| !root.join(path).is_file())
        .collect();
    if !missing.is_empty() {
        assert!(
            !required,
            "FITMENT_REQUIRE_FIXTURES=1 but {} fixtures are missing, such as {}; run tools/fetch-fixtures.sh core",
            missing.len(),
            missing[0]
        );
        eprintln!("skipped: run tools/fetch-fixtures.sh core");
        return;
    }
    let mut failures = Vec::new();
    for fixture in fixtures {
        let path = fixture["path"].as_str().unwrap();
        let output = Command::cargo_bin("fitment")
            .unwrap()
            .arg("features")
            .arg(root.join(path))
            .output()
            .unwrap();
        let Ok(document) = serde_json::from_slice::<Value>(&output.stdout) else {
            failures.push(format!(
                "{path}: no document; exit {:?}",
                output.status.code()
            ));
            continue;
        };
        let mut reasons: Vec<&str> = document["refusals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|refusal| refusal["code"].as_str().unwrap())
            .collect();
        reasons.dedup();
        let code = if fixture["features"] == "accepted" {
            0
        } else {
            3
        };
        if output.status.code() != Some(code) {
            failures.push(format!(
                "{path}: exit {:?}, expected {code}",
                output.status.code()
            ));
        }
        let pinned: Vec<&str> = fixture["reasons"]
            .as_array()
            .map(|r| r.iter().map(|c| c.as_str().unwrap()).collect())
            .unwrap_or_default();
        if let Some(problem) = common::reads_back(&output.stdout) {
            failures.push(format!("{path}: {problem}"));
        }
        if document["status"] == "accepted" {
            failures.extend(accepted(&document["part"], fixture).map(|f| format!("{path}: {f}")));
        }
        if document["status"] != fixture["features"]
            || reasons != pinned
            || reasons.contains(&"internal")
        {
            failures.push(format!(
                "{path}: {} {reasons:?}, pinned {} {pinned:?}",
                document["status"], fixture["features"]
            ));
        }
    }
    let pinned: BTreeSet<String> = fixtures
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_owned())
        .collect();
    for path in step_files(&root, &root) {
        if !path.starts_with("ocp/") && !pinned.contains(&path) {
            failures.push(format!("{path}: fetched but not pinned"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// What is wrong with an accepted fixture's part: its numbers of planes
/// and of unknown plane outlines against the pins, and the face partition.
fn accepted(part: &Value, fixture: &Value) -> impl Iterator<Item = String> {
    let mut problems = Vec::new();
    let planes = part["planes"].as_array().map(Vec::len);
    if planes.map(|n| n as u64) != fixture["planes"].as_u64() {
        problems.push(format!("{planes:?} planes, pinned {}", fixture["planes"]));
    }
    let unknown = part["planes"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|plane| plane["faces"].as_array().into_iter().flatten())
        .filter(|face| face["outline"]["status"] == "unknown")
        .count();
    if Some(unknown as u64) != fixture["unknown_outlines"].as_u64() {
        problems.push(format!(
            "{unknown} unknown outlines, pinned {}",
            fixture["unknown_outlines"]
        ));
    }
    let full = part["cylinders"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["class"]["kind"] == "full" || c["class"]["kind"] == "full_one_end_irregular")
        .count();
    if Some(full as u64) != fixture["full_cylinders"].as_u64() {
        problems.push(format!(
            "{full} full cylinders, pinned {}",
            fixture["full_cylinders"]
        ));
    }
    for key in ["holes", "shafts"] {
        let mut kinds = serde_json::Map::new();
        for stack in part[key].as_array().into_iter().flatten() {
            let kind = stack["kind"].as_str().unwrap_or("").to_owned();
            let n = kinds.get(&kind).and_then(Value::as_u64).unwrap_or(0);
            kinds.insert(kind, (n + 1).into());
        }
        if Value::Object(kinds.clone()) != fixture[key] {
            problems.push(format!("{key} by kind {kinds:?}, pinned {}", fixture[key]));
        }
    }
    problems.extend(common::expect::partition(part));
    problems.into_iter()
}

/// Every `.stp` and `.step` file under `dir`, as paths relative to `root`
/// with `/` separators.
fn step_files(root: &Path, dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(step_files(root, &path));
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("stp") || e.eq_ignore_ascii_case("step"))
        {
            let relative = path.strip_prefix(root).unwrap();
            let parts: Vec<_> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            found.push(parts.join("/"));
        }
    }
    found
}
