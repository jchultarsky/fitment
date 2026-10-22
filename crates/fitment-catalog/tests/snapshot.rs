//! What a snapshot reads (`docs/research/m5-catalog.md`, QR4, CT7, CT12):
//! a stored document is used only when its row's preimage hash binds every
//! key and status column to its bytes; a row moved to another key fails;
//! and no order depends on SQLite's unordered selects.

mod common;

use std::path::Path;

use common::{catalog, copy, scratch};
use fitment_catalog::document::{CatalogDocument, Entry};
use fitment_catalog::{BadName, BadRow, Catalog, Error, Reference, Sha256Hex};
use rusqlite::{Connection, params};

/// A catalog holding the bracket and p01, with their SHA-256s.
fn two(test: &str) -> (Catalog, Sha256Hex, Sha256Hex) {
    let dir = scratch(test);
    let mut catalog = catalog(&dir);
    let x = copy("candidates/m_bracket.stp", &dir, "m_bracket.stp");
    let y = copy("data/p01_through.stp", &dir, "p01.stp");
    let run = catalog.ingest(&[x, y], None).unwrap();
    let x = run[0].sha256.clone().unwrap();
    let y = run[1].sha256.clone().unwrap();
    (catalog, x, y)
}

/// Rewrites X's features row through a raw connection, the insert-only
/// trigger aside: `set` is the `SET` list that changes it.
fn rewrite(root: &Path, sha: &Sha256Hex, set: &str) {
    let conn = Connection::open(root.join("catalog.db")).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF; DROP TRIGGER features_immutable;")
        .unwrap();
    conn.execute(
        "CREATE TEMP TABLE kept AS SELECT * FROM features WHERE sha256 = ?1",
        [sha.as_str()],
    )
    .unwrap();
    conn.execute(&format!("UPDATE kept SET {set}"), []).unwrap();
    conn.execute("DELETE FROM features WHERE sha256 = ?1", [sha.as_str()])
        .unwrap();
    conn.execute_batch("INSERT INTO features SELECT * FROM kept; DROP TABLE kept;")
        .unwrap();
}

fn corrupt(catalog: &mut Catalog, sha: &Sha256Hex) -> bool {
    let snapshot = catalog.snapshot().unwrap();
    matches!(snapshot.features(sha).unwrap(), Err(BadRow::Corrupt(_)))
}

/// CT12 (review F2), `store.rs::each_preimage_field_is_bound` at the load
/// check: changing any one key or preimage column of a features row, the
/// hash kept, makes the row unusable.
#[test]
fn each_preimage_field_is_bound() {
    let (mut catalog, x, y) = two("bound");
    let root = catalog.root().to_owned();
    {
        let snapshot = catalog.snapshot().unwrap();
        assert!(snapshot.features(&x).unwrap().is_ok());
    }
    let other = "ab".repeat(32);
    for set in [
        format!("part_digest = '{other}'"),
        "status = 'refused', part_digest = NULL, first_refusal = 'x'".to_owned(),
        "doc = CAST(replace(CAST(doc AS TEXT), '\"holes\"', '\"holes\" ') AS BLOB)".to_owned(),
    ] {
        let (mut fresh, x, _) = two(&format!("bound-{}", set.len()));
        let root = fresh.root().to_owned();
        rewrite(&root, &x, &set);
        assert!(corrupt(&mut fresh, &x), "{set}");
    }
    // The version column: no row of this build's version is left, so the
    // entry is stale, never judged as this build's read.
    rewrite(&root, &y, "extractor_version = extractor_version - 1");
    let snapshot = catalog.snapshot().unwrap();
    assert!(matches!(
        snapshot.features(&y).unwrap(),
        Err(BadRow::Stale { .. })
    ));
}

/// CT12 (review F2): X's features row copied under Y's key, Y's own
/// removed, fails its hash.
#[test]
fn a_features_row_copied_to_another_key_fails_its_hash() {
    let (mut catalog, x, y) = two("copied");
    let conn = Connection::open(catalog.root().join("catalog.db")).unwrap();
    conn.execute("DELETE FROM features WHERE sha256 = ?1", [y.as_str()])
        .unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF; DROP TRIGGER tokens_accepted_only;")
        .unwrap();
    conn.execute(
        "INSERT INTO features SELECT ?2, extractor_version, status, encoding, doc, doc_sha256,
                part_digest, product_id, product_name, holes, shafts, planes, cones, other_faces,
                complete, first_refusal
         FROM features WHERE sha256 = ?1",
        params![x.as_str(), y.as_str()],
    )
    .unwrap();
    drop(conn);
    assert!(corrupt(&mut catalog, &y));
    assert!(!corrupt(&mut catalog, &x));
}

