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

fn policies() -> [MatchPolicy; 3] {
    let loose =
        TolerancePolicy::new("cli", 0.05, TolerancePolicy::default().angular_deg()).unwrap();
    [
        MatchPolicy::default(),
        MatchPolicy::new(
            TolerancePolicy::default(),
            Clearance::Functional,
            Grip::Hard,
        ),
        // A wider window, as `--tolerance-mm 0.05` gives (review, F2).
        MatchPolicy::new(loose, Clearance::Strict, Grip::Hard),
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

/// CT6 (QR11): a catalog run, full or exhaustive, makes no call through
/// the catalog's file seam, so opens no path under `blobs/`: it reads the
/// index alone (SQLite's own reads of the database bypass the seam).
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

/// CT3 (IX1): level 1 over the stored postings and tokens admits exactly
/// what it admits over the in-memory index of the same tokens, id for id,
/// for every committed socket under four policies.
#[test]
fn the_sql_source_agrees_with_the_index_in_memory() {
    let (mut catalog, _) = committed("sources");
    let mut memory = fitment_core::ShortlistIndex::new();
    {
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
            let tokens =
                fitment_core::IndexTokens::from_json(std::str::from_utf8(&tokens).unwrap())
                    .unwrap();
            memory.insert(u64::try_from(key).unwrap(), &tokens);
        }
    }
    let mut compared = 0;
    for (case, part) in SOCKETS {
        let document = socket(case, part);
        let coarse = MatchPolicy::new(
            TolerancePolicy::new("coarse", 0.5, 1.0).unwrap(),
            Clearance::Functional,
            Grip::Hard,
        );
        for policy in policies().into_iter().chain([coarse]) {
            let matcher =
                Matcher::new(&document, policy, MatchOptions::default(), &NoFitVerifier).unwrap();
            let snapshot = catalog.snapshot().unwrap();
            let sql = snapshot.admitted(&matcher).unwrap();
            assert_eq!(sql, matcher.admitted(&memory).unwrap(), "{case} {part}");
            compared += 1;
        }
    }
    assert_eq!(compared, 16);
}

fn bracket_matcher(document: &SocketDocument) -> Matcher<'_> {
    Matcher::new(
        document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .unwrap()
}

/// QR2, CR7 and DB4 (CT9): a run reads one snapshot: a replace committed
/// after the snapshot opened is not seen; the old revision is judged.
#[test]
fn a_run_reads_one_snapshot() {
    let (mut catalog, _) = committed("snapshot");
    let root = catalog.root().to_owned();
    let document = socket("s01_bolted", "BRACKET");
    let matcher = bracket_matcher(&document);
    let snapshot = catalog.snapshot().unwrap();
    let _ = snapshot.generation();
    let mut writer = Catalog::open(&root, Options::default()).unwrap();
    // The bracket's part in new bytes: its header changed.
    let text = fs::read_to_string(core_tests().join("candidates/m_bracket.stp")).unwrap();
    let again = root.parent().unwrap().join("again.stp");
    fs::write(
        &again,
        text.replacen("FILE_NAME(", "FILE_NAME( /* again */ ", 1),
    )
    .unwrap();
    writer
        .replace(
            &fitment_catalog::Reference::parse("m_bracket").unwrap(),
            &again,
        )
        .unwrap()
        .unwrap();
    let run = snapshot
        .match_catalog(&matcher, &Selection::Exhaustive, 2)
        .unwrap()
        .unwrap();
    let labels: Vec<&str> = run.results.iter().map(|(_, l)| l[0].as_str()).collect();
    assert!(labels.contains(&"catalog:m_bracket@r1"), "{labels:?}");
    drop(snapshot);
    let fresh = catalog.snapshot().unwrap();
    let run = fresh
        .match_catalog(&matcher, &Selection::Exhaustive, 2)
        .unwrap()
        .unwrap();
    assert!(
        run.results
            .iter()
            .any(|(_, l)| l[0] == "catalog:m_bracket@r2")
    );
}

/// IX9: a tokens row whose hash fails is a damaged row: named, not judged.
#[test]
fn a_damaged_tokens_row_is_named() {
    let (mut catalog, _) = committed("tokens");
    catalog
        .connection()
        .execute_batch(
            "DROP TRIGGER tokens_immutable;
             UPDATE tokens SET tokens_sha256 = lower(hex(randomblob(32)))
             WHERE sha256 = (SELECT sha256 FROM live WHERE id = 'm_bracket');",
        )
        .unwrap();
    let document = socket("s01_bolted", "BRACKET");
    let matcher = bracket_matcher(&document);
    let snapshot = catalog.snapshot().unwrap();
    let run = snapshot
        .match_catalog(&matcher, &Selection::Full, 1)
        .unwrap()
        .unwrap();
    assert_eq!(run.corrupt, ["m_bracket@r1"]);
    assert!(
        !run.results
            .iter()
            .any(|(_, l)| l[0] == "catalog:m_bracket@r1")
    );
}

/// IX10: a content key that is no level-1 id refuses the run, never wraps.
#[test]
fn a_negative_key_is_damage() {
    let (mut catalog, _) = committed("key");
    catalog
        .connection()
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             UPDATE posting SET content = -5
             WHERE content = (SELECT b.key FROM blob b JOIN live l ON l.sha256 = b.sha256
                              WHERE l.id = 'm_bracket');",
        )
        .unwrap();
    let document = socket("s01_bolted", "BRACKET");
    let matcher = bracket_matcher(&document);
    let snapshot = catalog.snapshot().unwrap();
    let run = snapshot.match_catalog(&matcher, &Selection::Full, 1);
    assert!(
        matches!(run, Err(fitment_catalog::Error::Corrupt(_))),
        "{run:?}"
    );
}

