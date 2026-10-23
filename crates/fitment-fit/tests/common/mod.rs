//! What the fake-helper tests share: s01's bracket socket from
//! fitment-core's committed assembly, a matcher over a verifier, a
//! directory of each test's own, and fast deadlines.

#![allow(dead_code)] // each test file uses its own part of it

use std::path::PathBuf;
use std::time::Duration;

use fitment_core::fit::{Finding, FitCode};
use fitment_core::{
    CandidateResult, FitStatus, FitVerifier, MatchOptions, MatchPolicy, Matcher, SocketDocument,
};
use fitment_fit::HelperOptions;

/// s01's bolted assembly, the bracket's socket's source.
pub const S01: &[u8] = include_bytes!("../../../fitment-core/tests/assemblies/s01_bolted.stp");

/// The bracket: its interface passes at a unique placement.
pub const BRACKET: &[u8] = include_bytes!("../../../fitment-core/tests/candidates/m_bracket.stp");

/// s01's bracket socket.
pub fn s01() -> SocketDocument {
    SocketDocument::extract(S01, Some("BRACKET"), None).unwrap()
}

/// A fresh directory for `test`, under Cargo's target tmp.
pub fn root(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("fitment-fit")
        .join(format!("{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Deadlines a test's fake helper meets on a loaded runner: 20 s each;
/// on Windows let past M7-50 (test-support's override), since the fake
/// is no kernel. The real helper's tests never use these.
pub fn fast() -> HelperOptions {
    HelperOptions::default()
        .with_timeout(Duration::from_secs(20))
        .with_setup_timeout(Duration::from_secs(20))
        .with_unproven_platform()
}

/// Deadlines a test means its fake helper to miss: 2 s each; past M7-50
/// on Windows, as [`fast`].
pub fn short() -> HelperOptions {
    HelperOptions::default()
        .with_timeout(Duration::from_secs(2))
        .with_setup_timeout(Duration::from_secs(2))
        .with_unproven_platform()
}

/// The bracket judged against s01's socket with `verifier`.
pub fn judge(verifier: &dyn FitVerifier) -> CandidateResult {
    let document = s01();
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        verifier,
    )
    .unwrap();
    let result = matcher.judge_step(BRACKET);
    assert!(
        result.sockets[0].checks.iter().all(|(_, c)| c.passed()),
        "the bracket's interface passes"
    );
    result
}

/// The candidate's fit code, if its fit is not verified.
pub fn code(result: &CandidateResult) -> Option<FitCode> {
    result.fit.as_deref().and_then(|e| e.outcome().code())
}

/// Every finding of the candidate's fit.
pub fn findings(result: &CandidateResult) -> Vec<Finding> {
    result
        .fit
        .as_deref()
        .map(|e| e.outcome().findings().to_vec())
        .unwrap_or_default()
}

/// True if the candidate's fit is `verified`.
pub fn verified(result: &CandidateResult) -> bool {
    matches!(result.verdict.fit(), FitStatus::Verified { .. })
}

/// True if a process `pid` still runs (Unix: `kill -0`).
#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Waits up to `within` for `done`.
pub fn eventually(within: Duration, done: impl Fn() -> bool) -> bool {
    let until = std::time::Instant::now() + within;
    while std::time::Instant::now() < until {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}
