//! FL1 to FL5: the checked helper and the session. An [`OcctHelper`] is
//! built once from the fit directory's manifest (FL2); an [`OcctSession`]
//! per run or job, from a helper and the socket's assembly, implements
//! [`FitVerifier`]. It holds at most `jobs` helper processes, spawned
//! lazily, each taken by one judging thread for one candidate and given
//! back; a helper that fails in any of FV8's ways is killed, reaped and
//! replaced, one that has served `max_checks` checks is retired, and a
//! respawned helper's scene must be the first's, bit for bit (FL3).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use fitment_core::fit::{FitBody, FitFailure, FitMeasures, SceneMeasures};
use fitment_core::{FitIdentity, FitRequest, FitVerifier, SocketDocument, sha256_hex};

use crate::dir::{FitDir, HelperFiles, Manifest, PrepareError, SCRIPT};
use crate::exit::ExitClass;
use crate::nonce::Nonces;
use crate::process::{Ended, Process, Spawn, Spawner, Wait};
use crate::protocol::{
    self, EncodeError, HelloRequest, ProtocolError, Request, Sent, Stress, quoted, wire,
};
use crate::{BINDING, FIT_PROTOCOL, KERNEL, KERNEL_NAME, KERNEL_VERSION, PYTHON};

/// How a session runs its helpers (FL1, FL3, FL4, FL5).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HelperOptions {
    /// A candidate's `place` and `check` together, writes included
    /// (`fit.timeout_seconds`, `--fit-timeout`): 120 s.
    pub timeout: Duration,
    /// `hello` and `scene` (`fit.setup_timeout`): 300 s.
    pub setup_timeout: Duration,
    /// The most helpers a session holds (`--fit-jobs`): 1 by default.
    pub jobs: usize,
    /// The checks a helper serves before it is retired: 256.
    pub max_checks: u32,
    /// The memory a helper may map (`fit.memory_mib`): 4,096 MiB, by
    /// `RLIMIT_AS` on Linux and a job object on Windows (FY3).
    pub memory_mib: u64,
    /// FL5: a process group of the helper's own, so a terminal's signals
    /// never reach it (the server's); the CLI's helpers stay in fitment's,
    /// so a user's Ctrl-C ends both.
    pub own_process_group: bool,
}

impl Default for HelperOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
            setup_timeout: Duration::from_secs(300),
            jobs: 1,
            max_checks: 256,
            memory_mib: 4096,
            own_process_group: false,
        }
    }
}

impl HelperOptions {
    /// These options with a candidate's deadline of `timeout`.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// With `hello` and `scene`'s deadline of `setup_timeout`.
    #[must_use]
    pub fn with_setup_timeout(mut self, setup_timeout: Duration) -> Self {
        self.setup_timeout = setup_timeout;
        self
    }

    /// With at most `jobs` helpers per session (at least one).
    #[must_use]
    pub fn with_jobs(mut self, jobs: usize) -> Self {
        self.jobs = jobs.max(1);
        self
    }

    /// With each helper retired after `max_checks` checks (at least one).
    #[must_use]
    pub fn with_max_checks(mut self, max_checks: u32) -> Self {
        self.max_checks = max_checks.max(1);
        self
    }

    /// With each helper's memory bounded at `memory_mib` MiB (at least 1).
    #[must_use]
    pub fn with_memory_mib(mut self, memory_mib: u64) -> Self {
        self.memory_mib = memory_mib.max(1);
        self
    }

    /// With the helper in a process group of its own, or not.
    #[must_use]
    pub fn with_own_process_group(mut self, own: bool) -> Self {
        self.own_process_group = own;
        self
    }
}

/// One resource limit a helper reports it set on itself (FY3).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limit {
    /// Its name, such as `RLIMIT_AS`.
    pub name: String,
    /// True if the OS took it.
    pub took: bool,
    /// Its value, if it has one.
    pub value: Option<u64>,
}

/// A helper's handshake, as `fitment fit setup` prints it (FH3, FU4).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Handshake {
    /// The helper's process id, the one fitment spawned.
    pub pid: u32,
    /// Its Python's version.
    pub python: String,
    /// Its `sysconfig` platform tag, for the log only.
    pub platform: String,
    /// The limits it set, and whether each took.
    pub limits: Vec<Limit>,
}

/// One helper's failure, for the log (FV9): what the fit was told, why,
/// how the process ended, and the last 64 KiB of its stderr, escaped.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HelperReport {
    /// The failure the fit records, or would have, on its last try.
    pub failure: FitFailure,
    /// Why, for a person.
    pub why: String,
    /// The process, if one was spawned.
    pub pid: Option<u32>,
    /// How it ended, once reaped.
    pub status: Option<String>,
    /// The end of its stderr, escaped.
    pub stderr: String,
    /// Its handshake, when the handshake itself held and the helper is
    /// refused for its platform (M7-50), so `fitment fit setup` can say what
    /// answered.
    pub handshake: Option<Handshake>,
}

