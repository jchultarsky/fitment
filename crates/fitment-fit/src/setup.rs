//! FL2, FU4: `fitment fit setup`'s internals, the only step that runs uv
//! and the only one that may download. It writes the embedded script and
//! lock into the fit directory (FL6), asks uv for the locked environment
//! (`uv run --locked --script fit_helper.py --print-env`), writes the
//! manifest from the answer, then spawns a helper from the manifest exactly
//! as a run would, checks its handshake (FH3) and runs the kernel's
//! self-test. A run and the server never run uv: they read the manifest.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::dir::{FitDir, HelperFiles, Manifest, PrepareError, SCRIPT};
use crate::process::Wait;
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
    /// uv did not finish within its deadline; it was killed and reaped.
    #[error("uv did not finish within {0:?}")]
    UvTimeout(Duration),
    /// uv's end could not be waited for; it was killed and reaped.
    #[error("uv's end could not be read: {0}")]
    UvWait(String),
    /// uv's cache would not outlast the run, so the site the manifest
    /// names would vanish.
    #[error("the helper's environment would not last: {0}")]
    UvCache(String),
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

/// uv while it runs: killed and reaped however `print_env` returns, so no
/// early return or panic leaves it running.
struct Uv(Child);

impl Drop for Uv {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// FL2: `uv run --locked --script <script> --print-env`, under the
/// deadline: what the locked environment is. uv gets the user's own
/// environment, so its cache and Python installs stay where uv puts them
/// (`UV_CACHE_DIR`, `UV_PYTHON_INSTALL_DIR`); a cache uv removes after the
/// run is refused before it starts (the reviews of PR 2b).
fn print_env(script: &Path, options: &SetupOptions) -> Result<PrintEnv, SetupError> {
    let temp = std::env::temp_dir();
    if let Some(why) = cache_refusal(
        std::env::var_os("UV_NO_CACHE").as_deref(),
        std::env::var_os("UV_CACHE_DIR").as_deref(),
        &temp,
    ) {
        return Err(SetupError::UvCache(why));
    }
    let env = run_print_env(script, options)?;
    if let Some(why) = site_refusal(&env.site, &temp) {
        return Err(SetupError::UvCache(why));
    }
    Ok(env)
}

/// uv's run itself, its stdout parsed.
fn run_print_env(script: &Path, options: &SetupOptions) -> Result<PrintEnv, SetupError> {
    let mut uv = Uv(Command::new(&options.uv)
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
        })?);
    let out = uv.0.stdout.take().map(drain);
    let err = uv.0.stderr.take().map(drain);
    // A deadline `Instant` cannot hold waits a year, never panics.
    let until = Wait::new(options.timeout, None).deadline;
    let status = loop {
        match uv.0.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(50)),
            Ok(None) => return Err(SetupError::UvTimeout(options.timeout)),
            Err(e) => return Err(SetupError::UvWait(e.to_string())),
        }
    };
    drop(uv);
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

/// True if `path` lies under `temp`, as written or once resolved.
fn under(path: &Path, temp: &Path) -> bool {
    let resolve = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    let (path, temp_resolved) = (resolve(path), resolve(temp));
    path.starts_with(temp) || path.starts_with(&temp_resolved)
}

/// The reviews of PR 2b: the manifest names the site of uv's environment,
/// in uv's cache, so a cache uv removes after the run (`UV_NO_CACHE`) or one
/// under the temporary directory (which the OS may clear) would leave every
/// later spawn without it. Setup keeps the user's cache, so it is cheap and
/// offline after the first, and refuses those two.
fn cache_refusal(
    no_cache: Option<&OsStr>,
    cache_dir: Option<&OsStr>,
    temp: &Path,
) -> Option<String> {
    let set = no_cache.is_some_and(|v| {
        !(v.is_empty() || v.eq_ignore_ascii_case("0") || v.eq_ignore_ascii_case("false"))
    });
    if set {
        return Some(
            "UV_NO_CACHE is set: uv would make the helper's environment in a cache it removes; unset it"
                .to_owned(),
        );
    }
    cache_dir
        .filter(|d| !d.is_empty() && under(Path::new(d), temp))
        .map(|_| {
            "UV_CACHE_DIR is under the temporary directory, which the OS may clear: name a lasting one"
                .to_owned()
        })
}

