//! CA2 (`docs/research/m6-api.md`, VT38): every point an [`Observer`] is
//! asked at stops the work there. A cancelled ingest or reindex keeps the
//! batches it committed and nothing of the one it was building; a
//! cancelled match returns `NotRun::Cancelled`, with no result; a
//! cancelled verify returns `Error::Cancelled`; a cancelled gc keeps what
//! it unlinked. The catalog's invariants hold after each.

mod common;

use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{core_tests, invariants, scratch};
use fitment_catalog::{
    Catalog, Error, Input, NotRun, Observer, Options, Progress, Scope, Selection,
};
use fitment_core::{MatchOptions, MatchPolicy, Matcher, NoFitVerifier, SocketDocument};

/// Cancels from its `k`th question on (from 0), counts them, and notes
/// how many it had been asked at each batch committed, with the files or
/// entries then settled.
struct StopAt {
    k: usize,
    asked: AtomicUsize,
    commits: Mutex<Vec<(usize, usize)>>,
}

impl StopAt {
    fn new(k: usize) -> Self {
        Self {
            k,
            asked: AtomicUsize::new(0),
            commits: Mutex::new(Vec::new()),
        }
    }

    fn asked(&self) -> usize {
        self.asked.load(Ordering::Relaxed)
    }

    /// The files or entries its run's committed batches settled. A batch
    /// ends after 64 files or a second (DB3), so a run's batches are its
    /// own: what a stopped run keeps is read from its own commits.
    fn kept(&self) -> usize {
        let commits = self.commits.lock().unwrap();
        commits.iter().map(|(_, done)| *done).max().unwrap_or(0)
    }

    /// The questions a stop at each point of this full run is put at: the
    /// first, one inside the first batch, each commit's and the one after
    /// it, and the last. Another run may batch otherwise; each stops at its
    /// question whatever it was asked about there.
    fn points(&self) -> Vec<usize> {
        let mut points = vec![0, 5, self.asked() - 1];
        for (asked, _) in self.commits.lock().unwrap().iter() {
            points.extend([asked - 1, *asked]);
        }
        points.sort_unstable();
        points.dedup();
        points.retain(|k| *k < self.asked());
        points
    }
}

impl Observer for StopAt {
    fn cancelled(&self) -> bool {
        self.asked.fetch_add(1, Ordering::Relaxed) >= self.k
    }

    fn progress(&self, event: Progress<'_>) {
        if let Progress::Committed { done, .. } = event {
            self.commits.lock().unwrap().push((self.asked(), done));
        }
    }
}

/// 66 copies of the bracket, each its own bytes: two batches (DB3).
fn variants() -> Vec<(Vec<u8>, String)> {
    let bracket = fs::read_to_string(core_tests().join("candidates/m_bracket.stp")).unwrap();
    (0..66)
        .map(|n| {
            let text = bracket.replacen("FILE_NAME('", &format!("FILE_NAME('v{n}"), 1);
            (text.into_bytes(), format!("v{n:02}.stp"))
        })
        .collect()
}

/// The blob files under `root`, and the blob rows.
fn blobs(root: &Path) -> (usize, i64) {
    let files = fs::read_dir(root.join("blobs/sha256"))
        .unwrap()
        .map(|fan| fs::read_dir(fan.unwrap().path()).unwrap().count())
        .sum();
    let catalog = Catalog::open(root, Options::default()).unwrap();
    let rows = catalog
        .connection()
        .query_row("SELECT count(*) FROM blob", [], |r| r.get(0))
        .unwrap();
    (files, rows)
}

/// Ingest: stopped before the first file, inside the first batch, before
/// and after each commit, and at its last question.
fn ingest_stops(dir: &Path) {
    let files = variants();
    let inputs: Vec<Input<'_>> = files
        .iter()
        .map(|(bytes, name)| Input::new(bytes, name, name))
        .collect();
    let root = dir.join("ingest-all");
    let (mut catalog, _) = Catalog::init(&root, Options::default()).unwrap();
    let all = StopAt::new(usize::MAX);
    let run = catalog.ingest_inputs(&inputs, None, &all).unwrap();
    assert!(run.stopped.is_none());
    assert_eq!(run.lines.len(), 66);
    let batches = all.commits.lock().unwrap().len();
    assert!(batches >= 2, "{batches} batches");
    assert_eq!(
        all.asked(),
        66 + batches,
        "a question per file and per commit"
    );
    for k in all.points() {
        let root = dir.join(format!("ingest-{k}"));
        let (mut catalog, _) = Catalog::init(&root, Options::default()).unwrap();
        let stop = StopAt::new(k);
        let run = catalog.ingest_inputs(&inputs, None, &stop).unwrap();
        assert!(matches!(run.stopped, Some(Error::Cancelled)), "{k}");
        assert_eq!(stop.asked(), k + 1, "stopped at its question");
        let committed = stop.kept();
        assert_eq!(run.lines.len(), committed, "{k}");
        assert!(run.iter().all(|l| l.revision.is_some()), "{k}");
        drop(catalog);
        assert_eq!(invariants(&root), Vec::<String>::new(), "{k}");
        // The batch being built leaves no file behind.
        let (files, rows) = blobs(&root);
        assert_eq!(i64::try_from(files).unwrap(), rows, "{k}");
        assert_eq!(rows, i64::try_from(committed).unwrap(), "{k}");
    }
}