/// M7-50: until the weekly Windows corpus run (VT77) is green, a helper on
/// Windows is refused: no fit is verified there.
pub const WINDOWS_UNPROVEN: &str = "the fit kernel is not yet proven on Windows: no fit is verified there until the weekly Windows corpus run is green (M7-50)";

/// M7-50's rule: the refusal, if the helper runs on Windows, by this
/// build's own target or by the platform tag the helper states
/// (`sysconfig`'s: `win-amd64`, `win32`, `win-arm64`), so neither alone can
/// let an unproven kernel through.
pub(crate) fn platform_refusal(platform: &str) -> Option<&'static str> {
    refused_on(cfg!(windows), platform)
}

/// M7-50's rule for a build on Windows or not.
fn refused_on(windows: bool, platform: &str) -> Option<&'static str> {
    (windows || platform.starts_with("win")).then_some(WINDOWS_UNPROVEN)
}

/// One case of the self-test (FU4): what the kernel measured, what it
/// must, and whether it did.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SelfTestCase {
    /// `touching`, `overlapping` or `apart`.
    pub name: String,
    /// The Common's volume the kernel measured, in mm³.
    pub common_mm3: f64,
    /// The distance it measured, in mm.
    pub distance_mm: f64,
    /// Its alerts, by class name.
    pub alerts: Vec<String>,
    /// The Common's volume two unit boxes have, in mm³.
    pub expected_common_mm3: f64,
    /// Their distance, in mm.
    pub expected_distance_mm: f64,
    /// True if the kernel measured both within 1e-9 (relative for the
    /// volume) and raised no alert.
    pub passed: bool,
}

/// FU4: a helper's handshake and its self-test.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SelfTest {
    /// The handshake.
    pub handshake: Handshake,
    /// The three cases, in order.
    pub cases: Vec<SelfTestCase>,
}

impl SelfTest {
    /// True if every case passed.
    pub fn passed(&self) -> bool {
        self.cases.iter().all(|c| c.passed)
    }

    fn of(handshake: Handshake, reply: wire::SelfTestReply) -> Self {
        let cases = reply
            .cases
            .into_iter()
            .map(|c| {
                let (common, distance) = match c.name.as_str() {
                    "overlapping" => (1e-3, 0.0),
                    "apart" => (0.0, 0.02),
                    _ => (0.0, 0.0),
                };
                let passed = c.alerts.is_empty()
                    && (c.common_mm3 - common).abs() <= 1e-9 * common.max(1e-3)
                    && (c.distance_mm - distance).abs() <= 1e-9;
                SelfTestCase {
                    name: c.name,
                    common_mm3: c.common_mm3,
                    distance_mm: c.distance_mm,
                    alerts: c.alerts,
                    expected_common_mm3: common,
                    expected_distance_mm: distance,
                    passed,
                }
            })
            .collect();
        Self { handshake, cases }
    }
}

/// Why a helper could not be checked (FU2): a fit directory that is not
/// ready, or a helper that failed its handshake.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum HelperError {
    /// The fit directory refuses.
    #[error(transparent)]
    Prepare(#[from] PrepareError),
    /// The helper failed.
    #[error("the fit helper failed: {}", .0.why)]
    Failed(Box<HelperReport>),
}

/// The classes of FV8, and the handshake's refusal (FH3).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Timeout,
    Cancelled,
    Protocol,
    Crash,
    Resource,
    Unavailable(String),
}

/// A step that failed, and why, for the log.
#[derive(Debug)]
struct Failed {
    kind: Kind,
    why: String,
    handshake: Option<Box<Handshake>>,
}

impl Failed {
    fn new(kind: Kind, why: impl Into<String>) -> Self {
        Self {
            kind,
            why: why.into(),
            handshake: None,
        }
    }

    fn protocol(why: impl Into<String>) -> Self {
        Self::new(Kind::Protocol, why)
    }

    fn unavailable(sentence: impl Into<String>, why: impl Into<String>) -> Self {
        Self::new(Kind::Unavailable(sentence.into()), why)
    }

    /// The failure a fit records: a crash on the request's second try is
    /// `kernel_crashed`; before the request, a protocol failure.
    fn failure(&self) -> FitFailure {
        match &self.kind {
            Kind::Timeout => FitFailure::Timeout,
            Kind::Cancelled => FitFailure::Cancelled,
            Kind::Protocol => FitFailure::Protocol,
            Kind::Crash => FitFailure::Crashed,
            Kind::Resource => FitFailure::ResourceExhausted,
            Kind::Unavailable(sentence) => FitFailure::Unavailable(sentence.clone()),
        }
    }
}

impl From<Ended> for Failed {
    fn from(ended: Ended) -> Self {
        let (kind, why) = match ended {
            Ended::Deadline => (Kind::Timeout, "the helper missed its deadline".to_owned()),
            Ended::WriteDeadline => (
                Kind::Protocol,
                "writing a request missed its deadline".to_owned(),
            ),
            Ended::Cancelled => (Kind::Cancelled, "the fit was cancelled".to_owned()),
            Ended::Protocol(why) => (Kind::Protocol, why),
            Ended::Exited { class, status } => {
                let kind = match class {
                    ExitClass::Crash => Kind::Crash,
                    ExitClass::Resource => Kind::Resource,
                    ExitClass::Interrupted => Kind::Cancelled,
                    ExitClass::Other => Kind::Protocol,
                };
                (
                    kind,
                    format!("the helper ended ({status}) without answering"),
                )
            }
        };
        Self::new(kind, why)
    }
}