/// CR8 and CR9: quarantined entries are counted, never searched; named,
/// one is judged as its refused document: a Reject.
#[test]
fn quarantine_is_counted_and_judged_only_when_named() {
    let (mut catalog, _) = committed("quarantine");
    let document = socket("s01_bolted", "BRACKET");
    let matcher = bracket_matcher(&document);
    let snapshot = catalog.snapshot().unwrap();
    let full = snapshot
        .match_catalog(&matcher, &Selection::Full, 1)
        .unwrap()
        .unwrap();
    assert!(full.quarantined > 0);
    assert!(
        !full
            .results
            .iter()
            .any(|(_, l)| l[0] == "catalog:r04_assembly@r1")
    );
    let named = vec![fitment_catalog::Reference::parse("r04_assembly").unwrap()];
    let run = snapshot
        .match_catalog(&matcher, &Selection::Named(named), 1)
        .unwrap()
        .unwrap();
    assert_eq!(run.results.len(), 1);
    assert_eq!(run.results[0].0.status, fitment_core::Status::Refused);
    assert_eq!(run.results[0].0.verdict.kind(), VerdictKind::Reject);
}

/// QR8: one job or four give the same document on a run of many groups.
#[test]
fn jobs_change_no_byte() {
    let (mut catalog, _) = committed("jobs");
    let document = socket("s01_bolted", "BRACKET");
    let matcher = bracket_matcher(&document);
    let snapshot = catalog.snapshot().unwrap();
    let documents: Vec<String> = [1, 4]
        .into_iter()
        .map(|jobs| {
            let run = snapshot
                .match_catalog(&matcher, &Selection::Exhaustive, jobs)
                .unwrap()
                .unwrap();
            assert!(run.judged > 20);
            MatchDocument::new(&matcher, run.results).to_json()
        })
        .collect();
    assert_eq!(documents[0], documents[1]);
}

/// A verifier that panics whenever it is asked: with an identity, it is
/// asked for the original's control at the first part whose interface
/// passes (docs/research/m7-fit.md, FT9), and again at the next, since a
/// control that panicked never ran.
struct Panics(Vec<u8>);

impl fitment_core::FitVerifier for Panics {
    fn identity(&self) -> Option<fitment_core::FitIdentity> {
        Some(fitment_core::FitIdentity::new("panics", "1"))
    }

    fn assembly(&self) -> Option<&[u8]> {
        Some(&self.0)
    }

    fn measure(
        &self,
        _: &fitment_core::FitRequest<'_>,
    ) -> Result<fitment_core::fit::FitMeasures, fitment_core::fit::FitFailure> {
        panic!("a fit verifier's bug");
    }
}

/// The PR 4a reviews: a judgment that panics is a fitment bug, reported
/// apart from damage, the same on one thread or two.
#[test]
fn a_panicking_judgment_is_a_bug_not_damage() {
    let (mut catalog, _) = committed("panics");
    let document = socket("s01_bolted", "BRACKET");
    let assembly = fs::read(core_tests().join("assemblies/s01_bolted.stp")).unwrap();
    let panics = Panics(assembly);
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &panics,
    )
    .unwrap();
    // A matcher with a fit verifier runs with the catalog's files (FC6).
    let blobs = catalog.fit_blobs();
    let snapshot = catalog.snapshot().unwrap();
    let named = vec![
        fitment_catalog::Reference::parse("m_bracket").unwrap(),
        fitment_catalog::Reference::parse("m_bracket_lug").unwrap(),
    ];
    let runs: Vec<_> = [1, 2]
        .into_iter()
        .map(|jobs| {
            let run = snapshot
                .match_catalog_fitted(
                    &matcher,
                    &Selection::Named(named.clone()),
                    jobs,
                    &fitment_catalog::NoObserver,
                    &blobs,
                )
                .unwrap()
                .unwrap();
            (run.bugs, run.corrupt, run.results.len())
        })
        .collect();
    assert_eq!(runs[0], runs[1]);
    assert_eq!(runs[0].0, ["m_bracket@r1", "m_bracket_lug@r1"]);
    assert_eq!(runs[0].1, Vec::<String>::new());
}

