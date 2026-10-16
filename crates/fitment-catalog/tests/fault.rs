//! Faults and the order of durable steps in ingest
//! (`docs/research/m5-catalog.md`, CT13, ST3, ST6, review F6): a fault at
//! any file call leaves the database all or nothing and the invariants
//! holding, and a rerun completes the ingest; every blob a commit names has
//! its fan-out directory synced after its rename or reuse check and before
//! that commit.

mod common;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::{copy, invariants, scratch};
use fitment_catalog::fs::{Call, CountingFs};
use fitment_catalog::{Catalog, Options, Outcome, Point, Sha256Hex};
use rusqlite::Connection;

/// C's three files: one added, its copy a duplicate, one quarantined.
fn inputs(dir: &Path) -> Vec<PathBuf> {
    let inputs = dir.join("in");
    fs::create_dir_all(&inputs).unwrap();
    copy("candidates/m_bracket.stp", &inputs, "m_bracket.stp");
    copy("candidates/m_bracket.stp", &inputs, "copy of m_bracket.stp");
    fs::write(inputs.join("junk.stp"), "not STEP").unwrap();
    vec![inputs]
}

fn entries(root: &Path) -> i64 {
    let conn = Connection::open(root.join("catalog.db")).unwrap();
    conn.query_row("SELECT count(*) FROM entry", [], |r| r.get(0))
        .unwrap()
}

/// The number of calls opening makes, and opening and ingesting.
fn count(dir: &Path) -> (usize, usize) {
    let root = dir.join("count");
    Catalog::init(&root, Options::default()).unwrap();
    let seam = Arc::new(CountingFs::new());
    let mut catalog =
        Catalog::open_with(&root, Options::default(), Box::new(Shared(seam.clone()))).unwrap();
    let opened = seam.calls().len();
    let lines = catalog.ingest(&inputs(dir), None).unwrap();
    assert_eq!(lines.len(), 3);
    (opened, seam.calls().len())
}

/// A seam the test keeps a handle on.
struct Shared(Arc<CountingFs>);

impl fitment_catalog::Fs for Shared {
    fn symlink_metadata(&self, path: &Path) -> io::Result<fs::Metadata> {
        self.0.symlink_metadata(path)
    }
    fn read_capped(&self, path: &Path, cap: u64) -> io::Result<fitment_catalog::Content> {
        self.0.read_capped(path, cap)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        self.0.read_dir(path)
    }
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.0.create_dir_all(path)
    }
    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.0.write_new(path, bytes)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.0.rename(from, to)
    }
    fn set_readonly(&self, path: &Path, readonly: bool) -> io::Result<()> {
        self.0.set_readonly(path, readonly)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.0.remove_file(path)
    }
    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        self.0.sync_dir(path)
    }
    fn point(&self, at: Point) -> io::Result<()> {
        self.0.point(at)
    }
}

/// CT13 for ingest: for every call k of a batch and each of ENOSPC, EIO
/// and ENOENT, the database holds exactly the files reported added (a
/// failed commit none of them), the invariants hold, and a rerun gives the
/// full result.
#[test]
fn an_ingest_is_all_or_nothing_under_any_fault() {
    let dir = scratch("faults");
    let (opened, total) = count(&dir);
    assert!(total > opened + 5, "the ingest makes file calls");
    let kinds = [
        io::ErrorKind::StorageFull,
        io::ErrorKind::Other,
        io::ErrorKind::NotFound,
    ];
    let mut faulted = 0;
    for k in opened..total {
        for kind in kinds {
            let root = dir.join(format!("k{k}-{kind:?}"));
            Catalog::init(&root, Options::default()).unwrap();
            let seam = CountingFs::failing_at(k, kind);
            let mut catalog =
                Catalog::open_with(&root, Options::default(), Box::new(seam)).unwrap();
            let lines = catalog.ingest(&inputs(&dir), None).unwrap();
            faulted += usize::from(
                lines
                    .iter()
                    .any(|l| matches!(l.outcome, Outcome::Error { .. })),
            );
            drop(catalog);
            // A file whose read fails is that file's error; the commit holds
            // exactly the lines reported added.
            let added = lines.iter().filter(|l| l.outcome == Outcome::Added).count();
            let n = entries(&root);
            assert_eq!(
                n,
                i64::try_from(added).unwrap(),
                "k {k} {kind:?}: {lines:?}"
            );
            let problems: Vec<String> = invariants(&root)
                .into_iter()
                .filter(|p| !p.starts_with("IN6"))
                .collect();
            assert!(problems.is_empty(), "k {k} {kind:?}: {problems:?}");
            let mut again = Catalog::open(&root, Options::default()).unwrap();
            again.ingest(&inputs(&dir), None).unwrap();
            drop(again);
            assert_eq!(entries(&root), 2, "k {k} {kind:?}: the rerun");
            let problems: Vec<String> = invariants(&root)
                .into_iter()
                .filter(|p| !p.starts_with("IN6"))
                .collect();
            assert!(problems.is_empty(), "k {k} {kind:?} rerun: {problems:?}");
        }
    }
    assert!(faulted > 0, "some faults fail the batch");
}

/// The final path of `bytes`' blob under `root`.
fn blob(root: &Path, bytes: &[u8]) -> (PathBuf, PathBuf) {
    let sha = Sha256Hex::of(bytes);
    let fan = root.join("blobs/sha256").join(sha.fan_out());
    (fan.join(sha.as_str()), fan)
}