impl From<EncodeError> for Failed {
    fn from(e: EncodeError) -> Self {
        Self::protocol(e.to_string())
    }
}

impl From<ProtocolError> for Failed {
    fn from(e: ProtocolError) -> Self {
        Self::protocol(e.0)
    }
}

/// A running helper that has passed its handshake.
#[derive(Debug)]
struct Live {
    process: Process,
    scene: Option<(String, SceneMeasures)>,
    /// FB5: the SHA-256 of the component scene it holds beside the
    /// socket's, if any.
    other: Option<String>,
    checks: u32,
    handshake: Handshake,
}

/// The checked helper (FL1, FL2, FH3): a fit directory whose manifest
/// agrees with this build, and the identity every fit it measures is
/// decided for.
#[derive(Debug)]
pub struct OcctHelper {
    dir: FitDir,
    files: HelperFiles,
    options: HelperOptions,
    nonces: Nonces,
    identity: FitIdentity,
    spawner: Spawner,
}

impl OcctHelper {
    /// FL2: the helper of the fit directory at `dir`, for the script and
    /// lock `files` this build embeds. Nothing is spawned; the directory,
    /// its manifest, the files the manifest names, and `tmp/`'s writability
    /// are checked.
    ///
    /// # Errors
    ///
    /// [`PrepareError`]: each a `helper_unavailable`, "run `fitment fit
    /// setup`".
    pub fn open(
        dir: &Path,
        files: HelperFiles,
        options: HelperOptions,
    ) -> Result<Self, PrepareError> {
        let dir = FitDir::open(dir)?;
        dir.sweep();
        dir.check(&files)?;
        let probe = dir.helper_dir()?;
        std::fs::remove_dir(&probe).map_err(|e| PrepareError::TmpUnwritable {
            path: probe.clone(),
            why: e.to_string(),
        })?;
        let identity = FitIdentity::new(KERNEL, KERNEL_VERSION).with_helper(
            BINDING,
            PYTHON,
            FIT_PROTOCOL,
            files.script_sha256(),
            files.lock_sha256(),
        );
        Ok(Self {
            dir,
            files,
            options,
            nonces: Nonces::new(),
            identity,
            spawner: Spawner::default(),
        })
    }

    /// The identity of every fit this helper measures (FM2).
    pub fn identity(&self) -> &FitIdentity {
        &self.identity
    }

    /// Its options.
    pub fn options(&self) -> &HelperOptions {
        &self.options
    }

    /// Its fit directory.
    pub fn dir(&self) -> &FitDir {
        &self.dir
    }

    /// FU2: spawns one helper, checks its handshake (FH3) and ends it
    /// (FL5): what a run does before any candidate.
    ///
    /// # Errors
    ///
    /// [`HelperError`] with the reason.
    pub fn check(&self) -> Result<Handshake, HelperError> {
        match self.start(None) {
            Ok(mut live) => {
                let handshake = live.handshake.clone();
                self.finish(&mut [&mut live]);
                Ok(handshake)
            }
            Err((failed, process)) => Err(HelperError::Failed(Box::new(report(
                &failed,
                process.as_deref(),
            )))),
        }
    }

