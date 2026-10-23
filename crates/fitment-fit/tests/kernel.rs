//! VT70, VT71, VT74's uv half and s01 through the real helper
//! (`docs/research/m7-fit.md`, section 11, PR 2b): `fitment fit setup`'s
//! internals make a fit directory with uv, and the embedded helper, run
//! from its manifest, measures with Open CASCADE. These need uv and a
//! download (or uv's cache): they run when `FITMENT_FIT_KERNEL=1` is set,
//! as CI's `fit-kernel` job sets it on three OSes, and say they are skipped
//! otherwise. On Windows the helper is refused until the weekly Windows
//! corpus run is green (M7-50): there they check setup, the handshake and
//! the refusal, not verification.

mod common;

use std::fmt::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use common::{BRACKET, S01, code, eventually, findings, s01, verified};
use fitment_core::fit::{FindingAt, FitCode, FitFailure, FitScene};
use fitment_core::{
    CandidateResult, FitStatus, FitVerifier, MatchOptions, MatchPolicy, Matcher, Rigid,
    VerdictKind, sha256_hex,
};
use fitment_fit::testing::{probe, raise_at_pair, stress};
use fitment_fit::{
    HelperError, HelperOptions, OcctHelper, OcctSession, SetupCheck, SetupOptions, SetupReport,
    WINDOWS_UNPROVEN, setup,
};

/// The lug: a tab 5 mm into the plate, in no requirement's region.
const LUG: &[u8] = include_bytes!("../../fitment-core/tests/candidates/m_bracket_lug.stp");

/// The fit directory this test binary set up, once.
struct Kernel {
    report: SetupReport,
}

