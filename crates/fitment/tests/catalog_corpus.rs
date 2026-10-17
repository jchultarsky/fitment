//! M5's exit test on the corpus (`docs/research/m5-catalog.md`, CT1, CT2,
//! section 4.17): every corpus part file, the hostile ones aside (CT10's),
//! ingested into one catalog; for every match case's socket under both
//! policies, level 1 over the stored index admits what it admits over the
//! same tokens in memory (CT3), a full run is the file run over level 1's
//! survivors, digest
//! for digest, and holds every candidate the file run over all files
//! passes; an exhaustive run is the file run over every live file. Without
//! the corpus this passes with a note, unless `FITMENT_REQUIRE_CORPUS=1`;
//! in a debug build it passes with a note, as it takes minutes there.

#![cfg(feature = "catalog")]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use fitment_catalog::{Catalog, NotRun, Options, Selection};
use fitment_core::{
    CandidateResult, MatchDocument, MatchOptions, MatchPolicy, Matcher, NoFitVerifier,
    SocketDocument, VerdictKind,
};
use fitment_corpus::{Corpus, Policy, gate};

/// The corpus, or `None` with a note; in a debug build too, where this
/// takes minutes (SC7).
fn corpus() -> Option<Corpus> {
    Corpus::locate_release_only()
}

/// Every file judged, in parallel, as `fitment match` over it would.
fn judge_files(matcher: &Matcher<'_>, files: &[(String, Vec<u8>)]) -> Vec<CandidateResult> {
    let chunk = files.len().div_ceil(16).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|(_, b)| matcher.judge_step(b))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    })
}

/// The sockets the corpus's match cases are judged against, each with its
/// review applied (M6-38); the socket gate's edits aside, as the catalog
/// judges the socket as extracted.
fn sockets(corpus: &Corpus) -> Vec<(String, SocketDocument)> {
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
        out.push((
            format!("{} {} {}", key.source, key.part, key.review),
            document,
        ));
    }
    out
}

/// A catalog of the corpus part files `hostile` or not (CT10's `h_`
/// parts), in `scratch`; and each live entry's label with its file's
/// bytes.
fn ingest(dir: &Path, scratch: &Path, hostile: bool) -> (Catalog, Vec<(String, Vec<u8>)>) {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "stp"))
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("h_") == hostile)
        .collect();
    paths.sort();
    let _ = fs::remove_dir_all(scratch);
    let (mut catalog, _) = Catalog::init(scratch, Options::default()).unwrap();
    let run = catalog.ingest(&paths, None).unwrap();
    let files: BTreeMap<String, Vec<u8>> = run
        .iter()
        .filter(|l| l.revision.is_some())
        .map(|l| {
            let label = format!(
                "catalog:{}@r{}",
                l.id.as_ref().unwrap(),
                l.revision.unwrap()
            );
            (label, fs::read(&l.input).unwrap())
        })
        .collect();
    (catalog, files.into_iter().collect())
}