    /// FL2, FH3: a helper spawned from the manifest, past its handshake;
    /// or why not, with the process if one was spawned.
    fn start(&self, cancel: Option<&AtomicBool>) -> Result<Live, (Failed, Option<Box<Process>>)> {
        let fail = |f: Failed| (f, None);
        let manifest = self
            .dir
            .check(&self.files)
            .map_err(|e| fail(Failed::unavailable(e.sentence(), e.to_string())))?;
        let dir = self
            .dir
            .helper_dir()
            .map_err(|e| fail(Failed::unavailable(e.sentence(), e.to_string())))?;
        let spawn = Spawn {
            program: manifest.interpreter.clone(),
            args: vec![
                OsString::from("-I"),
                self.dir.path().join(SCRIPT).into_os_string(),
                OsString::from("--site"),
                manifest.site.clone().into_os_string(),
            ],
            dir: dir.clone(),
            own_group: self.options.own_process_group,
        };
        let mut process = match self.spawner.spawn(spawn) {
            Ok(process) => process,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(fail(Failed::unavailable(
                    "the helper's interpreter cannot be started",
                    format!("spawning the helper: {e}"),
                )));
            }
        };
        match self.hello(&mut process, &manifest, cancel) {
            Ok(handshake) => Ok(Live {
                process,
                scene: None,
                other: None,
                checks: 0,
                handshake,
            }),
            Err(failed) => {
                process.kill();
                Err((failed, Some(Box::new(process))))
            }
        }
    }

    /// FH3: the handshake, and the helper's identity checked against the
    /// manifest and this build. A crash before it is no request's: a
    /// protocol failure, not a retry.
    fn hello(
        &self,
        process: &mut Process,
        manifest: &Manifest,
        cancel: Option<&AtomicBool>,
    ) -> Result<Handshake, Failed> {
        let body = HelloRequest {
            protocol: FIT_PROTOCOL,
            helper_sha256: manifest.helper_sha256.clone(),
            lock_sha256: manifest.lock_sha256.clone(),
            binding: manifest.binding.clone(),
            kernel: manifest.kernel.clone(),
            python: manifest.python.clone(),
            site: manifest.site.to_string_lossy().into_owned(),
            timeout_seconds: seconds(self.options.timeout),
            setup_timeout_seconds: seconds(self.options.setup_timeout),
            memory_mib: hello_bounds(&self.options).0,
            max_checks: hello_bounds(&self.options).1,
        };
        let wait = Wait::new(self.options.setup_timeout, cancel);
        let request = protocol::hello(self.nonces.next(), &body)?;
        let reply = round(process, request, &wait, protocol::hello_reply).map_err(|f| {
            if f.kind == Kind::Crash {
                Failed::protocol(format!("{} before its handshake", f.why))
            } else {
                f
            }
        })?;
        identify(&reply, process.pid(), manifest, &self.files)?;
        let refusal = platform_refusal(&reply.platform);
        let handshake = Handshake {
            pid: reply.pid,
            python: reply.python,
            platform: reply.platform,
            limits: reply
                .limits
                .into_iter()
                .map(|l| Limit {
                    name: l.name,
                    took: l.took,
                    value: l.value,
                })
                .collect(),
        };
        match refusal {
            // M7-50: the handshake held; the platform is not proven.
            Some(sentence) => Err(Failed {
                handshake: Some(Box::new(handshake)),
                ..Failed::unavailable(sentence, sentence)
            }),
            None => Ok(handshake),
        }
    }

    /// FU4: spawns one helper, checks its handshake (FH3), runs the
    /// kernel's self-test (two unit boxes touching, overlapping by 1e-3 mm
    /// and apart by 0.02 mm) and ends it (FL5): what `fitment fit setup`
    /// does once the manifest is written.
    ///
    /// # Errors
    ///
    /// [`HelperError`] with the reason; a helper refused for its platform
    /// (M7-50) carries its handshake in the report.
    pub fn self_test(&self) -> Result<SelfTest, HelperError> {
        self.self_test_with(None)
    }

    /// The self-test, with VT70's `stress` if one is given.
    pub(crate) fn self_test_with(&self, stress: Option<Stress>) -> Result<SelfTest, HelperError> {
        let mut live = self.start(None).map_err(|(failed, process)| {
            HelperError::Failed(Box::new(report(&failed, process.as_deref())))
        })?;
        let wait = Wait::new(self.options.setup_timeout, None);
        let measured = protocol::self_test(self.nonces.next(), stress)
            .map_err(Failed::from)
            .and_then(|request| {
                round(&mut live.process, request, &wait, protocol::self_test_reply)
            });
        match measured {
            Ok(reply) => {
                let handshake = live.handshake.clone();
                self.finish(&mut [&mut live]);
                Ok(SelfTest::of(handshake, reply))
            }
            Err(failed) => {
                live.process.kill();
                Err(HelperError::Failed(Box::new(report(
                    &failed,
                    Some(&live.process),
                ))))
            }
        }
    }

    /// FL5: `bye` to each, then 2 s, then a kill; every one reaped.
    fn finish(&self, lives: &mut [&mut Live]) {
        for live in lives.iter_mut() {
            let bye = protocol::bye(self.nonces.next()).ok().map(|r| r.frame);
            live.process.begin_finish(bye);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        for live in lives.iter_mut() {
            live.process.end_finish(deadline);
        }
    }
}

/// FH3: the helper's `hello` against the spawned process, the manifest
/// and this build; each refusal a fixed sentence with no path in it.
fn identify(
    reply: &protocol::wire::HelloReply,
    pid: u32,
    manifest: &Manifest,
    files: &HelperFiles,
) -> Result<(), Failed> {
    let refuse = |sentence: String| Err(Failed::unavailable(sentence.clone(), sentence));
    if reply.protocol != FIT_PROTOCOL {
        return refuse(format!(
            "the helper speaks protocol {}, not {FIT_PROTOCOL}",
            reply.protocol
        ));
    }
    if reply.pid != pid {
        return refuse(
            "the helper's pid is not the spawned process's: a launcher sits between fitment and the kernel"
                .to_owned(),
        );
    }
    if let Some(why) = &reply.unavailable {
        return refuse(format!("the helper cannot measure: {}", quoted(why)));
    }
    let checks = [
        (
            reply.helper_sha256 == files.script_sha256(),
            "the helper's script is not the one this build embeds".to_owned(),
        ),
        (
            reply.lock_sha256 == files.lock_sha256(),
            "the helper's lock is not the one this build embeds".to_owned(),
        ),
        (
            reply.binding == BINDING,
            format!(
                "the helper imports {}, not {BINDING}",
                quoted(&reply.binding)
            ),
        ),
        (
            reply.kernel == KERNEL_NAME,
            format!(
                "the helper's kernel is {}, not {KERNEL_NAME}",
                quoted(&reply.kernel)
            ),
        ),
        (
            reply.python == manifest.python,
            format!(
                "the helper runs Python {}, not the manifest's {}",
                quoted(&reply.python),
                manifest.python
            ),
        ),
        (
            Path::new(&reply.site) == manifest.site,
            "the helper added another site than the manifest's".to_owned(),
        ),
    ];
    match checks.into_iter().find(|(fine, _)| !fine) {
        Some((_, sentence)) => refuse(sentence),
        None => Ok(()),
    }
}

