//! Golden files: `fitment features` on every committed unit part, compared
//! byte for byte with `tests/golden/features/<stem>.json`; and `fitment
//! parts` and `fitment socket` on every committed unit assembly, with
//! `tests/golden/parts/<case>.json` and `tests/golden/socket/<case>--<part>.json`
//! for each target its manifest names.
//!
//! Goldens freeze bytes; they do not prove them right. Correctness comes
//! from fitment-core's tests, which check parts against their construction.
//! This test catches any unversioned change in output, and, run on three
//! operating systems, any difference between them.
//!
//! To accept a change, raise `EXTRACTOR_VERSION` (or, for sockets,
//! `SOCKET_RULES_VERSION`) and run
//! `FITMENT_BLESS=1 cargo test -p fitment --test golden`. Blessing writes
//! missing goldens and deletes goldens whose input is gone, but rewrites a
//! changed golden only if one of its versions went up, and never runs in
//! CI.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests/data")
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/features")
}

/// Files in `dir` with extension `ext`, by stem.
fn by_stem(dir: &Path, ext: &str) -> BTreeMap<String, PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return BTreeMap::new();
    };
    entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        })
        .map(|path| {
            (
                path.file_stem().unwrap().to_string_lossy().into_owned(),
                path,
            )
        })
        .collect()
}

/// The versions a document states, of those named in `keys`.
fn versions(document: &str, keys: &[&str]) -> Vec<u64> {
    let value: serde_json::Value = serde_json::from_str(document).unwrap();
    keys.iter().map(|k| value[*k].as_u64().unwrap()).collect()
}

/// `actual` against the golden at `golden`; with `bless`, written when
/// missing or when one of `keys`' versions went up.
fn compare(
    name: &str,
    golden: &Path,
    actual: &str,
    keys: &[&str],
    bless: bool,
    failures: &mut Vec<String>,
) {
    match fs::read_to_string(golden) {
        Ok(expected) if expected == actual => {}
        Ok(expected) if bless => {
            let (old, new) = (versions(&expected, keys), versions(actual, keys));
            if new.iter().zip(&old).any(|(n, o)| n > o) {
                fs::write(golden, actual).unwrap();
            } else {
                failures.push(format!(
                    "{name}: output changed but {} still {new:?}; raise one first",
                    keys.join(" and ")
                ));
            }
        }
        Ok(expected) => failures.push(format!(
            "{name}: output differs from {}\n{}\
             if intended, raise {} and run FITMENT_BLESS=1 cargo test -p fitment --test golden",
            golden.display(),
            first_difference(&expected, actual),
            keys.join(" or ")
        )),
        Err(_) if bless => fs::write(golden, actual).unwrap(),
        Err(_) => failures.push(format!(
            "{name}: no golden; run FITMENT_BLESS=1 cargo test -p fitment --test golden"
        )),
    }
}

/// The first line where `expected` and `actual` differ, with context.
fn first_difference(expected: &str, actual: &str) -> String {
    let expected: Vec<&str> = expected.lines().collect();
    let actual: Vec<&str> = actual.lines().collect();
    let at = expected
        .iter()
        .zip(&actual)
        .position(|(e, a)| e != a)
        .unwrap_or(expected.len().min(actual.len()));
    let from = at.saturating_sub(3);
    let mut out = String::new();
    for line in expected.iter().take(at + 4).skip(from) {
        writeln!(out, "  - {line}").unwrap();
    }
    for line in actual.iter().take(at + 4).skip(from) {
        writeln!(out, "  + {line}").unwrap();
    }
    format!("first difference at line {}:\n{out}", at + 1)
}

#[test]
fn features_output_matches_the_goldens() {
    let bless = std::env::var_os("FITMENT_BLESS").is_some_and(|v| v == "1");
    assert!(
        !(bless && std::env::var_os("CI").is_some()),
        "do not bless in CI: run FITMENT_BLESS=1 locally and commit the goldens"
    );
    let inputs = by_stem(&data_dir(), "stp");
    assert!(
        !inputs.is_empty(),
        "no unit parts in {}",
        data_dir().display()
    );
    let goldens = by_stem(&golden_dir(), "json");
    if bless {
        fs::create_dir_all(golden_dir()).unwrap();
    }

    let mut failures = Vec::new();
    for (stem, input) in &inputs {
        let output = Command::cargo_bin("fitment")
            .unwrap()
            .args(["features", "--format", "json"])
            .arg(input)
            .output()
            .unwrap();
        let actual = String::from_utf8(output.stdout).unwrap();
        let status = serde_json::from_str::<serde_json::Value>(&actual)
            .unwrap_or_else(|e| panic!("{stem}: not JSON: {e}"))["status"]
            .as_str()
            .unwrap()
            .to_owned();
        let code = output.status.code();
        let expected_code = if status == "accepted" { 0 } else { 3 };
        if code != Some(expected_code) {
            failures.push(format!("{stem}: {status}, but exit code {code:?}"));
        }
        let golden = golden_dir().join(format!("{stem}.json"));
        compare(
            stem,
            &golden,
            &actual,
            &["extractor_version"],
            bless,
            &mut failures,
        );
    }
    for (stem, golden) in &goldens {
        if !inputs.contains_key(stem) {
            if bless {
                fs::remove_file(golden).unwrap();
            } else {
                failures.push(format!(
                    "{stem}: golden has no input in {}",
                    data_dir().display()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The unit assemblies' `parts` documents and their manifest's socket
/// targets' documents, against `tests/golden/parts` and
/// `tests/golden/socket`.
#[test]
fn parts_and_socket_output_match_the_goldens() {
    let bless = std::env::var_os("FITMENT_BLESS").is_some_and(|v| v == "1");
    assert!(
        !(bless && std::env::var_os("CI").is_some()),
        "do not bless in CI: run FITMENT_BLESS=1 locally and commit the goldens"
    );
    let assemblies = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests/assemblies");
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(assemblies.join("manifest.json")).unwrap())
            .unwrap();
    let mut wanted: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut failures = Vec::new();
    if bless {
        fs::create_dir_all(golden.join("parts")).unwrap();
        fs::create_dir_all(golden.join("socket")).unwrap();
    }
    let run = |args: &[&str]| {
        let output = Command::cargo_bin("fitment")
            .unwrap()
            .args(args)
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap()
    };
    for case in manifest["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let file = assemblies.join(case["file"].as_str().unwrap());
        let file = file.to_str().unwrap();
        let path = golden.join("parts").join(format!("{id}.json"));
        let actual = run(&["parts", file, "--format", "json"]);
        compare(
            id,
            &path,
            &actual,
            &["extractor_version"],
            bless,
            &mut failures,
        );
        wanted.insert(format!("parts/{id}"), path);
        for part in case["expect"]["sockets"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, _)| k)
        {
            let name = format!("{id}--{part}");
            let path = golden.join("socket").join(format!("{name}.json"));
            let actual = run(&["socket", file, "--part", part]);
            let keys = ["extractor_version", "socket_rules_version"];
            compare(&name, &path, &actual, &keys, bless, &mut failures);
            wanted.insert(format!("socket/{name}"), path);
        }
    }
    for kind in ["parts", "socket"] {
        for (stem, path) in by_stem(&golden.join(kind), "json") {
            if !wanted.contains_key(&format!("{kind}/{stem}")) {
                if bless {
                    fs::remove_file(path).unwrap();
                } else {
                    failures.push(format!("{kind}/{stem}: golden has no input"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