/// A catalog of the variants in `root`, every entry stale: its
/// properties gone.
fn stale(root: &Path, inputs: &[Input<'_>]) -> Catalog {
    let (mut catalog, _) = Catalog::init(root, Options::default()).unwrap();
    catalog
        .ingest_inputs(inputs, None, &StopAt::new(usize::MAX))
        .unwrap();
    catalog
        .connection()
        .execute("DELETE FROM properties", [])
        .unwrap();
    catalog
}

/// Reindex: stopped before its first entry, inside its first batch,
/// before and after each commit, and at its last question.
fn reindex_stops(dir: &Path) {
    let files = variants();
    let inputs: Vec<Input<'_>> = files
        .iter()
        .map(|(bytes, name)| Input::new(bytes, name, name))
        .collect();
    let all = StopAt::new(usize::MAX);
    let mut catalog = stale(&dir.join("reindex-all"), &inputs);
    let run = catalog
        .reindex_observed(&Scope::Stale, &all)
        .unwrap()
        .unwrap();
    assert!(run.stopped.is_none());
    assert_eq!(run.lines.len(), 66);
    for k in all.points() {
        let root = dir.join(format!("reindex-{k}"));
        let mut catalog = stale(&root, &inputs);
        let stop = StopAt::new(k);
        let run = catalog
            .reindex_observed(&Scope::Stale, &stop)
            .unwrap()
            .unwrap();
        assert_eq!(stop.asked(), k + 1, "stopped at its question");
        let committed = stop.kept();
        assert_eq!(run.lines.len(), committed, "{k}");
        assert!(matches!(run.stopped, Some(Error::Cancelled)), "{k}");
        let rows: i64 = catalog
            .connection()
            .query_row("SELECT count(*) FROM properties", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, i64::try_from(committed).unwrap(), "{k}");
        // Entries still without properties are stale, which verify leaves
        // to `list` (RX2): nothing else is wrong.
        assert_eq!(catalog.verify(false, false).unwrap(), Vec::new(), "{k}");
    }
}

/// A match: stopped before it starts and before each group, one thread or
/// several: no result.
fn match_stops(dir: &Path) {
    let root = dir.join("match");
    let (mut catalog, _) = Catalog::init(&root, Options::default()).unwrap();
    let inputs = [core_tests().join("candidates"), core_tests().join("data")];
    catalog.ingest(&inputs, None).unwrap();
    let bytes = fs::read(core_tests().join("assemblies/s01_bolted.stp")).unwrap();
    let socket = SocketDocument::extract(&bytes, Some("BRACKET"), None).unwrap();
    let matcher = Matcher::new(
        &socket,
        MatchPolicy::default(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let all = StopAt::new(usize::MAX);
    let run = snapshot
        .match_catalog_observed(&matcher, &Selection::Exhaustive, 1, &all)
        .unwrap()
        .unwrap();
    let asked = all.asked();
    assert!(run.judged > 1 && asked == run.judged + 1, "{asked}");
    for jobs in [1, 4] {
        for k in [0, 1, 2, asked - 1] {
            let stop = StopAt::new(k);
            let run = snapshot
                .match_catalog_observed(&matcher, &Selection::Exhaustive, jobs, &stop)
                .unwrap();
            assert!(matches!(run, Err(NotRun::Cancelled)), "{k} on {jobs}");
        }
    }
}

/// Verify: stopped at its first question, its second, half-way and its
/// last: `Error::Cancelled`; asked one question fewer than it asks, it
/// would have stopped at its last, so all of them stop it.
fn verify_stops(dir: &Path) {
    let root = dir.join("verify");
    let (mut catalog, _) = Catalog::init(&root, Options::default()).unwrap();
    let inputs = [core_tests().join("candidates")];
    catalog.ingest(&inputs, None).unwrap();
    let all = StopAt::new(usize::MAX);
    let verified = catalog.verify_observed(true, false, &all).unwrap();
    assert_eq!(verified.problems, Vec::new());
    let asked = all.asked();
    // Four phases, each blob read twice (plain and deep), the 256 fan-outs.
    assert!(asked > 256, "{asked}");
    for k in [0, 1, asked / 2, asked - 1] {
        let stop = StopAt::new(k);
        let stopped = catalog.verify_observed(true, false, &stop);
        assert!(matches!(stopped, Err(Error::Cancelled)), "{k}");
    }
}

/// gc: 1,001 orphans make two unlink batches; stopped before its rows,
/// between the batches, and before its truncation.
fn gc_stops(dir: &Path) {
    for (k, unlinked) in [(0, 0), (1, 0), (2, 1000), (3, 1001)] {
        let root = dir.join(format!("gc-{k}"));
        let (mut catalog, _) = Catalog::init(&root, Options::default()).unwrap();
        for n in 0..1001_u32 {
            let name = format!("{:064x}", u64::from(n) + 1);
            let fan = root.join("blobs/sha256").join(&name[..2]);
            fs::write(fan.join(&name), b"orphan").unwrap();
        }
        let collected = catalog.gc_observed(false, false, &StopAt::new(k)).unwrap();
        assert!(collected.cancelled, "{k}");
        // Its document says so: it removed only what it lists.
        let document = fitment_catalog::document::GcDocument::new(&collected, false, false);
        assert!(document.to_json().contains("\"cancelled\": true"), "{k}");
        assert_eq!(collected.unlinked.len(), unlinked, "{k}");
        let (files, _) = {
            drop(catalog);
            blobs(&root)
        };
        assert_eq!(files, 1001 - unlinked, "{k}");
    }
}

/// VT38: each stop point CA2 names stops the work there.
#[test]
fn each_stop_point_stops() {
    let dir = scratch("stop-points");
    ingest_stops(&dir);
    reindex_stops(&dir);
    match_stops(&dir);
    verify_stops(&dir);
    gc_stops(&dir);
}

/// Panics when told of its second judgment.
struct PanicsWhenTold(AtomicUsize);

impl Observer for PanicsWhenTold {
    fn cancelled(&self) -> bool {
        false
    }

    fn progress(&self, event: Progress<'_>) {
        if let Progress::Judged { .. } = event {
            let n = self.0.fetch_add(1, Ordering::Relaxed);
            assert!(n == 0, "an observer that fails");
        }
    }
}

/// The review of PR 7a: an observer's panic during a catalog match goes
/// on to the caller, on one thread and on four, and is never read as the
/// groups' fitment bugs.
#[test]
fn an_observers_panic_goes_on_whatever_the_threads() {
    let dir = scratch("observer-panic");
    let (mut catalog, _) = Catalog::init(&dir.join("cat"), Options::default()).unwrap();
    let inputs = [core_tests().join("candidates"), core_tests().join("data")];
    catalog.ingest(&inputs, None).unwrap();
    let bytes = fs::read(core_tests().join("assemblies/s01_bolted.stp")).unwrap();
    let socket = SocketDocument::extract(&bytes, Some("BRACKET"), None).unwrap();
    let matcher = Matcher::new(
        &socket,
        MatchPolicy::default(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let quiet = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcomes: Vec<bool> = [1, 4]
        .into_iter()
        .map(|jobs| {
            let observer = PanicsWhenTold(AtomicUsize::new(0));
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                snapshot.match_catalog_observed(&matcher, &Selection::Exhaustive, jobs, &observer)
            }))
            .is_err()
        })
        .collect();
    std::panic::set_hook(quiet);
    assert_eq!(outcomes, [true, true], "panicked on 1 and 4 threads");
}

/// Records every progress event's text.
#[derive(Default)]
struct Heard(Mutex<Vec<String>>);

impl Observer for Heard {
    fn cancelled(&self) -> bool {
        false
    }

    fn progress(&self, event: Progress<'_>) {
        self.0.lock().unwrap().push(format!("{event:?}"));
    }
}

/// The review of PR 7a: a phase of verify that reads blobs, and gc's
/// unlinking, each end with an event whose `done` is its `total`.
#[test]
fn progress_ends_at_its_total() {
    let dir = scratch("progress-total");
    let (mut catalog, _) = Catalog::init(&dir.join("cat"), Options::default()).unwrap();
    catalog
        .ingest(&[core_tests().join("candidates")], None)
        .unwrap();
    let blobs: i64 = catalog
        .connection()
        .query_row("SELECT count(*) FROM blob", [], |r| r.get(0))
        .unwrap();
    let blobs = usize::try_from(blobs).unwrap();
    let heard = Heard::default();
    catalog.verify_observed(true, false, &heard).unwrap();
    catalog.gc_observed(false, false, &heard).unwrap();
    let events = heard.0.lock().unwrap();
    let last = |prefix: &str| {
        events
            .iter()
            .rev()
            .find(|e| e.starts_with(prefix))
            .cloned()
            .unwrap_or_default()
    };
    let total = |n: usize| format!("done: {n}, total: {n} }}");
    assert!(
        last("Verifying { phase: \"blobs\"").ends_with(&total(blobs)),
        "{events:?}"
    );
    assert!(
        last("Verifying { phase: \"deep\"").ends_with(&total(blobs)),
        "{events:?}"
    );
    // gc looks at every file of the store, each one named here.
    assert!(last("Unlinked").ends_with(&total(blobs)), "{events:?}");
}