/// One request and its reply under `wait`, decoded by `decode`; a reply
/// that breaks the protocol kills the helper.
fn round<T>(
    process: &mut Process,
    request: Request,
    wait: &Wait<'_>,
    decode: fn(&[u8], &Sent) -> Result<T, ProtocolError>,
) -> Result<T, Failed> {
    process.send(request.frame, wait)?;
    let frame = process.receive(wait)?;
    decode(&frame.header, &request.sent).map_err(|e| {
        process.kill();
        Failed::from(e)
    })
}

/// A failure's report, with what its process left.
fn report(failed: &Failed, process: Option<&Process>) -> HelperReport {
    HelperReport {
        failure: failed.failure(),
        why: failed.why.clone(),
        pid: process.map(Process::pid),
        status: process.and_then(Process::status),
        stderr: process.map(Process::stderr).unwrap_or_default(),
        handshake: failed.handshake.as_deref().cloned(),
    }
}

/// The memory and the checks `hello` states, each at least one, however
/// the options were built (the reviews of PR 2b).
fn hello_bounds(options: &HelperOptions) -> (u64, u32) {
    (options.memory_mib.max(1), options.max_checks.max(1))
}

/// A deadline in whole seconds, at least one, for the helper's own bounds
/// (FL4, FY3).
fn seconds(d: Duration) -> u64 {
    d.as_secs()
        .saturating_add(u64::from(d.subsec_nanos() > 0))
        .max(1)
}

/// Why a session cannot be opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SessionError {
    /// The assembly is not the one the socket was read from (FU1).
    #[error(
        "the assembly is not the socket's source: its SHA-256 is {found}, the socket's {expected}"
    )]
    NotTheSource {
        /// The socket's `source.sha256`.
        expected: String,
        /// The assembly's.
        found: String,
    },
}

/// The helpers a session holds.
#[derive(Debug, Default)]
struct Pool {
    idle: Vec<Live>,
    live: usize,
}

/// A fit session (FL1): one run's or one job's helpers, for one socket's
/// assembly. Implements [`FitVerifier`]; which helper serves a candidate
/// never changes a byte of its result.
#[derive(Debug)]
pub struct OcctSession<'h> {
    helper: &'h OcctHelper,
    assembly: Vec<u8>,
    cancel: Option<&'h AtomicBool>,
    pool: Mutex<Pool>,
    freed: Condvar,
    scenes: Mutex<BTreeMap<String, Vec<u8>>>,
    ended: AtomicBool,
    handshaken: AtomicBool,
    reports: Mutex<Vec<HelperReport>>,
    spawned: AtomicUsize,
    #[cfg(feature = "test-support")]
    panic_next: AtomicBool,
    #[cfg(feature = "test-support")]
    raise_at: Mutex<Option<usize>>,
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One of a session's `jobs` places for a helper (FL1), given back when
/// dropped, so that a panic between taking a helper and giving it back
/// never leaks one (the review of PR 2a, P1).
struct Slot<'s, 'h> {
    session: &'s OcctSession<'h>,
    held: bool,
}

impl Slot<'_, '_> {
    /// The place passes to the pool with its helper.
    fn keep(mut self) {
        self.held = false;
    }
}

impl Drop for Slot<'_, '_> {
    fn drop(&mut self) {
        if self.held {
            let mut pool = locked(&self.session.pool);
            pool.live = pool.live.saturating_sub(1);
            drop(pool);
            self.session.freed.notify_one();
        }
    }
}

impl<'h> OcctSession<'h> {
    /// FL1: a session of `helper` for `socket`, whose assembly is
    /// `assembly`. No helper is spawned until a candidate needs one (FX3).
    ///
    /// # Errors
    ///
    /// [`SessionError::NotTheSource`].
    pub fn open(
        helper: &'h OcctHelper,
        assembly: Vec<u8>,
        socket: &SocketDocument,
    ) -> Result<Self, SessionError> {
        let found = sha256_hex(&assembly);
        if found != socket.source().sha256 {
            return Err(SessionError::NotTheSource {
                expected: socket.source().sha256.clone(),
                found,
            });
        }
        Ok(Self {
            helper,
            assembly,
            cancel: None,
            pool: Mutex::new(Pool::default()),
            freed: Condvar::new(),
            scenes: Mutex::new(BTreeMap::new()),
            ended: AtomicBool::new(false),
            handshaken: AtomicBool::new(false),
            reports: Mutex::new(Vec::new()),
            spawned: AtomicUsize::new(0),
            #[cfg(feature = "test-support")]
            panic_next: AtomicBool::new(false),
            #[cfg(feature = "test-support")]
            raise_at: Mutex::new(None),
        })
    }

