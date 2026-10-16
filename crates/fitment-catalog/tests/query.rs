//! A match against the catalog (`docs/research/m5-catalog.md`, CM2, CM4,
//! QR1 to QR11, CT1, CT2, CT6, CT7 on the committed data): a full run is
//! the file run over level 1's survivors, digest for digest; every pass of
//! the file run is among them; an exhaustive run is the file run over every
//! live file; no blob is opened; damaged rows are reported, not judged.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use common::{Shared, catalog, core_tests, scratch};
use fitment_catalog::fs::CountingFs;
use fitment_catalog::{Catalog, NotRun, Options, Selection};
use fitment_core::{
    Clearance, Grip, MatchDocument, MatchOptions, MatchPolicy, Matcher, NoFitVerifier,
    SocketDocument, TolerancePolicy, VerdictKind,
};

const SOCKETS: [(&str, &str); 4] = [
    ("s01_bolted", "BRACKET"),
    ("s01_bolted", "NUT"),
    ("s02_cbore", "BLOCK"),
    ("s03_dowels", "PLATE"),
];

fn socket(case: &str, part: &str) -> SocketDocument {
    let bytes = fs::read(core_tests().join("assemblies").join(format!("{case}.stp"))).unwrap();
    SocketDocument::extract(&bytes, Some(part), None).unwrap()
}

fn policies() -> [MatchPolicy; 2] {
    [
        MatchPolicy::default(),
        MatchPolicy::new(
            TolerancePolicy::default(),
            Clearance::Functional,
            Grip::Hard,
        ),
    ]
}

/// A catalog of every committed candidate and unit part, and each entry's
/// file by its label.
fn committed(test: &str) -> (Catalog, BTreeMap<String, PathBuf>) {
    let dir = scratch(test);
    let mut catalog = catalog(&dir);
    let inputs = [core_tests().join("candidates"), core_tests().join("data")];
    let run = catalog.ingest(&inputs, None).unwrap();
    let files = run
        .iter()
        .filter(|l| l.revision.is_some())
        .map(|l| {
            let id = l.id.as_ref().unwrap();
            (format!("catalog:{id}@r1"), l.input.clone())
        })
        .collect();
    (catalog, files)
}

/// CT1 and CM2, CM4 (and CT2): for every committed socket and both
/// policies, the full run's document is the file run's over its survivors,
/// and holds every candidate the file run over all files passes; the
/// exhaustive run's is the file run's over all.
#[test]
fn a_catalog_run_is_the_file_run_restricted() {
    let (mut catalog, files) = committed("restricted");
    let all: Vec<(&String, &PathBuf)> = files.iter().collect();
    let mut passes = 0;
    for (case, part) in SOCKETS {
        let document = socket(case, part);
        for policy in policies() {
            let matcher =
                Matcher::new(&document, policy, MatchOptions::default(), &NoFitVerifier).unwrap();
            let snapshot = catalog.snapshot().unwrap();
            let full = snapshot
                .match_catalog(&matcher, &Selection::Full, 2)
                .unwrap()
                .unwrap();
            assert!(full.corrupt.is_empty(), "{case} {part}: {:?}", full.corrupt);
            // Each file judged once, as `fitment match` over it would.
            let judged: BTreeMap<&String, _> = all
                .iter()
                .map(|(label, path)| (*label, matcher.judge_step(&fs::read(path).unwrap())))
                .collect();
            let labelled = |labels: &mut dyn Iterator<Item = &String>| {
                let results = labels
                    .map(|l| (judged[l].clone(), vec![l.clone()]))
                    .collect();
                MatchDocument::new(&matcher, results)
            };
            let catalog_doc = MatchDocument::new(&matcher, full.results.clone());
            let restricted = labelled(&mut full.results.iter().map(|(_, l)| &l[0]));
            assert_eq!(catalog_doc.to_json(), restricted.to_json(), "{case} {part}");
            let everything = labelled(&mut all.iter().map(|(l, _)| *l));
            for (label, result) in &judged {
                if result.verdict.kind() != VerdictKind::Reject {
                    passes += 1;
                    assert!(
                        full.results.iter().any(|(_, l)| l[0] == **label),
                        "{case} {part}: {label} passes, and level 1 hid it"
                    );
                }
            }
            let exhaustive = snapshot
                .match_catalog(&matcher, &Selection::Exhaustive, 3)
                .unwrap()
                .unwrap();
            let exhaustive = MatchDocument::new(&matcher, exhaustive.results);
            assert_eq!(exhaustive.digest(), everything.digest(), "{case} {part}");
        }
    }
    assert!(passes > 0);
}

/// CT6 (QR11): a catalog run, full or exhaustive, makes no file call at
/// all: it reads only the index.
#[test]
fn a_match_reads_only_the_index() {
    let (catalog, _) = committed("index-only");
    let root = catalog.root().to_owned();
    drop(catalog);
    let seam = Arc::new(CountingFs::new());
    let mut counted =
        Catalog::open_with(&root, Options::default(), Box::new(Shared(seam.clone()))).unwrap();
    let document = socket("s01_bolted", "BRACKET");
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .unwrap();
    let opened = seam.calls().len();
    let snapshot = counted.snapshot().unwrap();
    for selection in [Selection::Full, Selection::Exhaustive] {
        let run = snapshot
            .match_catalog(&matcher, &selection, 2)
            .unwrap()
            .unwrap();
        assert_ne!(run.results.len(), 0);
    }
    assert_eq!(seam.calls().len(), opened, "{:?}", &seam.calls()[opened..]);
}