/// The site uv reported must outlast the run, and lie outside the
/// temporary directory.
fn site_refusal(site: &Path, temp: &Path) -> Option<String> {
    if !site.is_dir() {
        return Some(
            "uv's environment is gone after its run: uv's cache is not lasting".to_owned(),
        );
    }
    under(site, temp).then(|| {
        "uv's environment is under the temporary directory, which the OS may clear: set UV_CACHE_DIR to a lasting one"
            .to_owned()
    })
}

/// Everything a pipe holds, on a thread of its own.
fn drain(mut from: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut all = Vec::new();
        let _ = from.read_to_end(&mut all);
        all
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reviews of PR 2b: a cache uv removes, or one under the temporary
    /// directory, is refused before uv runs; any other is the user's.
    #[test]
    fn a_cache_that_would_not_last_is_refused() {
        let temp = std::env::temp_dir();
        let os = |s: &str| std::ffi::OsString::from(s);
        assert!(cache_refusal(Some(&os("1")), None, &temp).is_some());
        assert!(cache_refusal(Some(&os("true")), None, &temp).is_some());
        for off in ["", "0", "false", "FALSE"] {
            assert_eq!(cache_refusal(Some(&os(off)), None, &temp), None, "{off:?}");
        }
        let in_temp = temp.join("uv-cache");
        assert!(cache_refusal(None, Some(in_temp.as_os_str()), &temp).is_some());
        let lasting = Path::new(env!("CARGO_MANIFEST_DIR")).join("uv-cache");
        assert_eq!(cache_refusal(None, Some(lasting.as_os_str()), &temp), None);
        assert_eq!(cache_refusal(None, None, &temp), None);
        assert!(site_refusal(&temp.join("no-such-site-for-fitment"), &temp).is_some());
        assert!(site_refusal(&temp, &temp).is_some());
        assert_eq!(
            site_refusal(Path::new(env!("CARGO_MANIFEST_DIR")), &temp),
            None
        );
    }

    /// A uv of the test's own: a shell script.
    #[cfg(unix)]
    fn fake_uv(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let uv = dir.join("uv");
        std::fs::write(&uv, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&uv, std::fs::Permissions::from_mode(0o755)).unwrap();
        uv
    }

    /// The reviews of PR 2b: a deadline `Instant` cannot hold never panics,
    /// and uv's own failure is reported with what it said; uv past its
    /// deadline is killed and reaped, never left running.
    #[cfg(unix)]
    #[test]
    fn uv_s_ends_are_reported_and_it_is_never_left_running() {
        let dir = std::env::temp_dir().join(format!("fitment-fit-uv-{}", std::process::id()));
        let script = dir.join("fit_helper.py");
        let failing = fake_uv(
            &dir.join("a"),
            "echo 'no wheel for this platform' >&2; exit 2",
        );
        let options = SetupOptions::default()
            .with_uv(&failing)
            .with_timeout(Duration::MAX);
        match run_print_env(&script, &options) {
            Err(SetupError::Uv { said, .. }) => assert!(said.contains("no wheel"), "{said}"),
            other => unreachable!("uv's failure was not reported: {other:?}"),
        }
        let pid = dir.join("pid");
        let hanging = fake_uv(
            &dir.join("b"),
            &format!("echo $$ > '{}'; exec sleep 30", pid.display()),
        );
        let options = SetupOptions::default()
            .with_uv(&hanging)
            .with_timeout(Duration::from_millis(500));
        let started = Instant::now();
        assert!(matches!(
            run_print_env(&script, &options),
            Err(SetupError::UvTimeout(_))
        ));
        assert!(started.elapsed() < Duration::from_secs(10));
        let pid = std::fs::read_to_string(&pid).unwrap();
        let alive = Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        assert!(!alive, "uv {pid} is left running");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