/// CT7: with SQLite's unordered selects reversed, every read gives the same
/// answer.
#[test]
fn no_read_depends_on_unordered_selects() {
    let dir = scratch("order");
    let mut catalog = catalog(&dir);
    let inputs = [
        copy("candidates/m_bracket.stp", &dir, "Bracket.stp"),
        copy("data/p01_through.stp", &dir, "a.stp"),
        copy("data/p03_blind_flat.stp", &dir, "A_2.stp"),
        copy("data/r04_assembly.stp", &dir, "b.stp"),
    ];
    catalog.ingest(&inputs, None).unwrap();
    let read = |catalog: &mut Catalog| {
        let snapshot = catalog.snapshot().unwrap();
        let list = snapshot.list().unwrap();
        let unknown = snapshot.resolve(&Reference::parse("a_").unwrap()).unwrap();
        let bracket = snapshot
            .resolve(&Reference::parse("bracket").unwrap())
            .unwrap()
            .unwrap();
        let history = snapshot.history(&bracket).unwrap();
        (list, unknown, history)
    };
    let plain = read(&mut catalog);
    catalog
        .connection()
        .execute_batch("PRAGMA reverse_unordered_selects = ON")
        .unwrap();
    assert_eq!(read(&mut catalog), plain);
    let ids: Vec<&str> = plain.0.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(ids, ["a", "A_2", "b", "Bracket"]);
}

/// A hand-edited id or revision is a damaged row: never printed raw, never
/// shown as another value (PH6, the PR 2b review).
#[test]
fn a_hand_edited_id_or_revision_is_corrupt() {
    for edit in [
        "UPDATE entry SET id = 'm_bracket' || char(27) || '[31m'",
        "UPDATE revision SET revision = 4294967297",
    ] {
        let (mut catalog, _, _) = two(&format!("edited-{}", edit.len()));
        let conn = Connection::open(catalog.root().join("catalog.db")).unwrap();
        conn.execute_batch(&format!("PRAGMA ignore_check_constraints = ON; {edit}"))
            .unwrap();
        let snapshot = catalog.snapshot().unwrap();
        assert!(matches!(snapshot.list(), Err(Error::Corrupt(_))), "{edit}");
    }
}

/// ID7: a content prefix two live entries share names neither.
#[test]
fn a_shared_content_prefix_is_ambiguous() {
    let (mut catalog, x, _) = two("ambiguous");
    let twin = format!("{}{}", &x.as_str()[..12], "0".repeat(52));
    let conn = Connection::open(catalog.root().join("catalog.db")).unwrap();
    conn.execute_batch(&format!(
        "INSERT INTO name VALUES ('twin', 1);
         INSERT INTO entry (id, id_fold, created_generation) VALUES ('twin', 'twin', 2);
         INSERT INTO revision (entry, id_fold, revision, sha256, added_generation)
           VALUES (last_insert_rowid(), 'twin', 1, '{twin}', 2);"
    ))
    .unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let asked = Reference::parse(&format!("sha256:{}", &x.as_str()[..12])).unwrap();
    let Ok(Err(BadName::Ambiguous { ids, .. })) = snapshot.resolve(&asked) else {
        panic!("not ambiguous");
    };
    assert_eq!(ids.len(), 2);
}

/// ID6 and ID9 (the PR 2b review's F2): a revision of an earlier, deleted
/// entry of an id ingested again is shown as deleted, without bytes, under
/// its own entry's id; the live entry's revision is the id's current one.
#[test]
fn a_deleted_entrys_revision_is_shown_as_deleted() {
    let (mut catalog, x, _) = two("reborn");
    let conn = Connection::open(catalog.root().join("catalog.db")).unwrap();
    // As PR 3's delete will: tombstone the entry, supersede its revision;
    // then the same bytes under the id again, as r2 of a new entry.
    conn.execute_batch(&format!(
        "UPDATE clock SET generation = 3;
         UPDATE entry SET deleted_generation = 3 WHERE id = 'm_bracket';
         UPDATE revision SET superseded_generation = 3 WHERE id_fold = 'm_bracket';
         UPDATE name SET last_revision = 2 WHERE id_fold = 'm_bracket';
         INSERT INTO entry (id, id_fold, created_generation) VALUES ('M_Bracket', 'm_bracket', 3);
         INSERT INTO revision (entry, id_fold, revision, sha256, added_generation)
           VALUES (last_insert_rowid(), 'm_bracket', 2, '{x}', 3);"
    ))
    .unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let old = snapshot
        .resolve(&Reference::parse("m_bracket@r1").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        (old.id.as_str(), old.current, old.id_current),
        ("m_bracket", None, Some(2))
    );
    let listed = snapshot.entry(&old).unwrap();
    assert_eq!(listed.bytes, None);
    assert!(!listed.current);
    let now = snapshot
        .resolve(&Reference::parse("m_bracket").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        (now.id.as_str(), now.revision, now.current),
        ("M_Bracket", 2, Some(2))
    );
    assert!(snapshot.entry(&now).unwrap().bytes.is_some());
}

