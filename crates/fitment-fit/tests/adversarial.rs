//! VT55 (`docs/research/m7-fit.md`): the fake helper (VT78's mechanics)
//! as a crashing, hanging, dripping, lying or garbling kernel, through the
//! session and fitment-core's matcher on s01's bracket socket. Every
//! scenario ends with FV8's or FH3's code (or FY4's own, `conservation`),
//! never `verified`, on every OS, with no Python. A clean fake's measures
//! cross the channel unchanged and make the bracket a `Match` (FH4).

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::{BRACKET, S01, code, fast, findings, judge, root, s01, short, verified};
use fitment_core::fit::{FitCode, FitFailure};
use fitment_core::{CandidateResult, VerdictKind};
use fitment_fit::testing::{Death, Fake, Replaying, Scenario, write_probe};
use fitment_fit::{HelperError, HelperOptions, HelperReport, OcctSession};

fn main() {
    fitment_fit::harness![
        a_clean_helper_s_measures_are_the_kernel_s,
        frames_that_break_fh1_never_verify,
        replies_that_break_fh5_never_verify,
        a_stale_or_crossed_reply_is_refused,
        measures_out_of_their_ranges_never_verify,
        the_handshake_refuses_another_helper,
        a_missing_library_is_named,
        kernel_noise_reaches_the_log_and_never_a_frame,
        a_crash_is_retried_once_in_a_fresh_helper,
        each_signal_fv8_names_is_classified,
        crashes_before_and_after_hello_never_verify,
        hangs_and_drips_miss_their_deadlines,
        a_helper_that_stops_reading_misses_its_write_deadline,
        a_cancel_kills_the_helper,
    ];
}

/// The bracket judged through a session of `scenario`'s fake, with
/// `options`: its result, every helper failure the session logged, and how
/// many helpers it spawned.
fn through(
    test: &str,
    scenario: Scenario,
    options: HelperOptions,
) -> (CandidateResult, Vec<HelperReport>, usize) {
    let root = root(&format!("{test}-{}", scenario.name()));
    let fake = Fake::install(&root, scenario);
    let helper = fake.helper(options);
    let document = s01();
    let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
    let replaying = Replaying::new(&session, &fake);
    let result = judge(&replaying);
    (result, session.take_reports(), session.spawned())
}

/// Runs `check` on every one of `all` at once, each on its own thread.
fn each<T: Copy + Send + Sync>(all: &[T], check: impl Fn(T) + Sync) {
    std::thread::scope(|s| {
        for &one in all {
            let check = &check;
            s.spawn(move || check(one));
        }
    });
}

/// `scenario` ends the bracket's fit `not_verified` with `expected`, and
/// marks the run incomplete or not.
fn ends(test: &str, scenario: Scenario, expected: FitCode, incomplete: bool) -> Vec<HelperReport> {
    let options = if expected == FitCode::HelperTimeout {
        short()
    } else {
        fast()
    };
    let (result, reports, _) = through(test, scenario, options);
    assert!(!verified(&result), "{scenario:?} verified");
    assert_eq!(
        result.verdict.kind(),
        VerdictKind::InterfaceMatch,
        "{scenario:?}"
    );
    assert_eq!(code(&result), Some(expected), "{scenario:?}: {reports:#?}");
    assert_eq!(result.fit_incomplete(), incomplete, "{scenario:?}");
    reports
}

/// FH4, FH2: a clean helper's replies, the exact kernel's, reach core
/// unchanged for the control and the candidate: scene, place and check,
/// every body, placement and pair in order, the candidate's payload hashed
/// by the helper as sent. Core decides the bracket's `Match`.
fn a_clean_helper_s_measures_are_the_kernel_s() {
    let root = root("clean");
    let fake = Fake::install(&root, Scenario::Replay);
    let helper = fake.helper(fast());
    let document = s01();
    let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
    let replaying = Replaying::new(&session, &fake);
    let result = judge(&replaying);
    assert_eq!(
        result.verdict.kind(),
        VerdictKind::Match,
        "{:?}",
        findings(&result)
    );
    assert!(verified(&result) && !result.fit_incomplete());
    let passed = replaying.passed();
    assert_eq!(passed.len(), 2, "the control, then the candidate");
    for p in &passed {
        assert_eq!(p.exact, p.got);
    }
    assert_eq!(session.take_reports(), []);
    assert_eq!(session.spawned(), 1, "one helper serves both");
    // The helper wrote the candidate's payload where it works.
    let tmp = fake.dir.join(fitment_fit::TMP);
    let helper_dir = std::fs::read_dir(&tmp)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        std::fs::read(helper_dir.join("payload-0.bin")).unwrap(),
        BRACKET
    );
    drop(session);
    assert_eq!(
        std::fs::read_dir(&tmp).unwrap().count(),
        0,
        "FL5: its directory removed"
    );
}

