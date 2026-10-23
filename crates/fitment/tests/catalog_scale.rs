//! CT11, scale (`docs/research/m5-catalog.md`, section 8.3; weekly CI, or
//! by hand): a catalog of `tools/scale-step.py`'s scaled copies, every
//! corpus socket run against it with level 1 and with `--exhaustive`. The
//! full run must be the exhaustive run restricted to level 1's survivors,
//! digest for digest, and hide no pass (CT1, CT2). Timings are reported;
//! the one gate is a ceiling against an order-of-magnitude regression.
//!
//! ```text
//! tools/scale-step.py corpus/out scaled 10000 --manifest corpus/out/manifest.json
//! FITMENT_SCALE_DIR=scaled cargo test --release -p fitment --test catalog_scale -- --nocapture
//! ```
//!
//! VT79 (`docs/research/m7-fit.md`, R8), the weekly fit leg: the same
//! catalog with the real helper (`FITMENT_FIT_DIR`, a directory `fitment
//! fit setup` made), every corpus socket's full run with the fit the file
//! run with the fit over level 1's survivors, digest for digest; the
//! fits asked, their time each and in all reported.
//!
//! Without `FITMENT_SCALE_DIR` (and for VT79 `FITMENT_FIT_DIR`) they pass
//! with a note.

#![cfg(feature = "catalog")]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fitment_catalog::{Catalog, NotRun, Options, Selection};
use fitment_core::{
    MatchDocument, MatchOptions, MatchPolicy, Matcher, NoFitVerifier, SocketDocument, VerdictKind,
};
use fitment_corpus::Corpus;

/// Threads per run, as the design's ceiling is stated.
const JOBS: usize = 16;

/// The ceiling per query, at 10⁴ parts: an order of magnitude over the
/// prototype's timings, so only a regression of that size fails.
fn ceiling(parts: usize) -> Duration {
    Duration::from_secs(10 * u64::try_from(parts.div_ceil(10_000)).unwrap_or(1))
}

/// The corpus's directory (`docs/research/m6-api.md`, SC7): a scale run
/// needs its sockets.
fn corpus() -> PathBuf {
    Corpus::at(&Corpus::default_dir(), true)
        .expect("a required corpus")
        .dir()
        .to_path_buf()
}

/// The corpus's match sockets, by case and part.
fn sockets(corpus: &Path) -> BTreeSet<(String, String)> {
    let corpus = Corpus::at(corpus, true).expect("a required corpus");
    corpus
        .socket_groups()
        .into_keys()
        .map(|k| (k.source, k.part))
        .collect()
}

/// A catalog in `root` of the files in `scaled`, verified; and how many
/// entries it holds.
fn build(root: &Path, scaled: PathBuf) -> (Catalog, usize) {
    let (catalog, parts, _) = build_named(root, scaled);
    (catalog, parts)
}

/// As [`build`], with each entry's file by its label (`catalog:<id>@r1`).
fn build_named(root: &Path, scaled: PathBuf) -> (Catalog, usize, BTreeMap<String, PathBuf>) {
    let _ = fs::remove_dir_all(root);
    let (mut catalog, _) = Catalog::init(root, Options::default()).unwrap();
    let started = Instant::now();
    let run = catalog.ingest(&[scaled], None).unwrap();
    let parts = run.iter().filter(|l| l.revision.is_some()).count();
    let files: BTreeMap<String, PathBuf> = run
        .iter()
        .filter(|l| l.revision.is_some())
        .filter_map(|l| Some((format!("catalog:{}@r1", l.id.as_ref()?), l.input.clone())))
        .collect();
    let quarantined = run
        .iter()
        .filter(|l| l.revision.is_some())
        .filter(|l| {
            l.summary
                .as_ref()
                .is_some_and(|s| s.status == fitment_catalog::Status::Refused)
        })
        .count();
    eprintln!(
        "ingest: {parts} entries ({quarantined} quarantined) in {:?}",
        started.elapsed()
    );
    let started = Instant::now();
    let problems = catalog.verify(false, false).unwrap();
    eprintln!("verify: {:?}", started.elapsed());
    assert_eq!(problems, Vec::new());
    (catalog, parts, files)
}