/// CT3: the catalog's tokens, in memory, to compare level 1's two sources.
fn in_memory(catalog: &Catalog) -> fitment_core::ShortlistIndex {
    let mut memory = fitment_core::ShortlistIndex::new();
    let conn = catalog.connection();
    let mut stmt = conn
        .prepare("SELECT b.key, t.tokens FROM tokens t JOIN blob b ON b.sha256 = t.sha256")
        .unwrap();
    let rows: Vec<(i64, Vec<u8>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (key, tokens) in rows {
        let text = std::str::from_utf8(&tokens).unwrap();
        let tokens = fitment_core::IndexTokens::from_json(text).unwrap();
        memory.insert(u64::try_from(key).unwrap(), &tokens);
    }
    memory
}

/// One socket under one policy: CT3, CT1 and CT2.
fn check(
    catalog: &mut Catalog,
    memory: &fitment_core::ShortlistIndex,
    matcher: &Matcher<'_>,
    all: &[(String, Vec<u8>)],
    name: &str,
) -> (std::time::Duration, std::time::Duration) {
    let judged = judge_files(matcher, all);
    let file_run = |labels: &[&String]| {
        let results = labels
            .iter()
            .map(|l| {
                let at = all.iter().position(|(m, _)| m == *l).unwrap();
                (judged[at].clone(), vec![(*l).clone()])
            })
            .collect();
        MatchDocument::new(matcher, results)
    };
    let everything = file_run(&all.iter().map(|(l, _)| l).collect::<Vec<_>>());
    let snapshot = catalog.snapshot().unwrap();
    assert_eq!(
        snapshot.admitted(matcher).unwrap(),
        matcher.admitted(memory).unwrap(),
        "{name}: level 1's two sources"
    );
    let started = std::time::Instant::now();
    let full = snapshot
        .match_catalog(matcher, &Selection::Full, 16)
        .unwrap();
    let full_time = started.elapsed();
    match full {
        Ok(full) => {
            assert!(full.corrupt.is_empty(), "{name}: {:?}", full.corrupt);
            let survivors: Vec<&String> = full.results.iter().map(|(_, l)| &l[0]).collect();
            let catalog_doc = MatchDocument::new(matcher, full.results.clone());
            assert_eq!(
                catalog_doc.digest(),
                file_run(&survivors).digest(),
                "{name}"
            );
            for ((label, _), result) in all.iter().zip(&judged) {
                if result.verdict.kind() != VerdictKind::Reject {
                    assert!(survivors.contains(&label), "{name}: {label} hidden");
                }
            }
        }
        // A blocked socket passes nothing (IX8).
        Err(NotRun::Blocked { .. }) => assert!(
            judged
                .iter()
                .all(|r| r.verdict.kind() == VerdictKind::Reject),
            "{name}: blocked, yet a file passes"
        ),
        Err(other) => panic!("{name}: {other:?}"),
    }
    let started = std::time::Instant::now();
    let exhaustive = snapshot
        .match_catalog(matcher, &Selection::Exhaustive, 16)
        .unwrap()
        .unwrap();
    let every_time = started.elapsed();
    let exhaustive = MatchDocument::new(matcher, exhaustive.results);
    assert_eq!(
        exhaustive.digest(),
        everything.digest(),
        "{name}: exhaustive"
    );
    (full_time, every_time)
}

#[test]
fn the_corpus_through_a_catalog_is_the_file_run() {
    let Some(corpus) = corpus() else {
        return;
    };
    let scratch =
        std::env::temp_dir().join(format!("fitment-catalog-corpus-{}", std::process::id()));
    let (mut catalog, all) = ingest(corpus.dir(), &scratch, false);
    let memory = in_memory(&catalog);
    // The wider window is `--tolerance-mm 0.05`'s (review, F2).
    let policies = [Policy::Default, Policy::Functional, Policy::Tolerance005];
    let mut checked = 0;
    for (name, document) in sockets(&corpus) {
        for policy in policies {
            let Ok(matcher) = Matcher::new(
                &document,
                policy.match_policy(),
                MatchOptions::default(),
                &NoFitVerifier,
            ) else {
                continue;
            };
            let _ = check(&mut catalog, &memory, &matcher, &all, &name);
            checked += 1;
        }
    }
    assert!(checked > 60, "{checked} socket runs");
    let _ = fs::remove_dir_all(&scratch);
}

/// CT10 and QR9: a catalog of the hostile parts alone, every corpus
/// socket run against it: each catalog run, full and exhaustive, within
/// M4's hostile bound of 30 s, and each the file run over the same files
/// (CT1, CT2, CT3).
#[test]
fn hostile_parts_through_a_catalog_are_the_file_run() {
    let Some(corpus) = corpus() else {
        return;
    };
    let scratch =
        std::env::temp_dir().join(format!("fitment-catalog-hostile-{}", std::process::id()));
    let (mut catalog, all) = ingest(corpus.dir(), &scratch, true);
    assert_eq!(all.len(), 4, "the four h_ parts");
    let memory = in_memory(&catalog);
    let mut checked = 0;
    for (name, document) in sockets(&corpus) {
        let Ok(matcher) = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            &NoFitVerifier,
        ) else {
            continue;
        };
        let (full, every) = check(&mut catalog, &memory, &matcher, &all, &name);
        // Each catalog run within M4's hostile bound.
        let bound = std::time::Duration::from_secs(30);
        assert!(full < bound && every < bound, "{name}: {full:?}, {every:?}");
        checked += 1;
    }
    assert!(checked > 20, "{checked} sockets");
    let _ = fs::remove_dir_all(&scratch);
}