/// FH1: bytes that are no frame, a header cut short, a header over its
/// cap, a payload announced in a reply, a frame cut by the helper's exit,
/// and kernel noise on the frames' channel: each `helper_protocol`, the
/// helper replaced and the run incomplete.
fn frames_that_break_fh1_never_verify() {
    each(
        &[
            Scenario::Garbage,
            Scenario::ShortFrame,
            Scenario::OversizedHeader,
            Scenario::AnnouncedPayload,
            Scenario::ExitsMidReply,
            Scenario::NoiseOnFrames,
        ],
        |scenario| {
            ends("fh1", scenario, FitCode::HelperProtocol, true);
        },
    );
}

/// FH5: `NaN`, a key not asked for, a nullable key left out, a pair
/// missing, twice or not asked, a reply to another operation, an `error`
/// frame: each `helper_protocol`.
fn replies_that_break_fh5_never_verify() {
    each(
        &[
            Scenario::NanInReply,
            Scenario::ExtraKey,
            Scenario::MissingKey,
            Scenario::MissingPair,
            Scenario::ExtraPair,
            Scenario::PairNotAsked,
            Scenario::WrongOp,
            Scenario::ErrorFrame,
        ],
        |scenario| {
            ends("fh5", scenario, FitCode::HelperProtocol, true);
        },
    );
}

/// FH2: a reply whose nonce, request header's SHA-256 or digest of the
/// request is not its request's is never read as its reply.
fn a_stale_or_crossed_reply_is_refused() {
    each(
        &[
            Scenario::WrongNonce,
            Scenario::WrongRequestSha256,
            Scenario::WrongDigest,
        ],
        |scenario| {
            let reports = ends("fh2", scenario, FitCode::HelperProtocol, true);
            assert!(
                reports
                    .iter()
                    .any(|r| r.why.contains("not its request's") || r.why.contains("not core's")),
                "{scenario:?}: {reports:#?}"
            );
        },
    );
}

/// FY4: a negative volume and a residual above its Common are protocol
/// failures; a cut that loses volume is `conservation`, which core finds.
fn measures_out_of_their_ranges_never_verify() {
    ends(
        "fy4",
        Scenario::NegativeVolume,
        FitCode::HelperProtocol,
        true,
    );
    ends(
        "fy4",
        Scenario::ResidualAboveCommon,
        FitCode::HelperProtocol,
        true,
    );
    let (result, _, _) = through("fy4", Scenario::ConservationMiss, fast());
    assert!(!verified(&result));
    assert_eq!(result.verdict.kind(), VerdictKind::InterfaceMatch);
    assert!(
        findings(&result)
            .iter()
            .any(|f| f.code == FitCode::Conservation),
        "{:?}",
        findings(&result)
    );
}

/// FH3: a helper stating another protocol, pid (a launcher between),
/// script, lock, binding, kernel, site or Python is refused at its
/// handshake: `helper_unavailable`, before any request, the run complete.
fn the_handshake_refuses_another_helper() {
    each(
        &[
            Scenario::OtherProtocol,
            Scenario::OtherPid,
            Scenario::OtherScript,
            Scenario::OtherLock,
            Scenario::OtherBinding,
            Scenario::OtherKernel,
            Scenario::OtherSite,
            Scenario::OtherPython,
        ],
        |scenario| {
            let root = root(&format!("fh3-{}", scenario.name()));
            let fake = Fake::install(&root, scenario);
            match fake.helper(fast()).check() {
                Err(HelperError::Failed(report)) => {
                    assert!(
                        matches!(report.failure, FitFailure::Unavailable(_)),
                        "{scenario:?}: {report:?}"
                    );
                    assert!(!report.why.contains(root.to_str().unwrap()), "FV9: no path");
                }
                other => panic!("{scenario:?}: {other:?}"),
            }
            ends("fh3", scenario, FitCode::HelperUnavailable, false);
        },
    );
}

/// FH3: a helper whose kernel cannot be imported names the library.
fn a_missing_library_is_named() {
    let root = root("import");
    let fake = Fake::install(&root, Scenario::ImportError);
    let Err(HelperError::Failed(report)) = fake.helper(fast()).check() else {
        panic!("the helper was accepted");
    };
    assert!(report.why.contains("libGL.so.1"), "{report:?}");
    ends(
        "import",
        Scenario::ImportError,
        FitCode::HelperUnavailable,
        false,
    );
    let (result, _, _) = through("import", Scenario::ImportError, fast());
    assert!(
        findings(&result)
            .iter()
            .any(|f| f.detail.contains("libGL.so.1"))
    );
}

/// FH2: what the kernel prints reaches stderr, kept for the log, and
/// never a frame: the bracket is still a `Match`.
fn kernel_noise_reaches_the_log_and_never_a_frame() {
    let root = root("noise");
    let fake = Fake::install(&root, Scenario::NoiseOnStderr);
    let helper = fake.helper(fast());
    let document = s01();
    let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
    let replaying = Replaying::new(&session, &fake);
    let result = judge(&replaying);
    assert_eq!(result.verdict.kind(), VerdictKind::Match);
    let logged = session.stderr();
    assert!(
        common::eventually(Duration::from_secs(2), || session
            .stderr()
            .iter()
            .any(|s| s.contains("ERR StepFile"))),
        "{logged:?}"
    );
}

