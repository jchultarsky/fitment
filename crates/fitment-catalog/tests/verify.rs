//! `verify` (`docs/research/m5-catalog.md`, VF1 to VF4, CT12): a clean
//! catalog has no problem; each kind of damage is found by plain `verify`,
//! save a row consistent in itself but wrong for its bytes, which only
//! `--deep` finds; a key no path may be built from is reported without
//! touching the file system.

mod common;

use std::fs;
use std::path::Path;

use common::{catalog, copy, scratch};
use fitment_catalog::{Catalog, ProblemKind, Sha256Hex, preimage};
use rusqlite::{Connection, params};

/// A catalog holding the bracket, with its SHA-256.
fn one(test: &str) -> (Catalog, Sha256Hex) {
    let dir = scratch(test);
    let mut catalog = catalog(&dir);
    let file = copy("candidates/m_bracket.stp", &dir, "m_bracket.stp");
    let run = catalog.ingest(&[file], None).unwrap();
    let sha = run[0].sha256.clone().unwrap();
    assert_eq!(catalog.verify(true, true).unwrap(), Vec::new());
    (catalog, sha)
}

/// A raw connection with the insert-only triggers off.
fn raw(root: &Path) -> Connection {
    let conn = Connection::open(root.join("catalog.db")).unwrap();
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS features_immutable; DROP TRIGGER IF EXISTS tokens_immutable;",
    )
    .unwrap();
    conn
}

fn kinds(catalog: &mut Catalog, deep: bool) -> Vec<(ProblemKind, String)> {
    catalog
        .verify(deep, false)
        .unwrap()
        .into_iter()
        .map(|p| (p.kind, p.detail))
        .collect()
}

