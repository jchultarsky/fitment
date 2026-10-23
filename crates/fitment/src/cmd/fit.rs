//! The fit verifier in the CLI (`docs/research/m7-fit.md`, section 4.11):
//! `fitment fit setup` (FU4), and what `fitment match --fit ASSEMBLY`
//! does before any candidate (FU1, FU2): the assembly checked against the
//! socket's source, the fit directory's manifest read, one helper's
//! handshake checked, and a session of at most `--fit-jobs` helpers opened.

use std::fmt::Write as _;
use std::io::Write as _;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use fitment_core::{FitVerifier, NoFitVerifier, SocketDocument, sha256_hex};
use fitment_fit::{
    FitDir, HelperFiles, HelperOptions, OcctHelper, OcctSession, SetupCheck, SetupOptions,
    SetupReport,
};

use super::text::{note, printable};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Prepare the fit helper: write its script and lock to the fit
    /// directory, have uv make its locked environment (the only download,
    /// about 60 MB the first time), write the manifest, then start a helper
    /// from it as a run would and run its self-test.
    ///
    /// Exit codes: 0 ready, 1 error (uv missing, no wheel for this
    /// platform, offline without a cache, a missing shared library).
    Setup {
        /// The fit directory (default: `XDG_CACHE_HOME/fitment/fit`,
        /// `HOME/.cache/fitment/fit` or `LOCALAPPDATA/fitment/fit`).
        #[arg(long, value_name = "DIR")]
        fit_dir: Option<PathBuf>,
        /// The uv to run (default: `uv` on `PATH`).
        #[arg(long, value_name = "PATH")]
        uv: Option<PathBuf>,
    },
}

/// `fitment fit setup` (FU4): 0 with the identity, the limits and the
/// paths on stdout, or 1 with the reason.
pub fn run(args: &Args) -> ExitCode {
    let Command::Setup { fit_dir, uv } = &args.command;
    let Ok(dir) = directory(fit_dir.as_deref()) else {
        return ExitCode::FAILURE;
    };
    let mut options = SetupOptions::default();
    if let Some(uv) = uv {
        options = options.with_uv(uv);
    }
    match fitment_fit::setup(&dir, &options) {
        Ok(report) => {
            // A closed stdout is not an error: setup is done.
            let _ = std::io::stdout()
                .lock()
                .write_all(setup_text(&report).as_bytes());
            ExitCode::SUCCESS
        }
        Err(error) => {
            note(&format!(
                "fitment: fit setup: {}",
                printable(&error.to_string())
            ));
            ExitCode::FAILURE
        }
    }
}

/// FU4's report: the paths, the identity, the limits that took and the
/// self-test, or the platform's refusal (M7-50).
fn setup_text(report: &SetupReport) -> String {
    let m = &report.manifest;
    let shown = |p: &Path| printable(&p.display().to_string());
    let mut out = format!(
        "fit directory  {}\ninterpreter    {} (Python {})\nsite           {}\nbinding        {}\nkernel         {}\nhelper         sha256 {}\nlock           sha256 {}\n",
        shown(&report.dir),
        shown(&m.interpreter),
        printable(&m.python),
        shown(&m.site),
        printable(&m.binding),
        printable(&m.kernel),
        m.helper_sha256,
        m.lock_sha256,
    );
    let handshake = match &report.check {
        SetupCheck::Passed(test) => &test.handshake,
        SetupCheck::Refused { handshake, .. } => handshake,
        _ => return out,
    };
    let limits: Vec<String> = handshake
        .limits
        .iter()
        .map(|l| match (l.took, l.value) {
            (true, Some(v)) => format!("{} {v}", printable(&l.name)),
            (true, None) => printable(&l.name),
            (false, _) => format!("{} not set", printable(&l.name)),
        })
        .collect();
    let _ = write!(
        out,
        "platform       {}\nlimits         {}\n",
        printable(&handshake.platform),
        limits.join(", ")
    );
    match &report.check {
        SetupCheck::Passed(test) => {
            let cases: Vec<String> = test
                .cases
                .iter()
                .map(|c| format!("{} passed", printable(&c.name)))
                .collect();
            let _ = writeln!(out, "self-test      {}", cases.join(", "));
        }
        SetupCheck::Refused { sentence, .. } => {
            let _ = writeln!(out, "refused        {}", printable(sentence));
        }
        _ => {}
    }
    out
}