/// FV8: a crash is retried once in a fresh helper; one that recovers is
/// a `Match`, one that crashes again `kernel_crashed`, the run complete.
fn a_crash_is_retried_once_in_a_fresh_helper() {
    let (result, reports, spawned) = through("crash", Scenario::CrashOnce, fast());
    assert_eq!(result.verdict.kind(), VerdictKind::Match, "{reports:#?}");
    assert_eq!(spawned, 2, "the crashed helper and a fresh one");
    assert_eq!(reports.len(), 1, "{reports:#?}");
    let reports = ends(
        "crash",
        Scenario::Dies(Death::Segv),
        FitCode::KernelCrashed,
        false,
    );
    assert_eq!(
        reports.len(),
        4,
        "two tries each, the control's and the candidate's"
    );
}

/// FV8: `SIGSEGV`, `SIGBUS`, `SIGABRT` and `SIGFPE` are crashes;
/// `SIGXCPU`, `SIGXFSZ` and a `SIGKILL` fitment did not send are resource
/// failures, which make the run incomplete. Windows has no signals: its
/// crashes are NTSTATUS codes, and resource kills have no mark to read.
fn each_signal_fv8_names_is_classified() {
    each(&Death::ALL, |death| {
        if death.crash() {
            ends(
                "signal",
                Scenario::Dies(death),
                FitCode::KernelCrashed,
                false,
            );
        } else if cfg!(unix) {
            ends(
                "signal",
                Scenario::Dies(death),
                FitCode::ResourceExhausted,
                true,
            );
        }
    });
}

/// FV8: a helper that crashes before its handshake fails no request: a
/// protocol failure. One that crashes after it, at its first request, is
/// retried, then `kernel_crashed`.
fn crashes_before_and_after_hello_never_verify() {
    let reports = ends(
        "hello",
        Scenario::CrashBeforeHello,
        FitCode::HelperProtocol,
        true,
    );
    assert!(
        reports
            .iter()
            .all(|r| r.why.contains("before its handshake")),
        "{reports:#?}"
    );
    ends(
        "hello",
        Scenario::CrashAfterHello,
        FitCode::KernelCrashed,
        false,
    );
}

/// FL4: a helper that never answers, and one that answers a byte a
/// second, are killed at their deadline: `helper_timeout`, the run
/// incomplete, and no run waits much longer than the deadlines it has.
fn hangs_and_drips_miss_their_deadlines() {
    each(&[Scenario::Hang, Scenario::Drips], |scenario| {
        let started = Instant::now();
        ends("deadline", scenario, FitCode::HelperTimeout, true);
        // The control's deadline and the candidate's, 2 s each, and the
        // judging: never the fake's own minute.
        assert!(
            started.elapsed() < Duration::from_secs(40),
            "{scenario:?}: {:?}",
            started.elapsed()
        );
    });
}

/// FL4: a helper that stops reading cannot block a write: the write has
/// the request's deadline, and missing it is a protocol failure.
fn a_helper_that_stops_reading_misses_its_write_deadline() {
    let root = root("stops-reading");
    let fake = Fake::install(&root, Scenario::StopsReading);
    let helper = fake.helper(fast());
    let started = Instant::now();
    let written = write_probe(&helper, 64 << 20, Duration::from_secs(1));
    assert_eq!(written, Err(FitFailure::Protocol));
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "{:?}",
        started.elapsed()
    );
    // A helper that reads takes the same write within it.
    let root = self::root("reads");
    let fake = Fake::install(&root, Scenario::Hang);
    assert_eq!(
        write_probe(&fake.helper(fast()), 64 << 20, Duration::from_secs(20)),
        Ok(())
    );
}

/// FL4, FX4: a cancel is read every 100 ms while a helper is awaited: the
/// helper is killed and the fit is `cancelled`, long before its deadline.
fn a_cancel_kills_the_helper() {
    let root = root("cancel");
    let fake = Fake::install(&root, Scenario::Hang);
    let options = HelperOptions::default().with_setup_timeout(Duration::from_secs(60));
    let helper = fake.helper(options);
    let document = s01();
    let flag = AtomicBool::new(false);
    let session = OcctSession::open(&helper, S01.to_vec(), &document)
        .unwrap()
        .with_cancel(&flag);
    let replaying = Replaying::new(&session, &fake);
    let set = std::sync::Mutex::new(None);
    let result = std::thread::scope(|s| {
        s.spawn(|| {
            // Once a helper is up and awaited.
            assert!(common::eventually(Duration::from_secs(30), || session
                .spawned()
                > 0));
            std::thread::sleep(Duration::from_millis(300));
            *set.lock().unwrap() = Some(Instant::now());
            flag.store(true, Ordering::Relaxed);
        });
        judge(&replaying)
    });
    let set = set.lock().unwrap().unwrap();
    assert!(
        set.elapsed() < Duration::from_secs(5),
        "{:?}",
        set.elapsed()
    );
    assert_eq!(code(&result), Some(FitCode::Cancelled));
    assert!(result.fit_incomplete() && !verified(&result));
    let reports = session.take_reports();
    assert!(
        reports.iter().any(|r| r.status.is_some()),
        "killed and reaped: {reports:#?}"
    );
}
