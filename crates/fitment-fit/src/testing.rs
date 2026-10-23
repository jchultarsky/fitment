//! VT78: the fake helper's mechanics, under the `test-support` feature,
//! which no release build enables. A test installs a fit directory whose
//! manifest names the test binary itself as the interpreter and whose
//! "script" names a [`Scenario`] (a closed enum, so a typo fails to
//! compile); the session spawns it exactly as it would spawn Python
//! (`<interpreter> -I <dir>/fit_helper.py --site <site>`, FL2), and the
//! test binary's harness ([`harness()`], called by each test file's `main`)
//! first checks whether it was started so, and if it was, speaks FH's
//! frames as the scenario says instead of running tests. No Python is
//! needed on any OS.
//!
//! The fake's replies are an exact kernel's (`fitment_core::fit::testing::exact`):
//! [`Replaying`] wraps a session, computes them for each request in the
//! test's own process and leaves them in the scenario's state directory
//! under each request's digest, where the fake reads them, echoes the
//! request's nonce and digests, and breaks them as its scenario says.

mod fake;
mod harness;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use fitment_core::fit::{FitFailure, FitMeasures};
use fitment_core::{FitIdentity, FitRequest, FitVerifier};

pub use fake::fake_main;
pub use harness::{Test, harness};

/// VT78: a test file's `main`: `fitment_fit::harness![test_a, test_b]`
/// runs the functions named as tests ([`harness()`]), after becoming a fake
/// helper if the binary was started as one. The coverage test finds each
/// test by its line in this list.
#[macro_export]
#[doc(hidden)]
macro_rules! harness {
    ($($test:ident),* $(,)?) => {
        $crate::testing::harness(&[$((stringify!($test), $test as fn())),*])
    };
}

use crate::dir::{FitDir, HelperFiles, Manifest};
use crate::frame::{self, Caps};
use crate::protocol::{self, Expect, Op, PairKey, Sent};
use crate::session::{HelperOptions, OcctHelper, OcctSession};

/// The Python version a fake helper's manifest and handshake state.
pub const FAKE_PYTHON: &str = "3.13.0";

/// How a fake helper ends a request (VT78).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Death {
    /// `SIGSEGV`; on Windows an access violation's code.
    Segv,
    /// `SIGBUS`; on Windows an in-page error's code.
    Bus,
    /// `SIGABRT`: `abort()`, on Windows a fast fail.
    Abrt,
    /// `SIGFPE`; on Windows a division by zero's code.
    Fpe,
    /// `SIGXCPU` (Unix only).
    Xcpu,
    /// `SIGXFSZ` (Unix only).
    Xfsz,
    /// A `SIGKILL` fitment did not send (Unix only).
    Kill,
}

impl Death {
    /// Every death.
    pub const ALL: [Self; 7] = [
        Self::Segv,
        Self::Bus,
        Self::Abrt,
        Self::Fpe,
        Self::Xcpu,
        Self::Xfsz,
        Self::Kill,
    ];

    /// Its signal's name, without `SIG`.
    pub fn signal(self) -> &'static str {
        match self {
            Self::Segv => "SEGV",
            Self::Bus => "BUS",
            Self::Abrt => "ABRT",
            Self::Fpe => "FPE",
            Self::Xcpu => "XCPU",
            Self::Xfsz => "XFSZ",
            Self::Kill => "KILL",
        }
    }

    /// True for a crash (FV8), false for a resource kill.
    pub fn crash(self) -> bool {
        matches!(self, Self::Segv | Self::Bus | Self::Abrt | Self::Fpe)
    }
}

