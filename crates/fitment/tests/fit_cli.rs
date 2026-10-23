//! `fitment match --fit` and `fitment fit setup` run as a user would
//! (`docs/research/m7-fit.md`, section 4.11, FU1 to FU6, FC6, FM6, FG3,
//! M7-49), against VT78's fake helper on every OS, with no Python: the
//! fake's fit directory holds the script and lock this build embeds, so
//! the release binary's own checks take it, and its manifest names this
//! test binary as the interpreter. Each request's replies are the exact
//! kernel's, left by the same run made in this process through
//! `Replaying`, so every document the binary writes must be the library's
//! byte for byte; the fake breaks them as its scenario says.
//!
//! This file is its own harness (`harness = false`): the binary is
//! re-executed as the fake helper.

fn main() {
    #[cfg(feature = "fit")]
    fit::tests();
}

#[cfg(feature = "fit")]
mod fit {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use assert_cmd::Command;
    use fitment_core::fit::FitBody;
    use fitment_core::fit::testing::{exact, interfere};
    use fitment_core::{
        CandidateResult, MatchDocument, MatchOptions, MatchPolicy, Matcher, SelectedComponent,
        SocketDocument, sha256_hex,
    };
    use fitment_fit::testing::{Fake, Replaying};
    use fitment_fit::{HelperOptions, OcctSession};
    use serde_json::Value;

    fn core(path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fitment-core/tests")
            .join(path)
    }

    /// A fresh directory for `test`, under Cargo's target tmp.
    fn root(test: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("fit-cli")
            .join(format!("{test}-{}", std::process::id()));
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

    /// s01's assembly, the bracket's socket document and the bracket and
    /// lug, copied into `dir` under their own names; the document as the
    /// binary reads it back.
    fn inputs(dir: &Path) -> SocketDocument {
        let assembly = fs::read(core("assemblies/s01_bolted.stp")).unwrap();
        fs::write(dir.join("s01_bolted.stp"), &assembly).unwrap();
        for name in ["m_bracket.stp", "m_bracket_lug.stp"] {
            fs::copy(core("candidates").join(name), dir.join(name)).unwrap();
        }
        let text = SocketDocument::extract(&assembly, Some("BRACKET"), None)
            .unwrap()
            .to_json();
        fs::write(dir.join("bracket.json"), &text).unwrap();
        SocketDocument::from_json(&text).unwrap()
    }

    /// The exact kernel's measures, the lug measured 299.4 mm³ into the
    /// plate, as P2 found it.
    fn kernel(request: &fitment_core::FitRequest<'_>) -> fitment_core::fit::FitMeasures {
        let lug = sha256_hex(&fs::read(core("candidates/m_bracket_lug.stp")).unwrap());
        let mut measures = exact(request);
        if matches!(request.body(), FitBody::Step { sha256, .. } if *sha256 == lug) {
            let plan = request.plan(&measures.scene, &measures.place);
            let plate = plan.asked().position(|p| p.name.starts_with("PLATE"));
            assert!(interfere(&mut measures, &plan, plate.unwrap(), 299.4));
        }
        measures
    }

    /// Deadlines a test's helper meets on a loaded runner; on Windows the
    /// library's own runs are let past M7-50 (test-support's override), as
    /// the fake is no kernel. The binary never is: there, `--fit` exits 1.
    fn fast() -> HelperOptions {
        HelperOptions::default()
            .with_timeout(Duration::from_secs(20))
            .with_setup_timeout(Duration::from_secs(20))
            .with_unproven_platform()
    }

    /// M7-50 on Windows: the binary refuses its helper before any
    /// candidate, exit 1, nothing on stdout; true there, when a test that
    /// expects a fit stops.
    fn refused_on_windows(code: i32, stdout: &str, stderr: &str) -> bool {
        if !cfg!(windows) {
            return false;
        }
        assert_eq!(code, 1, "{stderr}");
        assert_eq!(stdout, "");
        assert!(stderr.contains("Windows"), "{stderr}");
        true
    }

    /// The document of the bracket and the lug (and the original, when
    /// asked) judged in this process through `fake`'s helper, labelled as
    /// the binary labels them; its replies are left for the binary.
    fn library(
        dir: &Path,
        fake: &Fake,
        options: HelperOptions,
        original: bool,
    ) -> (String, Vec<CandidateResult>) {
        let document = inputs(dir);
        let assembly = fs::read(dir.join("s01_bolted.stp")).unwrap();
        let helper = fake.helper(options);
        let session = OcctSession::open(&helper, assembly.clone(), &document).unwrap();
        let replaying = Replaying::answering(&session, fake, kernel);
        let matcher = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            &replaying,
        )
        .unwrap();
        let original = original.then(|| {
            let target = SelectedComponent::target(&assembly, &document).unwrap();
            matcher.judge_original(&target, target.source())
        });
        let results: Vec<(CandidateResult, Vec<String>)> = ["m_bracket.stp", "m_bracket_lug.stp"]
            .iter()
            .map(|name| {
                let bytes = fs::read(dir.join(name)).unwrap();
                (matcher.judge_step(&bytes), vec![(*name).to_owned()])
            })
            .collect();
        let judged = results.iter().map(|(r, _)| r.clone()).collect();
        let doc = MatchDocument::with_original(&matcher, results, original.as_ref());
        (doc.to_json(), judged)
    }

