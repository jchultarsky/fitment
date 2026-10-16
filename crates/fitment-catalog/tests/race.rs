//! Writers at once (`docs/research/m5-catalog.md`, CT14, DB2, CC1, CC2,
//! CC6): eight writers, each on its own connection, ingest overlapping
//! sets while others delete and prune; every content ends live at most
//! once, no writer fails busy, and the invariants hold.

mod common;

use std::fs;
use std::path::PathBuf;
use std::thread;

use common::{core_tests, invariants, scratch};
use fitment_catalog::{Catalog, Options, Outcome, Reference};
use rusqlite::Connection;

#[test]
fn eight_writers_keep_one_live_entry_per_content() {
    let dir = scratch("race");
    let root = dir.join("cat");
    Catalog::init(&root, Options::default()).unwrap();
    let names: Vec<PathBuf> = fs::read_dir(core_tests().join("data"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "stp"))
        .collect();
    let writers: Vec<_> = (0..8)
        .map(|w| {
            let root = root.clone();
            // Each writer a different, overlapping slice of the unit set.
            let files: Vec<PathBuf> = names
                .iter()
                .enumerate()
                .filter(|(k, _)| (k + w) % 3 != 0)
                .map(|(_, p)| p.clone())
                .collect();
            thread::spawn(move || {
                let mut catalog = Catalog::open(&root, Options::default()).unwrap();
                let run = catalog.ingest(&files, None).unwrap();
                assert!(run.stopped.is_none(), "writer {w}: {:?}", run.stopped);
                assert!(
                    !run.iter()
                        .any(|l| matches!(l.outcome, Outcome::Error { .. })),
                    "writer {w}"
                );
                if w % 4 == 3 {
                    let id = Reference::parse("p01_through").unwrap();
                    let _ = catalog.delete(&[id]).unwrap();
                    catalog.prune_revisions().unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    let conn = Connection::open(root.join("catalog.db")).unwrap();
    let twice: i64 = conn
        .query_row(
            "SELECT count(*) FROM (SELECT sha256 FROM live GROUP BY sha256 HAVING count(*) > 1)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(twice, 0);
    assert_eq!(invariants(&root), Vec::<String>::new());
}

/// The PR 3a review's F3: a gc that unlinks an orphan an ingest is reusing,
/// between the ingest's read of it and its use, does not fail the ingest.
#[test]
fn an_ingest_survives_a_gc_of_the_orphan_it_reuses() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use fitment_catalog::fs::{Call, CountingFs};

    let dir = scratch("reuse-gc");
    let root = dir.join("cat");
    Catalog::init(&root, Options::default()).unwrap();
    let source = common::copy("candidates/m_bracket.stp", &dir, "m_bracket.stp");
    let sha = fitment_catalog::Sha256Hex::of(&fs::read(&source).unwrap());
    let blob = root
        .join("blobs/sha256")
        .join(sha.fan_out())
        .join(sha.as_str());
    fs::copy(&source, &blob).unwrap();
    let once = AtomicBool::new(false);
    let gc_root = root.clone();
    let seam = Arc::new(CountingFs::with_hook(move |call| {
        if *call == Call::Read(blob.clone()) && !once.swap(true, Ordering::SeqCst) {
            Catalog::open(&gc_root, Options::default())
                .unwrap()
                .gc(false)
                .unwrap();
        }
    }));
    let mut catalog =
        Catalog::open_with(&root, Options::default(), Box::new(common::Shared(seam))).unwrap();
    let run = catalog.ingest(&[source], None).unwrap();
    assert_eq!(run[0].outcome, Outcome::Added);
    drop(catalog);
    assert_eq!(invariants(&root), Vec::<String>::new());
}