    /// FX4: this session, cancelled when `flag` is set: every wait reads
    /// it every 100 ms and kills its helper; the fit is `cancelled`.
    #[must_use]
    pub fn with_cancel(mut self, flag: &'h AtomicBool) -> Self {
        self.cancel = Some(flag);
        self
    }

    /// How many helpers it has spawned.
    pub fn spawned(&self) -> usize {
        self.spawned.load(Ordering::Relaxed)
    }

    /// Every helper failure so far, for the log; taken.
    pub fn take_reports(&self) -> Vec<HelperReport> {
        std::mem::take(&mut *locked(&self.reports))
    }

    /// The end of each idle helper's stderr, escaped, for the log.
    pub fn stderr(&self) -> Vec<String> {
        locked(&self.pool)
            .idle
            .iter()
            .map(|l| l.process.stderr())
            .collect()
    }

    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    }

    fn record(&self, failed: &Failed, process: Option<&Process>) {
        locked(&self.reports).push(report(failed, process));
    }

    /// FL1: a place for a helper, and the idle helper in it if there is
    /// one: an idle helper, else a new place while fewer than `jobs` are
    /// held, else the next one given back, waiting at most until
    /// `deadline` (the review of PR 2a, P1).
    fn place(&self, deadline: Instant) -> Result<(Slot<'_, 'h>, Option<Live>), Failed> {
        let mut pool = locked(&self.pool);
        loop {
            if self.cancelled() {
                return Err(Ended::Cancelled.into());
            }
            if let Some(live) = pool.idle.pop() {
                let slot = Slot {
                    session: self,
                    held: true,
                };
                return Ok((slot, Some(live)));
            }
            if pool.live < self.helper.options.jobs.max(1) {
                pool.live += 1;
                drop(pool);
                let slot = Slot {
                    session: self,
                    held: true,
                };
                return Ok((slot, None));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Failed::new(
                    Kind::Timeout,
                    "no helper was free before the request's deadline",
                ));
            }
            pool = self
                .freed
                .wait_timeout(pool, left.min(Duration::from_millis(100)))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// FL2: a new helper; once a session has had one, a helper that cannot
    /// start depends on the machine, not on the installation the session
    /// was opened with: a protocol failure, the run incomplete (the review
    /// of PR 2a, P2).
    fn spawn(&self) -> Result<Live, Failed> {
        self.spawned.fetch_add(1, Ordering::Relaxed);
        match self.helper.start(self.cancel) {
            Ok(live) => {
                self.handshaken.store(true, Ordering::Relaxed);
                Ok(live)
            }
            Err((mut failed, process)) => {
                if matches!(failed.kind, Kind::Unavailable(_))
                    && self.handshaken.load(Ordering::Relaxed)
                {
                    failed.kind = Kind::Protocol;
                    failed.why = format!("after the session's first helper: {}", failed.why);
                }
                self.record(&failed, process.as_deref());
                Err(failed)
            }
        }
    }

    /// FL3: back to the pool, or retired after `max_checks` checks.
    fn give(&self, slot: Slot<'_, 'h>, mut live: Live) {
        if live.checks >= self.helper.options.max_checks {
            self.helper.finish(&mut [&mut live]);
            drop(slot);
        } else {
            locked(&self.pool).idle.push(live);
            slot.keep();
            self.freed.notify_one();
        }
    }

    /// FL3: the scene `request` names, loaded in `live` if it is not; a
    /// respawned helper's must be the first's, bit for bit, or the session
    /// ends.
    fn scene(&self, live: &mut Live, request: &FitRequest<'_>) -> Result<SceneMeasures, Failed> {
        let digest = request.scene().request_digest();
        if let Some((loaded, measures)) = &live.scene {
            if *loaded == digest {
                return Ok(measures.clone());
            }
        }
        let wait = Wait::new(self.helper.options.setup_timeout, self.cancel);
        let sent = protocol::scene(self.helper.nonces.next(), request.scene(), &self.assembly)?;
        let measures = round(&mut live.process, sent, &wait, protocol::scene_measures)?;
        let bits = serde_json::to_vec(&protocol::scene_reply(&measures))
            .map_err(|e| Failed::protocol(e.to_string()))?;
        let mut scenes = locked(&self.scenes);
        match scenes.get(&digest) {
            Some(first) if *first != bits => {
                self.ended.store(true, Ordering::Relaxed);
                live.process.kill();
                return Err(Failed::protocol(
                    "a respawned helper's scene is not the first's: the session ends",
                ));
            }
            Some(_) => {}
            None => {
                scenes.insert(digest.clone(), bits);
            }
        }
        drop(scenes);
        live.scene = Some((digest, measures.clone()));
        Ok(measures)
    }

    /// FB5: the scene of a component's own assembly, when the request's
    /// body is a component of another assembly than the socket's, loaded
    /// in `live` beside the socket's if it is not already. The helper
    /// holds two scenes, the socket's first; a third replaces the other.
    fn other_scene(&self, live: &mut Live, request: &FitRequest<'_>) -> Result<(), Failed> {
        let FitBody::Component {
            assembly: Some(bytes),
            assembly_sha256,
            ..
        } = request.body()
        else {
            return Ok(());
        };
        if live.other.as_deref() == Some(assembly_sha256.as_str()) {
            return Ok(());
        }
        let wait = Wait::new(self.helper.options.setup_timeout, self.cancel);
        let sent = protocol::component_scene(self.helper.nonces.next(), bytes)?;
        round(&mut live.process, sent, &wait, protocol::scene_measures)?;
        live.other = Some(assembly_sha256.clone());
        Ok(())
    }

    /// One candidate's `place` and `check` on `live`, under one deadline.
    fn serve(&self, live: &mut Live, request: &FitRequest<'_>) -> Result<FitMeasures, Failed> {
        let scene = self.scene(live, request)?;
        self.other_scene(live, request)?;
        #[cfg(feature = "test-support")]
        // The review of PR 2a (P1): a panic while a helper is taken.
        assert!(
            !self.panic_next.swap(false, Ordering::Relaxed),
            "a panic forced by the test while a helper is taken"
        );
        let wait = Wait::new(self.helper.options.timeout, self.cancel);
        let sent = protocol::place(self.helper.nonces.next(), request)?;
        let place = round(&mut live.process, sent, &wait, protocol::place_measures)?;
        let plan = request.plan(&scene, &place);
        #[cfg(feature = "test-support")]
        let raise_at = locked(&self.raise_at).take();
        #[cfg(not(feature = "test-support"))]
        let raise_at = None;
        let sent = protocol::check(self.helper.nonces.next(), &plan, raise_at)?;
        let check = round(&mut live.process, sent, &wait, protocol::check_measures)?;
        live.checks += 1;
        Ok(FitMeasures::new(scene, place, check))
    }

    /// The measures of `request` in the helper of `slot`, or `None` in a
    /// new one. An idle helper that sent anything while idle breaks the
    /// protocol and is retired (the review of PR 2a, P5).
    fn serve_in<'s>(
        &'s self,
        slot: Slot<'s, 'h>,
        live: Option<Live>,
        request: &FitRequest<'_>,
    ) -> Result<FitMeasures, (Failed, Slot<'s, 'h>)> {
        let mut live = match live {
            Some(live) => live,
            None => match self.spawn() {
                Ok(live) => live,
                Err(failed) => return Err((failed, slot)),
            },
        };
        let served = match live.process.unasked() {
            Some(why) => Err(Failed::protocol(why)),
            None => self.serve(&mut live, request),
        };
        match served {
            Ok(measures) => {
                self.give(slot, live);
                Ok(measures)
            }
            Err(failed) => {
                // Reaped before it is reported, whatever ended the request.
                live.process.kill();
                self.record(&failed, Some(&live.process));
                drop(live);
                Err((failed, slot))
            }
        }
    }
}

