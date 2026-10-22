//! CA2 (`docs/research/m6-api.md`, VT38): an observer changes no document.
//! Every document the catalog makes, from the ingest's to each match's,
//! is the same bytes under `NoObserver` as under an observer that records
//! every event it is told: on the committed parts and assemblies always,
//! and on the corpus's part files and sockets in a release build (a debug
//! build skips the corpus with a note, as it takes minutes there).

#![cfg(feature = "catalog")]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use fitment_catalog::document::{CatalogDocument, GcDocument, VerifyDocument};
use fitment_catalog::{Catalog, Input, NoObserver, Observer, Options, Progress, Scope, Selection};
use fitment_core::{Clearance, Grip, SocketDocument, TolerancePolicy};
use fitment_core::{MatchDocument, MatchOptions, MatchPolicy, Matcher, NoFitVerifier};
use fitment_corpus::{Corpus, gate};

/// Records every event, asked or told; never cancels.
#[derive(Default)]
struct Recording {
    asked: AtomicUsize,
    events: Mutex<Vec<String>>,
}

impl Observer for Recording {
    fn cancelled(&self) -> bool {
        self.asked.fetch_add(1, Ordering::Relaxed);
        false
    }

    fn progress(&self, event: Progress<'_>) {
        let text = match event {
            Progress::Ingested(line) => format!("ingested {}", line.input.display()),
            other => format!("{other:?}"),
        };
        self.events.lock().unwrap().push(text);
    }
}

impl Recording {
    fn told(&self, prefix: &str) -> usize {
        let events = self.events.lock().unwrap();
        events.iter().filter(|e| e.starts_with(prefix)).count()
    }
}

/// The committed parts, candidates and assemblies.
fn committed() -> Vec<PathBuf> {
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests");
    ["data", "candidates", "assemblies"]
        .iter()
        .flat_map(|d| {
            fs::read_dir(core.join(d))
                .unwrap()
                .map(|e| e.unwrap().path())
        })
        .filter(|p| p.extension().is_some_and(|e| e == "stp"))
        .collect()
}

/// The committed sockets, each its assembly and part.
fn committed_sockets() -> Vec<(String, SocketDocument)> {
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests/assemblies");
    [
        ("s01_bolted", "BRACKET"),
        ("s01_bolted", "NUT"),
        ("s02_cbore", "BLOCK"),
        ("s03_dowels", "PLATE"),
    ]
    .iter()
    .map(|(case, part)| {
        let bytes = fs::read(core.join(format!("{case}.stp"))).unwrap();
        let socket = SocketDocument::extract(&bytes, Some(part), None).unwrap();
        (format!("{case} {part}"), socket)
    })
    .collect()
}

/// The corpus's sockets, each with its review applied (M6-38).
fn corpus_sockets(corpus: &Corpus) -> Vec<(String, SocketDocument)> {
    let mut out = Vec::new();
    for (key, cases) in corpus.socket_groups() {
        if key.edit.is_some() {
            continue;
        }
        let bytes = fs::read(corpus.dir().join(format!("{}.stp", key.source))).unwrap();
        let Ok(mut document) = SocketDocument::extract(&bytes, Some(&key.part), None) else {
            continue;
        };
        gate::apply_review(cases[0].value(), &mut document).unwrap();
        out.push((format!("{} {}", key.source, key.part), document));
    }
    out
}