/// What a fake helper does (VT55 to VT57, VT78). Unless it says
/// otherwise, it answers every request with the exact kernel's replies
/// [`Replaying`] left for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Scenario {
    /// Every reply as given.
    Replay,
    /// As `Replay`, writing kernel noise on stderr before each reply, as
    /// the real helper's fd 1 does (FH2).
    NoiseOnStderr,
    /// Crashes at its first helper's first `place`; later helpers serve.
    CrashOnce,
    /// As `CrashOnce`, and a later helper's scene differs from the first's
    /// in the last bit of a volume (FL3).
    CrashOnceThenAnotherScene,
    /// Serves, then stays up after `bye` and its stdin's end (FL5).
    IgnoresBye,
    /// Serves, after writing what it was started with to the state
    /// directory: its arguments, environment, working directory and open
    /// descriptors (FY2).
    Report,
    /// Crashes before reading anything.
    CrashBeforeHello,
    /// Crashes at its first `scene`.
    CrashAfterHello,
    /// Dies at `place`, as the death says.
    Dies(Death),
    /// Writes half of its `scene` reply and exits 0.
    ExitsMidReply,
    /// Answers `hello`, then reads and never answers.
    Hang,
    /// Never reads, never answers.
    IgnoresStdin,
    /// Answers `hello`, then reads nothing more.
    StopsReading,
    /// Answers `scene` one byte a second.
    Drips,
    /// A launcher: spawns its own child to serve, and waits.
    Launcher,
    /// Answers `scene` with bytes that are no frame.
    Garbage,
    /// Answers `scene` with a frame whose header is cut short.
    ShortFrame,
    /// Answers `scene` with a header announced over 16 MiB.
    OversizedHeader,
    /// Answers `scene` announcing a payload it never sends.
    AnnouncedPayload,
    /// A `scene` reply holding `NaN`.
    NanInReply,
    /// A `scene` reply with a key no one asked for.
    ExtraKey,
    /// A `check` reply leaving out a nullable key.
    MissingKey,
    /// A `check` reply missing its last pair.
    MissingPair,
    /// A `check` reply with a pair twice.
    ExtraPair,
    /// A `check` reply naming a pair not asked.
    PairNotAsked,
    /// A `place` reply to a `scene` request.
    WrongOp,
    /// An `error` frame for `scene`.
    ErrorFrame,
    /// A `scene` reply with another nonce.
    WrongNonce,
    /// A `scene` reply with another request header's SHA-256.
    WrongRequestSha256,
    /// A `scene` reply with another digest of the request.
    WrongDigest,
    /// A `scene` reply with a negative volume.
    NegativeVolume,
    /// A `check` reply whose residual is above its Common.
    ResidualAboveCommon,
    /// A `check` reply whose cut loses volume (FY4).
    ConservationMiss,
    /// Kernel noise on its frames' channel before the `scene` reply.
    NoiseOnFrames,
    /// A handshake stating protocol 2.
    OtherProtocol,
    /// A handshake stating another pid.
    OtherPid,
    /// A handshake stating another script's SHA-256.
    OtherScript,
    /// A handshake stating another lock's SHA-256.
    OtherLock,
    /// A handshake stating another binding.
    OtherBinding,
    /// A handshake stating another kernel.
    OtherKernel,
    /// A handshake stating another site.
    OtherSite,
    /// A handshake stating another Python.
    OtherPython,
    /// A handshake that cannot import the kernel, naming the missing
    /// library (FH3).
    ImportError,
}

impl Scenario {
    /// Its name in a fake script.
    pub fn name(self) -> String {
        match self {
            Self::Dies(death) => format!("dies-{}", death.signal().to_ascii_lowercase()),
            other => format!("{other:?}"),
        }
    }

    /// The scenario named `name`.
    pub fn named(name: &str) -> Option<Self> {
        if let Some(signal) = name.strip_prefix("dies-") {
            return Death::ALL
                .into_iter()
                .find(|d| d.signal().eq_ignore_ascii_case(signal))
                .map(Self::Dies);
        }
        ALL.into_iter().find(|s| s.name() == name)
    }
}

/// Every scenario but the deaths.
const ALL: [Scenario; 42] = [
    Scenario::Replay,
    Scenario::NoiseOnStderr,
    Scenario::CrashOnce,
    Scenario::CrashOnceThenAnotherScene,
    Scenario::IgnoresBye,
    Scenario::Report,
    Scenario::CrashBeforeHello,
    Scenario::CrashAfterHello,
    Scenario::ExitsMidReply,
    Scenario::Hang,
    Scenario::IgnoresStdin,
    Scenario::StopsReading,
    Scenario::Drips,
    Scenario::Launcher,
    Scenario::Garbage,
    Scenario::ShortFrame,
    Scenario::OversizedHeader,
    Scenario::AnnouncedPayload,
    Scenario::NanInReply,
    Scenario::ExtraKey,
    Scenario::MissingKey,
    Scenario::MissingPair,
    Scenario::ExtraPair,
    Scenario::PairNotAsked,
    Scenario::WrongOp,
    Scenario::ErrorFrame,
    Scenario::WrongNonce,
    Scenario::WrongRequestSha256,
    Scenario::WrongDigest,
    Scenario::NegativeVolume,
    Scenario::ResidualAboveCommon,
    Scenario::ConservationMiss,
    Scenario::NoiseOnFrames,
    Scenario::OtherProtocol,
    Scenario::OtherPid,
    Scenario::OtherScript,
    Scenario::OtherLock,
    Scenario::OtherBinding,
    Scenario::OtherKernel,
    Scenario::OtherSite,
    Scenario::OtherPython,
    Scenario::ImportError,
];