impl FitVerifier for OcctSession<'_> {
    fn identity(&self) -> Option<FitIdentity> {
        Some(self.helper.identity.clone())
    }

    fn assembly(&self) -> Option<&[u8]> {
        Some(&self.assembly)
    }

    /// FV8: the helper's measures, or how it failed. A crash is retried
    /// once in a fresh helper, spawned in the crashed one's place, so the
    /// retry never waits for another; a second is `kernel_crashed`.
    fn measure(&self, request: &FitRequest<'_>) -> Result<FitMeasures, FitFailure> {
        if self.ended.load(Ordering::Relaxed) {
            return Err(FitFailure::Protocol);
        }
        let options = &self.helper.options;
        let wait = Wait::new(options.setup_timeout.saturating_add(options.timeout), None);
        let (slot, live) = self.place(wait.deadline).map_err(|f| f.failure())?;
        match self.serve_in(slot, live, request) {
            Ok(measures) => Ok(measures),
            Err((failed, slot)) if failed.kind == Kind::Crash && !self.cancelled() => self
                .serve_in(slot, None, request)
                .map_err(|(f, _)| f.failure()),
            Err((failed, _)) => Err(failed.failure()),
        }
    }
}

impl Drop for OcctSession<'_> {
    /// FL5: every helper ends with the session, `bye` first.
    fn drop(&mut self) {
        let mut idle = std::mem::take(&mut locked(&self.pool).idle);
        let mut lives: Vec<&mut Live> = idle.iter_mut().collect();
        self.helper.finish(&mut lives);
    }
}

/// The review of PR 2a (P1), for the tests: the session's next `serve`
/// panics while its helper is taken.
#[cfg(feature = "test-support")]
pub(crate) fn panic_in_next_serve(session: &OcctSession<'_>) {
    session.panic_next.store(true, Ordering::Relaxed);
}

