//! Stated properties in the store (`docs/research/m5-catalog.md`, section
//! 4.13, PR3, M5-26, CT12, CT18): captured at ingest from the one buffer,
//! bound to their content by their preimage hash, checked by `verify`,
//! listed `stale (properties)` without a current row, and never read by a
//! match.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{catalog, core_tests, invariants, scratch};
use fitment_catalog::preimage;
use fitment_catalog::{
    Catalog, EntryStatus, Error, NotRun, ProblemKind, Reference, Selection, Sha256Hex,
};
use fitment_core::{
    Clearance, Grip, MatchOptions, MatchPolicy, Matcher, NoFitVerifier, PROPERTIES_VERSION,
    SocketDocument, StatedProperties, TolerancePolicy,
};
use rusqlite::{Connection, params};

/// The bracket with a user-defined property `#90000` on its product
/// definition, NX's `MaterialMissingAssignments`, written to `dir`.
fn stated_bracket(dir: &Path, name: &str, value: &str) -> PathBuf {
    let text = fs::read_to_string(core_tests().join("candidates/m_bracket.stp")).unwrap();
    let end = text.rfind("ENDSEC;").unwrap();
    let extra = format!(
        "#90000=PROPERTY_DEFINITION('MaterialMissingAssignments','user defined attribute',#5);\n\
         #90001=PROPERTY_DEFINITION_REPRESENTATION(#90000,#90002);\n\
         #90002=REPRESENTATION('',(#90003),#2);\n\
         #90003=DESCRIPTIVE_REPRESENTATION_ITEM('MaterialMissingAssignments','{value}');\n"
    );
    let path = dir.join(name);
    fs::write(&path, format!("{}{extra}{}", &text[..end], &text[end..])).unwrap();
    path
}

/// A catalog of the stated bracket and a refused file.
fn stated(test: &str) -> (Catalog, Sha256Hex) {
    let dir = scratch(test);
    let mut catalog = catalog(&dir);
    let bracket = stated_bracket(&dir, "bracket.stp", "TRUE");
    let junk = dir.join("junk.stp");
    fs::write(&junk, "not STEP").unwrap();
    let run = catalog.ingest(&[bracket, junk], None).unwrap();
    (catalog, run[0].sha256.clone().unwrap())
}

fn conn(catalog: &Catalog) -> Connection {
    Connection::open(catalog.root().join("catalog.db")).unwrap()
}

fn status(catalog: &mut Catalog, id: &str) -> Option<EntryStatus> {
    let snapshot = catalog.snapshot().unwrap();
    let named = snapshot
        .resolve(&Reference::parse(id).unwrap())
        .unwrap()
        .unwrap();
    snapshot.entry(&named).unwrap().status
}

