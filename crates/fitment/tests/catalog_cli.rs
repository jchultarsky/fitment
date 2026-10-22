//! `fitment catalog init`, `ingest`, `list` and `get`, run as a user would
//! (`docs/research/m5-catalog.md`, section 7, CT15, CT16): exit codes,
//! lines, documents, references, and bytes handed out only checked (ST11).

#![cfg(feature = "catalog")]

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

/// Clears read-only flags, so a catalog can be removed on every OS.
fn writable(dir: &Path) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            writable(&path);
        } else if let Ok(meta) = fs::metadata(&path) {
            let mut p = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            p.set_readonly(false);
            let _ = fs::set_permissions(&path, p);
        }
    }
}

/// A fresh directory for one test.
fn scratch(test: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("fitment-catalog-cli-{}-{test}", std::process::id()));
    writable(&dir);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs `fitment catalog ARGS` in `dir`: (code, stdout, stderr).
fn catalog(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = fitment()
        .current_dir(dir)
        .arg("catalog")
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// C's prototype set (`C/proto/run.txt`), in `dir/in`.
fn prototype(dir: &Path) -> PathBuf {
    let inputs = dir.join("in");
    fs::create_dir_all(&inputs).unwrap();
    for (from, to) in [
        ("candidates/m_bracket.stp", "m_bracket.stp"),
        ("candidates/m_bracket.stp", "copy of m_bracket.stp"),
        ("data/p03_blind_flat.stp", "a b.stp"),
        ("data/p01_through.stp", "a_b.stp"),
        ("data/r04_assembly.stp", "r04_assembly.stp"),
    ] {
        fs::copy(core(from), inputs.join(to)).unwrap();
    }
    fs::write(inputs.join("junk.stp"), "not STEP").unwrap();
    inputs
}

/// CT16: init twice; a directory that is not empty; one that is not a
/// catalog; no -C.
#[test]
fn init_and_what_is_not_a_catalog() {
    let dir = scratch("init");
    assert_eq!(catalog(&dir, &["init", "-C", "parts"]).0, 0);
    let (code, _, err) = catalog(&dir, &["init", "-C", "parts"]);
    assert_eq!(code, 0);
    assert!(err.contains("already a catalog"), "{err}");
    fs::create_dir_all(dir.join("mine")).unwrap();
    fs::write(dir.join("mine/notes.txt"), "x").unwrap();
    assert_eq!(catalog(&dir, &["init", "-C", "mine"]).0, 2);
    let (code, _, err) = catalog(&dir, &["list", "-C", "mine"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("not a catalog: run fitment catalog init -C mine"),
        "{err}"
    );
    assert_eq!(catalog(&dir, &["list"]).0, 2);
    // -C anywhere after `catalog`.
    assert_eq!(catalog(&dir, &["-C", "parts", "list"]).0, 0);
}

/// CT16 on C's prototype set: added, quarantined, duplicate, conflict
/// (exit 2); again, unchanged (exit 2); without the conflict, exit 3 for
/// the quarantined; `--id` with another entry's bytes, exit 2.
#[test]
fn ingest_reports_each_file_and_exits_by_the_worst() {
    let dir = scratch("ingest");
    let inputs = prototype(&dir);
    catalog(&dir, &["init", "-C", "parts"]);
    let (code, out, err) = catalog(&dir, &["ingest", "-C", "parts", "in"]);
    assert_eq!(code, 2, "{out}{err}");
    let words: Vec<(&str, &str)> = out
        .lines()
        .map(|l| {
            let mut w = l.split_whitespace();
            (w.next().unwrap(), w.next().unwrap())
        })
        .collect();
    assert_eq!(
        words,
        [
            ("added", "a_b"),
            ("quarantined", "junk"),
            ("added", "m_bracket"),
            ("quarantined", "r04_assembly"),
            ("conflict", "a_b"),
            ("duplicate", "copy_of_m_bracket"),
        ]
    );
    assert!(
        err.contains("6 files: 2 added, 2 quarantined, 1 duplicate, 1 conflict"),
        "{err}"
    );
    let (code, out, _) = catalog(&dir, &["ingest", "-C", "parts", "in"]);
    assert_eq!(code, 2);
    assert_eq!(
        out.lines().filter(|l| l.starts_with("unchanged")).count(),
        4
    );
    fs::remove_file(inputs.join("a b.stp")).unwrap();
    assert_eq!(catalog(&dir, &["ingest", "-C", "parts", "in"]).0, 3);
    let bracket = inputs.join("m_bracket.stp");
    let (code, out, _) = catalog(
        &dir,
        &[
            "ingest",
            "-C",
            "parts",
            "--id",
            "X",
            bracket.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 2);
    assert!(out.starts_with("duplicate"), "{out}");
    // A clean directory: 0, twice; list the same bytes each time.
    let clean = dir.join("clean");
    fs::create_dir_all(&clean).unwrap();
    fs::copy(core("data/p05_cbore.stp"), clean.join("p05_cbore.stp")).unwrap();
    assert_eq!(catalog(&dir, &["ingest", "-C", "parts", "clean"]).0, 0);
    let first = catalog(&dir, &["list", "-C", "parts", "--format", "json"]).1;
    assert_eq!(catalog(&dir, &["ingest", "-C", "parts", "clean"]).0, 0);
    assert_eq!(
        catalog(&dir, &["list", "-C", "parts", "--format", "json"]).1,
        first
    );
}

/// CL3, CL9: the list document: its counts, its order by id case-folded,
/// and no path of this machine in it.
#[test]
fn list_writes_a_catalog_document() {
    let dir = scratch("list");
    prototype(&dir);
    catalog(&dir, &["init", "-C", "parts"]);
    catalog(&dir, &["ingest", "-C", "parts", "in"]);
    let (code, out, err) = catalog(&dir, &["list", "-C", "parts", "--format", "json"]);
    assert_eq!(code, 0);
    assert!(
        err.contains("generation 2: 4 entries: 2 indexed, 2 quarantined, 0 stale"),
        "{err}"
    );
    let doc: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc["format"], "fitment.catalog");
    assert_eq!(doc["counts"]["entries"], 4);
    let ids: Vec<&str> = doc["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["a_b", "junk", "m_bracket", "r04_assembly"]);
    assert_eq!(doc["entries"][2]["features"]["holes"], 5);
    assert_eq!(doc["entries"][1]["refusals"][0]["code"], "unparseable");
    assert!(!out.contains(dir.to_str().unwrap()));
    let (_, quarantined, _) = catalog(&dir, &["list", "-C", "parts", "--status", "quarantined"]);
    assert!(quarantined.contains("    assembly: "), "{quarantined}");
    assert!(!quarantined.contains("m_bracket"));
}

/// GT2, section 7.4: `get` by id, revision and content; `--format
/// features` is `fitment features` of the file, byte for byte; a
/// quarantined entry's is its refused document (exit 3); the bytes go to
/// `-o` only, and never inside the catalog.
#[test]
fn get_shows_an_entry_its_document_and_its_bytes() {
    let dir = scratch("get");
    let inputs = prototype(&dir);
    catalog(&dir, &["init", "-C", "parts"]);
    catalog(&dir, &["ingest", "-C", "parts", "in"]);
    let (code, out, err) = catalog(&dir, &["get", "-C", "parts", "m_bracket"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("revision  r1 (current)"), "{out}");
    assert!(err.contains("m_bracket r1 sha256:1b77ea97"), "{err}");
    let (code, features, _) = catalog(
        &dir,
        &["get", "-C", "parts", "m_bracket", "--format", "features"],
    );
    assert_eq!(code, 0);
    let direct = fitment()
        .args(["features", inputs.join("m_bracket.stp").to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(features.as_bytes(), direct.stdout.as_slice());
    let (code, refused, _) = catalog(
        &dir,
        &["get", "-C", "parts", "junk", "--format", "features"],
    );
    assert_eq!(code, 3);
    assert!(refused.contains("\"unparseable\""), "{refused}");
    assert_eq!(
        catalog(
            &dir,
            &["get", "-C", "parts", "m_bracket", "--format", "step"]
        )
        .0,
        2
    );
    for inside in ["parts/x.stp", "parts/", "parts", "parts/.", "in"] {
        let args = [
            "get",
            "-C",
            "parts",
            "m_bracket",
            "--format",
            "step",
            "-o",
            inside,
        ];
        assert_eq!(catalog(&dir, &args).0, 2, "{inside}");
    }
    let (code, _, _) = catalog(
        &dir,
        &[
            "get",
            "-C",
            "parts",
            "sha256:1b77ea9724e6",
            "--format",
            "step",
            "-o",
            "x.stp",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(
        fs::read(dir.join("x.stp")).unwrap(),
        fs::read(inputs.join("m_bracket.stp")).unwrap()
    );
    let (code, json, _) = catalog(
        &dir,
        &["get", "-C", "parts", "M_BRACKET@r1", "--format", "json"],
    );
    assert_eq!(code, 0);
    let doc: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(doc["entries"][0]["history"][0]["event"], "ingest");
    assert_eq!(doc["counts"]["entries"], 4);
}

/// ID7 and ID9: unknown ids suggest; short and ambiguous content
/// references, and every malformed revision, exit 2.
#[test]
fn references_are_strict() {
    let dir = scratch("refs");
    prototype(&dir);
    catalog(&dir, &["init", "-C", "parts"]);
    catalog(&dir, &["ingest", "-C", "parts", "in"]);
    let (code, _, err) = catalog(&dir, &["get", "-C", "parts", "m_brackt"]);
    assert_eq!(code, 2);
    assert!(err.contains("did you mean m_bracket?"), "{err}");
    for bad in [
        "m_bracket@R1",
        "m_bracket@r01",
        "m_bracket@r0",
        "m_bracket@1",
        "m_bracket@r",
        "m_bracket@r1x",
        "m_bracket@r9",
        "sha256:1b77ea97",
        "a'b",
        "a%",
        "a;",
    ] {
        assert_eq!(catalog(&dir, &["get", "-C", "parts", bad]).0, 2, "{bad}");
    }
}

/// ST11 (review F4(a)): a blob altered on disk is never handed out: `get
/// --format step` exits 1, says "damaged", and leaves `-o`'s file as it was.
#[test]
fn get_step_refuses_a_damaged_blob() {
    let dir = scratch("damaged");
    prototype(&dir);
    catalog(&dir, &["init", "-C", "parts"]);
    catalog(&dir, &["ingest", "-C", "parts", "in"]);
    let sha = "1b77ea9724e67c4bdc709a871a00a05580dfebb36e4109f3c027d84afe826c33";
    let blob = dir.join("parts/blobs/sha256").join(&sha[..2]).join(sha);
    let original = fs::read(&blob).unwrap();
    writable(&dir.join("parts/blobs"));
    fs::write(dir.join("x.stp"), "before").unwrap();
    let mut flipped = original.clone();
    flipped[100] ^= 1;
    for altered in [flipped, original[..original.len() - 1].to_vec()] {
        fs::write(&blob, &altered).unwrap();
        let (code, _, err) = catalog(
            &dir,
            &[
                "get",
                "-C",
                "parts",
                "m_bracket",
                "--format",
                "step",
                "-o",
                "x.stp",
            ],
        );
        assert_eq!(code, 1);
        assert!(
            err.contains("damaged") && err.contains("fitment catalog verify"),
            "{err}"
        );
        assert_eq!(fs::read(dir.join("x.stp")).unwrap(), b"before");
    }
}

/// CT15 and PH6: names holding escapes, newlines and bidirectional
/// controls never reach the terminal raw; JSON holds them exactly. Unix
/// only: Windows forbids control characters in a file name.
#[cfg(unix)]
#[test]
fn hostile_names_are_escaped() {
    let dir = scratch("hostile");
    let inputs = dir.join("in");
    fs::create_dir_all(&inputs).unwrap();
    let name = "evil\u{1b}[31mred\u{202e}.stp";
    fs::copy(core("data/p05_cbore.stp"), inputs.join(name)).unwrap();
    catalog(&dir, &["init", "-C", "parts"]);
    let mut all = String::new();
    for args in [
        vec!["ingest", "-C", "parts", "in"],
        vec!["list", "-C", "parts"],
        vec!["get", "-C", "parts", "evil_31mred_"],
    ] {
        let (code, out, err) = catalog(&dir, &args);
        assert_eq!(code, 0, "{args:?}: {err}");
        all.push_str(&out);
        all.push_str(&err);
    }
    assert!(
        !all.contains('\u{1b}') && !all.contains('\u{202e}'),
        "{all:?}"
    );
    assert!(all.contains("evil\\u{1b}[31mred\\u{202e}.stp"), "{all}");
    let json = catalog(&dir, &["list", "-C", "parts", "--format", "json"]).1;
    let doc: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(doc["entries"][0]["file_name"], name);
}

/// A catalog named `file:…` is a directory, never an SQLite URI. Unix
/// only: Windows forbids `:` in a file name.
#[cfg(unix)]
#[test]
fn a_catalog_named_like_a_uri_is_a_directory() {
    let dir = scratch("uri");
    fs::create_dir_all(dir.join("cat")).unwrap();
    assert_eq!(catalog(&dir, &["init", "-C", "file:cat"]).0, 0);
    assert!(dir.join("file:cat/catalog.db").is_file());
    assert!(!dir.join("cat/catalog.db").exists());
    assert_eq!(catalog(&dir, &["list", "-C", "file:cat"]).0, 0);
}

/// BRKT at r1 (the bracket) and `a` (p01), in `dir/parts`; the files in
/// `dir/in`.
fn brkt(dir: &Path) -> PathBuf {
    let inputs = dir.join("in");
    fs::create_dir_all(&inputs).unwrap();
    for (from, to) in [
        ("candidates/m_bracket.stp", "m_bracket.stp"),
        ("candidates/m_bracket_thick.stp", "m_bracket_thick.stp"),
        ("data/p01_through.stp", "a.stp"),
    ] {
        fs::copy(core(from), inputs.join(to)).unwrap();
    }
    fs::write(inputs.join("junk.stp"), "not STEP").unwrap();
    catalog(dir, &["init", "-C", "parts"]);
    catalog(
        dir,
        &["ingest", "-C", "parts", "--id", "BRKT", "in/m_bracket.stp"],
    );
    catalog(dir, &["ingest", "-C", "parts", "in/a.stp"]);
    inputs
}

fn sha256(path: &Path) -> String {
    fitment_core::sha256_hex(&fs::read(path).unwrap())
}

/// Section 7.5 (CR3, CR4): replace's exits; the entry is unchanged by
/// every refusal.
#[test]
fn replace_refuses_without_changing_the_entry() {
    let dir = scratch("replace");
    brkt(&dir);
    let replace = |file: &str| catalog(&dir, &["replace", "-C", "parts", "BRKT", file]);
    let (code, out, _) = replace("in/m_bracket_thick.stp");
    assert_eq!(code, 0);
    assert!(out.starts_with("replaced     BRKT  r2"), "{out}");
    assert_eq!(replace("in/junk.stp").0, 3);
    assert_eq!(replace("in/a.stp").0, 2);
    assert_eq!(replace("parts/catalog.db").0, 2);
    let (code, out, _) = replace("in/m_bracket_thick.stp");
    assert_eq!(code, 0);
    assert!(out.starts_with("unchanged"), "{out}");
    let (_, table, _) = catalog(&dir, &["get", "-C", "parts", "BRKT"]);
    assert!(table.contains("revision  r2 (current)"), "{table}");
}

/// CT16, `catalog_cli.rs::replace_keeps_the_old_revisions_bytes` (review
/// F4(b)): r1's bytes stay readable after a replace, until `prune
/// --revisions`; its history stays after.
#[test]
fn replace_keeps_the_old_revisions_bytes() {
    let dir = scratch("kept");
    let inputs = brkt(&dir);
    catalog(
        &dir,
        &["replace", "-C", "parts", "BRKT", "in/m_bracket_thick.stp"],
    );
    let (code, _, err) = catalog(
        &dir,
        &[
            "get", "-C", "parts", "BRKT@r1", "--format", "step", "-o", "old.stp",
        ],
    );
    assert_eq!(code, 0);
    let old = sha256(&inputs.join("m_bracket.stp"));
    assert!(err.contains(&format!("BRKT r1 sha256:{old}")), "{err}");
    assert_eq!(sha256(&dir.join("old.stp")), old);
    assert_eq!(
        catalog(
            &dir,
            &["get", "-C", "parts", "BRKT@r1", "--format", "features"]
        )
        .0,
        2
    );
    let (_, list, err) = catalog(&dir, &["list", "-C", "parts"]);
    assert!(list.contains("BRKT") && !list.contains(&old[..8]), "{list}");
    assert!(err.contains("1 old revisions kept"), "{err}");
    assert_eq!(catalog(&dir, &["verify", "-C", "parts"]).0, 0);
    assert_eq!(catalog(&dir, &["prune", "-C", "parts"]).0, 2);
    let (code, out, _) = catalog(&dir, &["prune", "-C", "parts", "--revisions"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("pruned       BRKT  r1"), "{out}");
    assert_eq!(
        catalog(
            &dir,
            &[
                "get", "-C", "parts", "BRKT@r1", "--format", "step", "-o", "x.stp"
            ]
        )
        .0,
        2
    );
    let (code, table, _) = catalog(&dir, &["get", "-C", "parts", "BRKT@r1"]);
    assert_eq!(code, 0);
    assert!(table.contains("prune"), "{table}");
    assert_eq!(
        catalog(&dir, &["prune", "-C", "parts", "--revisions"]).1,
        ""
    );
    assert_eq!(catalog(&dir, &["verify", "-C", "parts", "--deep"]).0, 0);
}

/// CT16, `catalog_cli.rs::strict_revision_references`, its second half
/// (review F4(b)): after a replace, a command that changes an entry takes
/// only its current revision, and changes nothing otherwise.
#[test]
fn changes_take_only_the_current_revision() {
    let dir = scratch("strict");
    brkt(&dir);
    catalog(
        &dir,
        &["replace", "-C", "parts", "BRKT", "in/m_bracket_thick.stp"],
    );
    let (code, _, err) = catalog(
        &dir,
        &["replace", "-C", "parts", "BRKT@r1", "in/m_bracket.stp"],
    );
    assert_eq!(code, 2);
    assert!(err.contains("BRKT is at r2, not r1"), "{err}");
    assert_eq!(catalog(&dir, &["delete", "-C", "parts", "BRKT@r1"]).0, 2);
    assert_eq!(
        catalog(&dir, &["delete", "-C", "parts", "sha256:1b77ea9724e6"]).0,
        2
    );
    assert_eq!(
        catalog(&dir, &["delete", "-C", "parts", "a", "nosuch"]).0,
        2
    );
    assert!(
        catalog(&dir, &["list", "-C", "parts"]).1.contains("\na "),
        "a is still listed"
    );
    let (code, out, _) = catalog(&dir, &["delete", "-C", "parts", "BRKT@r2"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("deleted      BRKT  r2"), "{out}");
    // Both revisions' files are gone; the history stays.
    let blobs = dir.join("parts/blobs/sha256");
    for sha in [
        sha256(&dir.join("in/m_bracket.stp")),
        sha256(&dir.join("in/m_bracket_thick.stp")),
    ] {
        assert!(!blobs.join(&sha[..2]).join(&sha).exists());
    }
    let (_, table, _) = catalog(&dir, &["get", "-C", "parts", "BRKT"]);
    assert!(table.contains("delete"), "{table}");
}

/// Sections 7.8 and 7.9: verify reports a damaged file (exit 3); gc
/// removes an orphan.
#[test]
fn verify_reports_and_gc_removes() {
    let dir = scratch("verify");
    let inputs = brkt(&dir);
    let sha = sha256(&inputs.join("m_bracket.stp"));
    let blob = dir.join("parts/blobs/sha256").join(&sha[..2]).join(&sha);
    writable(&dir.join("parts/blobs"));
    let original = fs::read(&blob).unwrap();
    fs::write(&blob, b"other").unwrap();
    let (code, out, err) = catalog(&dir, &["verify", "-C", "parts"]);
    assert_eq!(code, 3);
    assert!(out.starts_with("damaged") && out.contains("BRKT"), "{out}");
    assert!(err.contains("1 problem"), "{err}");
    fs::write(&blob, &original).unwrap();
    let orphan = sha256(&inputs.join("m_bracket_thick.stp"));
    let path = dir
        .join("parts/blobs/sha256")
        .join(&orphan[..2])
        .join(&orphan);
    fs::copy(inputs.join("m_bracket_thick.stp"), &path).unwrap();
    // Abandoned long ago: a fresh one may be an ingest in flight.
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86_400))
        .unwrap();
    assert_eq!(catalog(&dir, &["verify", "-C", "parts"]).0, 3);
    let (code, _, err) = catalog(&dir, &["gc", "-C", "parts"]);
    assert_eq!(code, 0);
    assert!(err.contains("1 orphan files removed"), "{err}");
    assert!(!path.exists());
    assert_eq!(catalog(&dir, &["verify", "-C", "parts"]).0, 0);
}

/// The PR 3b review: replace checks its reference before the file (ID9),
/// takes a quarantined entry's own bytes as unchanged (CR4), and writes a
/// `fitment.catalog` document with `--format json` (CL3).
#[test]
fn replace_checks_its_reference_first() {
    let dir = scratch("replace-ref");
    brkt(&dir);
    catalog(
        &dir,
        &["replace", "-C", "parts", "BRKT", "in/m_bracket_thick.stp"],
    );
    let (code, _, err) = catalog(&dir, &["replace", "-C", "parts", "BRKT@r1", "in/junk.stp"]);
    assert_eq!(code, 2);
    assert!(err.contains("BRKT is at r2, not r1"), "{err}");
    assert_eq!(
        catalog(&dir, &["replace", "-C", "parts", "nosuch", "in/junk.stp"]).0,
        2
    );
    catalog(&dir, &["ingest", "-C", "parts", "in/junk.stp"]);
    let (code, out, _) = catalog(&dir, &["replace", "-C", "parts", "junk", "in/junk.stp"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("unchanged"), "{out}");
    let (code, json, err) = catalog(
        &dir,
        &[
            "replace",
            "-C",
            "parts",
            "BRKT",
            "in/m_bracket.stp",
            "--format",
            "json",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let doc: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(doc["changes"][0]["result"], "replaced");
    assert_eq!(doc["changes"][0]["revision"], 3);
    assert_eq!(doc["entries"][0]["revision"], 3);
    assert!(err.contains("BRKT replaced: r2 is now r3"), "{err}");
}

/// The PR 3b review: a content two pruned revisions name is counted once,
/// and the lines add up to the total.
#[test]
fn prune_counts_each_content_once() {
    let dir = scratch("prune-count");
    brkt(&dir);
    for next in ["in/m_bracket_thick.stp", "in/m_bracket.stp", "in/a.stp"] {
        let _ = catalog(&dir, &["replace", "-C", "parts", "BRKT", next]);
    }
    catalog(&dir, &["delete", "-C", "parts", "a"]);
    catalog(&dir, &["replace", "-C", "parts", "BRKT", "in/a.stp"]);
    let (code, out, err) = catalog(&dir, &["prune", "-C", "parts", "--revisions"]);
    assert_eq!(code, 0);
    let lines: u64 = out
        .lines()
        .filter_map(|l| l.split_whitespace().nth(4)?.parse::<u64>().ok())
        .sum();
    assert!(
        err.contains(&format!(", {lines} bytes released")),
        "{out}{err}"
    );
}

/// PH6 and VF1 (the PR 3b review): a hand-edited id is a damaged row to
/// replace and prune, never printed raw, and verify reports it.
#[test]
fn a_hand_edited_id_is_damage_everywhere() {
    let dir = scratch("edited-id");
    brkt(&dir);
    let catalog_dir = dir.join("parts");
    let raw =
        fitment_catalog::Catalog::open(&catalog_dir, fitment_catalog::Options::default()).unwrap();
    raw.connection()
        .execute(
            "UPDATE entry SET id = 'a' || char(27) || '[31m' WHERE id = 'a'",
            [],
        )
        .unwrap();
    drop(raw);
    let (code, out, err) = catalog(&dir, &["replace", "-C", "parts", "BRKT", "in/a.stp"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(!err.contains('\u{1b}') && !out.contains('\u{1b}'));
    let (code, out, _) = catalog(&dir, &["verify", "-C", "parts"]);
    assert_eq!(code, 3);
    assert!(
        out.starts_with("integrity") && !out.contains('\u{1b}'),
        "{out}"
    );
}

/// The bracket with stated properties, as `dir/stated.stp`: NX's
/// `MaterialMissingAssignments`, a name with a right-to-left override, and
/// a measure with its unit.
fn stated_bracket(dir: &Path) -> PathBuf {
    let text = fs::read_to_string(core("candidates/m_bracket.stp")).unwrap();
    let end = text.rfind("ENDSEC;").unwrap();
    let extra = "\
#90000=PROPERTY_DEFINITION('MaterialMissingAssignments','user defined attribute',#5);
#90001=PROPERTY_DEFINITION_REPRESENTATION(#90000,#90002);
#90002=REPRESENTATION('',(#90003),#2);
#90003=DESCRIPTIVE_REPRESENTATION_ITEM('MaterialMissingAssignments','TRUE');
#90010=PROPERTY_DEFINITION('evil\\X2\\202E\\X0\\name','user defined attribute',#5);
#90011=PROPERTY_DEFINITION_REPRESENTATION(#90010,#90012);
#90012=REPRESENTATION('',(#90013),#2);
#90013=DESCRIPTIVE_REPRESENTATION_ITEM('evil','M6x20');
#90020=PROPERTY_DEFINITION('Density','user defined attribute',#5);
#90021=PROPERTY_DEFINITION_REPRESENTATION(#90020,#90022);
#90022=REPRESENTATION('',(#90023),#2);
#90023=MEASURE_REPRESENTATION_ITEM('Density',POSITIVE_RATIO_MEASURE(1000.0),#9);
#90030=PROPERTY_DEFINITION('Fake','user defined attribute',#5);
#90031=PROPERTY_DEFINITION_REPRESENTATION(#90030,#90032);
#90032=REPRESENTATION('',(#90033),#2);
#90033=DESCRIPTIVE_REPRESENTATION_ITEM('Fake','FALSE''  -> material: unassigned');
";
    let path = dir.join("stated.stp");
    fs::write(&path, format!("{}{extra}{}", &text[..end], &text[end..])).unwrap();
    path
}

/// PR7 and M5-26 (CT18): `get` shows each stated property escaped, with
/// its meaning under the shipped map, and its document holds them; `list`
/// holds none, and shows an entry without a current row as
/// `stale (properties)`.
#[test]
fn get_shows_the_stated_properties() {
    let dir = scratch("properties");
    catalog(&dir, &["init", "-C", "parts"]);
    stated_bracket(&dir);
    assert_eq!(catalog(&dir, &["ingest", "-C", "parts", "stated.stp"]).0, 0);
    let (code, table, _) = catalog(&dir, &["get", "-C", "parts", "stated"]);
    assert_eq!(code, 0);
    for line in [
        "versions  extractor 6, signature 2, properties 1",
        "properties\n",
        "    #90000 'MaterialMissingAssignments' = 'TRUE'  -> material: unassigned\n",
        "    #90010 'evil\\u{202e}name' = 'M6x20'\n",
        "    #90020 'Density' = '1000.0'  POSITIVE_RATIO_MEASURE  unit #9\n",
        "    #90030 'Fake' = 'FALSE\\'  -> material: unassigned'\n",
    ] {
        assert!(table.contains(line), "{line:?} in\n{table}");
    }
    let (_, json, _) = catalog(&dir, &["get", "-C", "parts", "stated", "--format", "json"]);
    let doc: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(doc["build"]["properties_version"], 1);
    let entry = &doc["entries"][0];
    assert_eq!(entry["versions"]["properties_version"], 1);
    let stated = entry["properties"]["stated"].as_array().unwrap();
    assert_eq!(stated.len(), 4);
    assert_eq!(stated[3]["mapped"], Value::Null);
    assert_eq!(stated[0]["mapped"]["field"], "material");
    assert_eq!(stated[0]["mapped"]["meaning"], "unassigned");
    assert_eq!(stated[1]["name"], "evil\u{202e}name");
    assert_eq!(stated[1]["mapped"], Value::Null);
    assert_eq!(entry["properties"]["truncated"], false);

    let (_, json, _) = catalog(&dir, &["list", "-C", "parts", "--format", "json"]);
    let doc: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(doc["entries"][0]["properties"], Value::Null);

    assert!(delete_properties(&dir), "one properties row");
    let (_, table, err) = catalog(&dir, &["list", "-C", "parts", "--status", "stale"]);
    assert!(table.contains("stale (properties)"), "{table}");
    assert!(err.contains("1 stale"), "{err}");
    let (_, table, _) = catalog(&dir, &["get", "-C", "parts", "stated"]);
    assert!(
        table.contains("properties  - (not captured by this build"),
        "{table}"
    );
    assert!(!table.contains("#90000"), "{table}");
}

/// QR4's rule for a damaged properties row (the PR 5 reviews): `get` still
/// shows the entry and its history, says the row is damaged, and exits 1.
#[test]
fn a_damaged_properties_row_is_named_not_shown() {
    let dir = scratch("damaged-properties");
    catalog(&dir, &["init", "-C", "parts"]);
    stated_bracket(&dir);
    catalog(&dir, &["ingest", "-C", "parts", "stated.stp"]);
    let edited =
        fitment_catalog::Catalog::open(&dir.join("parts"), fitment_catalog::Options::default())
            .unwrap();
    edited
        .connection()
        .execute_batch(
            "DROP TRIGGER properties_immutable;
             UPDATE properties SET stated = CAST(replace(CAST(stated AS TEXT), 'TRUE', 'FALSE') AS BLOB);",
        )
        .unwrap();
    drop(edited);
    let (code, table, err) = catalog(&dir, &["get", "-C", "parts", "stated"]);
    assert_eq!(code, 1);
    assert!(
        table.contains("properties  damaged: not shown\n"),
        "{table}"
    );
    assert!(table.contains("history\n    r1"), "{table}");
    assert!(
        err.contains("stated: its stored properties are damaged"),
        "{err}"
    );
    let (code, json, _) = catalog(&dir, &["get", "-C", "parts", "stated", "--format", "json"]);
    assert_eq!(code, 1);
    let doc: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(doc["entries"][0]["properties"], Value::Null);
}

/// Deletes every properties row of the catalog in `dir/parts`.
fn delete_properties(dir: &Path) -> bool {
    let catalog =
        fitment_catalog::Catalog::open(&dir.join("parts"), fitment_catalog::Options::default())
            .unwrap();
    catalog
        .connection()
        .execute("DELETE FROM properties", [])
        .unwrap()
        == 1
}

/// Rewrites the catalog in `dir/parts` as signature version 1 wrote it:
/// each tokens row's version and preimage hash, and every posting.
fn at_signature_one(dir: &Path) {
    use fitment_catalog::{Sha256Hex, preimage};
    let catalog =
        fitment_catalog::Catalog::open(&dir.join("parts"), fitment_catalog::Options::default())
            .unwrap();
    let conn = catalog.connection();
    conn.execute_batch("PRAGMA foreign_keys = OFF; DROP TRIGGER tokens_immutable;")
        .unwrap();
    let rows: Vec<(String, u32, Vec<u8>)> = conn
        .prepare("SELECT sha256, extractor_version, tokens FROM tokens")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (sha, ev, tokens) in rows {
        let hash = preimage::tokens(&Sha256Hex::parse(&sha).unwrap(), ev, 1, &tokens);
        conn.execute(
            "UPDATE tokens SET signature_version = 1, tokens_sha256 = ?1 WHERE sha256 = ?2",
            [hash.as_str(), sha.as_str()],
        )
        .unwrap();
    }
    conn.execute_batch("UPDATE posting SET signature_version = 1")
        .unwrap();
}

/// CT8 and section 7.7: a stale catalog lists its entries stale; reindex
/// prints each transition and a summary, exits 0, and leaves nothing
/// stale; an unknown reference exits 2 before anything is done; a damaged
/// file exits 1 and stays stale; gc --versions counts what it removes.
#[test]
fn reindex_brings_a_stale_catalog_back() {
    let dir = scratch("reindex");
    let inputs = prototype(&dir);
    catalog(&dir, &["init", "-C", "parts"]);
    catalog(&dir, &["ingest", "-C", "parts", inputs.to_str().unwrap()]);
    at_signature_one(&dir);
    let (_, table, _) = catalog(&dir, &["list", "-C", "parts", "--status", "stale"]);
    assert!(table.contains("stale (signature 1)"), "{table}");

    let (code, _, err) = catalog(&dir, &["reindex", "-C", "parts", "nothing"]);
    assert_eq!(code, 2, "{err}");
    let (code, out, err) = catalog(&dir, &["reindex", "-C", "parts"]);
    assert_eq!(code, 0, "{err}");
    let line = out
        .lines()
        .find(|l| l.contains(" m_bracket "))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(line.starts_with("indexed "), "{line}");
    assert!(line.ends_with("stale (signature 1) -> indexed"), "{line}");
    assert!(
        err.contains(
            "2 reindexed: 2 indexed, 0 quarantined; 0 newly searched, 0 no longer searched"
        ),
        "{err}"
    );
    let (_, table, _) = catalog(&dir, &["list", "-C", "parts", "--status", "stale"]);
    assert!(!table.contains("stale"), "{table}");
    let (code, out, _) = catalog(
        &dir,
        &["reindex", "-C", "parts", "--all", "--format", "json"],
    );
    assert_eq!(code, 0);
    let doc: Value = serde_json::from_str(&out).unwrap();
    let changes = doc["changes"].as_array().unwrap();
    assert!(
        changes
            .iter()
            .all(|c| c["detail"].as_str().unwrap().ends_with("(nothing missing)"))
    );

    // A damaged file: reindex cannot rebuild it, and says so.
    let catalog_root = dir.join("parts");
    let conn =
        fitment_catalog::Catalog::open(&catalog_root, fitment_catalog::Options::default()).unwrap();
    let sha: String = conn
        .connection()
        .query_row("SELECT sha256 FROM live WHERE id = 'm_bracket'", [], |r| {
            r.get(0)
        })
        .unwrap();
    conn.connection()
        .execute_batch("DELETE FROM properties")
        .unwrap();
    drop(conn);
    let blob = catalog_root.join("blobs/sha256").join(&sha[..2]).join(&sha);
    writable(&catalog_root);
    fs::write(&blob, "not the bytes").unwrap();
    let (code, out, err) = catalog(&dir, &["reindex", "-C", "parts", "m_bracket"]);
    assert_eq!(code, 1, "{err}");
    assert!(out.starts_with("damaged "), "{out}");
    assert!(
        err.contains("m_bracket: damaged: the file's SHA-256 is not its name: left stale"),
        "{err}"
    );

    let (code, _, err) = catalog(&dir, &["gc", "-C", "parts", "--versions"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("rows of older versions removed"), "{err}");
}

/// RX6 (the PR 6 review): a part an older gate accepted and this one
/// refuses leaves the search loudly: its line, its reason on stderr, and
/// exit 3.
#[test]
fn a_part_that_leaves_the_search_exits_3() {
    use fitment_catalog::Sha256Hex;
    use fitment_catalog::preimage::{self, Status};
    let dir = scratch("leaves");
    let inputs = prototype(&dir);
    catalog(&dir, &["init", "-C", "parts"]);
    catalog(&dir, &["ingest", "-C", "parts", inputs.to_str().unwrap()]);
    let opened =
        fitment_catalog::Catalog::open(&dir.join("parts"), fitment_catalog::Options::default())
            .unwrap();
    let conn = opened.connection();
    let sha = |id: &str| -> String {
        conn.query_row("SELECT sha256 FROM live WHERE id = ?1", [id], |r| r.get(0))
            .unwrap()
    };
    // junk, as an older extractor that accepted it as a bracket stored it.
    let (junk, bracket) = (sha("junk"), sha("m_bracket"));
    let (digest, doc): (String, Vec<u8>) = conn
        .query_row(
            "SELECT part_digest, doc FROM features WHERE sha256 = ?1",
            [&bracket],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let old = fitment_core::EXTRACTOR_VERSION - 1;
    let hash = preimage::features(
        &Sha256Hex::parse(&junk).unwrap(),
        old,
        Status::Accepted,
        Some(&Sha256Hex::parse(&digest).unwrap()),
        &doc,
    );
    conn.execute("DELETE FROM features WHERE sha256 = ?1", [&junk])
        .unwrap();
    conn.execute(
        "INSERT INTO features SELECT ?1, ?2, status, encoding, doc, ?3, part_digest, product_id,
                product_name, holes, shafts, planes, cones, other_faces, complete, first_refusal
         FROM features WHERE sha256 = ?4",
        (junk.as_str(), old, hash.as_str(), bracket.as_str()),
    )
    .unwrap();
    drop(opened);
    let (code, out, err) = catalog(&dir, &["reindex", "-C", "parts"]);
    assert_eq!(code, 3, "{err}");
    let line = out.lines().find(|l| l.contains(" junk ")).unwrap();
    assert!(line.starts_with("quarantined "), "{line}");
    assert!(line.ends_with("(no longer searched)"), "{line}");
    assert!(err.contains("1 no longer searched"), "{err}");
    assert!(
        err.contains("junk: no longer searched: unparseable: "),
        "{err}"
    );
}

/// CL5: a reader that closes stdout early (`| head`) ends the output
/// quietly, and the command keeps its own exit code.
#[test]
fn a_closed_stdout_ends_output_quietly() {
    let dir = scratch("closed");
    let inputs = prototype(&dir);
    catalog(&dir, &["init", "-C", "parts"]);
    catalog(&dir, &["ingest", "-C", "parts", inputs.to_str().unwrap()]);
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("fitment"))
        .current_dir(&dir)
        .args(["catalog", "list", "-C", "parts", "--format", "json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Closed before the command can write.
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(!err.contains("error"), "{err}");
}

/// PH5: on Windows a catalog whose path is not Unicode is refused, since
/// SQLite needs UTF-8 there.
#[cfg(windows)]
#[test]
fn a_path_that_is_not_unicode_is_refused() {
    use std::os::windows::ffi::OsStringExt as _;
    let dir = scratch("unicode");
    // A lone surrogate: a valid Windows name, not Unicode.
    let name = std::ffi::OsString::from_wide(&[0x0063, 0xD800]);
    let root = dir.join(name);
    let error =
        fitment_catalog::Catalog::init(&root, fitment_catalog::Options::default()).unwrap_err();
    assert!(
        matches!(error, fitment_catalog::Error::NotUnicode(_)),
        "{error:?}"
    );
}

/// SZ4: `gc --vacuum` says what VACUUM needs before it runs it.
#[test]
fn vacuum_says_what_it_needs() {
    let dir = scratch("vacuum");
    let inputs = prototype(&dir);
    catalog(&dir, &["init", "-C", "parts"]);
    catalog(&dir, &["ingest", "-C", "parts", inputs.to_str().unwrap()]);
    let (code, _, err) = catalog(&dir, &["gc", "-C", "parts", "--vacuum"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("VACUUM needs free space about the database's size ("),
        "{err}"
    );
}

/// `actual` against `tests/golden/catalog/<name>`; written when missing
/// under `FITMENT_BLESS=1`, never rewritten: a changed golden is a
/// changed output, which the design must record first (VT39).
fn golden(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/catalog")
        .join(name);
    match fs::read_to_string(&path) {
        Ok(expected) => assert!(expected == actual, "{name}:\n{actual}\nis not\n{expected}"),
        Err(_) if std::env::var_os("FITMENT_BLESS").is_some_and(|v| v == "1") => {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, actual).unwrap();
        }
        Err(e) => panic!("{name}: no golden ({e}): run with FITMENT_BLESS=1"),
    }
}

/// CU1 (`docs/research/m6-api.md`, VT39): `delete` and `prune --revisions`
/// with `--format json` print `fitment.catalog` with `changes` (goldens),
/// each change's `input` the reference as given (delete) or the revision
/// pruned; the table, stderr and exit codes are as without it.
#[test]
fn delete_and_prune_print_their_documents() {
    let dir = scratch("cu1");
    brkt(&dir);
    catalog(
        &dir,
        &["replace", "-C", "parts", "BRKT", "in/m_bracket_thick.stp"],
    );
    catalog(&dir, &["ingest", "-C", "parts", "in/junk.stp"]);
    let twin = scratch("cu1-table");
    brkt(&twin);
    catalog(
        &twin,
        &["replace", "-C", "parts", "BRKT", "in/m_bracket_thick.stp"],
    );
    catalog(&twin, &["ingest", "-C", "parts", "in/junk.stp"]);
    let (code, out, err) = catalog(
        &dir,
        &["prune", "-C", "parts", "--revisions", "--format", "json"],
    );
    assert_eq!(code, 0);
    golden("prune.json", &out);
    let (table_code, table, table_err) = catalog(&twin, &["prune", "-C", "parts", "--revisions"]);
    assert_eq!((table_code, table_err), (code, err));
    assert!(table.starts_with("pruned       BRKT  r1"), "{table}");
    let delete = ["delete", "-C", "parts", "a", "JUNK@r1", "junk"];
    let (code, out, err) = catalog(&dir, &[&delete[..], &["--format", "json"]].concat());
    assert_eq!(code, 0);
    golden("delete.json", &out);
    let (table_code, table, table_err) = catalog(&twin, &delete);
    assert_eq!((table_code, table_err), (code, err));
    assert_eq!(table.lines().count(), 2, "{table}");
    // A refusal prints no document: exit 2, as before.
    let (code, out, _) = catalog(
        &dir,
        &["delete", "-C", "parts", "nobody", "--format", "json"],
    );
    assert_eq!((code, out.as_str()), (2, ""));
    let (code, out, _) = catalog(&dir, &["prune", "-C", "parts", "--format", "json"]);
    assert_eq!((code, out.as_str()), (2, ""));
}

/// Marks `file` as written long ago: abandoned, not in flight.
fn abandoned(file: &Path) {
    fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86_400))
        .unwrap();
}

/// CU2 and CA5 (VT39): `verify` and `gc` with `--format json` print
/// `fitment.verify` and `fitment.gc` (goldens), exit codes unchanged:
/// `verify` exits 3 on a problem, and its table is as before.
#[test]
fn verify_and_gc_print_their_documents() {
    let dir = scratch("cu2");
    let inputs = brkt(&dir);
    let (code, out, _) = catalog(&dir, &["verify", "-C", "parts", "--format", "json"]);
    assert_eq!(code, 0);
    golden("verify-clean.json", &out);
    let sha = sha256(&inputs.join("m_bracket.stp"));
    let blobs = dir.join("parts/blobs/sha256");
    writable(&blobs);
    fs::write(blobs.join(&sha[..2]).join(&sha), b"other").unwrap();
    let temporary = dir.join("parts/tmp/old.part");
    fs::write(&temporary, b"abandoned").unwrap();
    abandoned(&temporary);
    let (code, table, err) = catalog(&dir, &["verify", "-C", "parts"]);
    assert_eq!(code, 3);
    golden("verify.txt", &table);
    let (json_code, out, json_err) = catalog(&dir, &["verify", "-C", "parts", "--format", "json"]);
    assert_eq!((json_code, json_err), (code, err));
    golden("verify.json", &out);
    // gc: an orphan and the temporary file, both abandoned long ago.
    let bracket = fs::read(inputs.join("m_bracket.stp")).unwrap();
    fs::write(blobs.join(&sha[..2]).join(&sha), bracket).unwrap();
    let orphan = sha256(&inputs.join("m_bracket_thick.stp"));
    let path = blobs.join(&orphan[..2]).join(&orphan);
    fs::copy(inputs.join("m_bracket_thick.stp"), &path).unwrap();
    abandoned(&path);
    let (code, out, err) = catalog(
        &dir,
        &["gc", "-C", "parts", "--versions", "--format", "json"],
    );
    assert_eq!(code, 0);
    assert!(
        err.contains("1 orphan files removed, 1 temporary files removed"),
        "{err}"
    );
    golden("gc.json", &out);
    let (code, out, _) = catalog(&dir, &["gc", "-C", "parts"]);
    assert_eq!((code, out.as_str()), (0, ""), "the table prints nothing");
    assert_eq!(catalog(&dir, &["verify", "-C", "parts"]).0, 0);
}

/// CA4 (VT39): a file that cannot be read says its error's kind, not the
/// operating system's text, in the table and the document (goldens).
#[test]
fn an_unreadable_file_says_its_errors_kind() {
    let dir = scratch("ca4");
    catalog(&dir, &["init", "-C", "parts"]);
    let (code, table, _) = catalog(&dir, &["ingest", "-C", "parts", "nowhere.stp"]);
    assert_eq!(code, 1);
    golden("ingest-unreadable.txt", &table);
    let (code, out, _) = catalog(
        &dir,
        &["ingest", "-C", "parts", "nowhere.stp", "--format", "json"],
    );
    assert_eq!(code, 1);
    golden("ingest-unreadable.json", &out);
}
