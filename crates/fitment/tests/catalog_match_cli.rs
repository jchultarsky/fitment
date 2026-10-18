//! `fitment match --catalog`, run as a user would
//! (`docs/research/m5-catalog.md`, section 7.12, CM1 to CM10, QR1; CT1,
//! CT2, CT7, CT16): its exits, its stderr line and header, its labels, and
//! every document it writes the file run's, digest for digest.

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

/// The catalog's generation: DB3 also cuts a batch after a second, so
/// the setup's one ingest of 13 files is one batch or more on a slow
/// runner, and the line names whichever generation it made.
fn generation(dir: &Path) -> u64 {
    let (_, out, _) = run(dir, &["catalog", "list", "-C", "parts", "--format", "json"]);
    serde_json::from_str::<Value>(&out).unwrap()["generation"]
        .as_u64()
        .unwrap()
}

fn json(text: &str) -> Value {
    let doc: Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        fitment_corpus::checker::problems(&doc),
        Vec::<String>::new()
    );
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
fn one_catalog_per_run() {
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
        &["match", "bracket.json", "in/m_bracket.stp", "--exhaustive"],
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
    let line = format!(
        "catalog parts  generation {}  entries 13: 13 searched, 0 quarantined",
        generation(&dir)
    );
    assert!(err.contains(&line), "{err}");
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
    let line = format!("catalog parts  generation {}  ", generation(&dir));
    assert!(lines[1].starts_with(&line), "{table}");
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
/// open question 3): one job, two or sixteen give the same document,
/// stderr and exit, for file runs and catalog runs; a file that cannot be
/// read is reported in the files' argument order.
#[test]
fn jobs_do_not_change_a_file_runs_bytes() {
    let dir = scratch("jobs");
    setup(&dir);
    let file_run = [
        "match",
        "bracket.json",
        "in/m_plate03.stp",
        "nowhere.stp",
        "in/m_bracket.stp",
        "in/m_bracket_lug.stp",
        "--format",
        "json",
    ];
    let catalog_run = [
        "match",
        "bracket.json",
        "-C",
        "parts",
        "--exhaustive",
        "--format",
        "json",
    ];
    for base in [&file_run[..], &catalog_run[..]] {
        let mut seen = Vec::new();
        for jobs in ["1", "2", "16"] {
            let mut args = base.to_vec();
            args.extend(["--jobs", jobs]);
            seen.push(run(&dir, &args));
        }
        assert!(seen.windows(2).all(|w| w[0] == w[1]), "{base:?}: {seen:?}");
    }
    let (code, _, err) = run(&dir, &file_run);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("nowhere.stp"), "{err}");
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

/// CR8 and CM10: quarantined entries are counted and pointed to; a
/// catalog table names each candidate's PRODUCT.id.
#[test]
fn quarantine_is_counted_and_products_shown() {
    let dir = scratch("cr8");
    setup(&dir);
    fs::write(dir.join("junk.stp"), "not STEP").unwrap();
    run(&dir, &["catalog", "ingest", "-C", "parts", "junk.stp"]);
    let (_, table, _) = run(&dir, &["match", "bracket.json", "-C", "parts"]);
    assert!(
        table.contains(
            "1 quarantined, not searched (fitment catalog list -C parts --status quarantined)"
        ),
        "{table}"
    );
    assert!(
        table.contains("catalog:m_bracket@r1  1b77ea97  'm_bracket'"),
        "{table}"
    );
}

/// Runs `sql` on the catalog in `dir/parts` through a raw connection.
fn edit(dir: &Path, sql: &str) {
    let catalog =
        fitment_catalog::Catalog::open(&dir.join("parts"), fitment_catalog::Options::default())
            .unwrap();
    catalog.connection().execute_batch(sql).unwrap();
}

/// CT16 (review F1): a part digest forged with its preimage hash
/// recomputed, as a bug at ingest would write it, is caught by the
/// recomputed digest (QR4): the entry is named, not judged, and the run
/// exits 1.
#[test]
fn a_part_digest_forged_with_its_hash_is_still_caught() {
    use fitment_catalog::{Sha256Hex, Status, preimage};
    let dir = scratch("forged");
    setup(&dir);
    let catalog =
        fitment_catalog::Catalog::open(&dir.join("parts"), fitment_catalog::Options::default())
            .unwrap();
    let conn = catalog.connection();
    let row = |id: &str| -> (String, Vec<u8>, String) {
        conn.query_row(
            "SELECT f.sha256, f.doc, f.part_digest FROM features f
             JOIN live l ON l.sha256 = f.sha256 WHERE l.id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };
    let (sha, doc, _) = row("m_bracket");
    let (_, _, other) = row("m_bracket_thick");
    let sha = Sha256Hex::parse(&sha).unwrap();
    let forged = Sha256Hex::parse(&other).unwrap();
    let hash = preimage::features(
        &sha,
        fitment_core::EXTRACTOR_VERSION,
        Status::Accepted,
        Some(&forged),
        &doc,
    );
    conn.execute_batch("DROP TRIGGER features_immutable")
        .unwrap();
    conn.execute(
        "UPDATE features SET part_digest = ?1, doc_sha256 = ?2 WHERE sha256 = ?3",
        [forged.as_str(), hash.as_str(), sha.as_str()],
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
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("m_bracket@r1: its stored rows are damaged"),
        "{err}"
    );
    assert!(!labels(&json(&out)).contains(&"catalog:m_bracket@r1".to_owned()));
}

/// CT16 (review F7, the risk recorded in section 4.12): a passing part's
/// postings lost, a full run omits it and finds nothing; --exhaustive
/// passes it; verify reports the damage.
#[test]
fn a_lost_posting_hides_a_part_until_verify() {
    let dir = scratch("lost");
    setup(&dir);
    edit(
        &dir,
        "PRAGMA foreign_keys = OFF;
         DELETE FROM posting WHERE content IN (SELECT b.key FROM blob b
           JOIN live l ON l.sha256 = b.sha256
           WHERE l.id IN ('m_bracket', 'm_bracket_lug', 'm_bracket_thick'));",
    );
    let (full, out, _) = run(&dir, &["match", "bracket.json", "-C", "parts"]);
    assert!(!out.contains("catalog:m_bracket@r1"), "{out}");
    let (exhaustive, out, _) = run(
        &dir,
        &["match", "bracket.json", "-C", "parts", "--exhaustive"],
    );
    assert!(out.contains("catalog:m_bracket@r1"), "{out}");
    assert_ne!(full, exhaustive);
    let (code, out, err) = run(&dir, &["catalog", "verify", "-C", "parts"]);
    assert_ne!(code, 0, "{out}{err}");
}

/// CT16 (review F7): a plate given the bracket's postings and tokens,
/// with their hash recomputed, as a bug at ingest would write them: level 1
/// admits it, and the judgment over its stored document still rejects it.
#[test]
fn a_corrupt_posting_never_passes_a_part() {
    use fitment_catalog::{Sha256Hex, preimage};
    let dir = scratch("corrupt-posting");
    setup(&dir);
    let catalog =
        fitment_catalog::Catalog::open(&dir.join("parts"), fitment_catalog::Options::default())
            .unwrap();
    let conn = catalog.connection();
    let row = |id: &str| -> (i64, String, Vec<u8>) {
        conn.query_row(
            "SELECT b.key, b.sha256, t.tokens FROM blob b JOIN live l ON l.sha256 = b.sha256
             JOIN tokens t ON t.sha256 = b.sha256 WHERE l.id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };
    let (good, good_sha, tokens) = row("m_bracket");
    let (plate, plate_sha, _) = row("m_plate03");
    let hash = preimage::tokens(
        &Sha256Hex::parse(&plate_sha).unwrap(),
        fitment_core::EXTRACTOR_VERSION,
        fitment_core::SIGNATURE_VERSION,
        &tokens,
    );
    conn.execute_batch("DROP TRIGGER tokens_immutable; PRAGMA foreign_keys = OFF;")
        .unwrap();
    conn.execute(
        "UPDATE tokens SET (tokens, tokens_sha256, lever_mm, planes, pairs_incomplete) =
           (SELECT ?1, ?2, lever_mm, planes, pairs_incomplete FROM tokens WHERE sha256 = ?3)
         WHERE sha256 = ?4",
        (
            &tokens[..],
            hash.as_str(),
            good_sha.as_str(),
            plate_sha.as_str(),
        ),
    )
    .unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO posting
           SELECT extractor_version, signature_version, sense, bin, ?1, stack, diameter_mm
           FROM posting WHERE content = ?2",
        [plate, good],
    )
    .unwrap();
    drop(catalog);
    let (_, out, err) = run(
        &dir,
        &["match", "bracket.json", "-C", "parts", "--format", "json"],
    );
    let doc = json(&out);
    let entry = doc["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["labels"][0] == "catalog:m_plate03@r1")
        .unwrap_or_else(|| panic!("level 1 admits it: {err}"));
    assert_eq!(entry["verdict"], "reject", "{entry}");
}