#[test]
fn a_scaled_catalog_is_the_file_run_restricted() {
    let Some(scaled) = std::env::var_os("FITMENT_SCALE_DIR").map(PathBuf::from) else {
        eprintln!("skipped: set FITMENT_SCALE_DIR (tools/scale-step.py)");
        return;
    };
    let corpus = corpus();
    let root = std::env::temp_dir().join(format!("fitment-scale-{}", std::process::id()));
    let (mut catalog, parts) = build(&root, scaled);

    let mut slowest = (Duration::ZERO, String::new());
    let mut checked = 0;
    let mut total_passes = 0;
    for (case, part) in sockets(&corpus) {
        let bytes = fs::read(corpus.join(format!("{case}.stp"))).unwrap();
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
        let name = format!("{case} {part}");
        let snapshot = catalog.snapshot().unwrap();
        let started = Instant::now();
        let full = snapshot
            .match_catalog(&matcher, &Selection::Full, JOBS)
            .unwrap();
        let full_time = started.elapsed();
        let started = Instant::now();
        let every = snapshot
            .match_catalog(&matcher, &Selection::Exhaustive, JOBS)
            .unwrap()
            .unwrap();
        let every_time = started.elapsed();
        let survivors = match &full {
            Ok(run) => {
                assert!(run.corrupt.is_empty(), "{name}");
                run.results.len()
            }
            Err(NotRun::Blocked { .. }) => 0,
            Err(other) => panic!("{name}: {other:?}"),
        };
        let passes = every
            .results
            .iter()
            .filter(|(r, _)| r.verdict.kind() != VerdictKind::Reject)
            .count();
        eprintln!(
            "{name:<32} level 1 kept {survivors:>6} in {full_time:>10.2?}; exhaustive {:>6} in {every_time:>10.2?}, {passes} pass",
            every.results.len()
        );
        total_passes += passes;
        if full_time > slowest.0 {
            slowest = (full_time, name.clone());
        }
        assert!(full_time < ceiling(parts), "{name}: {full_time:?}");
        let Ok(full) = full else {
            assert!(
                every
                    .results
                    .iter()
                    .all(|(r, _)| r.verdict.kind() == VerdictKind::Reject),
                "{name}: blocked, yet a part passes"
            );
            continue;
        };
        let kept: BTreeSet<&String> = full.results.iter().map(|(_, l)| &l[0]).collect();
        for (result, labels) in &every.results {
            if result.verdict.kind() != VerdictKind::Reject {
                assert!(kept.contains(&labels[0]), "{name}: {} hidden", labels[0]);
            }
        }
        let restricted: Vec<_> = every
            .results
            .iter()
            .filter(|(_, l)| kept.contains(&l[0]))
            .cloned()
            .collect();
        assert_eq!(
            MatchDocument::new(&matcher, full.results).digest(),
            MatchDocument::new(&matcher, restricted).digest(),
            "{name}"
        );
        checked += 1;
    }
    eprintln!(
        "slowest full run: {} in {:?}; {total_passes} passes, none hidden",
        slowest.1, slowest.0
    );
    // Sockets compared and passes found, so the run is never vacuous.
    assert!(checked > 15, "{checked} sockets");
    assert!(total_passes > 0, "no pass to hide");
    let _ = fs::remove_dir_all(&root);
}

/// VT79: with the real helper, each corpus socket's full run is the file
/// run over level 1's survivors, digest for digest, every fit complete;
/// the fits asked (the stored files read), their time and the runs' time
/// reported (R8).
#[cfg(feature = "fit")]
#[test]
fn a_scaled_catalog_with_the_fit_is_the_file_run_restricted() {
    use fitment_catalog::NoObserver;
    use fitment_fit::{HelperFiles, HelperOptions, OcctHelper, OcctSession};
    let (Some(scaled), Some(fit)) = (
        std::env::var_os("FITMENT_SCALE_DIR").map(PathBuf::from),
        std::env::var_os("FITMENT_FIT_DIR").map(PathBuf::from),
    ) else {
        eprintln!("skipped: set FITMENT_SCALE_DIR and FITMENT_FIT_DIR (fitment fit setup)");
        return;
    };
    let corpus = corpus();
    let root = std::env::temp_dir().join(format!("fitment-scale-fit-{}", std::process::id()));
    let (mut catalog, _, files) = build_named(&root, scaled);
    let helper = OcctHelper::open(
        &fit,
        HelperFiles::embedded(),
        HelperOptions::default().with_jobs(4),
    )
    .unwrap();
    let (mut checked, mut fits, mut fit_time) = (0, 0, Duration::ZERO);
    let started = Instant::now();
    for (case, part) in sockets(&corpus) {
        let bytes = fs::read(corpus.join(format!("{case}.stp"))).unwrap();
        let Ok(document) = SocketDocument::extract(&bytes, Some(&part), None) else {
            continue;
        };
        let Ok(session) = OcctSession::open(&helper, bytes.clone(), &document) else {
            continue;
        };
        let Ok(matcher) = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            &session,
        ) else {
            continue;
        };
        let blobs = catalog.fit_blobs();
        let run_started = Instant::now();
        let full = catalog
            .snapshot()
            .unwrap()
            .match_catalog_fitted(&matcher, &Selection::Full, JOBS, &NoObserver, &blobs)
            .unwrap();
        let Ok(full) = full else {
            continue;
        };
        let run_time = run_started.elapsed();
        let restricted: Vec<_> = full
            .results
            .iter()
            .map(|(_, labels)| {
                (
                    matcher.judge_step(&fs::read(&files[&labels[0]]).unwrap()),
                    labels.clone(),
                )
            })
            .collect();
        assert!(
            full.corrupt.is_empty() && full.bugs.is_empty(),
            "{case} {part}"
        );
        assert!(
            !matcher.fit_incomplete(full.results.iter().map(|(r, _)| r)),
            "{case} {part}"
        );
        assert_eq!(
            MatchDocument::new(&matcher, full.results).digest(),
            MatchDocument::new(&matcher, restricted).digest(),
            "{case} {part}"
        );
        if blobs.reads() > 0 {
            eprintln!(
                "{case:<24} {part:<12} {} fits in {run_time:.2?}",
                blobs.reads()
            );
        }
        fits += blobs.reads();
        fit_time += run_time;
        checked += 1;
    }
    eprintln!(
        "the fit at scale: {checked} sockets, {fits} fits; the full runs {fit_time:.2?} in all, at most {:.3} s a fit; the test {:.2?}",
        if fits == 0 {
            0.0
        } else {
            fit_time.as_secs_f64() / f64::from(u32::try_from(fits).unwrap_or(u32::MAX))
        },
        started.elapsed()
    );
    assert!(checked > 15, "{checked} sockets");
    assert!(fits > 0, "no fit asked");
    let _ = fs::remove_dir_all(&root);
}