/// RX3 and QR1: a stale entry refuses a full run; a named run of another
/// entry runs; a catalog of quarantined entries alone refuses a full run.
#[test]
fn stale_and_empty_catalogs_refuse_a_full_run() {
    let (mut catalog, _) = committed("stale");
    let document = socket("s01_bolted", "BRACKET");
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .unwrap();
    catalog
        .connection()
        .execute_batch(
            "DROP TRIGGER features_immutable; PRAGMA foreign_keys = OFF;
             UPDATE features SET extractor_version = extractor_version - 1
             WHERE sha256 = (SELECT sha256 FROM live WHERE id = 'm_block02');",
        )
        .unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let full = snapshot
        .match_catalog(&matcher, &Selection::Full, 1)
        .unwrap();
    assert!(
        matches!(full, Err(NotRun::Stale { count: 1, .. })),
        "{full:?}"
    );
    let bracket = fitment_catalog::Reference::parse("m_bracket").unwrap();
    let named = snapshot
        .match_catalog(&matcher, &Selection::Named(vec![bracket]), 1)
        .unwrap()
        .unwrap();
    assert_eq!(named.results.len(), 1);
    drop(snapshot);

    let dir = scratch("empty");
    let mut empty = common::catalog(&dir);
    let junk = dir.join("junk.stp");
    fs::write(&junk, "not STEP").unwrap();
    empty.ingest(&[junk], None).unwrap();
    let snapshot = empty.snapshot().unwrap();
    let full = snapshot
        .match_catalog(&matcher, &Selection::Full, 1)
        .unwrap();
    assert!(
        matches!(full, Err(NotRun::Empty { entries: 1 })),
        "{full:?}"
    );
}

/// QR4: a features row moved to another key is not judged, and named;
/// the rest are judged.
#[test]
fn a_damaged_row_is_named_not_judged() {
    let (mut catalog, _) = committed("damaged");
    let conn = catalog.connection();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF; DROP TRIGGER features_immutable;
         UPDATE features SET part_digest = (SELECT part_digest FROM features f2
             JOIN live l ON l.sha256 = f2.sha256 WHERE l.id = 'm_bracket_thick')
         WHERE sha256 = (SELECT sha256 FROM live WHERE id = 'm_bracket');",
    )
    .unwrap();
    let document = socket("s01_bolted", "BRACKET");
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let run = snapshot
        .match_catalog(&matcher, &Selection::Exhaustive, 1)
        .unwrap()
        .unwrap();
    assert_eq!(run.corrupt, ["m_bracket@r1"]);
    assert!(
        !run.results
            .iter()
            .any(|(_, l)| l[0] == "catalog:m_bracket@r1")
    );
    assert!(run.results.len() > 30);
}

/// QR6 and QR8: two files whose parts are equal are judged once, each
/// listed with its own source; one job or four give the same document.
#[test]
fn equal_parts_are_judged_once() {
    let dir = scratch("dedup");
    let mut catalog = catalog(&dir);
    let bracket = fs::read(core_tests().join("candidates/m_bracket.stp")).unwrap();
    let text = String::from_utf8(bracket).unwrap();
    let renamed = text.replacen("FILE_NAME(", "FILE_NAME( /* another export */ ", 1);
    assert_ne!(renamed, text);
    let a = dir.join("a.stp");
    let b = dir.join("b.stp");
    fs::write(&a, &text).unwrap();
    fs::write(&b, &renamed).unwrap();
    catalog.ingest(&[a, b], None).unwrap();
    let document = socket("s01_bolted", "BRACKET");
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let one = snapshot
        .match_catalog(&matcher, &Selection::Full, 1)
        .unwrap()
        .unwrap();
    assert_eq!((one.results.len(), one.judged), (2, 1));
    assert_ne!(one.results[0].0.source, one.results[1].0.source);
    let four = snapshot
        .match_catalog(&matcher, &Selection::Full, 4)
        .unwrap()
        .unwrap();
    assert_eq!(
        MatchDocument::new(&matcher, one.results).to_json(),
        MatchDocument::new(&matcher, four.results).to_json()
    );
}

/// QR1 and IX8: a blocked socket refuses a full run; a named run judges.
#[test]
fn a_blocked_socket_refuses_a_full_run() {
    let (mut catalog, _) = committed("blocked");
    let document = socket("s01_bolted", "BRACKET");
    let tiny = MatchPolicy::new(
        TolerancePolicy::new("tiny", 1e-7, 0.01).unwrap(),
        Clearance::Strict,
        Grip::Hard,
    );
    let matcher = Matcher::new(&document, tiny, MatchOptions::default(), &NoFitVerifier).unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let full = snapshot
        .match_catalog(&matcher, &Selection::Full, 1)
        .unwrap();
    assert!(matches!(full, Err(NotRun::Blocked { .. })), "{full:?}");
    let named = vec![fitment_catalog::Reference::parse("m_bracket").unwrap()];
    let run = snapshot
        .match_catalog(&matcher, &Selection::Named(named), 1)
        .unwrap()
        .unwrap();
    assert_eq!(run.results.len(), 1);
    assert_eq!(run.results[0].0.verdict.kind(), VerdictKind::Reject);
}