    /// The fit dir of `fake`, as an argument.
    fn fit_dir(fake: &Fake) -> String {
        fake.dir.display().to_string()
    }

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    /// The tests, each its own harness entry.
    pub(crate) fn tests() {
        fitment_fit::harness![
            a_fit_run_writes_the_library_s_document,
            a_helper_s_failure_writes_the_document_and_exits_1,
            a_check_past_fit_timeout_exits_1,
            no_manifest_names_fitment_fit_setup,
            another_assembly_is_a_usage_error,
            more_interface_passes_than_max_fits_are_refused,
            a_catalog_run_reads_each_passing_file_through_the_store,
            setup_without_uv_exits_1_naming_it,
            output_onto_the_fit_assembly_is_refused,
            a_helper_that_cannot_start_after_the_check_leaves_the_run_incomplete,
            the_first_error_does_not_depend_on_fit,
            every_handshake_failure_exits_1_with_nothing_written,
            a_control_that_fails_completely_exits_by_its_verdicts,
            max_fits_does_not_depend_on_jobs,
            a_catalog_run_with_a_failing_helper_exits_1_with_its_document,
        ];
    }

    /// FU1, FU3, FU5, FM6, FL1: `--fit` writes v2, the library's document byte
    /// for byte, the original's control and fit beside the candidates; the
    /// bracket matches and the lug is rejected (exit 0); the result never
    /// depends on `--jobs` or `--fit-jobs`. The table names the kernel and the
    /// helper, and each candidate's fit; stderr's summary counts the fits.
    fn a_fit_run_writes_the_library_s_document() {
        let dir = root("document");
        let fake = fitment_fit::testing::Fake::install_embedded(
            &dir,
            fitment_fit::testing::Scenario::Replay,
        );
        let (expected, _) = library(&dir, &fake, fast(), true);
        let fit = fit_dir(&fake);
        let base = [
            "match",
            "bracket.json",
            "m_bracket.stp",
            "m_bracket_lug.stp",
            "--original",
            "s01_bolted.stp",
            "--fit",
            "s01_bolted.stp",
            "--fit-dir",
            &fit,
        ];
        for jobs in [
            &["--jobs", "1"][..],
            &["--jobs", "4", "--fit-jobs", "2"][..],
        ] {
            let mut args = base.to_vec();
            args.extend(jobs);
            args.extend(["--format", "json"]);
            let (code, stdout, stderr) = run(&dir, &args);
            if refused_on_windows(code, &stdout, &stderr) {
                return;
            }
            assert_eq!(code, 0, "{stderr}");
            assert_eq!(stdout, expected, "{jobs:?}: {stderr}");
        }
        let doc = json(&expected);
        assert_eq!(doc["format_version"], 2);
        assert_eq!(doc["candidates"][0]["verdict"], "match");
        assert_eq!(doc["candidates"][1]["fit"]["status"], "interferes");
        assert_eq!(doc["fit"]["max_fits"], Value::Null);
        let (code, table, stderr) = run(&dir, &base);
        assert_eq!(code, 0, "{stderr}");
        let helper = &fitment_fit::SCRIPT_SHA256[..8];
        assert!(
            table.contains(&format!("fit: Open CASCADE 8.0.1 (helper {helper})")),
            "{table}"
        );
        assert!(
            table.contains("fit verified") && table.contains("fit interferes"),
            "{table}"
        );
        assert!(
            stderr.contains(
                "fit: Open CASCADE 8.0.1: 1 verified, 1 interferes, 0 not verified, 0 not checked"
            ),
            "{stderr}"
        );
    }

