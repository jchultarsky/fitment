//! Replace, delete, prune and gc (`docs/research/m5-catalog.md`, CR1 to
//! CR7, CR11, ID6, ID9, ST5, ST7, IN1 to IN9; CT16's library half): each
//! leaves the invariants holding, keeps what a past report judged until a
//! prune, and unlinks only what no row names.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::{catalog, copy, invariants, scratch};
use fitment_catalog::{
    Catalog, Collected, NotReplaced, Outcome, Reference, Replaced, Sha256Hex, Unnamed,
};

fn reference(text: &str) -> Reference {
    Reference::parse(text).unwrap()
}

fn sha(path: &Path) -> Sha256Hex {
    Sha256Hex::of(&fs::read(path).unwrap())
}

/// A catalog holding the bracket as BRKT and p01 as `a`, and the files.
fn setup(test: &str) -> (Catalog, PathBuf, PathBuf, PathBuf) {
    let dir = scratch(test);
    let mut catalog = catalog(&dir);
    let bracket = copy("candidates/m_bracket.stp", &dir, "m_bracket.stp");
    let thick = copy(
        "candidates/m_bracket_thick.stp",
        &dir,
        "m_bracket_thick.stp",
    );
    let a = copy("data/p01_through.stp", &dir, "a.stp");
    let brkt = fitment_catalog::Id::parse("BRKT").unwrap();
    catalog
        .ingest(std::slice::from_ref(&bracket), Some(&brkt))
        .unwrap();
    catalog.ingest(std::slice::from_ref(&a), None).unwrap();
    (catalog, bracket, thick, a)
}

/// CR3, CR4, ID9: replace keeps the old revision's bytes and drops its
/// derived rows; refuses a refused file, another entry's bytes and a stale
/// `ID@rN`, changing nothing; its own bytes are unchanged.
#[test]
fn replace_gives_a_new_revision_and_keeps_the_old_bytes() {
    let (mut catalog, bracket, thick, a) = setup("replace");
    let root = catalog.root().to_owned();
    let Ok(Replaced::Replaced { from, to, .. }) =
        catalog.replace(&reference("BRKT"), &thick).unwrap()
    else {
        panic!("not replaced");
    };
    assert_eq!((from.0, to.0), (1, 2));
    assert_eq!((from.1, to.1.clone()), (sha(&bracket), sha(&thick)));
    assert!(
        catalog.blob_path(&sha(&bracket)).is_file(),
        "r1's bytes are kept"
    );
    assert_eq!(invariants(&root), Vec::<String>::new());
    let generation = catalog.generation().unwrap();
    let junk = root.parent().unwrap().join("junk.stp");
    fs::write(&junk, "not STEP").unwrap();
    assert!(matches!(
        catalog.replace(&reference("BRKT"), &junk).unwrap(),
        Err(NotReplaced::File(f)) if matches!(f.outcome, Outcome::Refused { .. })
    ));
    assert!(matches!(
        catalog.replace(&reference("BRKT"), &a).unwrap(),
        Err(NotReplaced::Duplicate { of }) if of == "a"
    ));
    assert!(matches!(
        catalog.replace(&reference("BRKT@r1"), &bracket).unwrap(),
        Err(NotReplaced::Unnamed(Unnamed::NotCurrent {
            current: 2,
            asked: 1,
            ..
        }))
    ));
    assert!(matches!(
        catalog.replace(&reference("BRKT@r2"), &thick).unwrap(),
        Ok(Replaced::Unchanged { revision: 2, .. })
    ));
    assert_eq!(catalog.generation().unwrap(), generation, "nothing changed");
    // Back to the first bytes: r3, and r1's content is live again.
    assert!(matches!(
        catalog.replace(&reference("brkt"), &bracket).unwrap(),
        Ok(Replaced::Replaced { to: (3, _), .. })
    ));
    assert_eq!(invariants(&root), Vec::<String>::new());
    let snapshot = catalog.snapshot().unwrap();
    let r1 = snapshot.resolve(&reference("BRKT@r1")).unwrap().unwrap();
    assert_eq!((r1.current, r1.id_current), (Some(3), Some(3)));
}

