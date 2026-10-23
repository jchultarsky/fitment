//! FL2, FU4: `fitment fit setup`'s internals, the only step that runs uv
//! and the only one that may download. It writes the embedded script and
//! lock into the fit directory (FL6), asks uv for the locked environment
//! (`uv run --locked --script fit_helper.py --print-env`), writes the
//! manifest from the answer, then spawns a helper from the manifest exactly
//! as a run would, checks its handshake (FH3) and runs the kernel's
//! self-test. A run and the server never run uv: they read the manifest.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::dir::{FitDir, HelperFiles, Manifest, PrepareError, SCRIPT};
use crate::protocol::quoted;
use crate::session::{Handshake, HelperError, HelperOptions, OcctHelper, SelfTest};
use crate::{BINDING, PYTHON};

/// How `fitment fit setup` runs (FU4).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SetupOptions {
    /// The uv to run (`--uv`): `uv` on `PATH` by default.
    pub uv: PathBuf,
    /// How long uv may take, its downloads included: 900 s.
    pub timeout: Duration,
    /// The helper's options, for its handshake and self-test.
    pub helper: HelperOptions,
}

impl Default for SetupOptions {
    fn default() -> Self {
        Self {
            uv: PathBuf::from("uv"),
            timeout: Duration::from_secs(900),
            helper: HelperOptions::default(),
        }
    }
}

impl SetupOptions {
    /// With uv at `uv`.
    #[must_use]
    pub fn with_uv(mut self, uv: impl Into<PathBuf>) -> Self {
        self.uv = uv.into();
        self
    }

    /// With uv's deadline of `timeout`.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// With `helper`'s options for the handshake and self-test.
    #[must_use]
    pub fn with_helper(mut self, helper: HelperOptions) -> Self {
        self.helper = helper;
        self
    }
}

/// What the helper set up answered (FU4).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SetupCheck {
    /// Its handshake held and its self-test passed.
    Passed(SelfTest),
    /// M7-50: its handshake held, and its platform is refused until the
    /// weekly Windows corpus run is green; no fit is verified there.
    Refused {
        /// The handshake.
        handshake: Handshake,
        /// Why, the fixed sentence every fit there records.
        sentence: String,
    },
}

/// What `fitment fit setup` did: the manifest it wrote, and what the helper
/// it then spawned answered.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SetupReport {
    /// The fit directory.
    pub dir: PathBuf,
    /// The manifest written.
    pub manifest: Manifest,
    /// The helper's handshake and self-test.
    pub check: SetupCheck,
}

/// Why `fitment fit setup` failed (FF1 to FF3, FF6): exit 1, with the reason.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum SetupError {
    /// uv cannot be run (FF1).
    #[error("uv cannot be run as {uv}: install uv, or name it with --uv ({why})")]
    NoUv {
        /// The uv named.
        uv: PathBuf,
        /// Why.
        why: String,
    },
    /// uv could not make the locked environment: no wheel for this
    /// platform (FF2), offline without a cache (FF3), or another of uv's
    /// errors, with the end of what it said.
    #[error("uv could not make the helper's environment ({status}): {said}")]
    Uv {
        /// How uv ended.
        status: String,
        /// The end of its stderr, escaped.
        said: String,
    },
    /// uv did not finish within its deadline.
    #[error("uv did not finish within {0:?}")]
    UvTimeout(Duration),
    /// The environment uv made is not the one this build needs.
    #[error("the helper's environment is not this build's: {0}")]
    Environment(String),
    /// The fit directory cannot be prepared.
    #[error(transparent)]
    Prepare(#[from] PrepareError),
    /// The helper failed its handshake (a missing shared library is named,
    /// FF6), or could not run its self-test.
    #[error(transparent)]
    Helper(#[from] HelperError),
    /// The kernel's self-test measured something it must not.
    #[error("the kernel's self-test failed: {0}")]
    SelfTest(String),
}

/// What `--print-env` answers (FL2).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrintEnv {
    interpreter: PathBuf,
    site: PathBuf,
    python: String,
    binding: String,
}