    /// FV8, M7-15, FU5: a helper that breaks its protocol at every `place`
    /// leaves the control and each candidate `helper_protocol`: the document
    /// is written, the library's, and the run exits 1, its control incomplete
    /// as well as its candidates (1b's `fit_incomplete`).
    fn a_helper_s_failure_writes_the_document_and_exits_1() {
        use fitment_fit::testing::{Scenario, Stage};
        let dir = root("failure");
        let fake =
            fitment_fit::testing::Fake::install_embedded(&dir, Scenario::ErrorFrame(Stage::Place));
        let (expected, judged) = library(&dir, &fake, fast(), false);
        assert!(judged.iter().all(CandidateResult::fit_incomplete));
        let fit = fit_dir(&fake);
        let (code, stdout, stderr) = run(
            &dir,
            &[
                "match",
                "bracket.json",
                "m_bracket.stp",
                "m_bracket_lug.stp",
                "--fit",
                "s01_bolted.stp",
                "--fit-dir",
                &fit,
                "-o",
                "out.json",
            ],
        );
        if refused_on_windows(code, &stdout, &stderr) {
            return;
        }
        assert_eq!(code, 1, "{stderr}");
        assert!(!stdout.is_empty(), "the table is printed");
        assert_eq!(
            std::fs::read_to_string(dir.join("out.json")).unwrap(),
            expected
        );
        let doc = json(&expected);
        for candidate in doc["candidates"].as_array().unwrap() {
            assert_eq!(candidate["fit"]["code"], "helper_protocol", "{candidate}");
            assert_eq!(candidate["verdict"], "interface_match");
        }
        assert_eq!(doc["fit"]["control"][0]["fit"]["code"], "helper_protocol");
        assert!(stderr.contains("the fit is incomplete"), "{stderr}");
        assert!(
            stderr.contains("the original fails its own fit check"),
            "{stderr}"
        );
    }

    /// FL4, FU3: `--fit-timeout` bounds a check; one past it is
    /// `helper_timeout`, and the run exits 1 with its document.
    fn a_check_past_fit_timeout_exits_1() {
        let dir = root("timeout");
        let fake = fitment_fit::testing::Fake::install_embedded(
            &dir,
            fitment_fit::testing::Scenario::HangAtCheck,
        );
        let short = fast().with_timeout(Duration::from_secs(1));
        let (expected, _) = library(&dir, &fake, short, false);
        let fit = fit_dir(&fake);
        let (code, stdout, stderr) = run(
            &dir,
            &[
                "match",
                "bracket.json",
                "m_bracket.stp",
                "--fit",
                "s01_bolted.stp",
                "--fit-dir",
                &fit,
                "--fit-timeout",
                "1",
                "--format",
                "json",
            ],
        );
        if refused_on_windows(code, &stdout, &stderr) {
            return;
        }
        assert_eq!(code, 1, "{stderr}");
        let doc = json(&stdout);
        assert_eq!(doc["candidates"][0]["fit"]["code"], "helper_timeout");
        // The library's own run of the bracket timed out the same way.
        assert_eq!(
            json(&expected)["candidates"][0]["fit"]["code"],
            "helper_timeout"
        );
    }