/// CR1, CR2, ID6: delete is all or nothing across ids; the deleted entry's
/// files, kept revisions' included, are unlinked; the id continues at its
/// next revision when ingested again, as a new entry.
#[test]
fn delete_is_all_or_nothing_and_ids_continue() {
    let (mut catalog, bracket, thick, _) = setup("delete");
    let root = catalog.root().to_owned();
    catalog
        .replace(&reference("BRKT"), &thick)
        .unwrap()
        .unwrap();
    let refused = catalog
        .delete(&[
            reference("a"),
            reference("nosuch"),
            reference("sha256:0123456789ab"),
        ])
        .unwrap()
        .unwrap_err();
    assert_eq!(refused.len(), 2);
    assert!(
        catalog.snapshot().unwrap().list().unwrap().len() == 2,
        "a is still listed"
    );
    let done = catalog
        .delete(&[reference("BRKT"), reference("brkt")])
        .unwrap()
        .unwrap();
    assert_eq!(done.deleted.len(), 1);
    assert_eq!(done.released.contents.len(), 2, "both revisions' bytes");
    assert_eq!(done.released.orphans, Vec::<String>::new());
    assert!(!catalog.blob_path(&sha(&bracket)).exists());
    assert!(!catalog.blob_path(&sha(&thick)).exists());
    assert_eq!(invariants(&root), Vec::<String>::new());
    let brkt = fitment_catalog::Id::parse("BRKT").unwrap();
    let again = catalog
        .ingest(std::slice::from_ref(&bracket), Some(&brkt))
        .unwrap();
    assert_eq!(
        (again[0].outcome.clone(), again[0].revision),
        (Outcome::Added, Some(3))
    );
    let snapshot = catalog.snapshot().unwrap();
    let r1 = snapshot.resolve(&reference("BRKT@r1")).unwrap().unwrap();
    assert_eq!((r1.current, r1.id_current), (None, Some(3)));
    let events: Vec<&str> = snapshot
        .history(&r1)
        .unwrap()
        .iter()
        .map(|s| s.event)
        .collect();
    assert_eq!(events, ["ingest", "replace", "delete", "ingest"]);
}

/// CR11: prune releases every kept revision's bytes once; a content live
/// again in a later revision is kept; pruning twice prunes nothing.
#[test]
fn prune_releases_kept_revisions_once() {
    let (mut catalog, bracket, thick, _) = setup("prune");
    let root = catalog.root().to_owned();
    catalog
        .replace(&reference("BRKT"), &thick)
        .unwrap()
        .unwrap();
    catalog
        .replace(&reference("BRKT"), &bracket)
        .unwrap()
        .unwrap();
    // r1 (the bracket) is live again as r3; r2 (thick) is kept.
    let (pruned, released) = catalog.prune_revisions().unwrap();
    let revisions: Vec<u32> = pruned.iter().map(|p| p.revision).collect();
    assert_eq!(revisions, [1, 2]);
    assert_eq!(released.contents.len(), 1);
    assert_eq!(released.contents[0].0, sha(&thick));
    assert!(catalog.blob_path(&sha(&bracket)).is_file());
    assert!(!catalog.blob_path(&sha(&thick)).exists());
    assert_eq!(invariants(&root), Vec::<String>::new());
    let (again, _) = catalog.prune_revisions().unwrap();
    assert_eq!(again.len(), 0);
    let snapshot = catalog.snapshot().unwrap();
    let r2 = snapshot.resolve(&reference("BRKT@r2")).unwrap().unwrap();
    assert_eq!(snapshot.entry(&r2).unwrap().bytes, None);
}

/// ST5, ST7: gc unlinks an orphan blob and an old temporary file, and
/// leaves a recent one and a file that is not the store's.
#[test]
fn gc_removes_only_what_no_row_names() {
    let (mut catalog, _, thick, _) = setup("gc");
    let root = catalog.root().to_owned();
    let orphan = catalog.blob_path(&sha(&thick));
    fs::write(&orphan, fs::read(&thick).unwrap()).unwrap();
    common::age(&orphan);
    let stray = orphan.with_file_name("notes.txt");
    fs::write(&stray, "mine").unwrap();
    let old = root.join("tmp/1-1.part");
    let recent = root.join("tmp/1-2.part");
    fs::write(&old, "x").unwrap();
    fs::write(&recent, "y").unwrap();
    let file = fs::File::options().write(true).open(&old).unwrap();
    // Long ago: an abandoned write.
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(86_400))
        .unwrap();
    drop(file);
    let Collected {
        unlinked,
        temporaries,
        failed,
        ..
    } = catalog.gc(false, false).unwrap();
    assert_eq!(unlinked, [sha(&thick)]);
    assert_eq!(temporaries, ["1-1.part"]);
    assert_eq!(failed, Vec::<String>::new());
    assert!(!orphan.exists() && stray.exists() && recent.exists() && !old.exists());
    fs::remove_file(&stray).unwrap();
    fs::remove_file(&recent).unwrap();
    assert_eq!(invariants(&root), Vec::<String>::new());
}