/// ST3 (review, F6): each named blob's fan-out directory is synced after
/// the last rename to it, or read of it to reuse it, and before the
/// commit.
fn synced_before_commit(calls: &[Call], named: &[(PathBuf, PathBuf)]) -> Vec<String> {
    let Some(commit) = calls
        .iter()
        .position(|c| *c == Call::Point(Point::BeforeCommit))
    else {
        return vec!["no commit".into()];
    };
    let mut problems = Vec::new();
    for (path, fan) in named {
        let touched = calls[..commit].iter().rposition(|c| match c {
            Call::Rename(_, to) => to == path,
            Call::Read(read) => read == path,
            _ => false,
        });
        let Some(touched) = touched else {
            problems.push(format!("{}: never renamed or checked", path.display()));
            continue;
        };
        if !calls[touched..commit].contains(&Call::SyncDir(fan.clone())) {
            problems.push(format!("{}: not synced before its commit", path.display()));
        }
    }
    problems
}

/// CT13 (review, F6): fresh, reused and ST6-rewritten blobs in one batch.
#[test]
fn every_named_blob_has_its_directory_synced_before_commit() {
    let dir = scratch("synced");
    let root = dir.join("cat");
    Catalog::init(&root, Options::default()).unwrap();
    let inputs = dir.join("in");
    fs::create_dir_all(&inputs).unwrap();
    let fresh = copy("candidates/m_bracket.stp", &inputs, "fresh.stp");
    let reused = copy("data/p03_blind_flat.stp", &inputs, "reused.stp");
    let rewritten = copy("data/p01_through.stp", &inputs, "rewritten.stp");
    let named: Vec<(PathBuf, PathBuf)> = [&fresh, &reused, &rewritten]
        .iter()
        .map(|p| blob(&root, &fs::read(p).unwrap()))
        .collect();
    // An orphan of the reused file's bytes, as a stopped ingest leaves.
    fs::copy(&reused, &named[1].0).unwrap();
    // The rewritten file's blob goes once it is in place.
    let gone = named[2].0.clone();
    let once = AtomicBool::new(false);
    let seam = Arc::new(CountingFs::with_hook(move |call| {
        if *call == Call::SetReadonly(gone.clone(), true) && !once.swap(true, Ordering::SeqCst) {
            fs::remove_file(&gone).unwrap();
        }
    }));
    let mut catalog =
        Catalog::open_with(&root, Options::default(), Box::new(Shared(seam.clone()))).unwrap();
    let lines = catalog.ingest(&[inputs], None).unwrap();
    assert!(
        lines.iter().all(|l| l.outcome == Outcome::Added),
        "{lines:?}"
    );
    drop(catalog);
    let calls = seam.calls();
    let renames_to = |p: &PathBuf| {
        calls
            .iter()
            .filter(|c| matches!(c, Call::Rename(_, to) if to == p))
            .count()
    };
    assert_eq!(renames_to(&named[0].0), 1, "fresh: renamed once");
    assert_eq!(renames_to(&named[1].0), 0, "reused: never renamed");
    assert_eq!(renames_to(&named[2].0), 2, "rewritten: renamed again (ST6)");
    assert_eq!(synced_before_commit(&calls, &named), Vec::<String>::new());
    assert_eq!(invariants(&root), Vec::<String>::new());
    // The control: without the syncs, the check fails for every blob.
    let unsynced: Vec<Call> = calls
        .iter()
        .filter(|c| !matches!(c, Call::SyncDir(_)))
        .cloned()
        .collect();
    assert_eq!(synced_before_commit(&unsynced, &named).len(), 3);
}

/// CT13 (review, F6): an ingest stopped after its rename and before its
/// directory sync leaves an orphan; a second ingest of the same bytes
/// reuses it, and syncs its directory before committing.
#[test]
fn a_reused_orphan_has_its_directory_synced() {
    let dir = scratch("orphan");
    let root = dir.join("cat");
    Catalog::init(&root, Options::default()).unwrap();
    let file = copy("candidates/m_bracket.stp", &dir, "m_bracket.stp");
    let named = vec![blob(&root, &fs::read(&file).unwrap())];
    // Find the directory sync, then stop there.
    let probe = Arc::new(CountingFs::new());
    let probe_root = dir.join("probe");
    Catalog::init(&probe_root, Options::default()).unwrap();
    let mut catalog = Catalog::open_with(
        &probe_root,
        Options::default(),
        Box::new(Shared(probe.clone())),
    )
    .unwrap();
    catalog.ingest(std::slice::from_ref(&file), None).unwrap();
    let k = probe
        .calls()
        .iter()
        .position(|c| matches!(c, Call::SyncDir(p) if p.starts_with(probe_root.join("blobs"))))
        .unwrap();
    let mut stopped = Catalog::open_with(
        &root,
        Options::default(),
        Box::new(CountingFs::failing_at(k, io::ErrorKind::Other)),
    )
    .unwrap();
    let lines = stopped.ingest(std::slice::from_ref(&file), None).unwrap();
    assert!(matches!(lines[0].outcome, Outcome::Error { .. }));
    drop(stopped);
    assert!(named[0].0.is_file(), "the orphan is left");
    assert_eq!(entries(&root), 0);
    let seam = Arc::new(CountingFs::new());
    let mut catalog =
        Catalog::open_with(&root, Options::default(), Box::new(Shared(seam.clone()))).unwrap();
    let lines = catalog.ingest(&[file], None).unwrap();
    assert_eq!(lines[0].outcome, Outcome::Added);
    let calls = seam.calls();
    assert!(
        !calls.iter().any(|c| matches!(c, Call::Rename(..))),
        "reused, not renamed"
    );
    assert_eq!(synced_before_commit(&calls, &named), Vec::<String>::new());
}