/// The kernel, if this run asks for it (`FITMENT_FIT_KERNEL=1`); set up
/// once per test binary, in Cargo's target tmp.
fn kernel() -> Option<&'static Kernel> {
    static KERNEL: OnceLock<Option<Kernel>> = OnceLock::new();
    KERNEL
        .get_or_init(|| {
            if std::env::var_os("FITMENT_FIT_KERNEL").is_none_or(|v| v != "1") {
                return None;
            }
            let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
                .join("fitment-fit")
                .join(format!("kernel-{}", std::process::id()))
                .join("fit");
            let _ = std::fs::remove_dir_all(&dir);
            let started = std::time::Instant::now();
            let report = setup(&dir, &SetupOptions::default().with_helper(options()))
                .unwrap_or_else(|e| panic!("fitment fit setup: {e}"));
            eprintln!("kernel: set up in {:.2} s", started.elapsed().as_secs_f64());
            Some(Kernel { report })
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

/// Deadlines a real helper meets on a loaded runner.
fn options() -> HelperOptions {
    HelperOptions::default()
        .with_timeout(Duration::from_secs(120))
        .with_setup_timeout(Duration::from_secs(300))
}

fn helper(kernel: &Kernel, options: HelperOptions) -> OcctHelper {
    OcctHelper::open(
        &kernel.report.dir,
        fitment_fit::HelperFiles::embedded(),
        options,
    )
    .unwrap()
}

/// True where the helper is refused until VT77 is green (M7-50).
fn refused_here() -> bool {
    cfg!(windows)
}

/// A session whose every `measure` is timed: the kernel's share of a fit
/// (its place and check, and a helper's start and scene when one is
/// spawned), for the as-built numbers.
struct Timed<'a, 'h> {
    session: &'a OcctSession<'h>,
    took: std::sync::Mutex<Vec<f64>>,
}

impl FitVerifier for Timed<'_, '_> {
    fn identity(&self) -> Option<fitment_core::FitIdentity> {
        self.session.identity()
    }

    fn assembly(&self) -> Option<&[u8]> {
        self.session.assembly()
    }

    fn measure(
        &self,
        request: &fitment_core::FitRequest<'_>,
    ) -> Result<fitment_core::fit::FitMeasures, FitFailure> {
        let started = std::time::Instant::now();
        let measured = self.session.measure(request);
        self.took
            .lock()
            .unwrap()
            .push(started.elapsed().as_secs_f64());
        measured
    }
}

fn judge(session: &dyn FitVerifier, candidate: &[u8]) -> CandidateResult {
    let document = s01();
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        session,
    )
    .unwrap();
    matcher.judge_step(candidate)
}

/// VT74's uv half (FL2, FU4, FY3): setup writes the manifest from uv's
/// locked environment, its helper's handshake holds, the self-test's three
/// measures are the kernel's, and the limits that took are reported: on
/// Linux `RLIMIT_AS` and `PR_SET_PDEATHSIG`, on macOS neither (its
/// `RLIMIT_AS` raises), on Unix `RLIMIT_CPU`, `RLIMIT_CORE`, `RLIMIT_NOFILE`
/// and `RLIMIT_FSIZE`, on Windows the job object. On Windows, M7-50's
/// refusal after a handshake that held.
#[test]
fn setup_prepares_the_helper_and_its_self_test_passes() {
    let kernel = need_kernel!();
    let manifest = &kernel.report.manifest;
    assert!(manifest.interpreter.is_absolute() && manifest.interpreter.is_file());
    assert!(manifest.site.is_dir(), "{}", manifest.site.display());
    assert!(manifest.python.starts_with("3.13."), "{}", manifest.python);
    assert_eq!(manifest.binding, fitment_fit::BINDING);
    assert_eq!(
        manifest.helper_sha256,
        sha256_hex(fitment_fit::SCRIPT_BYTES)
    );
    assert_eq!(manifest.lock_sha256, sha256_hex(fitment_fit::LOCK_BYTES));
    let handshake = match &kernel.report.check {
        SetupCheck::Passed(test) => {
            assert!(!refused_here(), "M7-50: Windows is refused");
            assert_eq!(test.cases.len(), 3);
            for case in &test.cases {
                eprintln!(
                    "self-test {}: Common {} mm³, distance {} mm",
                    case.name, case.common_mm3, case.distance_mm
                );
                assert!(case.passed, "{case:?}");
            }
            &test.handshake
        }
        SetupCheck::Refused {
            handshake,
            sentence,
        } => {
            assert!(refused_here(), "refused off Windows: {sentence}");
            assert_eq!(sentence, WINDOWS_UNPROVEN);
            assert!(
                handshake.platform.starts_with("win"),
                "{}",
                handshake.platform
            );
            handshake
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(handshake.python, manifest.python);
    let took = |name: &str| {
        handshake
            .limits
            .iter()
            .find(|l| l.name == name)
            .unwrap_or_else(|| panic!("{name} is not reported"))
            .took
    };
    for limit in &handshake.limits {
        eprintln!(
            "limit {}: took {}, {:?}",
            limit.name, limit.took, limit.value
        );
    }
    assert_eq!(took("RLIMIT_AS"), cfg!(target_os = "linux"));
    assert_eq!(took("PR_SET_PDEATHSIG"), cfg!(target_os = "linux"));
    for unix in ["RLIMIT_CPU", "RLIMIT_CORE", "RLIMIT_NOFILE", "RLIMIT_FSIZE"] {
        assert_eq!(took(unix), cfg!(unix), "{unix}");
    }
    assert_eq!(took("JOB_OBJECT"), cfg!(windows));
}

/// s01 through the real helper (section 11, PR 2b's done-when; section
/// 3.3): the lug interferes with the plate, about 299.4 mm³ outside the
/// plate's slab, a `Reject`; the bracket is `verified`, a `Match`. Every
/// body binds by its faces and placement (FB2), every solid is claimed
/// once (FB6) and the candidate's bytes are the ones the helper read
/// (FB4). On Windows, both fits are M7-50's `helper_unavailable`.
#[test]
fn the_lug_interferes_and_the_bracket_verifies() {
    let kernel = need_kernel!();
    let helper = helper(kernel, options());
    let document = s01();
    let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
    let timed = Timed {
        session: &session,
        took: std::sync::Mutex::new(Vec::new()),
    };
    let bracket = judge(&timed, BRACKET);
    let lug = judge(&timed, LUG);
    let again = judge(&timed, BRACKET);
    // Each judge runs the control first (a new matcher), then the
    // candidate: the first measure starts the helper and reads the scene.
    eprintln!(
        "s01: measures (control, bracket, control, lug, control, bracket) in {:.3?} s",
        timed.took.lock().unwrap()
    );
    assert!(verified(&again), "{:?}", findings(&again));
    if refused_here() {
        for result in [&bracket, &lug] {
            assert_eq!(code(result), Some(FitCode::HelperUnavailable));
            assert!(
                findings(result)
                    .iter()
                    .any(|f| f.detail == WINDOWS_UNPROVEN),
                "{:?}",
                findings(result)
            );
        }
        return;
    }
    assert!(verified(&bracket), "{:?}", findings(&bracket));
    assert_eq!(bracket.verdict.kind(), VerdictKind::Match);
    let FitStatus::Interferes { interferences } = lug.verdict.fit() else {
        panic!("the lug: {:?} {:?}", lug.verdict.fit(), findings(&lug));
    };
    assert_eq!(lug.verdict.kind(), VerdictKind::Reject);
    assert_eq!(interferences.len(), 1, "{interferences:?}");
    let plate = &interferences[0];
    eprintln!(
        "s01: the lug interferes with {} by {} mm³",
        plate.neighbor, plate.volume_mm3
    );
    assert!(plate.neighbor.contains("PLATE"), "{plate:?}");
    assert!(
        (plate.volume_mm3 - 299.4).abs() < 0.01,
        "the lug's residual, P2's 299.40006: {}",
        plate.volume_mm3
    );
    assert_eq!(session.spawned(), 1, "one helper served both");
}

/// VT71 (FV2, M7-33): an exception the helper meets in one pair (here one
/// a test asks for) comes back as that pair's `helper_exception` alert, a
/// `kernel_alert`, and the request carries on with the next pair: no
/// other pair has a finding.
#[test]
fn an_exception_in_one_pair_is_that_pair_s_alert() {
    let kernel = need_kernel!();
    if refused_here() {
        return;
    }
    let helper = helper(kernel, options());
    let document = s01();
    let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &session,
    )
    .unwrap();
    // The control runs at the first interface pass (FT9); the raise is
    // asked of the candidate's own check after it.
    assert!(verified(&matcher.judge_step(BRACKET)));
    raise_at_pair(&session, 0);
    let result = matcher.judge_step(BRACKET);
    let all = findings(&result);
    assert_eq!(code(&result), Some(FitCode::KernelAlert), "{all:?}");
    let alert = all.iter().find(|f| f.code == FitCode::KernelAlert).unwrap();
    assert_eq!(alert.detail, "helper_exception", "{alert:?}");
    assert!(matches!(alert.at, FindingAt::Pair { .. }), "{alert:?}");
    for finding in &all {
        assert_eq!(
            finding.at, alert.at,
            "only the raising pair has a finding: {finding:?}"
        );
    }
    // The next candidate, in the same helper, is clean.
    let again = matcher.judge_step(BRACKET);
    assert!(verified(&again), "{:?}", findings(&again));
    assert_eq!(session.spawned(), 1);
}

/// P3's hostile files, made from s01's text: each one Open CASCADE reads
/// without a crash or a hang, two of them (truncated, random) printing
/// `**** ERR StepFile …` on its standard output.
fn hostile() -> Vec<(&'static str, Vec<u8>)> {
    const HEADER: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('h'),'2;1');\nFILE_NAME('h','2026-10-18T00:00:00',(''),(''),'','','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));\nENDSEC;\nDATA;\n";
    const END: &str = "ENDSEC;\nEND-ISO-10303-21;\n";
    let text = String::from_utf8_lossy(S01).into_owned();
    let mut random = Vec::with_capacity(200_000);
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..200_000 {
        x = x
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        random.push((x >> 56) as u8);
    }
    let points = vec!["0."; 2_000_000].join(",");
    let mut chain = String::new();
    for i in 1..=400_000 {
        writeln!(chain, "#{i}=CARTESIAN_POINT('',(0.,0.,{i}.));").unwrap();
    }
    vec![
        ("truncated_half", S01[..S01.len() / 2].to_vec()),
        ("random_bytes", random),
        (
            "dangling_reference",
            text.replacen("#1 = ", "#999999999 = ", 1).into_bytes(),
        ),
        (
            "self_reference",
            format!("{HEADER}#1=CARTESIAN_POINT('',(#1,#1,#1));\n#2=VERTEX_POINT('',#1);\n{END}")
                .into_bytes(),
        ),
        (
            "huge_list",
            format!("{HEADER}#1=CARTESIAN_POINT('',({points}));\n{END}").into_bytes(),
        ),
        (
            "many_entities_400k",
            format!("{HEADER}{chain}{END}").into_bytes(),
        ),
        (
            "nan_coordinates",
            text.replacen("CARTESIAN_POINT('',(", "CARTESIAN_POINT('',(1.E400,", 5)
                .into_bytes(),
        ),
    ]
}

/// VT71 (FH2, FY1, FB4): the real helper handed P3's hostile files
/// directly, bypassing the gate, as an assembly and as a file candidate in
/// s01's scene: every one answers within its deadline with a frame the
/// strict decoders read (the kernel's own noise on its standard output
/// never reaches a frame), and the candidate's SHA-256 is the bytes' own;
/// none of them is one solid. A scene whose bytes are not the SHA-256 it
/// claims is refused.
#[test]
fn hostile_files_end_in_a_frame_or_a_failure() {
    let kernel = need_kernel!();
    if refused_here() {
        return;
    }
    let helper = helper(
        kernel,
        HelperOptions::default()
            .with_timeout(Duration::from_secs(60))
            .with_setup_timeout(Duration::from_secs(60)),
    );
    let document = s01();
    let scene = FitScene::of(S01, &document).unwrap();
    let bracket = scene.bodies().iter().find(|b| b.selected).unwrap();
    let at = vec![("socket-1".to_owned(), bracket.path.clone(), Rigid::IDENTITY)];
    for (name, bytes) in hostile() {
        let started = std::time::Instant::now();
        let alone = probe(&helper, &bytes, &sha256_hex(&bytes), None);
        let (_, place) = probe(&helper, S01, &sha256_hex(S01), Some((&bytes, &at)))
            .unwrap_or_else(|r| panic!("{name} as a candidate: {r:?}"));
        let place = place.unwrap();
        eprintln!(
            "hostile {name} ({} bytes): as an assembly {}, as a candidate valid {}, in {:.2} s",
            bytes.len(),
            alone.as_ref().map_or_else(
                |r| format!("{:?}", r.failure),
                |(s, _)| format!("{} solids", s.solids.len())
            ),
            place.valid,
            started.elapsed().as_secs_f64()
        );
        alone.unwrap_or_else(|r| panic!("{name} as an assembly: {r:?}"));
        assert!(!place.valid, "{name} is no one solid");
        assert_eq!(place.sha256, Some(sha256_hex(&bytes)), "{name}");
    }
    let refused = probe(&helper, S01, &sha256_hex(b"another assembly"), None).unwrap_err();
    assert_eq!(refused.failure, FitFailure::Protocol, "{refused:?}");
    assert!(refused.why.contains("not the scene"), "{}", refused.why);
}

/// True if a process `pid` still runs.
fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        common::alive(pid)
    }
    #[cfg(windows)]
    {
        let listed = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        listed.contains(&format!("\"{pid}\""))
    }
}

