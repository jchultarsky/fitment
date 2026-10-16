//! `fitment match --catalog`, run as a user would
//! (`docs/research/m5-catalog.md`, section 7.12, CM1 to CM10, QR1; CT1,
//! CT2, CT7, CT16): its exits, its stderr line and header, its labels, and
//! every document it writes the file run's, digest for digest.

#![cfg(feature = "catalog")]

#[path = "common/checker.rs"]
mod checker;

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;

fn fitment() -> Command {
    Command::cargo_bin("fitment").unwrap()
}

fn core(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fitment-core/tests")
        .join(path)
}

fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "fitment-catalog-match-{}-{test}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs `fitment ARGS` in `dir`: (code, stdout, stderr).
fn run(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = fitment().current_dir(dir).args(args).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The bracket's socket, and a catalog `parts` of the committed
/// candidates, in `dir`.
fn setup(dir: &Path) -> PathBuf {
    let socket = dir.join("bracket.json");
    fitment()
        .args([
            "socket",
            core("assemblies/s01_bolted.stp").to_str().unwrap(),
        ])
        .args(["--part", "BRACKET", "-o", socket.to_str().unwrap()])
        .assert()
        .success();
    let inputs = dir.join("in");
    fs::create_dir_all(&inputs).unwrap();
    for entry in fs::read_dir(core("candidates")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "stp") {
            fs::copy(&path, inputs.join(path.file_name().unwrap())).unwrap();
        }
    }
    run(dir, &["catalog", "init", "-C", "parts"]);
    run(dir, &["catalog", "ingest", "-C", "parts", "in"]);
    socket
}

fn json(text: &str) -> Value {
    let doc: Value = serde_json::from_str(text).unwrap();
    assert_eq!(checker::problems(&doc), Vec::<String>::new());
    doc
}