/// CR4 and ID9 (the PR 3a reviews): a replace refused for its reference
/// leaves no file behind, a refused file no id, and an input inside the
/// catalog is refused; a delete of nothing changes nothing.
#[test]
fn refusals_leave_nothing_behind() {
    let (mut catalog, _, thick, _) = setup("refusals");
    let root = catalog.root().to_owned();
    let g = catalog.generation().unwrap();
    for bad in ["nosuch", "BRKT@r2"] {
        assert!(matches!(
            catalog.replace(&reference(bad), &thick).unwrap(),
            Err(NotReplaced::Unnamed(_))
        ));
    }
    assert!(!catalog.blob_path(&sha(&thick)).exists());
    let Err(NotReplaced::File(line)) = catalog
        .replace(&reference("BRKT"), root.parent().unwrap())
        .unwrap()
    else {
        panic!("a directory replaced");
    };
    assert_eq!(line.id, None);
    let inside = root.join("catalog.db");
    assert!(matches!(
        catalog.replace(&reference("BRKT"), &inside),
        Err(fitment_catalog::Error::InsideCatalog(_))
    ));
    let none = catalog.delete(&[]).unwrap().unwrap();
    assert_eq!(none.deleted.len(), 0);
    assert_eq!(catalog.generation().unwrap(), g);
    assert_eq!(catalog.verify(false, false).unwrap(), Vec::new());
}

/// CR5: a quarantined entry replaced by a file the gate accepts becomes
/// indexed.
#[test]
fn a_quarantined_entry_replaced_is_indexed() {
    let dir = scratch("cr5");
    let mut catalog = catalog(&dir);
    let junk = dir.join("junk.stp");
    fs::write(&junk, "not STEP").unwrap();
    catalog.ingest(&[junk], None).unwrap();
    let fixed = copy("data/p03_blind_flat.stp", &dir, "fixed.stp");
    catalog
        .replace(&reference("junk"), &fixed)
        .unwrap()
        .unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let listed = snapshot.list().unwrap();
    assert_eq!(
        listed[0].status,
        Some(fitment_catalog::EntryStatus::Indexed)
    );
    assert_eq!(listed[0].revision, 2);
}

/// SZ4: `gc --vacuum` shrinks the database after a delete, and leaves the
/// catalog consistent.
#[test]
fn gc_vacuums_on_request() {
    let (mut catalog, _, _, _) = setup("vacuum");
    let pages = |c: &Catalog| -> i64 {
        c.connection()
            .query_row("PRAGMA page_count", [], |r| r.get(0))
            .unwrap()
    };
    catalog
        .delete(&[reference("BRKT"), reference("a")])
        .unwrap()
        .unwrap();
    let before = pages(&catalog);
    catalog.gc(false, true).unwrap();
    assert!(pages(&catalog) < before);
    assert_eq!(catalog.verify(false, false).unwrap(), Vec::new());
}

/// RA7 (`docs/research/m6-api.md`, the review of PR 7b): a delete or a
/// replace made on condition of a generation the catalog is no longer at
/// changes nothing and says so, after its references are checked; at its
/// generation it is made.
#[test]
fn conditional_changes_are_made_only_at_their_generation() {
    let (mut catalog, _, thick, _) = setup("conditional");
    let at = catalog.generation().unwrap();
    let refused = catalog.delete_at(&[reference("a")], Some(at - 1)).unwrap();
    assert_eq!(
        refused,
        Err(vec![Unnamed::Changed {
            asked: at - 1,
            generation: at
        }])
    );
    let unknown = catalog
        .delete_at(&[reference("nope")], Some(at - 1))
        .unwrap();
    assert!(matches!(
        unknown.as_ref().map_err(Vec::as_slice),
        Err([Unnamed::Name(_)])
    ));
    assert_eq!(catalog.generation().unwrap(), at, "nothing changed");
    let bytes = fs::read(&thick).unwrap();
    let input = fitment_catalog::Input::new(&bytes, "thick.stp", "thick.stp");
    let observer = fitment_catalog::NoObserver;
    let stale = catalog
        .replace_input_at(&reference("BRKT"), &input, None, Some(at - 1), &observer)
        .unwrap();
    assert!(matches!(
        stale,
        Err(NotReplaced::Unnamed(Unnamed::Changed { .. }))
    ));
    assert_eq!(catalog.generation().unwrap(), at, "nothing changed");
    let made = catalog
        .replace_input_at(&reference("BRKT"), &input, None, Some(at), &observer)
        .unwrap();
    assert!(matches!(made, Ok(Replaced::Replaced { .. })));
    let now = catalog.generation().unwrap();
    assert!(
        catalog
            .delete_at(&[reference("a")], Some(now))
            .unwrap()
            .is_ok()
    );
    invariants(catalog.root());
}

