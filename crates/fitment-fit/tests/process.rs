//! VT56 (`docs/research/m7-fit.md`, FL1, FL3 to FL5): the helper's
//! process. A helper that ignores its stdin is killed at its deadline and
//! reaped, and no process of it remains; a launcher between fitment and the
//! kernel is refused, and its child shown to outlive its kill (why FL2
//! spawns the interpreter directly); a session holds at most `jobs`
//! helpers, retires one after `max_checks`, re-validates a respawned
//! helper's scene, and ends every helper with itself, `bye` first.

mod common;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use common::{S01, code, fast, judge, root, s01, short, verified};
use fitment_core::fit::{FitCode, FitFailure};
use fitment_core::{MatchOptions, MatchPolicy, Matcher, VerdictKind};
use fitment_fit::testing::{Fake, Replaying, Scenario};
use fitment_fit::{HelperError, OcctSession};

fn main() {
    fitment_fit::harness![
        a_helper_that_ignores_its_stdin_is_killed_and_reaped,
        a_launcher_is_refused_and_its_child_outlives_it,
        helpers_are_pooled_up_to_jobs,
        a_helper_is_retired_after_max_checks_and_its_successor_re_validated,
        a_respawned_helper_s_scene_must_be_the_first_s,
        every_helper_ends_with_its_session,
        a_helper_that_ignores_bye_is_killed_after_two_seconds,
        the_server_s_helpers_have_a_process_group_of_their_own,
        a_helper_unavailable_mid_session_leaves_the_run_incomplete,
        a_server_s_helper_is_killed_with_its_group,
    ];
}

/// The pids of every helper of `fake` that reported (`Scenario::Report`).
fn reported(fake: &Fake) -> BTreeSet<u32> {
    std::fs::read_dir(&fake.state)
        .unwrap()
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().into_owned();
            name.strip_prefix("report-")?
                .strip_suffix(".json")?
                .parse()
                .ok()
        })
        .collect()
}

/// FL4, FL5: a helper that never reads is killed at `hello`'s deadline,
/// reaped, and gone; a fit that meets it is `helper_timeout`.
fn a_helper_that_ignores_its_stdin_is_killed_and_reaped() {
    let root = root("ignores-stdin");
    let fake = Fake::install(&root, Scenario::IgnoresStdin);
    let helper = fake.helper(short());
    let started = Instant::now();
    let Err(HelperError::Failed(report)) = helper.check() else {
        panic!("the helper was accepted");
    };
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(report.failure, FitFailure::Timeout);
    assert!(report.status.is_some(), "reaped: {report:?}");
    #[cfg(unix)]
    assert!(
        !common::alive(report.pid.unwrap()),
        "no process of it remains"
    );
    let document = s01();
    let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
    let result = judge(&Replaying::new(&session, &fake));
    assert_eq!(code(&result), Some(FitCode::HelperTimeout));
    assert!(result.fit_incomplete());
    for report in session.take_reports() {
        assert!(report.status.is_some());
        #[cfg(unix)]
        assert!(!common::alive(report.pid.unwrap()));
    }
}

/// FL2, FH3: a launcher's child answers `hello` with its own pid, which is
/// not the process fitment spawned: refused. Killing the launcher leaves
/// the child running until it is done (Unix shows it by its pid).
fn a_launcher_is_refused_and_its_child_outlives_it() {
    let root = root("launcher");
    let fake = Fake::install(&root, Scenario::Launcher);
    let Err(HelperError::Failed(report)) = fake.helper(fast()).check() else {
        panic!("the launcher was accepted");
    };
    assert!(
        matches!(report.failure, FitFailure::Unavailable(_)),
        "{report:?}"
    );
    assert!(report.why.contains("launcher"), "{report:?}");
    assert!(report.status.is_some(), "the launcher is reaped");
    let grandchild: u32 = fake
        .read_state("grandchild.pid")
        .expect("the launcher's child")
        .trim()
        .parse()
        .unwrap();
    #[cfg(unix)]
    {
        assert!(
            common::alive(grandchild),
            "the kernel's process outlived its launcher"
        );
        assert!(common::eventually(Duration::from_secs(15), || {
            !common::alive(grandchild)
        }));
    }
    #[cfg(not(unix))]
    let _ = grandchild;
}