/// CT12, `verify.rs::plain_verify_checks_preimages_digests_and_postings`
/// (review F1, F2, F7): one catalog with a features row moved to another
/// key, a part digest forged with its hash recomputed, and a posting
/// changed; plain `verify` reports all three.
#[test]
fn plain_verify_checks_preimages_digests_and_postings() {
    let dir = scratch("plain");
    let mut catalog = catalog(&dir);
    let files = [
        copy("candidates/m_bracket.stp", &dir, "x.stp"),
        copy("data/p01_through.stp", &dir, "y.stp"),
        copy("data/p03_blind_flat.stp", &dir, "z.stp"),
    ];
    let run = catalog.ingest(&files, None).unwrap();
    let [x, y, z] = [0, 1, 2].map(|k| run[k].sha256.clone().unwrap());
    let conn = raw(catalog.root());
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    // Y's row replaced by a copy of X's under Y's key.
    conn.execute("DELETE FROM features WHERE sha256 = ?1", [y.as_str()])
        .unwrap();
    conn.execute(
        "INSERT INTO features SELECT ?2, extractor_version, status, encoding, doc, doc_sha256,
                part_digest, product_id, product_name, holes, shafts, planes, cones, other_faces,
                complete, first_refusal
         FROM features WHERE sha256 = ?1",
        params![x.as_str(), y.as_str()],
    )
    .unwrap();
    // Z's digest forged, its preimage hash recomputed.
    let forged = Sha256Hex::of(b"another part");
    let doc: Vec<u8> = conn
        .query_row(
            "SELECT doc FROM features WHERE sha256 = ?1",
            [z.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    let hash = preimage::features(
        &z,
        fitment_core::EXTRACTOR_VERSION,
        preimage::Status::Accepted,
        Some(&forged),
        &doc,
    );
    conn.execute(
        "UPDATE features SET part_digest = ?1, doc_sha256 = ?2 WHERE sha256 = ?3",
        params![forged.as_str(), hash.as_str(), z.as_str()],
    )
    .unwrap();
    // One of X's postings moved.
    conn.execute(
        "UPDATE posting SET diameter_mm = diameter_mm + 1
         WHERE content = (SELECT key FROM blob WHERE sha256 = ?1) AND stack = 0",
        [x.as_str()],
    )
    .unwrap();
    drop(conn);
    let found = catalog.verify(false, false).unwrap();
    let has = |kind: ProblemKind, sha: &Sha256Hex, text: &str| {
        found
            .iter()
            .any(|p| p.kind == kind && p.what == sha.as_str() && p.detail.contains(text))
    };
    assert!(
        has(ProblemKind::Damaged, &y, "the row's hash does not cover"),
        "{found:?}"
    );
    assert!(
        has(
            ProblemKind::Mismatch,
            &z,
            "part_digest is not the document's"
        ),
        "{found:?}"
    );
    assert!(
        has(ProblemKind::Mismatch, &x, "postings (signature 2)"),
        "{found:?}"
    );
}

/// CT12, `verify.rs::plain_verify_recomputes_postings` (review F7): a
/// posting's diameter moved by one ulp, one deleted, one added for a stack
/// that has none: each a `mismatch`.
#[test]
fn plain_verify_recomputes_postings() {
    for (n, edit) in [
        "UPDATE posting SET diameter_mm = CAST(diameter_mm AS REAL) + 1e-12 WHERE stack = 0",
        "DELETE FROM posting WHERE stack = 1",
        "INSERT INTO posting SELECT extractor_version, signature_version, sense, bin, content,
         99, diameter_mm FROM posting WHERE stack = 0",
    ]
    .iter()
    .enumerate()
    {
        let (mut catalog, _) = one(&format!("postings{n}"));
        raw(catalog.root()).execute_batch(edit).unwrap();
        let found = kinds(&mut catalog, false);
        assert!(
            found
                .iter()
                .any(|(k, d)| *k == ProblemKind::Mismatch && d.starts_with("postings")),
            "{edit}: {found:?}"
        );
    }
}

/// CT12: a features row moved to another key, a blob with a byte
/// appended, an orphan, an old temporary file and a document edited.
#[test]
fn plain_verify_finds_damage_in_rows_and_files() {
    let (mut catalog, sha) = one("damage");
    let root = catalog.root().to_owned();
    let blob = catalog.blob_path(&sha);
    common::make_writable(&root.join("blobs"));
    let mut bytes = fs::read(&blob).unwrap();
    bytes.push(b'\n');
    fs::write(&blob, &bytes).unwrap();
    fs::write(catalog.blob_path(&Sha256Hex::of(b"orphan")), b"orphan").unwrap();
    let conn = raw(&root);
    conn.execute(
        "UPDATE features SET doc = CAST(replace(CAST(doc AS TEXT), '\"holes\"', '\"holes\" ') AS BLOB)",
        [],
    )
    .unwrap();
    drop(conn);
    let found = kinds(&mut catalog, false);
    let has = |kind: ProblemKind| found.iter().any(|(k, _)| *k == kind);
    assert!(has(ProblemKind::Damaged), "{found:?}");
    assert!(has(ProblemKind::Orphan), "{found:?}");
    assert!(
        found
            .iter()
            .any(|(_, d)| d.contains("the row's hash does not cover")),
        "{found:?}"
    );
}

/// VF2 and CT12: a stored document edited, its preimage hash and part
/// digest recomputed, passes plain `verify` and fails `--deep`.
#[test]
fn only_deep_finds_a_consistent_forgery() {
    let (mut catalog, sha) = one("deep");
    let root = catalog.root().to_owned();
    let conn = raw(&root);
    let doc: Vec<u8> = conn
        .query_row(
            "SELECT doc FROM features WHERE sha256 = ?1",
            [sha.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    // A forgery core accepts: the part's PRODUCT.name rewritten.
    let mut value: serde_json::Value = serde_json::from_slice(&doc).unwrap();
    value["part"]["definition"]["product"]["name"] = "forged".into();
    let edited = fitment_core::FeaturesDocument::from_json(&value.to_string()).unwrap();
    let text = edited.to_compact_json();
    let digest = Sha256Hex::parse(&edited.part_digest().unwrap()).unwrap();
    let hash = preimage::features(
        &sha,
        fitment_core::EXTRACTOR_VERSION,
        preimage::Status::Accepted,
        Some(&digest),
        text.as_bytes(),
    );
    conn.execute(
        "UPDATE features SET doc = ?1, doc_sha256 = ?2, part_digest = ?3 WHERE sha256 = ?4",
        params![
            text.as_bytes(),
            hash.as_str(),
            digest.as_str(),
            sha.as_str()
        ],
    )
    .unwrap();
    drop(conn);
    assert_eq!(kinds(&mut catalog, false), Vec::new());
    let deep = kinds(&mut catalog, true);
    assert!(
        deep.iter()
            .any(|(k, d)| *k == ProblemKind::Mismatch && d.contains("not the file's")),
        "{deep:?}"
    );
}

/// PH1: a revision's content of `../../x`, written with the checks off, is
/// reported, and no path is built from it.
#[test]
fn a_key_that_is_no_sha256_builds_no_path() {
    let (mut catalog, _) = one("path");
    let conn = raw(catalog.root());
    conn.execute_batch(
        "PRAGMA ignore_check_constraints = ON;
         UPDATE revision SET sha256 = '../../x';",
    )
    .unwrap();
    drop(conn);
    let found = kinds(&mut catalog, false);
    assert!(
        found.iter().any(|(k, _)| *k == ProblemKind::Integrity),
        "{found:?}"
    );
}