/// VN3's keyset pages, for PR 7b's entry lists (`docs/research/m6-api.md`):
/// the pages of any size, each read from where the last ended, are `list`
/// whole and in its order, mixed-case ids among them; and a page's
/// document states the whole catalog's counts, as one over every entry
/// does.
#[test]
fn pages_are_the_list() {
    let dir = scratch("pages");
    let mut catalog = catalog(&dir);
    let inputs = [
        common::core_tests().join("candidates"),
        common::core_tests().join("data"),
    ];
    catalog.ingest(&inputs, None).unwrap();
    // A mixed-case id, its bytes a bracket whose header names another file.
    let bracket = std::fs::read_to_string(common::core_tests().join("candidates/m_bracket.stp"));
    let upper = bracket
        .unwrap()
        .replacen("FILE_NAME('", "FILE_NAME('upper", 1);
    let upper = upper.into_bytes();
    let id = fitment_catalog::Id::parse("ZZ_Upper").unwrap();
    let input = fitment_catalog::Input::new(&upper, "upper", "upper.stp").with_id(id);
    let run = catalog
        .ingest_inputs(&[input], None, &fitment_catalog::NoObserver)
        .unwrap();
    assert_eq!(run[0].outcome, fitment_catalog::Outcome::Added);
    let snapshot = catalog.snapshot().unwrap();
    let all = snapshot.list().unwrap();
    assert!(all.len() > 20);
    for limit in [1, 3, 7, 1000] {
        let mut paged = Vec::new();
        loop {
            let after = paged.last().map(|l: &fitment_catalog::Listed| l.id.clone());
            let page = snapshot.list_page(after.as_deref(), limit).unwrap();
            assert!(page.len() <= limit);
            if page.is_empty() {
                break;
            }
            paged.extend(page);
        }
        assert_eq!(paged, all, "pages of {limit}");
    }
    let generation = snapshot.generation();
    let entries = || all.iter().take(2).map(|l| Entry::of(l, None)).collect();
    let counts = snapshot.status_counts().unwrap();
    assert_eq!(
        CatalogDocument::page(generation, &counts, entries()).to_json(),
        CatalogDocument::new(generation, &all, entries(), None).to_json()
    );
}

/// The review of PR 7a: a page is a search of `entry_live_id` from its
/// cursor, never a scan from the index's start, so it costs its own
/// entries; the first page reads the index from its start.
#[test]
fn a_page_searches_the_index_from_its_cursor() {
    let dir = scratch("page-plan");
    let mut catalog = catalog(&dir);
    let snapshot = catalog.snapshot().unwrap();
    let [first, after] = snapshot.list_page_plans().unwrap();
    assert!(
        first
            .iter()
            .any(|s| s.contains("USING INDEX entry_live_id")),
        "{first:?}"
    );
    assert!(
        after
            .iter()
            .any(|s| s.starts_with("SEARCH e USING INDEX entry_live_id (id_fold>?)")),
        "{after:?}"
    );
}

/// The review of PR 7a: the cursor is the folded id, so an entry deleted
/// between two pages and added again under its id in another case is not
/// listed twice.
#[test]
fn an_entry_added_again_in_another_case_is_listed_once() {
    let dir = scratch("page-case");
    let mut catalog = catalog(&dir);
    let bracket = std::fs::read(common::core_tests().join("candidates/m_bracket.stp")).unwrap();
    let blind = std::fs::read(common::core_tests().join("data/p03_blind_flat.stp")).unwrap();
    let through = std::fs::read(common::core_tests().join("data/p01_through.stp")).unwrap();
    let input = |bytes, id: &str| {
        fitment_catalog::Input::new(bytes, "in", "in.stp")
            .with_id(fitment_catalog::Id::parse(id).unwrap())
    };
    let none = fitment_catalog::NoObserver;
    let inputs = [
        input(&bracket, "A"),
        input(&blind, "B"),
        input(&through, "C"),
    ];
    catalog.ingest_inputs(&inputs, None, &none).unwrap();
    let first: Vec<String> = {
        let snapshot = catalog.snapshot().unwrap();
        let page = snapshot.list_page(None, 2).unwrap();
        page.into_iter().map(|l| l.id).collect()
    };
    assert_eq!(first, ["A", "B"]);
    catalog
        .delete(&[Reference::parse("B").unwrap()])
        .unwrap()
        .unwrap();
    catalog
        .ingest_inputs(&[input(&blind, "b")], None, &none)
        .unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let rest: Vec<String> = snapshot
        .list_page(Some("B"), 2)
        .unwrap()
        .into_iter()
        .map(|l| l.id)
        .collect();
    assert_eq!(rest, ["C"]);
}
