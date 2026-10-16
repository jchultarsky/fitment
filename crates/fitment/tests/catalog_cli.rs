//! `fitment catalog init`, `ingest`, `list` and `get`, run as a user would
//! (`docs/research/m5-catalog.md`, section 7, CT15, CT16): exit codes,
//! lines, documents, references, and bytes handed out only checked (ST11).

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
    let (code, _, _) = catalog(
        &dir,
        &[
            "get",
            "-C",
            "parts",
            "m_bracket",
            "--format",
            "step",
            "-o",
            "parts/x.stp",
        ],
    );
    assert_eq!(code, 2);
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
/// controls never reach the terminal raw; JSON holds them exactly.
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

/// A catalog named `file:…` is a directory, never an SQLite URI.
#[test]
fn a_catalog_named_like_a_uri_is_a_directory() {
    let dir = scratch("uri");
    fs::create_dir_all(dir.join("cat")).unwrap();
    assert_eq!(catalog(&dir, &["init", "-C", "file:cat"]).0, 0);
    assert!(dir.join("file:cat/catalog.db").is_file());
    assert!(!dir.join("cat/catalog.db").exists());
    assert_eq!(catalog(&dir, &["list", "-C", "file:cat"]).0, 0);
}