/// FL1: a session holds at most `jobs` helpers; candidates judged on more
/// threads wait for one, and which helper serves a candidate changes
/// nothing of its result.
fn helpers_are_pooled_up_to_jobs() {
    let root = root("pool");
    let fake = Fake::install(&root, Scenario::Report);
    let helper = fake.helper(fast().with_jobs(2));
    let document = s01();
    let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
    let replaying = Replaying::new(&session, &fake);
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &replaying,
    )
    .unwrap();
    let results: Vec<_> = std::thread::scope(|s| {
        let judging: Vec<_> = (0..6)
            .map(|_| s.spawn(|| matcher.judge_step(common::BRACKET)))
            .collect();
        judging.into_iter().map(|j| j.join().unwrap()).collect()
    });
    for result in &results {
        assert_eq!(result.verdict.kind(), VerdictKind::Match);
    }
    assert!(session.spawned() <= 2, "{}", session.spawned());
    assert_eq!(reported(&fake).len(), session.spawned());
    for p in replaying.passed() {
        assert_eq!(p.exact, p.got);
    }
}

/// FL3: a helper serves `max_checks` checks and is retired; the next one
/// loads the scene again, which must be the first's, bit for bit.
fn a_helper_is_retired_after_max_checks_and_its_successor_re_validated() {
    let root = root("retire");
    let fake = Fake::install(&root, Scenario::Report);
    let helper = fake.helper(fast().with_max_checks(1));
    let document = s01();
    let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
    let replaying = Replaying::new(&session, &fake);
    for _ in 0..3 {
        assert_eq!(judge(&replaying).verdict.kind(), VerdictKind::Match);
    }
    // A matcher per judge: its control and its candidate, a helper each.
    assert_eq!(session.spawned(), 6);
    assert_eq!(reported(&fake).len(), 6);
    assert_eq!(session.take_reports(), []);
    #[cfg(unix)]
    for pid in reported(&fake) {
        assert!(
            common::eventually(Duration::from_secs(5), || !common::alive(pid)),
            "{pid}"
        );
    }
}

/// FL3: a helper respawned after a crash reads a scene that differs from
/// the first helper's in the last bit of one volume: the session ends,
/// and every fit after it is a protocol failure, the run incomplete.
fn a_respawned_helper_s_scene_must_be_the_first_s() {
    let root = root("respawn");
    let fake = Fake::install(&root, Scenario::CrashOnceThenAnotherScene);
    let helper = fake.helper(fast());
    let document = s01();
    let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
    let result = judge(&Replaying::new(&session, &fake));
    assert!(!verified(&result));
    assert_eq!(code(&result), Some(FitCode::HelperProtocol));
    assert!(result.fit_incomplete());
    let reports = session.take_reports();
    assert!(
        reports.iter().any(|r| r.why.contains("not the first's")),
        "{reports:#?}"
    );
}

/// FL5, FY2: when a session ends, every helper it holds gets `bye`, exits
/// and is reaped, and its working directory is removed.
fn every_helper_ends_with_its_session() {
    let root = root("ends");
    let fake = Fake::install(&root, Scenario::Report);
    let helper = fake.helper(fast().with_jobs(2));
    let document = s01();
    let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
    let replaying = Replaying::new(&session, &fake);
    assert_eq!(judge(&replaying).verdict.kind(), VerdictKind::Match);
    let tmp = fake.dir.join(fitment_fit::TMP);
    assert!(std::fs::read_dir(&tmp).unwrap().count() > 0);
    let started = Instant::now();
    drop(replaying);
    drop(session);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "bye ends a helper at once"
    );
    assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
    #[cfg(unix)]
    for pid in reported(&fake) {
        assert!(!common::alive(pid), "{pid}");
    }
}