/// The marker a fake script starts with.
const FAKE_SCRIPT: &str = "# fitment fake helper (VT78): read by the test binary, not Python";

/// A fake helper installed under a test's directory: its fit directory,
/// its files, and its state directory.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Fake {
    /// The fit directory.
    pub dir: PathBuf,
    /// The script and lock it was installed with.
    pub files: HelperFiles,
    /// Where [`Replaying`] leaves replies and the fake leaves what it
    /// reports.
    pub state: PathBuf,
}

impl Fake {
    /// FL2, FL6: a fit directory under `root` for `scenario`, set up as
    /// `fitment fit setup` would: the script and lock written whole, a
    /// site directory, and a manifest naming this test binary as the
    /// interpreter.
    ///
    /// # Panics
    ///
    /// If any of it cannot be written.
    #[allow(clippy::expect_used)] // test support: a setup failure fails the test
    pub fn install(root: &Path, scenario: Scenario) -> Self {
        let dir = root.join("fit");
        let state = root.join("state");
        let site = root.join("site-packages");
        fs::create_dir_all(&state).expect("the state directory");
        fs::create_dir_all(&site).expect("the site directory");
        let script = format!(
            "{FAKE_SCRIPT}\nscenario = {}\nstate = {}\n",
            scenario.name(),
            state.display()
        );
        let files = HelperFiles::new(script, format!("# the lock of {}\n", scenario.name()));
        let fit = FitDir::create(&dir).expect("the fit directory");
        fit.install(&files).expect("the helper's files");
        let interpreter = std::env::current_exe().expect("the test binary");
        fit.write_manifest(&Manifest::new(interpreter, site, FAKE_PYTHON, &files))
            .expect("the manifest");
        Self { dir, files, state }
    }

    /// The checked helper of this fit directory.
    ///
    /// # Panics
    ///
    /// If [`OcctHelper::open`] refuses it.
    #[allow(clippy::expect_used)] // test support
    pub fn helper(&self, options: HelperOptions) -> OcctHelper {
        OcctHelper::open(&self.dir, self.files.clone(), options).expect("the fake's helper")
    }

    /// A file the fake wrote to its state directory, if it did.
    pub fn read_state(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.state.join(name)).ok()
    }
}

/// One request [`Replaying`] passed on: the exact kernel's measures, and
/// what the session returned, each as `{:?}` (a box no reader can give is
/// NaN, never equal to itself).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Passed {
    /// The exact kernel's measures.
    pub exact: String,
    /// The session's answer.
    pub got: String,
}

/// A verifier over a session whose helper is a fake: for each request it
/// computes the exact kernel's replies, leaves them in the fake's state
/// directory under each request's digest, and asks the session.
pub struct Replaying<'a, 'h> {
    session: &'a OcctSession<'h>,
    state: PathBuf,
    passed: Mutex<Vec<Passed>>,
}

impl std::fmt::Debug for Replaying<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Replaying")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl<'a, 'h> Replaying<'a, 'h> {
    /// Replies for `fake`'s helper in `session`.
    pub fn new(session: &'a OcctSession<'h>, fake: &Fake) -> Self {
        Self {
            session,
            state: fake.state.clone(),
            passed: Mutex::new(Vec::new()),
        }
    }

    /// Every request passed on so far.
    pub fn passed(&self) -> Vec<Passed> {
        self.passed
            .lock()
            .map_or_else(|p| p.into_inner().clone(), |p| p.clone())
    }

    fn leave(&self, digest: &str, body: &impl serde::Serialize) {
        let Ok(bytes) = serde_json::to_vec(body) else {
            return;
        };
        let temporary = self
            .state
            .join(format!(".{digest}.{:?}", std::thread::current().id()));
        if fs::write(&temporary, bytes).is_ok() {
            let _ = fs::rename(&temporary, self.state.join(format!("{digest}.json")));
        }
    }
}

impl FitVerifier for Replaying<'_, '_> {
    fn identity(&self) -> Option<FitIdentity> {
        self.session.identity()
    }

    fn assembly(&self) -> Option<&[u8]> {
        self.session.assembly()
    }

