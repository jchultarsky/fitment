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

/// CT12: a blob with a byte appended, an abandoned orphan and temporary
/// file, and a document edited are each reported; a fresh orphan, maybe an
/// ingest in flight, is not.
#[test]
fn plain_verify_finds_damage_in_rows_and_files() {
    let (mut catalog, sha) = one("damage");
    let root = catalog.root().to_owned();
    let blob = catalog.blob_path(&sha);
    common::make_writable(&root.join("blobs"));
    let mut bytes = fs::read(&blob).unwrap();
    bytes.push(b'\n');
    fs::write(&blob, &bytes).unwrap();
    let orphan = Sha256Hex::of(b"orphan");
    fs::write(catalog.blob_path(&orphan), b"orphan").unwrap();
    let fresh = Sha256Hex::of(b"fresh");
    fs::write(catalog.blob_path(&fresh), b"fresh").unwrap();
    common::age(&catalog.blob_path(&orphan));
    let temporary = root.join("tmp/7-7.part");
    fs::write(&temporary, b"x").unwrap();
    common::age(&temporary);
    let conn = raw(&root);
    conn.execute(
        "UPDATE features SET doc = CAST(replace(CAST(doc AS TEXT), '\"holes\"', '\"holes\" ') AS BLOB)",
        [],
    )
    .unwrap();
    drop(conn);
    let found = catalog.verify(false, false).unwrap();
    let has = |kind: ProblemKind, what: &str, text: &str| {
        found
            .iter()
            .any(|p| p.kind == kind && p.what == what && p.detail.contains(text))
    };
    assert!(
        has(
            ProblemKind::Damaged,
            sha.as_str(),
            "SHA-256 is not its name"
        ),
        "{found:?}"
    );
    assert!(
        has(
            ProblemKind::Damaged,
            sha.as_str(),
            "the row's hash does not cover"
        ),
        "{found:?}"
    );
    assert!(
        has(ProblemKind::Orphan, orphan.as_str(), "no row names"),
        "{found:?}"
    );
    assert!(
        has(ProblemKind::Temporary, "tmp/7-7.part", "over an hour"),
        "{found:?}"
    );
    assert!(!found.iter().any(|p| p.what == fresh.as_str()), "{found:?}");
}

/// IN9 and VF1 (the PR 3a reviews): postings of a kept content, and
/// postings with no tokens row, are each a `mismatch`.
#[test]
fn stray_postings_are_reported() {
    let dir = scratch("stray");
    let mut catalog = catalog(&dir);
    let x = copy("candidates/m_bracket.stp", &dir, "x.stp");
    let thick = copy("candidates/m_bracket_thick.stp", &dir, "thick.stp");
    let y = copy("data/p01_through.stp", &dir, "y.stp");
    catalog.ingest(&[x, y], None).unwrap();
    catalog
        .replace(&fitment_catalog::Reference::parse("x").unwrap(), &thick)
        .unwrap()
        .unwrap();
    let conn = raw(catalog.root());
    conn.execute_batch(
        "INSERT INTO posting VALUES (6, 2, 0, 66, (SELECT b.key FROM blob b
            WHERE b.sha256 NOT IN (SELECT sha256 FROM live)), 0, 6.6);
         DELETE FROM tokens WHERE sha256 = (SELECT sha256 FROM live WHERE id = 'y');",
    )
    .unwrap();
    drop(conn);
    let found = kinds(&mut catalog, false);
    assert!(
        found
            .iter()
            .any(|(_, d)| d.contains("derived rows for content no current")),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|(_, d)| d.contains("postings without their tokens row")),
        "{found:?}"
    );
}

/// A version column out of `u32`'s range is reported, and the rest of the
/// catalog is still checked.
#[test]
fn a_wide_version_is_reported_not_fatal() {
    let (mut catalog, _) = one("wide");
    raw(catalog.root())
        .execute_batch(
            "PRAGMA foreign_keys = OFF; UPDATE features SET extractor_version = 4294967296;
             UPDATE clock SET generation = generation + 5;",
        )
        .unwrap();
    let found = kinds(&mut catalog, false);
    assert!(
        found
            .iter()
            .any(|(k, d)| *k == ProblemKind::Integrity && d.contains("a version column")),
        "{found:?}"
    );
    assert!(
        found.iter().any(|(_, d)| d.contains("but the last op")),
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

/// CA4 (`docs/research/m6-api.md`, VT38): no problem names an absolute
/// path or a library error's text. A missing fan-out directory is named
/// as the catalog's root names it (`blobs/sha256/ab`, absolute before);
/// a blob that cannot be read says its error's kind.
#[test]
fn no_problem_names_an_absolute_path() {
    let (mut catalog, sha) = one("absolute");
    let root = catalog.root().to_owned();
    let fan = if sha.fan_out() == "00" { "01" } else { "00" };
    fs::remove_dir(root.join("blobs/sha256").join(fan)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let blob = root
            .join("blobs/sha256")
            .join(sha.fan_out())
            .join(sha.as_str());
        fs::set_permissions(&blob, fs::Permissions::from_mode(0o000)).unwrap();
    }
    let problems = catalog.verify(false, false).unwrap();
    let absolute = root.to_string_lossy();
    let parent = root.parent().unwrap().to_string_lossy();
    for p in &problems {
        let text = format!("{} {}", p.what, p.detail);
        assert!(!text.contains(absolute.as_ref()), "{p:?}");
        assert!(!text.contains(parent.as_ref()), "{p:?}");
    }
    let missing: Vec<&str> = problems
        .iter()
        .filter(|p| p.kind == ProblemKind::Missing)
        .map(|p| p.what.as_str())
        .collect();
    assert_eq!(missing, [format!("blobs/sha256/{fan}")]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let unreadable: Vec<&str> = problems
            .iter()
            .filter(|p| p.what == sha.as_str())
            .map(|p| p.detail.as_str())
            .collect();
        assert_eq!(unreadable, ["cannot read the file: io: permission denied"]);
        let blob = root
            .join("blobs/sha256")
            .join(sha.fan_out())
            .join(sha.as_str());
        fs::set_permissions(&blob, fs::Permissions::from_mode(0o444)).unwrap();
    }
}