/// PR3: an accepted file's properties are captured with it, a refused
/// file's are not, and a replace leaves none for the old content.
#[test]
fn ingest_captures_the_stated_properties() {
    let (mut catalog, sha) = stated("capture");
    {
        let snapshot = catalog.snapshot().unwrap();
        let read = snapshot.properties(&sha).unwrap().unwrap();
        assert_eq!(read.stated.len(), 1);
        assert_eq!(read.stated[0].instance, 90000);
        assert_eq!(read.stated[0].value, "TRUE");
        let listed = snapshot.list().unwrap();
        assert_eq!(listed[0].versions.2, Some(PROPERTIES_VERSION));
        assert_eq!(listed[0].status, Some(EntryStatus::Indexed));
        assert_eq!(listed[1].versions.2, None);
    }
    let refused: i64 = conn(&catalog)
        .query_row(
            "SELECT count(*) FROM properties p JOIN live l ON l.sha256 = p.sha256 WHERE l.id = 'junk'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(refused, 0);
    assert_eq!(invariants(catalog.root()), Vec::<String>::new());

    let dir = catalog.root().parent().unwrap().to_owned();
    let next = stated_bracket(&dir, "next.stp", "FALSE");
    let reference = Reference::parse("bracket").unwrap();
    catalog.replace(&reference, &next).unwrap().unwrap();
    assert_eq!(invariants(catalog.root()), Vec::<String>::new());
    let left: i64 = conn(&catalog)
        .query_row(
            "SELECT count(*) FROM properties WHERE sha256 = ?1",
            [sha.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0);
}

/// CT12 (`store.rs::each_preimage_field_is_bound` for properties): the
/// content key, the truncation flag and the stated bytes are each bound to
/// the hash; a row with one changed is damaged at the load and in plain
/// `verify`.
#[test]
fn each_properties_field_is_bound() {
    for (n, set) in [
        "truncated = 1",
        "stated = CAST(replace(CAST(stated AS TEXT), 'TRUE', 'FALSE') AS BLOB)",
    ]
    .into_iter()
    .enumerate()
    {
        let (mut catalog, sha) = stated(&format!("bound-{n}"));
        let conn = conn(&catalog);
        conn.execute_batch("DROP TRIGGER properties_immutable")
            .unwrap();
        conn.execute(
            &format!("UPDATE properties SET {set} WHERE sha256 = ?1"),
            [sha.as_str()],
        )
        .unwrap();
        drop(conn);
        {
            let snapshot = catalog.snapshot().unwrap();
            assert!(
                matches!(snapshot.properties(&sha), Err(Error::Corrupt(_))),
                "{set}"
            );
        }
        let problems = catalog.verify(false, false).unwrap();
        assert!(
            problems
                .iter()
                .any(|p| p.kind == ProblemKind::Damaged && p.what == sha.as_str()),
            "{set}: {problems:?}"
        );
    }
    // A row copied under another content's key fails its hash.
    let (mut catalog, sha) = stated("bound-key");
    let dir = catalog.root().parent().unwrap().to_owned();
    let other = common::copy("data/p01_through.stp", &dir, "p01.stp");
    let run = catalog.ingest(&[other], None).unwrap();
    let other = run[0].sha256.clone().unwrap();
    let conn = conn(&catalog);
    conn.execute("DELETE FROM properties WHERE sha256 = ?1", [other.as_str()])
        .unwrap();
    conn.execute(
        "INSERT INTO properties SELECT ?2, properties_version, truncated, stated, stated_sha256
         FROM properties WHERE sha256 = ?1",
        params![sha.as_str(), other.as_str()],
    )
    .unwrap();
    drop(conn);
    let snapshot = catalog.snapshot().unwrap();
    assert!(matches!(
        snapshot.properties(&other),
        Err(Error::Corrupt(_))
    ));
    assert!(snapshot.properties(&sha).unwrap().is_some());
}

/// M5-26 and RX2: without a current properties row an entry lists as
/// `stale (properties)`, and a full run neither refuses nor skips it; a
/// row of another version, hashed as its own, is no problem for `verify`.
#[test]
fn stale_properties_never_refuse_a_match() {
    let (mut catalog, sha) = stated("stale");
    let conn = conn(&catalog);
    let (truncated, stated): (bool, Vec<u8>) = conn
        .query_row(
            "SELECT truncated, stated FROM properties WHERE sha256 = ?1",
            [sha.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    conn.execute("DELETE FROM properties WHERE sha256 = ?1", [sha.as_str()])
        .unwrap();
    drop(conn);
    assert_eq!(
        status(&mut catalog, "bracket"),
        Some(EntryStatus::StaleProperties(0))
    );
    let other = PROPERTIES_VERSION + 1;
    let hash = preimage::properties(&sha, other, truncated, &stated);
    self::conn(&catalog)
        .execute(
            "INSERT INTO properties VALUES (?1, ?2, ?3, ?4, ?5)",
            params![sha.as_str(), other, truncated, stated, hash.as_str()],
        )
        .unwrap();
    assert_eq!(
        status(&mut catalog, "bracket"),
        Some(EntryStatus::StaleProperties(other))
    );
    assert_eq!(catalog.verify(false, false).unwrap(), Vec::new());

    let bytes = fs::read(core_tests().join("assemblies/s01_bolted.stp")).unwrap();
    let socket = SocketDocument::extract(&bytes, Some("BRACKET"), None).unwrap();
    let policy = MatchPolicy::new(TolerancePolicy::default(), Clearance::Strict, Grip::Hard);
    let matcher = Matcher::new(&socket, policy, MatchOptions::default(), &NoFitVerifier).unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let run = snapshot
        .match_catalog(&matcher, &Selection::Full, 1)
        .unwrap();
    let run = match run {
        Ok(run) => run,
        Err(refused @ NotRun::Stale { .. }) => panic!("refused: {refused:?}"),
        Err(other) => panic!("{other:?}"),
    };
    assert_eq!(run.indexed, 1);
    let labels: Vec<&str> = run.results.iter().map(|(_, l)| l[0].as_str()).collect();
    assert_eq!(labels, ["catalog:bracket@r1"]);
}

/// CT12: a stated set rewritten with its hash recomputed, as a bug at
/// ingest would write it, is `--deep`'s to find: plain `verify` passes it.
#[test]
fn only_deep_finds_a_rewritten_capture() {
    let (mut catalog, sha) = stated("deep");
    let forged = StatedProperties::default().to_json();
    let hash = preimage::properties(&sha, PROPERTIES_VERSION, false, forged.as_bytes());
    let conn = conn(&catalog);
    conn.execute_batch("DROP TRIGGER properties_immutable")
        .unwrap();
    conn.execute(
        "UPDATE properties SET stated = ?2, stated_sha256 = ?3 WHERE sha256 = ?1",
        params![sha.as_str(), forged.as_bytes(), hash.as_str()],
    )
    .unwrap();
    drop(conn);
    assert_eq!(catalog.verify(false, false).unwrap(), Vec::new());
    let deep = catalog.verify(true, false).unwrap();
    assert!(
        deep.iter()
            .any(|p| p.kind == ProblemKind::Mismatch && p.detail.starts_with("properties")),
        "{deep:?}"
    );
}