/// VT71 (FL4, FL5): a real helper fitment kills, here at a check's
/// deadline of a millisecond (on Windows, at its refused handshake), is
/// reaped and leaves no process.
#[test]
fn a_killed_helper_leaves_no_process() {
    let kernel = need_kernel!();
    let helper = helper(kernel, options().with_timeout(Duration::from_millis(1)));
    let document = s01();
    let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
    let result = judge(&session, BRACKET);
    let expected = if refused_here() {
        FitCode::HelperUnavailable
    } else {
        FitCode::HelperTimeout
    };
    assert_eq!(code(&result), Some(expected), "{:?}", findings(&result));
    let reports = session.take_reports();
    assert_ne!(reports.len(), 0, "a helper was spawned and ended");
    for report in reports {
        let pid = report.pid.expect("a helper was spawned");
        assert!(
            eventually(Duration::from_secs(10), || !alive(pid)),
            "the helper {pid} still runs"
        );
    }
}

/// VT70 (FY3, FV8): the real helper's own bounds end it as a resource
/// kill, `resource_exhausted`: past its CPU limit by `SIGXCPU` (Linux and
/// macOS: the deadline it states is 2 s, fitment's wait 120 s), and past
/// `RLIMIT_AS` (Linux) by taking the memory it is bounded to; macOS cannot
/// set `RLIMIT_AS`, so there the memory is taken and the helper answers.
#[test]
fn the_helper_s_own_bounds_end_it() {
    let kernel = need_kernel!();
    if refused_here() {
        return;
    }
    let cpu = helper(
        kernel,
        options()
            .with_timeout(Duration::from_secs(2))
            .with_setup_timeout(Duration::from_secs(120)),
    );
    let started = std::time::Instant::now();
    let Err(HelperError::Failed(report)) = stress(&cpu, None, Some(60)) else {
        panic!("a helper past its CPU limit answered");
    };
    eprintln!(
        "CPU: ended in {:.2} s, {:?}",
        started.elapsed().as_secs_f64(),
        report.status
    );
    assert_eq!(report.failure, FitFailure::ResourceExhausted, "{report:?}");
    assert!(started.elapsed() < Duration::from_secs(60));
    let memory = helper(kernel, options());
    let taken = stress(&memory, Some(memory.options().memory_mib), None);
    if cfg!(target_os = "linux") {
        let Err(HelperError::Failed(report)) = taken else {
            panic!("a helper past RLIMIT_AS answered");
        };
        assert_eq!(report.failure, FitFailure::ResourceExhausted, "{report:?}");
    } else {
        assert!(taken.is_ok_and(|t| t.passed()), "macOS: no RLIMIT_AS");
    }
}

/// FL5: on Linux a helper asks to be killed when the thread that spawned
/// it ends (`PR_SET_PDEATHSIG` follows the thread, not the process), so
/// fitment-fit spawns every helper from a thread of the checked helper's
/// own: a helper a judging thread asked for serves on after that thread
/// has ended.
#[test]
fn a_helper_outlives_the_thread_that_asked_for_it() {
    let kernel = need_kernel!();
    if refused_here() {
        return;
    }
    let helper = helper(kernel, options());
    let document = s01();
    let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
    std::thread::scope(|s| {
        s.spawn(|| assert!(verified(&judge(&session, BRACKET))));
    });
    std::thread::sleep(Duration::from_millis(500));
    let again = judge(&session, BRACKET);
    assert!(verified(&again), "{:?}", findings(&again));
    assert_eq!(session.spawned(), 1, "the first helper served on");
    let _ = session.identity();
}