/// CX6 (the review of PR 7b): gc's collection alone leaves the log as it
/// was, and `vacuum` then runs on its own; `gc` is the two with the
/// truncation between, as before, nothing left to collect.
#[test]
fn gc_collects_without_truncating_and_vacuums_apart() {
    let (mut catalog, _, _, _) = setup("gc-apart");
    catalog.delete(&[reference("a")]).unwrap().unwrap();
    let before = catalog.checkpoint_passive().unwrap();
    let collected = catalog
        .gc_collect_observed(false, &fitment_catalog::NoObserver)
        .unwrap();
    assert!(!collected.cancelled);
    let after = catalog.checkpoint_passive().unwrap();
    assert!(after.log_pages >= before.log_pages, "no truncation");
    catalog.vacuum().unwrap();
    assert_eq!(catalog.gc(false, true).unwrap().unlinked, Vec::new());
    invariants(catalog.root());
}

/// ST11, OB3 (the review of PR 7b): a blob opened for a stream is hashed
/// in one pass, then read in a second that fails, `InvalidData`, at its
/// end if the file changed between them; a missing or altered file is
/// refused before the second pass.
#[test]
fn a_streamed_blob_is_checked_in_both_passes() {
    let (mut catalog, bracket, _, _) = setup("stream");
    let content = sha(&bracket);
    let bytes = fs::read(&bracket).unwrap();
    let read_all = |reader: &mut Box<dyn fitment_catalog::FsRead>| {
        let mut out = Vec::new();
        let mut buf = [0_u8; 1000];
        loop {
            match reader.read_chunk(&mut buf) {
                Ok(0) => return Ok(out),
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(e) => return Err(e),
            }
        }
    };
    let snapshot = catalog.snapshot().unwrap();
    drop(snapshot);
    let (length, mut reader) = catalog.open_blob(&content).unwrap();
    assert_eq!(length, u64::try_from(bytes.len()).unwrap());
    assert_eq!(read_all(&mut reader).unwrap(), bytes);
    let path = catalog.blob_path(&content);
    let (_, mut reader) = catalog.open_blob(&content).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    let mut altered = bytes.clone();
    altered[10] ^= 1;
    fs::write(&path, &altered).unwrap();
    let error = read_all(&mut reader).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(matches!(
        catalog.open_blob(&content).map(drop),
        Err(fitment_catalog::BadBlob::Damaged)
    ));
    fs::remove_file(&path).unwrap();
    assert!(matches!(
        catalog.open_blob(&content).map(drop),
        Err(fitment_catalog::BadBlob::Missing)
    ));
}

/// CA1, ID4 (the review of PR 7b): `ingest_order` is the order
/// `ingest_inputs` takes its inputs in, so a caller ties each line to its
/// own input, two inputs of one label included.
#[test]
fn ingest_order_ties_each_line_to_its_input() {
    let dir = scratch("order");
    let mut catalog = catalog(&dir);
    let block = fs::read(common::core_tests().join("candidates/m_block02.stp")).unwrap();
    let bracket = fs::read(common::core_tests().join("candidates/m_bracket.stp")).unwrap();
    let junk = b"not STEP".to_vec();
    let inputs = [
        fitment_catalog::Input::new(&junk, "same", "z.stp"),
        fitment_catalog::Input::new(&block, "same", "b.stp"),
        fitment_catalog::Input::new(&bracket, "same", "B.stp"),
    ];
    let order = fitment_catalog::ingest_order(&inputs);
    let run = catalog
        .ingest_inputs(&inputs, None, &fitment_catalog::NoObserver)
        .unwrap();
    assert_eq!(run.lines.len(), inputs.len());
    for (line, at) in run.lines.iter().zip(&order) {
        let expected = [&junk, &block, &bracket][*at];
        assert_eq!(line.sha256, Some(Sha256Hex::of(expected)), "{order:?}");
    }
}