/// QR10: an exhaustive run names a live entry whose blob row is gone,
/// never leaving it out silently.
#[test]
fn an_exhaustive_run_names_an_entry_without_its_row() {
    let (mut catalog, _) = committed("no-row");
    catalog
        .connection()
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             DELETE FROM blob WHERE sha256 = (SELECT sha256 FROM live WHERE id = 'm_bracket');",
        )
        .unwrap();
    let document = socket("s01_bolted", "BRACKET");
    let matcher = bracket_matcher(&document);
    let snapshot = catalog.snapshot().unwrap();
    let run = snapshot
        .match_catalog(&matcher, &Selection::Exhaustive, 1)
        .unwrap()
        .unwrap();
    assert_eq!(run.corrupt, ["m_bracket@r1"]);
}

/// Every committed socket's full and exhaustive runs under every policy,
/// as digests or refusals.
fn every_run(catalog: &mut Catalog) -> Vec<String> {
    let mut out = Vec::new();
    for (case, part) in SOCKETS {
        let document = socket(case, part);
        for policy in policies() {
            let matcher =
                Matcher::new(&document, policy, MatchOptions::default(), &NoFitVerifier).unwrap();
            let snapshot = catalog.snapshot().unwrap();
            for selection in [Selection::Full, Selection::Exhaustive] {
                out.push(
                    match snapshot.match_catalog(&matcher, &selection, 4).unwrap() {
                        Ok(run) => MatchDocument::new(&matcher, run.results)
                            .digest()
                            .to_owned(),
                        Err(refused) => format!("{refused:?}"),
                    },
                );
            }
        }
    }
    out
}

/// CT6, M5's exit criterion: with every stored file moved away after
/// ingest, each run gives the same document or the same refusal, and
/// `verify` reports each file missing.
#[test]
fn a_match_runs_with_the_store_moved_away() {
    let (mut catalog, files) = committed("moved-away");
    let before = every_run(&mut catalog);
    let root = catalog.root().to_owned();
    drop(catalog);
    common::make_writable(&root.join("blobs"));
    fs::rename(root.join("blobs"), root.with_extension("away")).unwrap();
    for b in 0..=255_u8 {
        fs::create_dir_all(root.join(format!("blobs/sha256/{b:02x}"))).unwrap();
    }
    let mut catalog = Catalog::open(&root, Options::default()).unwrap();
    assert_eq!(every_run(&mut catalog), before);
    let problems = catalog.verify(false, false).unwrap();
    let missing = problems
        .iter()
        .filter(|p| p.kind == fitment_catalog::ProblemKind::Missing)
        .count();
    assert_eq!(missing, files.len(), "{problems:?}");
}

/// CT7: two catalogs of the same files ingested one at a time in opposite
/// orders, so that their internal keys differ (all but the middle file's),
/// give the same runs; and so does either with SQLite's unordered selects
/// reversed.
#[test]
fn ingest_order_and_select_order_change_no_run() {
    let inputs: Vec<PathBuf> = {
        let mut files: Vec<PathBuf> = [core_tests().join("candidates"), core_tests().join("data")]
            .iter()
            .flat_map(|d| fs::read_dir(d).unwrap().map(|e| e.unwrap().path()))
            .filter(|p| p.extension().is_some_and(|e| e == "stp"))
            .collect();
        files.sort();
        files
    };
    let build = |test: &str, files: &[PathBuf]| {
        let mut catalog = catalog(&scratch(test));
        for file in files {
            catalog.ingest(std::slice::from_ref(file), None).unwrap();
        }
        catalog
    };
    let mut forward = build("order-forward", &inputs);
    let reversed: Vec<PathBuf> = inputs.iter().rev().cloned().collect();
    let mut backward = build("order-backward", &reversed);
    let runs = every_run(&mut forward);
    assert_eq!(every_run(&mut backward), runs);
    backward
        .connection()
        .execute_batch("PRAGMA reverse_unordered_selects = ON")
        .unwrap();
    assert_eq!(every_run(&mut backward), runs);
}