/// FL6: `--fit-dir`, else the default fit directory; exit 1 when neither
/// can be had.
fn directory(given: Option<&Path>) -> Result<PathBuf, ()> {
    if let Some(dir) = given {
        return Ok(std::path::absolute(dir).unwrap_or_else(|_| dir.to_owned()));
    }
    FitDir::default_path().ok_or_else(|| {
        note("error: no fit directory: set HOME (or LOCALAPPDATA), or give --fit-dir DIR");
    })
}

/// What `--fit` and its flags ask (FU1, FU3).
pub(super) struct Flags<'a> {
    /// `--fit`: the socket's assembly.
    pub assembly: Option<&'a Path>,
    /// `--fit-jobs`.
    pub jobs: Option<NonZeroUsize>,
    /// `--fit-timeout`, in seconds.
    pub timeout: Option<NonZeroU64>,
    /// `--fit-dir`.
    pub dir: Option<&'a Path>,
}

/// A checked helper and the assembly its sessions read (FU2).
pub(super) struct Helper {
    helper: OcctHelper,
    assembly: Vec<u8>,
}

/// The most helpers `--fit-jobs` defaults to (FL1, M7-11).
const DEFAULT_FIT_JOBS: usize = 4;

/// FU1, FU2: with `--fit`, the assembly read and checked against the
/// socket's source (exit 2 if it is another), the fit directory's manifest
/// read (FL2) and one helper's handshake checked (FH3), each failure exit
/// 1 with its reason and nothing on stdout; `None` without `--fit`.
/// `jobs` is the run's `--jobs`: `--fit-jobs` defaults to min(jobs, 4).
pub(super) fn helper(
    flags: &Flags<'_>,
    document: &SocketDocument,
    jobs: usize,
) -> Result<Option<Helper>, ExitCode> {
    let Some(path) = flags.assembly else {
        return Ok(None);
    };
    let shown = printable(&path.display().to_string());
    let assembly = std::fs::read(path).map_err(|error| {
        note(&format!("error: cannot read {shown}: {error}"));
        ExitCode::FAILURE
    })?;
    if sha256_hex(&assembly) != document.source().sha256 {
        note(&format!(
            "error: --fit {shown}: not the assembly the socket was read from"
        ));
        return Err(ExitCode::from(2));
    }
    let dir = directory(flags.dir).map_err(|()| ExitCode::FAILURE)?;
    let jobs = flags
        .jobs
        .map_or_else(|| jobs.clamp(1, DEFAULT_FIT_JOBS), NonZeroUsize::get);
    let mut options = HelperOptions::default().with_jobs(jobs);
    if let Some(seconds) = flags.timeout {
        options = options.with_timeout(Duration::from_secs(seconds.get()));
    }
    let helper = OcctHelper::open(&dir, HelperFiles::embedded(), options).map_err(|error| {
        let sentence = error.sentence();
        let hint = if sentence.contains("fitment fit setup") {
            ""
        } else {
            ": run `fitment fit setup` to prepare it"
        };
        note(&format!(
            "fitment: --fit: {}: {}{hint}",
            printable(&dir.display().to_string()),
            printable(&sentence)
        ));
        ExitCode::FAILURE
    })?;
    if let Err(error) = helper.check() {
        note(&format!(
            "fitment: --fit: the fit helper cannot be used: {}",
            printable(&error.to_string())
        ));
        return Err(ExitCode::FAILURE);
    }
    Ok(Some(Helper { helper, assembly }))
}

/// The session of `helper` for `document` (FL1): its helpers spawned when
/// a candidate first needs one.
pub(super) fn session<'h>(
    helper: Option<&'h Helper>,
    document: &SocketDocument,
) -> Result<Option<OcctSession<'h>>, ExitCode> {
    let Some(helper) = helper else {
        return Ok(None);
    };
    OcctSession::open(&helper.helper, helper.assembly.clone(), document)
        .map(Some)
        .map_err(|error| {
            note(&format!("error: --fit: {}", printable(&error.to_string())));
            ExitCode::from(2)
        })
}

/// The run's verifier: the session, or none.
pub(super) fn verifier<'s>(session: Option<&'s OcctSession<'_>>) -> &'s dyn FitVerifier {
    match session {
        Some(session) => session,
        None => &NoFitVerifier,
    }
}

/// FV9: each helper failure of the run, for the log: what the fit
/// recorded and why, escaped, on stderr; never in the document.
pub(super) fn report(session: Option<&OcctSession<'_>>) {
    let Some(session) = session else {
        return;
    };
    for report in session.take_reports() {
        note(&format!(
            "fitment: fit helper: {}{}",
            printable(&report.why),
            report
                .status
                .as_deref()
                .map_or_else(String::new, |s| format!(" ({})", printable(s)))
        ));
    }
}
