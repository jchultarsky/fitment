//! The CLI with the real helper (`docs/research/m7-fit.md`, section 11,
//! PR 3): VT74's CLI half (`fitment fit setup` as a user runs it, then
//! `match --fit` from its fit directory), s01's lug and bracket through
//! `fitment match --fit`, and VT64 (a catalog run with the fit is the file
//! run with the fit over level 1's survivors, digest for digest, dedup
//! judging every member in full). These need uv and a download (or uv's
//! cache): they run when `FITMENT_FIT_KERNEL=1` is set, as CI's
//! `fit-kernel` job sets it, and say they are skipped otherwise. On
//! Windows the helper is refused until the weekly Windows corpus run is
//! green (M7-50): there `match --fit` exits 1 saying so.

#![cfg(all(feature = "fit", feature = "catalog"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use assert_cmd::Command;
use fitment_catalog::{Catalog, NoObserver, Options, Selection};
use fitment_core::{MatchDocument, MatchOptions, MatchPolicy, Matcher, SocketDocument};
use fitment_fit::{HelperFiles, HelperOptions, OcctHelper, OcctSession};
use serde_json::Value;

fn core(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fitment-core/tests")
        .join(path)
}

fn tmp(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("fit-kernel")
        .join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs `fitment ARGS` in `dir`: (code, stdout, stderr).
fn run(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::cargo_bin("fitment")
        .unwrap()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The fit directory `fitment fit setup` made, once per test binary, if
/// this run asks for the kernel (`FITMENT_FIT_KERNEL=1`).
fn kernel() -> Option<&'static PathBuf> {
    static KERNEL: OnceLock<Option<PathBuf>> = OnceLock::new();
    KERNEL
        .get_or_init(|| {
            if std::env::var_os("FITMENT_FIT_KERNEL").is_none_or(|v| v != "1") {
                return None;
            }
            let root = tmp("setup");
            let started = std::time::Instant::now();
            let (code, stdout, stderr) = run(&root, &["fit", "setup", "--fit-dir", "fit"]);
            assert_eq!(code, 0, "fitment fit setup: {stderr}");
            eprintln!(
                "fitment fit setup: {:.2} s\n{stdout}",
                started.elapsed().as_secs_f64()
            );
            for line in [
                "fit directory",
                "interpreter",
                "binding",
                "kernel",
                "limits",
            ] {
                assert!(stdout.contains(line), "{line}: {stdout}");
            }
            if refused_here() {
                assert!(stdout.contains("refused"), "{stdout}");
            } else {
                assert!(
                    stdout.contains(
                        "self-test      touching passed, overlapping passed, apart passed"
                    ),
                    "{stdout}"
                );
            }
            Some(root.join("fit"))
        })
        .as_ref()
}

macro_rules! need_kernel {
    () => {
        match kernel() {
            Some(kernel) => kernel,
            None => {
                eprintln!("skipped: set FITMENT_FIT_KERNEL=1 to run the real helper (uv)");
                return;
            }
        }
    };
}

/// True where the helper is refused until VT77 is green (M7-50).
fn refused_here() -> bool {
    cfg!(windows)
}

/// VT74's CLI half, FU1 to FU5: setup through the binary, then the lug
/// and the bracket through `match --fit` from its fit directory: the lug
/// interferes with the plate, a `Reject`; the bracket verifies, a `Match`
/// (exit 0); the original's control verified. On Windows, exit 1 naming
/// the refusal, nothing on stdout.
#[test]
fn the_lug_interferes_and_the_bracket_matches_through_the_cli() {
    let fit = need_kernel!();
    let dir = tmp("s01");
    let assembly = core("assemblies/s01_bolted.stp");
    let socket = dir.join("bracket.json");
    let (code, _, stderr) = run(
        &dir,
        &[
            "socket",
            assembly.to_str().unwrap(),
            "--part",
            "BRACKET",
            "-o",
            socket.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    let bracket = core("candidates/m_bracket.stp");
    let lug = core("candidates/m_bracket_lug.stp");
    let started = std::time::Instant::now();
    let (code, stdout, stderr) = run(
        &dir,
        &[
            "match",
            socket.to_str().unwrap(),
            bracket.to_str().unwrap(),
            lug.to_str().unwrap(),
            "--original",
            assembly.to_str().unwrap(),
            "--fit",
            assembly.to_str().unwrap(),
            "--fit-dir",
            fit.to_str().unwrap(),
            "--format",
            "json",
        ],
    );
    eprintln!(
        "match --fit, s01: {:.2} s\n{stderr}",
        started.elapsed().as_secs_f64()
    );
    if refused_here() {
        assert_eq!(code, 1, "{stderr}");
        assert_eq!(stdout, "");
        assert!(stderr.contains("Windows"), "{stderr}");
        return;
    }
    assert_eq!(code, 0, "{stderr}");
    let doc: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["format_version"], 2);
    assert_eq!(doc["fit"]["control"][0]["fit"]["status"], "verified");
    let verdicts: Vec<(&str, &str)> = doc["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["verdict"].as_str().unwrap(),
                c["fit"]["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        verdicts,
        [("match", "verified"), ("reject", "interferes")],
        "{stdout}"
    );
    let residual = doc["candidates"][1]["fit"]["interferences"][0]["measured"]["residual_mm3"]
        .as_f64()
        .unwrap();
    assert!((residual - 299.400_06).abs() < 1e-3, "{residual}");
}

/// VT64 (M5's CT1 with the fit, FC6): for each committed socket, a
/// catalog run with the real helper is the file run with it over level
/// 1's survivors, digest for digest; the bracket's twin (equal part,
/// another header) is judged with its own file, never shared.
#[test]
fn a_catalog_run_with_the_fit_is_the_file_run_restricted() {
    let fit = need_kernel!();
    let dir = tmp("vt64");
    let extra = dir.join("extra");
    fs::create_dir_all(&extra).unwrap();
    let bracket = fs::read_to_string(core("candidates/m_bracket.stp")).unwrap();
    fs::write(
        extra.join("m_bracket_twin.stp"),
        bracket.replacen("FILE_NAME('", "FILE_NAME('twin", 1),
    )
    .unwrap();
    let (mut catalog, _) = Catalog::init(&dir.join("cat"), Options::default()).unwrap();
    let ingested = catalog
        .ingest(&[core("candidates"), extra.clone()], None)
        .unwrap();
    let files: std::collections::BTreeMap<String, PathBuf> = ingested
        .iter()
        .filter(|l| l.revision.is_some())
        .map(|l| {
            (
                format!("catalog:{}@r1", l.id.as_ref().unwrap()),
                l.input.clone(),
            )
        })
        .collect();
    let helper = OcctHelper::open(
        fit,
        HelperFiles::embedded(),
        HelperOptions::default().with_jobs(2),
    )
    .unwrap();
    let mut fitted = 0;
    for (case, part) in [
        ("s01_bolted", "BRACKET"),
        ("s01_bolted", "NUT"),
        ("s02_cbore", "BLOCK"),
        ("s03_dowels", "PLATE"),
    ] {
        let started = std::time::Instant::now();
        let assembly = fs::read(core("assemblies").join(format!("{case}.stp"))).unwrap();
        let document = SocketDocument::extract(&assembly, Some(part), None).unwrap();
        let session = OcctSession::open(&helper, assembly, &document).unwrap();
        let matcher = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            &session,
        )
        .unwrap();
        let blobs = catalog.fit_blobs();
        let full = {
            let snapshot = catalog.snapshot().unwrap();
            snapshot
                .match_catalog_fitted(&matcher, &Selection::Full, 2, &NoObserver, &blobs)
                .unwrap()
                .unwrap()
        };
        assert!(full.corrupt.is_empty() && full.bugs.is_empty());
        assert_eq!(full.judged, full.results.len(), "{case} {part}: no sharing");
        let restricted = full
            .results
            .iter()
            .map(|(_, labels)| {
                let bytes = fs::read(&files[&labels[0]]).unwrap();
                (matcher.judge_step(&bytes), labels.clone())
            })
            .collect();
        let catalog_doc = MatchDocument::new(&matcher, full.results);
        let file_doc = MatchDocument::new(&matcher, restricted);
        assert_eq!(catalog_doc.digest(), file_doc.digest(), "{case} {part}");
        assert!(!matcher.fit_incomplete(std::iter::empty()));
        fitted += blobs.reads();
        eprintln!(
            "{case} {part}: {} candidates, {} files read for their fits, {:.2} s",
            catalog_doc.summary().candidates,
            blobs.reads(),
            started.elapsed().as_secs_f64()
        );
    }
    assert!(fitted >= 2, "the bracket and its twin at least: {fitted}");
}

/// FB5 through the real helper: the bracket of another assembly (s01
/// under another header) is found in the helper's second scene by its
/// faces, and matches, as the socket's own bracket does.
#[test]
fn a_component_of_another_assembly_matches_through_the_real_helper() {
    let fit = need_kernel!();
    let assembly = fs::read(core("assemblies/s01_bolted.stp")).unwrap();
    let document = SocketDocument::extract(&assembly, Some("BRACKET"), None).unwrap();
    let other = String::from_utf8(assembly.clone())
        .unwrap()
        .replacen("FILE_NAME('", "FILE_NAME('other", 1)
        .into_bytes();
    let helper = OcctHelper::open(fit, HelperFiles::embedded(), HelperOptions::default()).unwrap();
    let session = OcctSession::open(&helper, assembly, &document).unwrap();
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &session,
    )
    .unwrap();
    let component = fitment_core::SelectedComponent::read(&other, Some("BRACKET"), None).unwrap();
    let result = matcher.judge_component_in(&component, component.source(), &other);
    let expected = if refused_here() {
        fitment_core::VerdictKind::InterfaceMatch
    } else {
        fitment_core::VerdictKind::Match
    };
    assert_eq!(
        result.verdict.kind(),
        expected,
        "{:?}",
        session.take_reports()
    );
}