/// The labels of a document's candidates.
fn labels(doc: &Value) -> Vec<String> {
    doc["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["labels"].as_array().unwrap().iter())
        .map(|l| l.as_str().unwrap().to_owned())
        .collect()
}

/// CM1: one catalog, never with files; --id and --exhaustive need it and
/// exclude each other.
#[test]
fn the_catalog_is_one_and_alone() {
    let dir = scratch("cm1");
    setup(&dir);
    for args in [
        &["match", "bracket.json", "-C", "parts", "-C", "parts"][..],
        &["match", "bracket.json", "in/m_bracket.stp", "-C", "parts"],
        &[
            "match",
            "bracket.json",
            "in/m_bracket.stp",
            "--id",
            "m_bracket",
        ],
        &[
            "match",
            "bracket.json",
            "-C",
            "parts",
            "--id",
            "m_bracket",
            "--exhaustive",
        ],
    ] {
        assert_eq!(run(&dir, args).0, 2, "{args:?}");
    }
}

/// CT1, CM2, CM4, CM6, CM8 and CT2: a full run lists level 1's survivors,
/// labelled catalog:<id>@r1, with the file run's digest over their files;
/// an exhaustive run has the file run's digest over every file; a named
/// run, the named file's.
#[test]
fn catalog_runs_are_file_runs() {
    let dir = scratch("runs");
    setup(&dir);
    let (code, out, err) = run(
        &dir,
        &["match", "bracket.json", "-C", "parts", "--format", "json"],
    );
    assert_eq!(code, 4, "{err}");
    assert!(
        err.contains("catalog parts  generation 2  entries 13: 13 searched, 0 quarantined"),
        "{err}"
    );
    let full = json(&out);
    let survivors = labels(&full);
    assert!(
        survivors
            .iter()
            .all(|l| l.starts_with("catalog:") && l.ends_with("@r1"))
    );
    let files: Vec<String> = survivors
        .iter()
        .map(|l| format!("in/{}.stp", &l["catalog:".len()..l.len() - 3]))
        .collect();
    let mut args = vec!["match", "bracket.json"];
    args.extend(files.iter().map(String::as_str));
    args.extend(["--format", "json"]);
    let (code, out, _) = run(&dir, &args);
    assert_eq!(code, 4);
    assert_eq!(json(&out)["digest"], full["digest"]);
    let (_, everything, _) = run(&dir, &["match", "bracket.json", "in", "--format", "json"]);
    let (_, exhaustive, _) = run(
        &dir,
        &[
            "match",
            "bracket.json",
            "-C",
            "parts",
            "--exhaustive",
            "--format",
            "json",
        ],
    );
    assert_eq!(json(&exhaustive)["digest"], json(&everything)["digest"]);
    let (code, named, _) = run(
        &dir,
        &[
            "match",
            "bracket.json",
            "-C",
            "parts",
            "--id",
            "m_bracket",
            "--format",
            "json",
        ],
    );
    let (_, file, _) = run(
        &dir,
        &[
            "match",
            "bracket.json",
            "in/m_bracket.stp",
            "--format",
            "json",
        ],
    );
    assert_eq!(code, 4);
    assert_eq!(json(&named)["digest"], json(&file)["digest"]);
}

/// CM8 and CM10: the table's header line, and rejects cut at --rejects.
#[test]
fn the_table_says_what_was_searched() {
    let dir = scratch("table");
    setup(&dir);
    let (_, table, _) = run(
        &dir,
        &[
            "match",
            "bracket.json",
            "-C",
            "parts",
            "--exhaustive",
            "--rejects",
            "2",
        ],
    );
    let lines: Vec<&str> = table.lines().collect();
    assert!(
        lines[1].starts_with("catalog parts  generation 2"),
        "{table}"
    );
    assert!(table.contains("more rejects (in the document)"), "{table}");
    assert_eq!(
        lines.iter().filter(|l| l.starts_with("reject")).count(),
        2,
        "{table}"
    );
    let (_, all, _) = run(
        &dir,
        &[
            "match",
            "bracket.json",
            "-C",
            "parts",
            "--exhaustive",
            "--rejects",
            "all",
        ],
    );
    assert!(!all.contains("more rejects"));
    assert_eq!(
        run(&dir, &["match", "bracket.json", "in", "--rejects", "x"]).0,
        2
    );
}

/// QR1 and RX3: a stale entry refuses the run, exit 3, nothing on stdout;
/// a blocked socket refuses a full run (3) and a named run judges (5).
#[test]
fn refusals_come_before_any_candidate() {
    let dir = scratch("qr1");
    setup(&dir);
    let (code, out, err) = run(
        &dir,
        &[
            "match",
            "bracket.json",
            "-C",
            "parts",
            "--tolerance-mm",
            "0.0000001",
        ],
    );
    assert_eq!((code, out.as_str()), (3, ""), "{err}");
    assert!(err.contains("fail every candidate"), "{err}");
    let (code, _, _) = run(
        &dir,
        &[
            "match",
            "bracket.json",
            "-C",
            "parts",
            "--id",
            "m_bracket",
            "--tolerance-mm",
            "0.0000001",
        ],
    );
    assert_eq!(code, 5);
    let catalog =
        fitment_catalog::Catalog::open(&dir.join("parts"), fitment_catalog::Options::default())
            .unwrap();
    catalog
        .connection()
        .execute_batch(
            "DROP TRIGGER features_immutable; PRAGMA foreign_keys = OFF;
             UPDATE features SET extractor_version = extractor_version - 1
             WHERE sha256 = (SELECT sha256 FROM live WHERE id = 'm_plate03');",
        )
        .unwrap();
    drop(catalog);
    let (code, out, err) = run(&dir, &["match", "bracket.json", "-C", "parts"]);
    assert_eq!((code, out.as_str()), (3, ""), "{err}");
    assert!(
        err.contains("1 parts were read by another fitment"),
        "{err}"
    );
    assert_eq!(run(&dir, &["match", "bracket.json", "-C", "nowhere"]).0, 2);
}

/// QR4: a damaged row is named on stderr, not judged, and the run exits 1.
#[test]
fn a_damaged_row_exits_1() {
    let dir = scratch("damaged");
    setup(&dir);
    let catalog =
        fitment_catalog::Catalog::open(&dir.join("parts"), fitment_catalog::Options::default())
            .unwrap();
    catalog
        .connection()
        .execute_batch(
            "DROP TRIGGER features_immutable; PRAGMA foreign_keys = OFF;
             UPDATE features SET doc_sha256 = (SELECT doc_sha256 FROM features LIMIT 1 OFFSET 1)
             WHERE sha256 = (SELECT sha256 FROM live WHERE id = 'm_bracket');",
        )
        .unwrap();
    drop(catalog);
    let (code, out, err) = run(
        &dir,
        &[
            "match",
            "bracket.json",
            "-C",
            "parts",
            "--exhaustive",
            "--format",
            "json",
        ],
    );
    assert_eq!(code, 1);
    assert!(
        err.contains("m_bracket@r1: its stored rows are damaged"),
        "{err}"
    );
    assert!(!labels(&json(&out)).contains(&"catalog:m_bracket@r1".to_owned()));
}

/// CT7, `match_cli.rs::jobs_do_not_change_a_file_runs_bytes` (the merge's
/// open question 3): one job, two or sixteen give the same document, table
/// and exit, for file runs and catalog runs.
#[test]
fn jobs_do_not_change_a_file_runs_bytes() {
    let dir = scratch("jobs");
    setup(&dir);
    fs::write(dir.join("in/unreadable.stp"), "x").unwrap();
    for base in [
        &["match", "bracket.json", "in"][..],
        &["match", "bracket.json", "-C", "parts", "--exhaustive"],
    ] {
        let mut seen = Vec::new();
        for jobs in ["1", "2", "16"] {
            let mut args = base.to_vec();
            args.extend(["--jobs", jobs]);
            let table = run(&dir, &args);
            args.extend(["--format", "json"]);
            let json = run(&dir, &args);
            seen.push((table.0, table.1, json.1));
        }
        assert!(seen.windows(2).all(|w| w[0] == w[1]), "{base:?}");
    }
}

/// CM6 (review F4(b)): after a replace, the label carries the revision.
#[test]
fn labels_carry_the_revision() {
    let dir = scratch("labels");
    setup(&dir);
    fs::copy(core("data/p01_through.stp"), dir.join("p01.stp")).unwrap();
    let (code, _, err) = run(
        &dir,
        &["catalog", "replace", "-C", "parts", "m_bracket", "p01.stp"],
    );
    assert_eq!(code, 0, "{err}");
    let (_, out, _) = run(
        &dir,
        &[
            "match",
            "bracket.json",
            "-C",
            "parts",
            "--exhaustive",
            "--format",
            "json",
        ],
    );
    let labels = labels(&json(&out));
    assert!(
        labels.contains(&"catalog:m_bracket@r2".to_owned()),
        "{labels:?}"
    );
    assert!(!labels.iter().any(|l| l == "catalog:m_bracket@r1"));
    // ST10: never an output inside the catalog.
    assert_eq!(
        run(
            &dir,
            &["match", "bracket.json", "-C", "parts", "-o", "parts/x.json"]
        )
        .0,
        2
    );
}

/// QR12: --original works with a catalog as with files: the assembly the
/// user names is read, and judged beside the catalog's candidates.
#[test]
fn the_original_is_judged_beside_a_catalog_run() {
    let dir = scratch("original");
    setup(&dir);
    let assembly = core("assemblies/s01_bolted.stp");
    let (code, out, err) = run(
        &dir,
        &[
            "match",
            "bracket.json",
            "-C",
            "parts",
            "--original",
            assembly.to_str().unwrap(),
            "--format",
            "json",
        ],
    );
    assert_eq!(code, 4, "{err}");
    let doc = json(&out);
    assert_eq!(doc["original"]["verdict"], "interface_match", "{doc}");
}
