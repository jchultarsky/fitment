//! Two reads the server needs (docs/research/m6-api.md, section 5.2's
//! review notes): `Catalog::inspect`, which checks a catalog writing
//! nothing (OP4's `check-config`), and `Snapshot::status_counts`, the
//! counts `fitment.info` states in one query (DC11).

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

use common::{catalog, copy, scratch};
use fitment_catalog::{Catalog, EntryStatus, Error, Options};
use rusqlite::Connection;

/// Every file under `dir` with its size and modification time.
fn listing(dir: &Path) -> BTreeMap<String, (u64, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                stack.push(path.clone());
            }
            out.insert(
                path.display().to_string(),
                (meta.len(), meta.modified().unwrap()),
            );
        }
    }
    out
}

/// `inspect` passes a catalog and changes no file of it: no `-wal` or
/// `-shm`, no pragma, no migration; it refuses what `open` refuses.
#[test]
fn inspect_reads_and_writes_nothing() {
    let dir = scratch("inspect_reads_and_writes_nothing");
    let root = dir.join("cat");
    drop(Catalog::init(&root, Options::default()).unwrap());
    let before = listing(&root);
    assert!(Catalog::inspect(&root).is_ok());
    assert_eq!(listing(&root), before);

    let conn = Connection::open(root.join("catalog.db")).unwrap();
    conn.pragma_update(None, "user_version", 9).unwrap();
    drop(conn);
    assert!(matches!(
        Catalog::inspect(&root),
        Err(Error::Newer { found: 9, .. })
    ));
    let conn = Connection::open(root.join("catalog.db")).unwrap();
    conn.pragma_update(None, "application_id", 7).unwrap();
    drop(conn);
    assert!(matches!(
        Catalog::inspect(&root),
        Err(Error::NotACatalog(_))
    ));
    std::fs::remove_file(root.join("FITMENT-CATALOG")).unwrap();
    assert!(matches!(
        Catalog::inspect(&root),
        Err(Error::NotACatalog(_))
    ));
    assert!(matches!(
        Catalog::inspect(&dir.join("missing")),
        Err(Error::NotACatalog(_))
    ));
}

/// `status_counts` is what counting `list`'s statuses gives, with an
/// indexed, a quarantined, a signature-stale and a properties-stale entry.
#[test]
fn status_counts_are_the_lists() {
    let dir = scratch("status_counts_are_the_lists");
    let mut catalog = catalog(&dir);
    let files = [
        copy("candidates/m_bracket.stp", &dir, "a.stp"),
        copy("data/p01_through.stp", &dir, "b.stp"),
        copy("data/p03_blind_flat.stp", &dir, "c.stp"),
        copy("data/r04_assembly.stp", &dir, "d.stp"),
    ];
    let run = catalog.ingest(&files, None).unwrap();
    let sha = |i: usize| run[i].sha256.clone().unwrap().as_str().to_owned();
    let conn = Connection::open(catalog.root().join("catalog.db")).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    conn.execute("DELETE FROM tokens WHERE sha256 = ?1", [sha(1)])
        .unwrap();
    conn.execute("DELETE FROM properties WHERE sha256 = ?1", [sha(2)])
        .unwrap();
    drop(conn);
    let snapshot = catalog.snapshot().unwrap();
    let listed = snapshot.list().unwrap();
    let counts = snapshot.status_counts().unwrap();
    let count = |want: fn(&EntryStatus) -> bool| {
        listed
            .iter()
            .filter(|l| l.status.as_ref().is_some_and(want))
            .count() as u64
    };
    assert_eq!(counts.entries, listed.len() as u64);
    assert_eq!(counts.indexed, count(|s| matches!(s, EntryStatus::Indexed)));
    assert_eq!(
        counts.quarantined,
        count(|s| matches!(s, EntryStatus::Quarantined))
    );
    assert_eq!(counts.stale, 2, "{listed:?}");
    assert_eq!(
        (counts.entries, counts.indexed, counts.quarantined),
        (4, 1, 1)
    );
}
