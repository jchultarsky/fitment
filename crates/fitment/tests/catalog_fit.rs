//! VT65 (`docs/research/m7-fit.md`, FC6, QR13, M7-21): a catalog match
//! with a fit verifier reaches a stored file only through the `FitBlobs`
//! handed to it, and only for a candidate whose fit is asked. With
//! `NoFitVerifier` no file is opened, `FitBlobs` handed or not (CT6 kept);
//! with a verifier that has an identity, the files read are exactly the
//! candidates whose every interface passed at a unique placement, each
//! hash-checked; dedup shares nothing, every member judged with its own
//! file; a damaged file is `stored_bytes_unreadable` and the run
//! incomplete (the CLI's exit 1, `fit_cli.rs`).

#![cfg(feature = "catalog")]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use fitment_catalog::fs::{Call, CountingFs};
use fitment_catalog::{Catalog, CatalogRun, FitBlobs, NoObserver, Options, Selection};
use fitment_core::fit::FitCode;
use fitment_core::fit::testing::MeasuresVerifier;
use fitment_core::{
    CandidateResult, FitIdentity, FitVerifier, MatchOptions, MatchPolicy, Matcher, NoFitVerifier,
    SocketDocument,
};

const SOCKETS: [(&str, &str); 4] = [
    ("s01_bolted", "BRACKET"),
    ("s01_bolted", "NUT"),
    ("s02_cbore", "BLOCK"),
    ("s03_dowels", "PLATE"),
];

fn core(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fitment-core/tests")
        .join(path)
}

