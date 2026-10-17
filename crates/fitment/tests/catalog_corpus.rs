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

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use fitment_catalog::{Catalog, NotRun, Options, Selection};
use fitment_core::{
    CandidateResult, Clearance, Grip, MatchDocument, MatchOptions, MatchPolicy, Matcher,
    NoFitVerifier, SocketDocument, TolerancePolicy, VerdictKind,
};
use serde_json::Value;

fn corpus() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        eprintln!("skipped: a debug build; run with --release");
        return None;
    }
    let dir = std::env::var_os("FITMENT_CORPUS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/out"),
        PathBuf::from,
    );
    let required = std::env::var_os("FITMENT_REQUIRE_CORPUS").is_some_and(|v| v == "1");
    if dir.join("manifest.json").is_file() {
        return Some(dir);
    }
    assert!(!required, "FITMENT_REQUIRE_CORPUS=1 but there is no corpus");
    eprintln!("skipped: no corpus; run tools/gen-corpus.py corpus/out");
    None
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

/// The sockets the corpus's match cases are judged against, by case.
fn sockets(dir: &Path) -> BTreeSet<(String, String)> {
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    manifest["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "match")
        .map(|c| {
            let s = &c["socket"];
            (
                s["case"].as_str().unwrap().to_owned(),
                s["part"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
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
) {
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
    match snapshot
        .match_catalog(matcher, &Selection::Full, 16)
        .unwrap()
    {
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
    let exhaustive = snapshot
        .match_catalog(matcher, &Selection::Exhaustive, 16)
        .unwrap()
        .unwrap();
    let exhaustive = MatchDocument::new(matcher, exhaustive.results);
    assert_eq!(
        exhaustive.digest(),
        everything.digest(),
        "{name}: exhaustive"
    );
}

#[test]
fn the_corpus_through_a_catalog_is_the_file_run() {
    let Some(dir) = corpus() else {
        return;
    };
    let scratch =
        std::env::temp_dir().join(format!("fitment-catalog-corpus-{}", std::process::id()));
    let (mut catalog, all) = ingest(&dir, &scratch, false);
    let memory = in_memory(&catalog);
    let policies = [
        MatchPolicy::default(),
        MatchPolicy::new(
            TolerancePolicy::default(),
            Clearance::Functional,
            Grip::Hard,
        ),
        // A wider window, as `--tolerance-mm 0.05` gives (review, F2).
        MatchPolicy::new(
            TolerancePolicy::new("cli", 0.05, TolerancePolicy::default().angular_deg()).unwrap(),
            Clearance::Strict,
            Grip::Hard,
        ),
    ];
    let mut checked = 0;
    for (case, part) in sockets(&dir) {
        let bytes = fs::read(dir.join(format!("{case}.stp"))).unwrap();
        let Ok(document) = SocketDocument::extract(&bytes, Some(&part), None) else {
            continue;
        };
        for policy in &policies {
            let Ok(matcher) = Matcher::new(
                &document,
                policy.clone(),
                MatchOptions::default(),
                &NoFitVerifier,
            ) else {
                continue;
            };
            check(
                &mut catalog,
                &memory,
                &matcher,
                &all,
                &format!("{case} {part}"),
            );
            checked += 1;
        }
    }
    assert!(checked > 60, "{checked} socket runs");
    let _ = fs::remove_dir_all(&scratch);
}

/// CT10 and QR9: a catalog of the hostile parts alone, every corpus
/// socket run against it: each run within M4's hostile bound of 30 s, and
/// each the file run over the same files (CT1, CT2, CT3).
#[test]
fn hostile_parts_through_a_catalog_are_the_file_run() {
    let Some(dir) = corpus() else {
        return;
    };
    let scratch =
        std::env::temp_dir().join(format!("fitment-catalog-hostile-{}", std::process::id()));
    let (mut catalog, all) = ingest(&dir, &scratch, true);
    assert_eq!(all.len(), 4, "the four h_ parts");
    let memory = in_memory(&catalog);
    let mut checked = 0;
    for (case, part) in sockets(&dir) {
        let bytes = fs::read(dir.join(format!("{case}.stp"))).unwrap();
        let Ok(document) = SocketDocument::extract(&bytes, Some(&part), None) else {
            continue;
        };
        let Ok(matcher) = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            &NoFitVerifier,
        ) else {
            continue;
        };
        let started = std::time::Instant::now();
        check(
            &mut catalog,
            &memory,
            &matcher,
            &all,
            &format!("{case} {part}"),
        );
        // The file runs, the catalog's full and exhaustive runs together.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(90),
            "{case} {part}: {:?}",
            started.elapsed()
        );
        checked += 1;
    }
    assert!(checked > 20, "{checked} sockets");
    let _ = fs::remove_dir_all(&scratch);
}