    /// FU2, FF3, VT74's CLI half: no manifest is exit 1 naming `fitment fit
    /// setup`, before any candidate, nothing on stdout.
    fn no_manifest_names_fitment_fit_setup() {
        let dir = root("no-manifest");
        inputs(&dir);
        fitment_fit::FitDir::create(&dir.join("empty")).unwrap();
        let (code, stdout, stderr) = run(
            &dir,
            &[
                "match",
                "bracket.json",
                "m_bracket.stp",
                "--fit",
                "s01_bolted.stp",
                "--fit-dir",
                "empty",
            ],
        );
        assert_eq!(code, 1, "{stderr}");
        assert!(stdout.is_empty(), "{stdout}");
        assert!(
            stderr.contains("the fit directory holds no helper manifest: run `fitment fit setup`"),
            "{stderr}"
        );
    }

    /// FU1, FF20: `--fit` naming another assembly than the socket's source
    /// is a usage error, exit 2, before any helper; so are the fit's flags
    /// without `--fit`.
    fn another_assembly_is_a_usage_error() {
        let dir = root("other-assembly");
        inputs(&dir);
        std::fs::copy(core("assemblies/s02_cbore.stp"), dir.join("s02.stp")).unwrap();
        let (code, stdout, stderr) = run(
            &dir,
            &[
                "match",
                "bracket.json",
                "m_bracket.stp",
                "--fit",
                "s02.stp",
                "--fit-dir",
                "nowhere",
            ],
        );
        assert_eq!(code, 2, "{stderr}");
        assert_eq!(stdout, "");
        assert!(
            stderr.contains("not the assembly the socket was read from"),
            "{stderr}"
        );
        for flag in [
            ["--max-fits", "1"],
            ["--fit-jobs", "2"],
            ["--fit-timeout", "5"],
        ] {
            let (code, _, stderr) = run(
                &dir,
                &["match", "bracket.json", "m_bracket.stp", flag[0], flag[1]],
            );
            assert_eq!(code, 2, "{flag:?}: {stderr}");
        }
    }