fn scratch(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("catalog-fit")
        .join(format!("{test}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A catalog of every committed candidate, and the bracket again under
/// another header (equal part, other bytes: QR6's duplicate), reopened
/// through a counting file system.
fn counted(test: &str) -> (Catalog, CountingFs, PathBuf) {
    let dir = scratch(test);
    let extra = dir.join("extra");
    fs::create_dir_all(&extra).unwrap();
    let bracket = fs::read_to_string(core("candidates/m_bracket.stp")).unwrap();
    let twin = bracket.replacen("FILE_NAME('", "FILE_NAME('twin", 1);
    assert_ne!(twin, bracket);
    fs::write(extra.join("m_bracket_twin.stp"), twin).unwrap();
    let root = dir.join("cat");
    let (mut catalog, _) = Catalog::init(&root, Options::default()).unwrap();
    catalog.ingest(&[core("candidates"), extra], None).unwrap();
    drop(catalog);
    let seam = CountingFs::new();
    let catalog = Catalog::open_with(&root, Options::default(), Box::new(seam.clone())).unwrap();
    (catalog, seam, root)
}

fn socket(case: &str, part: &str) -> (SocketDocument, Vec<u8>) {
    let bytes = fs::read(core("assemblies").join(format!("{case}.stp"))).unwrap();
    (
        SocketDocument::extract(&bytes, Some(part), None).unwrap(),
        bytes,
    )
}

/// A full run of `matcher` through `blobs`, and the stored files it read.
fn run(
    catalog: &mut Catalog,
    seam: &CountingFs,
    matcher: &Matcher<'_>,
    blobs: &FitBlobs,
) -> (CatalogRun, BTreeSet<PathBuf>) {
    let before = seam.calls().len();
    let run = {
        let snapshot = catalog.snapshot().unwrap();
        snapshot
            .match_catalog_fitted(matcher, &Selection::Full, 3, &NoObserver, blobs)
            .unwrap()
            .unwrap()
    };
    let read = seam.calls()[before..]
        .iter()
        .filter_map(|c| match c {
            Call::Read(path) => Some(path.clone()),
            _ => None,
        })
        .collect();
    (run, read)
}

/// True if the candidate's fit was asked: every socket's interface passed
/// at a unique placement.
fn asked(result: &CandidateResult) -> bool {
    result.fit.as_deref().is_some_and(|e| {
        !matches!(
            e.outcome().code(),
            Some(FitCode::PlacementNotUnique | FitCode::HelperUnavailable)
        )
    })
}

/// A file candidate's SHA-256.
fn sha(result: &CandidateResult) -> Option<&str> {
    match &result.source {
        fitment_core::CandidateSource::File(source) => Some(source.sha256.as_str()),
        _ => None,
    }
}

fn identity() -> FitIdentity {
    FitIdentity::new("open_cascade", "8.0.1")
}

/// CT6 kept, and FC6: no file read without a verifier that has an
/// identity; with one, one read per candidate whose fit is asked, its
/// own, and nothing else; the bracket's twin judged in full beside it.
#[test]
fn a_fit_reads_exactly_the_passing_candidates_files() {
    let (mut catalog, seam, root) = counted("reads");
    let blobs = catalog.fit_blobs();
    let mut fitted = 0;
    for (case, part) in SOCKETS {
        let (document, assembly) = socket(case, part);
        let none = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            &NoFitVerifier,
        )
        .unwrap();
        let (_, read) = run(&mut catalog, &seam, &none, &blobs);
        assert!(read.is_empty(), "{case} {part}: CT6: {read:?}");
        let verifier = MeasuresVerifier::exact(identity(), assembly);
        let matcher = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            &verifier as &dyn FitVerifier,
        )
        .unwrap();
        let (run, read) = run(&mut catalog, &seam, &matcher, &blobs);
        // QR6 with a verifier: no sharing, one judgment per member.
        assert_eq!(run.judged, run.results.len(), "{case} {part}");
        let expected: BTreeSet<PathBuf> = run
            .results
            .iter()
            .map(|(r, _)| r)
            .filter(|r| asked(r))
            .map(|r| {
                let sha = sha(r).unwrap();
                catalog.blob_path(&fitment_catalog::Sha256Hex::parse(sha).unwrap())
            })
            .collect();
        assert_eq!(read, expected, "{case} {part}");
        let asked_files = verifier
            .asked()
            .iter()
            .filter(|a| !a.candidate.starts_with("component "))
            .count();
        assert_eq!(asked_files, expected.len(), "{case} {part}");
        for path in &read {
            assert!(path.starts_with(root.join("blobs")), "{path:?}");
        }
        fitted += expected.len();
    }
    assert!(fitted >= 2, "the bracket and its twin at least: {fitted}");
}

/// FC6, FF21: a stored file damaged after ingest is
/// `stored_bytes_unreadable` for its candidate, the run incomplete; the
/// other candidates' fits stand.
#[test]
fn a_damaged_file_is_stored_bytes_unreadable_and_incomplete() {
    let (mut catalog, seam, _) = counted("damaged");
    let (document, assembly) = socket("s01_bolted", "BRACKET");
    let bracket = fs::read(core("candidates/m_bracket.stp")).unwrap();
    let sha = fitment_catalog::Sha256Hex::of(&bracket);
    let path = catalog.blob_path(&sha);
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)] // a test damages its own file
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    fs::write(&path, b"not the bracket").unwrap();
    let verifier = MeasuresVerifier::exact(identity(), assembly);
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &verifier as &dyn FitVerifier,
    )
    .unwrap();
    let blobs = catalog.fit_blobs();
    let (run, _) = run(&mut catalog, &seam, &matcher, &blobs);
    let results: Vec<&CandidateResult> = run.results.iter().map(|(r, _)| r).collect();
    let damaged = results
        .iter()
        .find(|r| self::sha(r) == Some(sha.as_str()))
        .unwrap();
    let code = damaged.fit.as_deref().and_then(|e| e.outcome().code());
    assert_eq!(code, Some(FitCode::StoredBytesUnreadable));
    assert!(damaged.fit_incomplete());
    assert!(matcher.fit_incomplete(results.iter().copied()));
    let twin = results
        .iter()
        .find(|r| asked(r) && self::sha(r) != Some(sha.as_str()))
        .unwrap();
    assert_eq!(
        twin.verdict.kind(),
        fitment_core::VerdictKind::Match,
        "its twin's own file reads"
    );
}