/// Every document a catalog of `files` gives under `observer`: the
/// ingest's (from bytes), the list's, a reindex of every entry's, a deep
/// verify's, a gc's, and each socket's full and exhaustive runs under two
/// policies.
fn documents(
    root: &Path,
    files: &[PathBuf],
    sockets: &[(String, SocketDocument)],
    observer: &dyn Observer,
) -> Vec<String> {
    let _ = fs::remove_dir_all(root);
    let (mut catalog, _) = Catalog::init(root, Options::default()).unwrap();
    let read: Vec<(Vec<u8>, String, String)> = files
        .iter()
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (fs::read(p).unwrap(), p.to_string_lossy().into_owned(), name)
        })
        .collect();
    let inputs: Vec<Input<'_>> = read
        .iter()
        .map(|(bytes, input, name)| Input::new(bytes, input, name))
        .collect();
    // One file to an ingest, one batch, so the two catalogs' generations
    // agree: a batch also ends after a second (DB3), at a time of its own.
    let mut out = Vec::new();
    for input in &inputs {
        let run = catalog
            .ingest_inputs(std::slice::from_ref(input), None, observer)
            .unwrap();
        assert!(run.stopped.is_none());
        let snapshot = catalog.snapshot().unwrap();
        let document = CatalogDocument::ingested(&snapshot, &run).unwrap();
        out.push(document.to_json());
    }
    let snapshot = catalog.snapshot().unwrap();
    let all = snapshot.list().unwrap();
    let entries = all
        .iter()
        .map(|l| fitment_catalog::document::Entry::of(l, None));
    let list = CatalogDocument::new(snapshot.generation(), &all, entries.collect(), None);
    out.push(list.to_json());
    drop(snapshot);
    let reindex = catalog
        .reindex_observed(&Scope::All, observer)
        .unwrap()
        .unwrap();
    let snapshot = catalog.snapshot().unwrap();
    out.push(
        CatalogDocument::reindexed(&snapshot, &reindex)
            .unwrap()
            .to_json(),
    );
    drop(snapshot);
    let verified = catalog.verify_observed(true, false, observer).unwrap();
    out.push(VerifyDocument::new(&verified, true, false).to_json());
    let collected = catalog.gc_observed(true, false, observer).unwrap();
    out.push(GcDocument::new(&collected, true, false).to_json());
    let functional = MatchPolicy::new(
        TolerancePolicy::default(),
        Clearance::Functional,
        Grip::Hard,
    );
    for (name, socket) in sockets {
        for policy in [MatchPolicy::default(), functional.clone()] {
            // A refused socket document judges nothing (VD9): its refusal
            // is what both runs give.
            let matcher =
                match Matcher::new(socket, policy, MatchOptions::default(), &NoFitVerifier) {
                    Ok(matcher) => matcher,
                    Err(refused) => {
                        out.push(format!("{name}: {refused:?}"));
                        continue;
                    }
                };
            let snapshot = catalog.snapshot().unwrap();
            for selection in [Selection::Full, Selection::Exhaustive] {
                let run = snapshot
                    .match_catalog_observed(&matcher, &selection, 4, observer)
                    .unwrap();
                out.push(match run {
                    Ok(run) => {
                        // The run's own facts, beside its document.
                        let facts = format!(
                            "{name}: generation {}, level 1 {:?}, selected {}, judged {}, \
                             corrupt {:?}, bugs {:?}\n",
                            run.generation,
                            run.level_one,
                            run.selected,
                            run.judged,
                            run.corrupt,
                            run.bugs
                        );
                        facts + &MatchDocument::new(&matcher, run.results).to_json()
                    }
                    Err(refused) => format!("{name}: {refused:?}"),
                });
            }
        }
    }
    out
}

/// The documents under `NoObserver` and under a recording observer, the
/// same bytes; and the recorder was asked and told.
fn same_under_both(test: &str, files: &[PathBuf], sockets: &[(String, SocketDocument)]) {
    let scratch =
        std::env::temp_dir().join(format!("fitment-observer-{test}-{}", std::process::id()));
    let quiet = documents(&scratch.join("quiet"), files, sockets, &NoObserver);
    let recording = Recording::default();
    let heard = documents(&scratch.join("heard"), files, sockets, &recording);
    assert_eq!(quiet.len(), heard.len());
    for (a, b) in quiet.iter().zip(&heard) {
        assert!(
            a == b,
            "{test}: a document differs under an observer:\n{a}\n{b}"
        );
    }
    assert_eq!(recording.told("ingested "), files.len());
    assert!(
        recording.told("Committed") > files.len(),
        "ingest's and reindex's batches"
    );
    assert!(recording.told("Judged") > 0);
    assert!(recording.told("Verifying") > 0);
    assert!(recording.asked.load(Ordering::Relaxed) > files.len());
    let _ = fs::remove_dir_all(&scratch);
}

/// VT38: every document the same under an observer, on the committed
/// files; and on the corpus in a release build.
#[test]
fn an_observer_changes_no_document() {
    same_under_both("committed", &committed(), &committed_sockets());
    let Some(corpus) = Corpus::locate_release_only() else {
        return;
    };
    // CT10's hostile parts aside, as `part_files` leaves them.
    let files = corpus.part_files();
    assert!(files.len() > 100);
    same_under_both("corpus", &files, &corpus_sockets(&corpus));
}