    /// M7-49, FU3, FF26: with `--max-fits 0` the bracket's interface pass is
    /// one too many: refused, exit 3, `too_large`, nothing written; with
    /// `--max-fits 1` the run is the library's, its cap stated in the
    /// document. The original is no candidate: it never counts.
    fn more_interface_passes_than_max_fits_are_refused() {
        let dir = root("max-fits");
        let fake = fitment_fit::testing::Fake::install_embedded(
            &dir,
            fitment_fit::testing::Scenario::Replay,
        );
        library(&dir, &fake, fast(), true);
        let fit = fit_dir(&fake);
        let args = |cap: &'static str| {
            vec![
                "match",
                "bracket.json",
                "m_bracket.stp",
                "--original",
                "s01_bolted.stp",
                "--fit",
                "s01_bolted.stp",
                "--max-fits",
                cap,
                "--format",
                "json",
                "-o",
                "out.json",
            ]
        };
        let mut refused = args("0");
        refused.extend(["--fit-dir", &fit]);
        let (code, stdout, stderr) = run(&dir, &refused);
        if refused_on_windows(code, &stdout, &stderr) {
            return;
        }
        assert_eq!(code, 3, "{stderr}");
        assert!(stdout.is_empty() && !dir.join("out.json").exists());
        assert!(
            stderr.contains("too_large") && stderr.contains("--max-fits 0"),
            "{stderr}"
        );
        let mut capped = args("1");
        capped.extend(["--fit-dir", &fit]);
        let (code, _, stderr) = run(&dir, &capped);
        assert_eq!(code, 0, "{stderr}");
        let doc = json(&std::fs::read_to_string(dir.join("out.json")).unwrap());
        assert_eq!(doc["fit"]["max_fits"], 1);
        assert_eq!(doc["candidates"][0]["verdict"], "match");
    }

    /// FC6, FU6, VT65 through the binary: a catalog run with `--fit` is the
    /// library's (each passing candidate's file read through `FitBlobs`), its
    /// stderr counting the files read, one per fit asked; a damaged file is
    /// `stored_bytes_unreadable` and exit 1, its document written.
    fn a_catalog_run_reads_each_passing_file_through_the_store() {
        use fitment_catalog::{Catalog, NoObserver, Options, Selection};
        let dir = root("catalog");
        let document = inputs(&dir);
        let (mut catalog, _) = Catalog::init(&dir.join("cat"), Options::default()).unwrap();
        catalog.ingest(&[core("candidates")], None).unwrap();
        let fake = fitment_fit::testing::Fake::install_embedded(
            &dir,
            fitment_fit::testing::Scenario::Replay,
        );
        let assembly = std::fs::read(dir.join("s01_bolted.stp")).unwrap();
        let helper = fake.helper(fast());
        let session = OcctSession::open(&helper, assembly, &document).unwrap();
        let replaying = Replaying::answering(&session, &fake, kernel);
        let matcher = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            &replaying,
        )
        .unwrap();
        let blobs = catalog.fit_blobs();
        let run_ = {
            let snapshot = catalog.snapshot().unwrap();
            snapshot
                .match_catalog_fitted(&matcher, &Selection::Full, 2, &NoObserver, &blobs)
                .unwrap()
                .unwrap()
        };
        let asked = replaying.passed().len() - 1; // the control reads no blob
        assert!(asked > 0);
        assert_eq!(
            blobs.reads(),
            asked,
            "one read per fit asked, and only then"
        );
        let expected = MatchDocument::new(&matcher, run_.results).to_json();
        let fit = fit_dir(&fake);
        let args = [
            "match",
            "bracket.json",
            "-C",
            "cat",
            "--fit",
            "s01_bolted.stp",
            "--fit-dir",
            &fit,
            "--format",
            "json",
        ];
        let (code, stdout, stderr) = run(&dir, &args);
        if refused_on_windows(code, &stdout, &stderr) {
            return;
        }
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, expected);
        assert!(
            stderr.contains(&format!("fit: {asked} stored files read")),
            "{stderr}"
        );
        // The bracket's file damaged in the store.
        let sha = fitment_catalog::Sha256Hex::parse(&sha256_hex(
            &std::fs::read(core("candidates/m_bracket.stp")).unwrap(),
        ))
        .unwrap();
        let path = catalog.blob_path(&sha);
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)] // a test damages its own file
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).unwrap();
        std::fs::write(&path, b"damaged").unwrap();
        drop(catalog);
        let (code, stdout, stderr) = run(&dir, &args);
        assert_eq!(code, 1, "{stderr}");
        let doc = json(&stdout);
        let bracket = doc["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["source"]["sha256"] == sha.as_str())
            .unwrap();
        assert_eq!(
            bracket["fit"]["code"], "stored_bytes_unreadable",
            "{bracket}"
        );
        assert_ne!(bracket["verdict"], "match");
        // The store's reason reaches stderr.
        assert!(
            stderr.contains("its stored file could not be read for its fit: damaged"),
            "{stderr}"
        );
    }

    /// FU4, FF1, VT74's CLI half: `fitment fit setup` with no uv to run exits
    /// 1 naming it, on every OS.
    fn setup_without_uv_exits_1_naming_it() {
        let dir = root("no-uv");
        let (code, stdout, stderr) = run(
            &dir,
            &[
                "fit",
                "setup",
                "--fit-dir",
                "fit",
                "--uv",
                "no-such-uv-anywhere",
            ],
        );
        assert_eq!(code, 1, "{stderr}");
        assert!(stdout.is_empty(), "{stdout}");
        assert!(stderr.contains("uv cannot be run"), "{stderr}");
    }

    /// The reviews of PR 3: `-o` naming the `--fit` assembly is a usage
    /// error, exit 2, the assembly unchanged.
    fn output_onto_the_fit_assembly_is_refused() {
        let dir = root("clobber");
        inputs(&dir);
        let before = fs::read(dir.join("s01_bolted.stp")).unwrap();
        let (code, stdout, stderr) = run(
            &dir,
            &[
                "match",
                "bracket.json",
                "m_bracket.stp",
                "--fit",
                "s01_bolted.stp",
                "-o",
                "s01_bolted.stp",
            ],
        );
        assert_eq!(code, 2, "{stderr}");
        assert_eq!(stdout, "");
        assert!(stderr.contains("it is an input"), "{stderr}");
        assert_eq!(fs::read(dir.join("s01_bolted.stp")).unwrap(), before);
    }

    /// FV8 (the reviews of PR 3): the binary checks a helper before any
    /// candidate, so a helper that cannot start after it (here `tmp/` made
    /// 0500 by the checked helper itself) is the machine's: the fits are
    /// `helper_protocol` and the run exits 1, its document written. Unix
    /// only: Windows has no such mode.
    fn a_helper_that_cannot_start_after_the_check_leaves_the_run_incomplete() {
        if !cfg!(unix) {
            return;
        }
        let dir = root("locked-tmp");
        inputs(&dir);
        let fake = Fake::install_embedded(&dir, fitment_fit::testing::Scenario::LocksTmp);
        let fit = fit_dir(&fake);
        let (code, _, stderr) = run(
            &dir,
            &[
                "match",
                "bracket.json",
                "m_bracket.stp",
                "--fit",
                "s01_bolted.stp",
                "--fit-dir",
                &fit,
                "-o",
                "out.json",
            ],
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let tmp = fake.dir.join(fitment_fit::TMP);
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert_eq!(code, 1, "{stderr}");
        let doc = json(&fs::read_to_string(dir.join("out.json")).unwrap());
        assert_eq!(doc["candidates"][0]["fit"]["code"], "helper_protocol");
        assert!(stderr.contains("the fit is incomplete"), "{stderr}");
    }

    /// FU1 (the reviews of PR 3): a run's first error does not depend on
    /// `--fit`: an `--original` that is not the socket's assembly, and a
    /// `--catalog` that is no catalog, are each exit 2, before a fit
    /// directory that is broken is looked at, as without `--fit`.
    fn the_first_error_does_not_depend_on_fit() {
        let dir = root("first-error");
        inputs(&dir);
        fs::copy(core("assemblies/s02_cbore.stp"), dir.join("s02_cbore.stp")).unwrap();
        fs::create_dir_all(dir.join("nocat")).unwrap();
        let fit = ["--fit", "s01_bolted.stp", "--fit-dir", "broken"];
        for (args, said) in [
            (
                &[
                    "match",
                    "bracket.json",
                    "m_bracket.stp",
                    "--original",
                    "s02_cbore.stp",
                ][..],
                "is not the assembly the socket was read from",
            ),
            (&["match", "bracket.json", "-C", "nocat"][..], "nocat"),
        ] {
            let (plain, _, plain_err) = run(&dir, args);
            let mut with_fit = args.to_vec();
            with_fit.extend(fit);
            let (code, stdout, stderr) = run(&dir, &with_fit);
            assert_eq!((plain, code), (2, 2), "{plain_err} / {stderr}");
            assert_eq!(stdout, "");
            assert!(stderr.contains(said), "{stderr}");
            assert_eq!(stderr, plain_err, "the same first error");
        }
    }

    /// FH3, M7-50 through the binary (the reviews of PR 3): a helper that
    /// fails its handshake, by its platform (Windows), its protocol, pid,
    /// script, lock, binding, kernel, site or Python, a missing library or
    /// a crash before `hello`, is exit 1 before any candidate, nothing on
    /// stdout and no `-o` written.
    fn every_handshake_failure_exits_1_with_nothing_written() {
        use fitment_fit::testing::Scenario;
        for scenario in [
            Scenario::WindowsPlatform,
            Scenario::OtherProtocol,
            Scenario::OtherPid,
            Scenario::OtherScript,
            Scenario::OtherLock,
            Scenario::OtherBinding,
            Scenario::OtherKernel,
            Scenario::OtherSite,
            Scenario::OtherPython,
            Scenario::ImportError,
            Scenario::CrashBeforeHello,
        ] {
            let dir = root(&format!("handshake-{}", scenario.name()));
            inputs(&dir);
            let fake = Fake::install_embedded(&dir, scenario);
            let fit = fit_dir(&fake);
            let (code, stdout, stderr) = run(
                &dir,
                &[
                    "match",
                    "bracket.json",
                    "m_bracket.stp",
                    "--fit",
                    "s01_bolted.stp",
                    "--fit-dir",
                    &fit,
                    "-o",
                    "out.json",
                ],
            );
            assert_eq!(code, 1, "{scenario:?}: {stderr}");
            assert_eq!(stdout, "", "{scenario:?}");
            assert!(!dir.join("out.json").exists(), "{scenario:?}");
            assert!(
                stderr.contains("the fit helper cannot be used"),
                "{scenario:?}: {stderr}"
            );
            if scenario == Scenario::WindowsPlatform {
                assert!(stderr.contains("Windows"), "{stderr}");
            }
        }
    }

    /// FT9, FV5 through the binary (the reviews of PR 3): a control that
    /// interferes, complete, withholds the bracket's `verified`
    /// (`original_fails_fit`) and keeps the lug's `Reject`; the run exits
    /// by its verdicts (4), not 1, and stderr says once why.
    fn a_control_that_fails_completely_exits_by_its_verdicts() {
        let dir = root("control-fails");
        let document = inputs(&dir);
        let fake = Fake::install_embedded(&dir, fitment_fit::testing::Scenario::Replay);
        let assembly = fs::read(dir.join("s01_bolted.stp")).unwrap();
        let helper = fake.helper(fast());
        let session = OcctSession::open(&helper, assembly, &document).unwrap();
        let replaying = Replaying::answering(&session, &fake, |request| {
            let mut measures = kernel(request);
            if matches!(request.body(), FitBody::Component { .. }) {
                let plan = request.plan(&measures.scene, &measures.place);
                let plate = plan.asked().position(|p| p.name.starts_with("PLATE"));
                assert!(interfere(&mut measures, &plan, plate.unwrap(), 16.0));
            }
            measures
        });
        let matcher = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            &replaying,
        )
        .unwrap();
        let results: Vec<_> = ["m_bracket.stp", "m_bracket_lug.stp"]
            .iter()
            .map(|n| {
                let bytes = fs::read(dir.join(n)).unwrap();
                (matcher.judge_step(&bytes), vec![(*n).to_owned()])
            })
            .collect();
        assert!(!matcher.fit_incomplete(results.iter().map(|(r, _)| r)));
        let expected = MatchDocument::new(&matcher, results).to_json();
        let fit = fit_dir(&fake);
        let (code, stdout, stderr) = run(
            &dir,
            &[
                "match",
                "bracket.json",
                "m_bracket.stp",
                "m_bracket_lug.stp",
                "--fit",
                "s01_bolted.stp",
                "--fit-dir",
                &fit,
                "--format",
                "json",
            ],
        );
        if refused_on_windows(code, &stdout, &stderr) {
            return;
        }
        assert_eq!(code, 4, "{stderr}");
        assert_eq!(stdout, expected);
        let doc = json(&stdout);
        assert_eq!(doc["fit"]["control"][0]["fit"]["status"], "interferes");
        assert_eq!(doc["candidates"][0]["fit"]["code"], "original_fails_fit");
        assert_eq!(doc["candidates"][1]["fit"]["status"], "interferes");
        assert_eq!(
            stderr
                .matches("the original fails its own fit check")
                .count(),
            1,
            "{stderr}"
        );
        assert!(!stderr.contains("incomplete"), "{stderr}");
    }

    /// M7-49 (the reviews of PR 3): the cap's verdict never depends on
    /// `--jobs` or `--fit-jobs`: the bracket and the lug both pass their
    /// interface, so `--max-fits 1` is refused and `--max-fits 2` writes
    /// the same document, on one thread and on several.
    fn max_fits_does_not_depend_on_jobs() {
        let dir = root("max-fits-jobs");
        let fake = Fake::install_embedded(&dir, fitment_fit::testing::Scenario::Replay);
        library(&dir, &fake, fast(), false);
        let fit = fit_dir(&fake);
        let mut documents = Vec::new();
        for jobs in [
            &["--jobs", "1"][..],
            &["--jobs", "4", "--fit-jobs", "2"][..],
        ] {
            for cap in ["1", "2"] {
                let mut args = vec![
                    "match",
                    "bracket.json",
                    "m_bracket.stp",
                    "m_bracket_lug.stp",
                    "--fit",
                    "s01_bolted.stp",
                    "--fit-dir",
                    &fit,
                    "--max-fits",
                    cap,
                    "--format",
                    "json",
                ];
                args.extend(jobs);
                let (code, stdout, stderr) = run(&dir, &args);
                if refused_on_windows(code, &stdout, &stderr) {
                    return;
                }
                if cap == "1" {
                    assert_eq!(code, 3, "{jobs:?}: {stderr}");
                    assert_eq!(stdout, "");
                } else {
                    assert_eq!(code, 0, "{jobs:?}: {stderr}");
                    documents.push(stdout);
                }
            }
        }
        assert_eq!(documents[0], documents[1]);
    }

    /// FV8 in a catalog run (the reviews of PR 3): a helper that breaks
    /// its protocol, or one whose check passes `--fit-timeout`, leaves the
    /// passing candidates' fits `helper_protocol` or `helper_timeout`, the
    /// document written, exit 1.
    fn a_catalog_run_with_a_failing_helper_exits_1_with_its_document() {
        use fitment_catalog::{Catalog, NoObserver, Options, Selection};
        use fitment_fit::testing::{Scenario, Stage};
        for (scenario, code_) in [
            (Scenario::ErrorFrame(Stage::Place), "helper_protocol"),
            (Scenario::HangAtCheck, "helper_timeout"),
        ] {
            let dir = root(&format!("catalog-{}", scenario.name()));
            let document = inputs(&dir);
            let (mut catalog, _) = Catalog::init(&dir.join("cat"), Options::default()).unwrap();
            catalog.ingest(&[core("candidates")], None).unwrap();
            let fake = Fake::install_embedded(&dir, scenario);
            let assembly = fs::read(dir.join("s01_bolted.stp")).unwrap();
            let helper = fake.helper(fast().with_timeout(Duration::from_secs(1)));
            let session = OcctSession::open(&helper, assembly, &document).unwrap();
            let replaying = Replaying::answering(&session, &fake, kernel);
            let matcher = Matcher::new(
                &document,
                MatchPolicy::default(),
                MatchOptions::default(),
                &replaying,
            )
            .unwrap();
            let blobs = catalog.fit_blobs();
            let snapshot = catalog.snapshot().unwrap();
            let judged = snapshot
                .match_catalog_fitted(&matcher, &Selection::Full, 2, &NoObserver, &blobs)
                .unwrap()
                .unwrap();
            drop(snapshot);
            assert!(matcher.fit_incomplete(judged.results.iter().map(|(r, _)| r)));
            let fit = fit_dir(&fake);
            let (code, stdout, stderr) = run(
                &dir,
                &[
                    "match",
                    "bracket.json",
                    "-C",
                    "cat",
                    "--fit",
                    "s01_bolted.stp",
                    "--fit-dir",
                    &fit,
                    "--fit-timeout",
                    "1",
                    "-o",
                    "out.json",
                ],
            );
            if refused_on_windows(code, &stdout, &stderr) {
                return;
            }
            assert_eq!(code, 1, "{scenario:?}: {stderr}");
            let doc = json(&fs::read_to_string(dir.join("out.json")).unwrap());
            let asked: Vec<&Value> = doc["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| c["fit"]["code"] == code_)
                .collect();
            assert!(!asked.is_empty(), "{scenario:?}: {doc}");
            assert!(
                doc["candidates"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|c| c["verdict"] != "match"),
                "{scenario:?}"
            );
        }
    }
}