    fn measure(&self, request: &FitRequest<'_>) -> Result<FitMeasures, FitFailure> {
        let exact = fitment_core::fit::testing::exact(request);
        let plan = request.plan(&exact.scene, &exact.place);
        self.leave(
            &request.scene().request_digest(),
            &protocol::scene_reply(&exact.scene),
        );
        self.leave(
            &request.place_digest(),
            &protocol::place_reply(&exact.place),
        );
        self.leave(
            plan.check_digest(),
            &protocol::check_reply(&exact.check, &plan),
        );
        let got = self.session.measure(request);
        let passed = Passed {
            exact: format!("{:?}", Ok::<_, FitFailure>(exact)),
            got: format!("{got:?}"),
        };
        match self.passed.lock() {
            Ok(mut all) => all.push(passed),
            Err(poisoned) => poisoned.into_inner().push(passed),
        }
        got
    }
}

/// VT55's write deadline (FL4): a helper of `helper` is sent a frame with
/// a payload of `bytes`, which must be written within `within`.
///
/// # Errors
///
/// The failure the fit would record.
pub fn write_probe(helper: &OcctHelper, bytes: usize, within: Duration) -> Result<(), FitFailure> {
    crate::session::write_probe(helper, bytes, within)
}

/// VT75: arbitrary bytes through the frame reader, as either side reads
/// frames, and through every reply decoder, against requests whose nonce
/// and digests are the bytes' own (so the decoders' checks past the
/// envelope are reached) and against fixed ones. It must never panic.
pub fn fuzz_frames(data: &[u8]) {
    for caps in [Caps::REPLY, Caps::REQUEST] {
        let mut from = data;
        while frame::read_frame(&mut from, caps).is_ok() {}
    }
    let mut headers = vec![data.to_vec()];
    if let Ok(read) = frame::read_frame(&mut &data[..], Caps::REPLY) {
        headers.push(read.header);
    }
    for header in &headers {
        for sent in sents(header) {
            let _ = protocol::hello_reply(header, &sent);
            let _ = protocol::scene_measures(header, &sent);
            let _ = protocol::place_measures(header, &sent);
            let _ = protocol::check_measures(header, &sent);
        }
    }
}

/// Requests for [`fuzz_frames`]: each operation, once with fixed values
/// and once with `header`'s own nonce and digests and lists.
fn sents(header: &[u8]) -> Vec<Sent> {
    let value: serde_json::Value = serde_json::from_slice(header).unwrap_or_default();
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    let ids = |v: &serde_json::Value, key: &str| -> Vec<u64> {
        v.get(key)
            .and_then(|k| k.as_array())
            .map(|a| a.iter().filter_map(serde_json::Value::as_u64).collect())
            .unwrap_or_default()
    };
    let list = |key: &str| -> Vec<serde_json::Value> {
        value
            .get("reply")
            .and_then(|r| r.get(key))
            .and_then(|l| l.as_array())
            .cloned()
            .unwrap_or_default()
    };
    let bodies = list("bodies")
        .iter()
        .map(|b| {
            (
                ids(b, "key"),
                b.get("landmarks")
                    .and_then(|l| l.as_array())
                    .map_or(0, Vec::len),
            )
        })
        .collect();
    let placements = list("placements")
        .iter()
        .map(|p| ids(p, "occurrence"))
        .collect();
    let pairs = list("pairs")
        .iter()
        .map(|p| PairKey {
            occurrence: ids(p, "occurrence"),
            body: ids(p, "body"),
            copy: p
                .get("copy")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            zones: p
                .get("zones")
                .and_then(|z| z.as_array())
                .map_or(0, Vec::len),
        })
        .collect();
    let own = |op, expect| Sent {
        op,
        nonce: text("nonce").unwrap_or_default(),
        header_sha256: text("request_sha256").unwrap_or_default(),
        digest: text("digest"),
        expect,
    };
    let fixed = |op, expect| Sent {
        op,
        nonce: "0".repeat(32),
        header_sha256: "0".repeat(64),
        digest: None,
        expect,
    };
    vec![
        own(Op::Hello, Expect::Nothing),
        own(Op::Scene, Expect::Scene(bodies)),
        own(Op::Place, Expect::Place(placements, 2)),
        own(Op::Check, Expect::Check(pairs)),
        fixed(Op::Scene, Expect::Scene(vec![(vec![1], 2)])),
        fixed(Op::Check, Expect::Check(Vec::new())),
    ]
}
