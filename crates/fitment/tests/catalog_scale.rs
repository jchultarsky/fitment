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
//! Without `FITMENT_SCALE_DIR` it passes with a note.

#![cfg(feature = "catalog")]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fitment_catalog::{Catalog, NotRun, Options, Selection};
use fitment_core::{
    MatchDocument, MatchOptions, MatchPolicy, Matcher, NoFitVerifier, SocketDocument, VerdictKind,
};
use serde_json::Value;

/// Threads per run, as the design's ceiling is stated.
const JOBS: usize = 16;

/// The ceiling per query, at 10⁴ parts: an order of magnitude over the
/// prototype's timings, so only a regression of that size fails.
fn ceiling(parts: usize) -> Duration {
    Duration::from_secs(10 * u64::try_from(parts.div_ceil(10_000)).unwrap_or(1))
}

fn corpus() -> PathBuf {
    std::env::var_os("FITMENT_CORPUS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/out"),
        PathBuf::from,
    )
}

/// The corpus's match sockets, by case and part.
fn sockets(corpus: &Path) -> BTreeSet<(String, String)> {
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(corpus.join("manifest.json")).unwrap()).unwrap();
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

#[test]
fn a_scaled_catalog_is_the_file_run_restricted() {
    let Some(scaled) = std::env::var_os("FITMENT_SCALE_DIR").map(PathBuf::from) else {
        eprintln!("skipped: set FITMENT_SCALE_DIR (tools/scale-step.py)");
        return;
    };
    let corpus = corpus();
    let root = std::env::temp_dir().join(format!("fitment-scale-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let (mut catalog, _) = Catalog::init(&root, Options::default()).unwrap();
    let started = Instant::now();
    let run = catalog.ingest(&[scaled], None).unwrap();
    let parts = run.iter().filter(|l| l.revision.is_some()).count();
    eprintln!("ingest: {parts} parts in {:?}", started.elapsed());
    let started = Instant::now();
    let problems = catalog.verify(false, false).unwrap();
    eprintln!("verify: {:?}", started.elapsed());
    assert_eq!(problems, Vec::new());

    let mut slowest = (Duration::ZERO, String::new());
    let mut checked = 0;
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
        eprintln!(
            "{name:<32} level 1 kept {survivors:>6} in {full_time:>10.2?}; exhaustive {:>6} in {every_time:>10.2?}",
            every.results.len()
        );
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
    eprintln!("slowest full run: {} in {:?}", slowest.1, slowest.0);
    assert!(checked > 20, "{checked} sockets");
    let _ = fs::remove_dir_all(&root);
}