/// FL5: a helper that ignores `bye` and its stdin's end is killed two
/// seconds later, and reaped.
fn a_helper_that_ignores_bye_is_killed_after_two_seconds() {
    let root = root("ignores-bye");
    let fake = Fake::install(&root, Scenario::IgnoresBye);
    let helper = fake.helper(fast());
    let document = s01();
    let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
    assert!(verified(&judge(&Replaying::new(&session, &fake))));
    let started = Instant::now();
    drop(session);
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(1900) && took < Duration::from_secs(5),
        "{took:?}"
    );
    assert_eq!(
        std::fs::read_dir(fake.dir.join(fitment_fit::TMP))
            .unwrap()
            .count(),
        0
    );
}

/// FL5: the server's helpers get a process group of their own, so a
/// terminal's signals never reach them; the CLI's stay in fitment's, so a
/// user's Ctrl-C ends both (Unix; read with `ps`).
fn the_server_s_helpers_have_a_process_group_of_their_own() {
    #[cfg(unix)]
    for own in [false, true] {
        let group_of = |pid: u32| {
            let out = std::process::Command::new("ps")
                .args(["-o", "pgid=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        let root = root(&format!("group-{own}"));
        let fake = Fake::install(&root, Scenario::Report);
        let helper = fake.helper(fast().with_own_process_group(own));
        let document = s01();
        let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
        assert!(verified(&judge(&Replaying::new(&session, &fake))));
        let pid = *reported(&fake).first().unwrap();
        let expected = if own {
            pid.to_string()
        } else {
            group_of(std::process::id())
        };
        assert_eq!(group_of(pid), expected, "own: {own}");
    }
}

/// The reviews of PR 2a (P2): once a session has had a helper, one that
/// cannot start (here its `tmp/` made unwritable mid-session) depends on
/// the machine: a protocol failure, the run incomplete, never a complete
/// `helper_unavailable` (Unix: Windows has no mode that stops it).
fn a_helper_unavailable_mid_session_leaves_the_run_incomplete() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let root = root("mid-session");
        let fake = Fake::install(&root, Scenario::Replay);
        let helper = fake.helper(fast().with_max_checks(1));
        let document = s01();
        let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
        let replaying = Replaying::new(&session, &fake);
        assert!(verified(&judge(&replaying)));
        let tmp = fake.dir.join(fitment_fit::TMP);
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = judge(&replaying);
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(code(&result), Some(FitCode::HelperProtocol));
        assert!(result.fit_incomplete());
        let reports = session.take_reports();
        assert!(
            reports
                .iter()
                .any(|r| r.why.contains("after the session's first helper")),
            "{reports:#?}"
        );
    }
}

/// The reviews of PR 2a (L2): the server's helper has a process group of
/// its own, and a kill ends the group: a child the helper started (here a
/// launcher's) dies with it, rather than living on while it holds the
/// pipes (Unix, by kill(1) on the group).
fn a_server_s_helper_is_killed_with_its_group() {
    #[cfg(unix)]
    {
        let root = root("group-kill");
        let fake = Fake::install(&root, Scenario::Launcher);
        let Err(HelperError::Failed(_)) = fake.helper(fast().with_own_process_group(true)).check()
        else {
            panic!("the launcher was accepted");
        };
        let grandchild: u32 = fake
            .read_state("grandchild.pid")
            .expect("the launcher's child")
            .trim()
            .parse()
            .unwrap();
        // Without the group's kill it lives three seconds past its stdin.
        assert!(
            common::eventually(Duration::from_millis(1500), || !common::alive(grandchild)),
            "the launcher's child outlived its group's kill"
        );
    }
}