/// VT71, for the tests: the session's next `check` asks the helper to
/// raise at the pair `index`, as its kernel might.
#[cfg(feature = "test-support")]
pub(crate) fn raise_at_pair(session: &OcctSession<'_>, index: usize) {
    *locked(&session.raise_at) = Some(index);
}

/// A placement a probe asks: its socket, occurrence and transform.
#[cfg(feature = "test-support")]
pub(crate) type ProbeAt = (String, Vec<u64>, fitment_core::Rigid);

/// What a probe of the real helper got (VT71): the scene's measures, and
/// the place's if a candidate was handed.
#[cfg(feature = "test-support")]
pub(crate) type Probed = (
    fitment_core::fit::SceneMeasures,
    Option<fitment_core::fit::PlaceMeasures>,
);

/// VT71, for the tests: a helper of `helper` handed `assembly` as a scene
/// that names no body, claiming the SHA-256 `claimed`, then, if given, the
/// file candidate `candidate` at `placements`; each reply decoded as
/// strictly as a session decodes it, under the helper's deadlines.
#[cfg(feature = "test-support")]
pub(crate) fn probe(
    helper: &OcctHelper,
    (assembly, claimed): (&[u8], &str),
    candidate: Option<(&[u8], &[ProbeAt])>,
) -> Result<Probed, Box<HelperReport>> {
    let mut live = helper
        .start(None)
        .map_err(|(failed, process)| Box::new(report(&failed, process.as_deref())))?;
    let rounds = (|| {
        let wait = Wait::new(helper.options.setup_timeout, None);
        let sent = protocol::scene_probe(helper.nonces.next(), claimed, assembly)?;
        let scene = round(&mut live.process, sent, &wait, protocol::scene_measures)?;
        let Some((bytes, placements)) = candidate else {
            return Ok((scene, None));
        };
        let wait = Wait::new(helper.options.timeout, None);
        let sent = protocol::place_probe(helper.nonces.next(), claimed, bytes, placements)?;
        let place = round(&mut live.process, sent, &wait, protocol::place_measures)?;
        Ok((scene, Some(place)))
    })();
    match rounds {
        Ok(probed) => {
            helper.finish(&mut [&mut live]);
            Ok(probed)
        }
        Err(failed) => {
            live.process.kill();
            Err(Box::new(report(&failed, Some(&live.process))))
        }
    }
}

/// VT55's write deadline (FL4), for the tests: a helper started from
/// `helper` is sent a frame with a payload of `bytes`, which must be
/// written within `within`.
#[cfg(feature = "test-support")]
pub(crate) fn write_probe(
    helper: &OcctHelper,
    bytes: usize,
    within: Duration,
) -> Result<(), FitFailure> {
    let mut live = helper.start(None).map_err(|(failed, _)| failed.failure())?;
    let payload = vec![0u8; bytes];
    let header = format!(
        r#"{{"op":"scene","nonce":"{}","payloads":[{bytes}],"request":{{}}}}"#,
        helper.nonces.next()
    );
    let frame =
        crate::frame::encode(header.as_bytes(), &[&payload]).map_err(|_| FitFailure::Protocol)?;
    let wait = Wait::new(within, None);
    live.process
        .send(frame, &wait)
        .map_err(|e| Failed::from(e).failure())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M7-50: every platform tag Windows' Python states is refused, with
    /// the sentence every fit there records; Linux's and macOS's are not.
    #[test]
    fn only_windows_is_refused() {
        for windows in [false, true] {
            for tag in ["win-amd64", "win32", "win-arm64"] {
                assert_eq!(refused_on(windows, tag), Some(WINDOWS_UNPROVEN), "{tag}");
            }
            for tag in ["linux-x86_64", "linux-aarch64", "macosx-11.0-arm64", "fake"] {
                // A build on Windows refuses whatever tag its helper states.
                let expected = windows.then_some(WINDOWS_UNPROVEN);
                assert_eq!(refused_on(windows, tag), expected, "{tag} {windows}");
            }
        }
        assert_eq!(
            platform_refusal("linux-x86_64"),
            cfg!(windows).then_some(WINDOWS_UNPROVEN)
        );
        assert!(WINDOWS_UNPROVEN.contains("M7-50"));
    }

    /// The reviews of PR 2b: the bounds `hello` states are never 0, however
    /// the options were built.
    #[test]
    fn hello_s_bounds_are_at_least_one() {
        let zero = HelperOptions {
            memory_mib: 0,
            max_checks: 0,
            ..HelperOptions::default()
        };
        assert_eq!(hello_bounds(&zero), (1, 1));
        assert_eq!(hello_bounds(&HelperOptions::default()), (4096, 256));
    }

    /// FL4, FY3: the bounds `hello` states are whole seconds, at least one.
    #[test]
    fn deadlines_are_stated_in_whole_seconds() {
        assert_eq!(seconds(Duration::from_millis(1)), 1);
        assert_eq!(seconds(Duration::from_millis(1500)), 2);
        assert_eq!(seconds(Duration::from_secs(120)), 120);
        assert_eq!(seconds(Duration::MAX), u64::MAX);
    }
}