/// FU4: prepares the fit directory at `dir` for this build's helper (the
/// embedded script and lock), as `options` say, and checks a helper from
/// it.
///
/// # Errors
///
/// [`SetupError`], each with its reason.
pub fn setup(dir: &Path, options: &SetupOptions) -> Result<SetupReport, SetupError> {
    setup_with(dir, &HelperFiles::embedded(), options)
}

/// [`setup`] for `files`, a test's own script and lock.
pub(crate) fn setup_with(
    dir: &Path,
    files: &HelperFiles,
    options: &SetupOptions,
) -> Result<SetupReport, SetupError> {
    let fit = FitDir::create(dir)?;
    fit.install(files)?;
    let env = print_env(&fit.path().join(SCRIPT), options)?;
    if env.binding != BINDING {
        return Err(SetupError::Environment(format!(
            "it holds {}, not {BINDING}",
            quoted(&env.binding)
        )));
    }
    if !env
        .python
        .strip_prefix(PYTHON)
        .is_some_and(|rest| rest.starts_with('.'))
    {
        return Err(SetupError::Environment(format!(
            "it runs Python {}, not {PYTHON}",
            quoted(&env.python)
        )));
    }
    let manifest = Manifest::new(env.interpreter, env.site, env.python, files)?;
    fit.write_manifest(&manifest)?;
    let helper = OcctHelper::open(fit.path(), files.clone(), options.helper.clone())?;
    let check = match helper.self_test() {
        Ok(test) if test.passed() => SetupCheck::Passed(test),
        Ok(test) => {
            let failed: Vec<String> = test
                .cases
                .iter()
                .filter(|c| !c.passed)
                .map(|c| {
                    format!(
                        "{}: Common {} mm³ and distance {} mm, alerts {:?}",
                        c.name, c.common_mm3, c.distance_mm, c.alerts
                    )
                })
                .collect();
            return Err(SetupError::SelfTest(failed.join("; ")));
        }
        Err(HelperError::Failed(report)) => match report.handshake.clone() {
            Some(handshake) => SetupCheck::Refused {
                handshake,
                sentence: report.why.clone(),
            },
            None => return Err(HelperError::Failed(report).into()),
        },
        Err(e) => return Err(e.into()),
    };
    Ok(SetupReport {
        dir: dir.to_owned(),
        manifest,
        check,
    })
}

/// FL2: `uv run --locked --script <script> --print-env`, under the
/// deadline: what the locked environment is. uv gets the user's own
/// environment, so its cache and Python installs stay where uv puts them
/// (`UV_CACHE_DIR`, `UV_PYTHON_INSTALL_DIR`).
fn print_env(script: &Path, options: &SetupOptions) -> Result<PrintEnv, SetupError> {
    let mut child = Command::new(&options.uv)
        .args(["run", "--locked", "--script"])
        .arg(script)
        .arg("--print-env")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SetupError::NoUv {
            uv: options.uv.clone(),
            why: e.to_string(),
        })?;
    let out = child.stdout.take().map(drain);
    let err = child.stderr.take().map(drain);
    let until = Instant::now() + options.timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SetupError::UvTimeout(options.timeout));
            }
        }
    };
    let joined =
        |t: Option<thread::JoinHandle<Vec<u8>>>| t.and_then(|t| t.join().ok()).unwrap_or_default();
    let (out, err) = (joined(out), joined(err));
    if !status.success() {
        let tail = &err[err.len().saturating_sub(4096)..];
        return Err(SetupError::Uv {
            status: status.to_string(),
            said: tail.escape_ascii().to_string(),
        });
    }
    let text = String::from_utf8_lossy(&out);
    let line = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    serde_json::from_str(line)
        .map_err(|e| SetupError::Environment(format!("--print-env said {}: {e}", quoted(line))))
}

/// Everything a pipe holds, on a thread of its own.
fn drain(mut from: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut all = Vec::new();
        let _ = from.read_to_end(&mut all);
        all
    })
}
