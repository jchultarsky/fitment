//! Golden files: `fitment features` on every committed unit part, compared
//! byte for byte with `tests/golden/features/<stem>.json`.
//!
//! Goldens freeze bytes; they do not prove them right. Correctness comes
//! from fitment-core's tests, which check parts against their construction.
//! This test catches any unversioned change in output, and, run on three
//! operating systems, any difference between them.
//!
//! To accept a change, raise `EXTRACTOR_VERSION` and run
//! `FITMENT_BLESS=1 cargo test -p fitment --test golden`. Blessing writes
//! missing goldens and deletes goldens whose input is gone, but rewrites a
//! changed golden only if its `extractor_version` went up, and never runs
//! in CI.

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

fn extractor_version(document: &str) -> u64 {
    let value: serde_json::Value = serde_json::from_str(document).unwrap();
    value["extractor_version"].as_u64().unwrap()
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
        match fs::read_to_string(&golden) {
            Ok(expected) if expected == actual => {}
            Ok(expected) if bless => {
                let (old, new) = (extractor_version(&expected), extractor_version(&actual));
                if new > old {
                    fs::write(&golden, &actual).unwrap();
                } else {
                    failures.push(format!(
                        "{stem}: output changed but EXTRACTOR_VERSION is still {new}; raise it first"
                    ));
                }
            }
            Ok(expected) => failures.push(format!(
                "{stem}: output differs from {}\n{}\
                 if intended, raise EXTRACTOR_VERSION and run FITMENT_BLESS=1 cargo test -p fitment --test golden",
                golden.display(),
                first_difference(&expected, &actual)
            )),
            Err(_) if bless => fs::write(&golden, &actual).unwrap(),
            Err(_) => failures.push(format!(
                "{stem}: no golden; run FITMENT_BLESS=1 cargo test -p fitment --test golden"
            )),
        }
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
