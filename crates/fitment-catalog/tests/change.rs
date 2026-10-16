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
    } = catalog.gc(false).unwrap();
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
