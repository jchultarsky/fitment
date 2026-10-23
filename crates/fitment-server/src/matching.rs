//! The judging path (docs/research/m6-api.md, SN1, SV4): the only module
//! that builds a `Matcher` or a `MatchDocument`, and the one that builds
//! the fit verifier the server judges with (SV8, OP9, SN11). It reads a
//! socket document as `fitment match` reads a file (UP6: MF11's versions,
//! then the strict reader, then MF2), builds the policy as the CLI's flags
//! do (DC7), and judges a match job's candidates as `fitment match` judges
//! its files, in windows (JS7), each read once (MF3), with components read
//! and chosen by core (CO5) and the original beside them (VD15, SN9). The
//! document is `MatchDocument::with_original`'s bytes (DC1); the run's exit
//! code is core's (CO2, JB4); its notes are the CLI's lines (JB10).
//!
//! Nothing here parses a document into a JSON value (SV7, DC1).

use std::sync::Arc;

use fitment_core::{
    CandidateResult, Clearance, FitVerifier, Grip, MatchDocument, MatchOptions, MatchPolicy,
    Matcher, NoFitVerifier, Refusal, SelectedComponent, SocketDocument, SocketFormatError,
    SocketRefused, Summary, TolerancePolicy,
};
use serde::{Deserialize, Serialize};

use crate::config::Fit;
use crate::http::error::ErrorCode;

/// The fit verifier, built once at start (SV8): core's trait is `Sync`
/// only, so the server holds it as `Send + Sync` to move it into blocking
/// tasks.
pub type Verifier = Arc<dyn FitVerifier + Send + Sync>;

/// The verifier `fit.verifier` names (OP9): `"none"`, `NoFitVerifier`, in
/// M6. `Config::validate` refuses any other value, so a typo never falls
/// back to `"none"` silently.
pub fn verifier(fit: &Fit) -> Verifier {
    debug_assert_eq!(fit.verifier, "none");
    Arc::new(NoFitVerifier)
}

/// The kernel and version the verifier names, for `fitment.info` (DC11):
/// `None` until M7.
pub fn kernel(verifier: &Verifier) -> Option<(String, String)> {
    verifier.kernel()
}

/// SN11, VT31: a fake verifier, under `test-support` only, that answers
/// every fit `verified`, so a `Match` is seen through the API. No
/// configuration names it, and no release build has it.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy, Default)]
pub struct VerifiesEverything;

#[cfg(feature = "test-support")]
impl FitVerifier for VerifiesEverything {
    fn check(&self, _request: &fitment_core::FitRequest<'_>) -> fitment_core::FitStatus {
        fitment_core::FitStatus::Verified {
            kernel: "test-support".into(),
            version: "1".into(),
        }
    }

    fn kernel(&self) -> Option<(String, String)> {
        Some(("test-support".into(), "1".into()))
    }
}

/// The build's versions a socket document is held to (MF11): its
/// `EXTRACTOR_VERSION` and `SOCKET_RULES_VERSION`.
pub type Build = (u32, u32);

/// Why a socket document is refused before any candidate is read (UP6,
/// JB2, MF2), each with its code (section 7.12) and in this order (ER5).
#[derive(Debug, Clone, PartialEq)]
pub enum GateError {
    /// Its bytes are not UTF-8.
    Utf8,
    /// Written by other versions than the build's (MF11, VD9).
    Stale {
        /// Its extractor's version.
        extractor: u32,
        /// Its socket rules' version.
        rules: u32,
    },
    /// The strict reader refuses it (`SocketDocument::from_json`).
    Unreadable(SocketFormatError),
    /// It states no socket: the assembly's refusals (MF2).
    Refused(Vec<Refusal>),
    /// `require_reviewed`, and requirements are unreviewed (MF2).
    Unreviewed {
        /// How many.
        count: usize,
        /// The first few, as `socket-1/bore:hole-88`.
        first: Vec<String>,
    },
}

impl GateError {
    /// Its code (section 7.12).
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Utf8 | Self::Unreadable(_) => ErrorCode::SocketUnreadable,
            Self::Stale { .. } => ErrorCode::SocketStale,
            Self::Refused(_) => ErrorCode::SocketRefused,
            Self::Unreviewed { .. } => ErrorCode::SocketUnreviewed,
        }
    }
}

/// MF2's refusal as the gate's.
fn gate_error(refused: SocketRefused) -> GateError {
    match refused {
        SocketRefused::Refused(refusals) => GateError::Refused(refusals),
        SocketRefused::Stale {
            extractor,
            socket_rules,
            ..
        } => GateError::Stale {
            extractor,
            rules: socket_rules,
        },
        SocketRefused::Unreviewed { count, first, .. } => GateError::Unreviewed { count, first },
        _ => GateError::Refused(Vec::new()),
    }
}

/// UP6, JB2: `bytes` read as `fitment match` reads a socket file: UTF-8,
/// then MF11's versions (a stale document is refused by them before any
/// strict read), then the strict reader. Blocking: behind HX8's parse
/// slots.
///
/// # Errors
///
/// [`GateError`].
pub fn read_socket(bytes: &[u8], build: Build) -> Result<SocketDocument, GateError> {
    let text = std::str::from_utf8(bytes).map_err(|_| GateError::Utf8)?;
    // CO2: the versions as `fitment match` reads them, by one core helper.
    if let Some((extractor, rules)) = SocketDocument::stated_versions(text) {
        if (extractor, rules) != build {
            return Err(GateError::Stale { extractor, rules });
        }
    }
    SocketDocument::from_json(text).map_err(GateError::Unreadable)
}

/// MF2 on a read document: accepted, current, and with `require_reviewed`
/// reviewed throughout, as `Matcher::new` holds it.
///
/// # Errors
///
/// [`GateError`].
pub fn admit(document: &SocketDocument, require_reviewed: bool) -> Result<(), GateError> {
    let options = MatchOptions::new(false, require_reviewed);
    Matcher::new(document, MatchPolicy::default(), options, &NoFitVerifier)
        .map(|_| ())
        .map_err(gate_error)
}

/// D6's clearance, as the request names it (DC7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ClearanceName {
    /// A clearance hole bounded by the original's too.
    Strict,
    /// Larger than the original passes, flagged.
    Functional,
}

/// DC7: the policy as `cmd/match_.rs` builds it from its flags: D9's
/// tolerances unless a value differs, then a `TolerancePolicy` named `cli`,
/// so the document's `policy.name` and digest are the CLI's; grip hard.
///
/// # Errors
///
/// The CLI's usage refusals as a sentence: a tolerance zero, negative or
/// non-finite, an angle of 90 degrees or more.
pub fn policy(
    tolerance_mm: Option<f64>,
    tolerance_deg: Option<f64>,
    clearance: ClearanceName,
) -> Result<MatchPolicy, String> {
    let default = TolerancePolicy::default();
    let linear = tolerance_mm.unwrap_or(default.linear_mm());
    let angular = tolerance_deg.unwrap_or(default.angular_deg());
    if angular >= 90.0 {
        return Err(format!(
            "an angular tolerance of {angular} degrees compares no axis: give less than 90"
        ));
    }
    let tolerance = if (linear, angular) == (default.linear_mm(), default.angular_deg()) {
        default
    } else {
        TolerancePolicy::new("cli", linear, angular).map_err(|e| e.to_string())?
    };
    let clearance = match clearance {
        ClearanceName::Strict => Clearance::Strict,
        ClearanceName::Functional => Clearance::Functional,
    };
    Ok(MatchPolicy::new(tolerance, clearance, Grip::Hard))
}

/// The policy as a `fitment.match` document states it (MF6), for
/// `fitment.run` (DC10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PolicyFacts {
    /// `default`, or `cli` for other tolerances (DC7).
    pub name: String,
    /// The linear tolerance, in mm.
    pub linear_mm: f64,
    /// The angular one, in degrees.
    pub angular_deg: f64,
    /// `strict` or `functional`.
    pub clearance: String,
    /// `hard` (D7).
    pub grip: String,
}

/// `policy`'s facts.
pub fn policy_facts(policy: &MatchPolicy) -> PolicyFacts {
    let tolerance = policy.tolerance();
    PolicyFacts {
        name: tolerance.name().to_owned(),
        linear_mm: tolerance.linear_mm(),
        angular_deg: tolerance.angular_deg(),
        clearance: match policy.clearance() {
            Clearance::Functional => "functional".into(),
            _ => "strict".into(),
        },
        grip: "hard".into(),
    }
}

/// One line `fitment match` writes on stderr for a run (JB10), in its
/// order: what the original's judgment noted, then each candidate's, then
/// the summary and the warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchNote {
    /// The summary, as the CLI writes it after the socket's name.
    Summary(String),
    /// A socket with no active requirement.
    EmptySocket(String),
    /// A socket's requirements that fail every candidate until removed.
    RejectsEvery(String, Vec<String>),
    /// How many requirements are unreviewed.
    Unreviewed(usize),
    /// `require_fit`, which nothing can meet without a fit verifier.
    RequireFit,
    /// A refused candidate's first reason: its label, code and message.
    Refused(String, String, String),
    /// A pass after a shortlist drop (SL1): a fitment bug.
    ShortlistUnsound(String),
    /// A judgment that panicked: a fitment bug.
    JudgingFailed(String),
    /// The original's assembly cannot be read (CO5).
    OriginalUnreadable,
    /// AL11's bounds stopped the original at this `socket/requirement`.
    OriginalExhausted(String),
    /// The original fails its own socket here (VD15): a fitment bug.
    OriginalFails(String),
    /// A catalog entry whose stored rows failed QR4: not judged.
    CatalogCorrupt(String),
    /// CM8's line: what the catalog run searched, and what it did not.
    CatalogSearched(String),
}

/// A file candidate, once (MF3): its upload's SHA-256 and every label it
/// was named by, in the request's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInput {
    /// `sha256` hex.
    pub sha256: String,
    /// Its labels (RA8).
    pub labels: Vec<String>,
}

/// A component candidate, read and chosen by core (CO5), with its labels
/// (CO3: one entry for one component however often named).
#[derive(Debug, Clone)]
pub struct ComponentInput {
    /// The component.
    pub component: SelectedComponent,
    /// Its labels.
    pub labels: Vec<String>,
}

/// What a file match judges (JS6, JS7).
pub struct Judging<'a> {
    /// The socket, read and gated again at dispatch (JB2).
    pub document: &'a SocketDocument,
    /// DC7.
    pub policy: MatchPolicy,
    /// `require_fit` and `require_reviewed`.
    pub options: MatchOptions,
    /// The server's fit verifier (SV8, SN11).
    pub verifier: &'a (dyn FitVerifier + Send + Sync),
    /// The file candidates, once each.
    pub files: Vec<FileInput>,
    /// The components, borrowed from the job's preparation.
    pub components: &'a [ComponentInput],
    /// The original (VD15, SN9): read, or `Err` when its assembly cannot
    /// be read (CO5); `None` when not asked.
    pub original: Option<Result<&'a SelectedComponent, ()>>,
    /// JS7: how many files are read and judged at once, the job's CPU
    /// permits.
    pub window: usize,
    /// A hook point (OP11), reached before each candidate's judgment,
    /// inside its caught panic: `match.judge`.
    pub hook: &'a (dyn Fn(&str) + Sync),
}

/// A run's result (JB4, JB10, DC10): the document's bytes and facts, its
/// exit code and notes, and what makes it exit 1.
#[derive(Debug, Clone)]
pub struct MatchRun {
    /// `MatchDocument::to_json()`, the CLI's `--format json` bytes (DC1).
    pub json: String,
    /// Its digest (MF7).
    pub digest: String,
    /// Its summary, copied, never recomputed (SN4).
    pub summary: Summary,
    /// Its outcome after `require_fit`: `match`, `interface_match` or
    /// `reject`.
    pub outcome: &'static str,
    /// VD14, through core (CO2): 0, 4 or 5; 1 for a fitment bug.
    pub exit: u8,
    /// The CLI's stderr lines (JB10).
    pub notes: Vec<MatchNote>,
    /// The labels of the candidates that are fitment bugs or whose
    /// judgment panicked.
    pub bugs: Vec<String>,
    /// A pass after a shortlist drop (SL1) among them.
    pub shortlist_unsound: bool,
    /// Why the original fails the run, if it does: unreadable, judged as a
    /// fitment bug, or where it
    /// fails its own socket (VD15).
    pub original: Option<OriginalProblem>,
    /// The original was judged.
    pub original_judged: bool,
    /// The document's distinct candidates.
    pub distinct: usize,
    /// The socket's unreviewed requirements (SN8).
    pub unreviewed: usize,
    /// The fit verifier the document names (MF6).
    pub fit_verifier: Option<String>,
}

/// How a judging ended.
pub enum Judged<E> {
    /// The job's stop flag was set between windows (JS7, JS8): nothing is
    /// kept (SN7).
    Stopped,
    /// An input could not be read: the reader's own end.
    Halted(E),
    /// MF2 refused the socket after all: gated at dispatch already
    /// (JB2), so only a broken invariant reaches it.
    Refused(GateError),
    /// The run's result.
    Done(Box<MatchRun>),
}

/// One candidate's judgment, a panic caught (the CLI's `judge_parallel`).
fn caught<F: FnOnce() -> CandidateResult>(judge: F) -> Option<CandidateResult> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(judge)).ok()
}

/// Each file's bytes judged on up to `window` scoped threads, in order,
/// as the CLI's `judge_parallel` does; a panic caught per file.
fn judge_window(
    matcher: &Matcher<'_>,
    files: &[Vec<u8>],
    window: usize,
    hook: &(dyn Fn(&str) + Sync),
) -> Vec<Option<CandidateResult>> {
    let one = |bytes: &Vec<u8>| {
        caught(|| {
            hook("match.judge");
            matcher.judge_step(bytes)
        })
    };
    let jobs = window.clamp(1, files.len().max(1));
    if jobs == 1 {
        return files.iter().map(one).collect();
    }
    let chunk = files.len().div_ceil(jobs);
    // The catalog's own runs judge on scoped threads within their permits
    // (QR8); a file run's window does the same (JS2, JS7).
    std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|part| {
                (
                    part.len(),
                    scope.spawn(move || part.iter().map(one).collect::<Vec<_>>()),
                )
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|(n, h)| h.join().unwrap_or_else(|_| vec![None; n]))
            .collect()
    })
}

/// The CLI's `reported`: a refused candidate's first reason, a shortlist
/// drop verification passes; true for a fitment bug (CO2).
fn reported(label: &str, result: &CandidateResult, notes: &mut Vec<MatchNote>) -> bool {
    if let Some(first) = result.refusals.first() {
        notes.push(MatchNote::Refused(
            label.to_owned(),
            first.code.name().to_owned(),
            first.message.clone(),
        ));
    }
    if result.shortlist_unsound() {
        notes.push(MatchNote::ShortlistUnsound(label.to_owned()));
    }
    result.is_fitment_bug()
}

/// What a run collects as it judges.
#[derive(Default)]
struct Collected {
    results: Vec<(CandidateResult, Vec<String>)>,
    notes: Vec<MatchNote>,
    bugs: Vec<String>,
    shortlist: bool,
}

impl Collected {
    fn take(&mut self, judged: Option<CandidateResult>, labels: Vec<String>) {
        let label = labels.first().cloned().unwrap_or_default();
        let Some(result) = judged else {
            self.notes.push(MatchNote::JudgingFailed(label.clone()));
            self.bugs.push(label);
            return;
        };
        if reported(&label, &result, &mut self.notes) {
            self.shortlist |= result.shortlist_unsound();
            self.bugs.push(label);
        }
        self.results.push((result, labels));
    }
}

/// The original judged first, as `fitment match --original` does (VD15):
/// its notes, and why it fails the run, if it does.
fn judge_original(
    matcher: &Matcher<'_>,
    original: Option<Result<&SelectedComponent, ()>>,
    notes: &mut Vec<MatchNote>,
) -> (Option<CandidateResult>, Option<OriginalProblem>) {
    match original {
        None => (None, None),
        Some(Err(())) => {
            notes.push(MatchNote::OriginalUnreadable);
            (None, Some(OriginalProblem::Unreadable))
        }
        Some(Ok(component)) => {
            let Some(result) = caught(|| matcher.judge_component(component, component.source()))
            else {
                notes.push(MatchNote::JudgingFailed("original".into()));
                return (None, Some(OriginalProblem::JudgingFailed));
            };
            let problem = original_notes(&result, notes);
            (Some(result), problem)
        }
    }
}

/// The original's judgment noted as `fitment match --original` notes it:
/// as any candidate's (a refusal, a shortlist drop), then, as the CLI's
/// `reported(..) || original_bug(..)` short-circuits, VD15's self-check
/// only when that reported no fitment bug. Where it fails still chooses
/// the error's code (JB3).
fn original_notes(result: &CandidateResult, notes: &mut Vec<MatchNote>) -> Option<OriginalProblem> {
    let bug = reported("original", result, notes);
    let check = result.original_check();
    if !bug {
        notes.extend(
            check
                .exhausted
                .iter()
                .cloned()
                .map(MatchNote::OriginalExhausted),
        );
        if let Some(at) = &check.fails_at {
            notes.push(MatchNote::OriginalFails(at.clone()));
        }
    }
    check
        .fails_at
        .map(OriginalProblem::Fails)
        .or_else(|| bug.then_some(OriginalProblem::Bug))
}

/// Why the original fails a run (VD15, CO5, JB3): the error's code is
/// chosen from this, never from its text.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OriginalProblem {
    /// Its assembly cannot be read (CO5): `run_incomplete`.
    Unreadable,
    /// Judging it panicked: `run_incomplete`.
    JudgingFailed,
    /// Its judgment is a fitment bug (M4-17) and it fails nowhere.
    Bug,
    /// It fails its own socket at this `socket/requirement`:
    /// `original_fails_own_socket`.
    Fails(String),
}

impl OriginalProblem {
    /// The run's words for it, recorded in `fitment.run` and the error's
    /// details.
    pub fn text(&self) -> String {
        match self {
            Self::Unreadable => "the original's assembly cannot be read".into(),
            Self::JudgingFailed => "judging the original failed".into(),
            Self::Bug => "the original's judgment is a fitment bug".into(),
            Self::Fails(at) => format!("the original fails its own socket at {at}"),
        }
    }
}

/// MF10's lines after the candidates: the summary, a warning per socket
/// every candidate fails, the unreviewed count, `require_fit`'s.
fn summarize(matcher: &Matcher<'_>, document: &MatchDocument, notes: &mut Vec<MatchNote>) {
    let s = document.summary();
    notes.push(MatchNote::Summary(format!(
        "{} candidates: {} match, {} interface match, {} reject; fit not checked ({})",
        s.candidates,
        s.matched,
        s.interface_match,
        s.reject,
        fitment_core::NO_FIT_VERIFIER
    )));
    let doc = matcher.document();
    for (socket, failing) in matcher.failing_all() {
        if doc.active(socket).next().is_none() {
            notes.push(MatchNote::EmptySocket(socket.to_owned()));
        } else if !failing.is_empty() {
            notes.push(MatchNote::RejectsEvery(
                socket.to_owned(),
                failing.iter().map(|f| (*f).to_owned()).collect(),
            ));
        }
    }
    let unreviewed: usize = matcher.unreviewed().iter().map(|u| u.1).sum();
    if unreviewed > 0 {
        notes.push(MatchNote::Unreviewed(unreviewed));
    }
    if matcher.options().require_fit() {
        notes.push(MatchNote::RequireFit);
    }
}

/// JS6, JS7, SN1: a file match: the matcher of the gated socket, the
/// original first, the files in windows of `window` (each read once by
/// `read` and dropped before the next window), `between` asked between
/// windows whether to go on, then the components; the document
/// `MatchDocument::with_original` writes. Blocking.
pub fn judge<E>(
    judging: Judging<'_>,
    read: &mut dyn FnMut(&str) -> Result<Vec<u8>, E>,
    between: &mut dyn FnMut(u64, u64) -> bool,
) -> Judged<E> {
    let fit: &dyn FitVerifier = judging.verifier;
    let matcher = match Matcher::new(judging.document, judging.policy, judging.options, fit) {
        Ok(matcher) => matcher,
        Err(refused) => return Judged::Refused(gate_error(refused)),
    };
    let mut collected = Collected::default();
    let (original, original_problem) =
        judge_original(&matcher, judging.original, &mut collected.notes);
    let total = u64::try_from(judging.files.len() + judging.components.len()).unwrap_or(u64::MAX);
    let mut done = 0_u64;
    let window = judging.window.max(1);
    for files in judging.files.chunks(window) {
        if !between(done, total) {
            return Judged::Stopped;
        }
        let mut bytes = Vec::with_capacity(files.len());
        for file in files {
            match read(&file.sha256) {
                Ok(read) => bytes.push(read),
                Err(halt) => return Judged::Halted(halt),
            }
        }
        let judged = judge_window(&matcher, &bytes, window, judging.hook);
        drop(bytes);
        for (result, file) in judged.into_iter().zip(files) {
            collected.take(result, file.labels.clone());
        }
        done += u64::try_from(files.len()).unwrap_or(0);
    }
    for component in judging.components {
        if !between(done, total) {
            return Judged::Stopped;
        }
        let one = &component.component;
        let judged = caught(|| {
            (judging.hook)("match.judge");
            matcher.judge_component(one, one.source())
        });
        collected.take(judged, component.labels.clone());
        done += 1;
    }
    if !between(done, total) {
        return Judged::Stopped;
    }
    Judged::Done(Box::new(finish(
        &matcher,
        collected,
        original.as_ref(),
        original_problem,
    )))
}

/// What a catalog match judges besides its catalog (JS6, SN10): the
/// socket gated again at dispatch, the policy and options, the verifier,
/// and the original (VD15).
pub struct CatalogJudging<'a> {
    /// The socket, read and gated again at dispatch (JB2).
    pub document: &'a SocketDocument,
    /// DC7.
    pub policy: MatchPolicy,
    /// `require_fit` and `require_reviewed`.
    pub options: MatchOptions,
    /// The server's fit verifier (SV8, SN11).
    pub verifier: &'a (dyn FitVerifier + Send + Sync),
    /// The original, as a file match's.
    pub original: Option<Result<&'a SelectedComponent, ()>>,
}

/// How a catalog match's judging ended.
pub enum CatalogJudged {
    /// MF2 refused the socket after all: a broken invariant (JB2).
    Refused(GateError),
    /// QR1 refused the run, JS12 bounded it, or its flag stopped it:
    /// nothing judged is kept (SN7, section 7.13).
    NotRun(crate::catalog::Searched),
    /// The run's result and what the catalog judged (DC10).
    Done(Box<MatchRun>, Box<crate::catalog::Matched>),
}

/// Where a catalog match searches (JS6, CX1, JS12, QR8): the server's
/// catalog, the selection, the exhaustive cap, the threads its permits
/// give, and the job's observer (CA2).
pub struct CatalogSearch<'a> {
    /// The catalog (RA1).
    pub dir: &'a crate::catalog::CatalogDir,
    /// What it judges.
    pub selection: &'a crate::catalog::Selection,
    /// JS12's `limits.max_catalog_candidates`.
    pub cap: u64,
    /// QR8: the job's CPU permits, which change no byte.
    pub jobs: usize,
    /// The job's stop flag and progress.
    pub watch: &'a crate::catalog::Watch<'a>,
}

/// JS6, SN1, SN10: a catalog match, as `fitment match S --catalog DIR`
/// runs it: the matcher of the gated socket, the original first (an M4
/// usage error before the catalog is read), then the catalog's run with
/// this matcher in one snapshot `catalog.rs` opens (SV4: the matcher stays
/// here, the catalog there); its bugs, damaged rows and candidates noted
/// in the CLI's order, CM8's line (`searched`), then the document
/// `MatchDocument::with_original` writes. Blocking.
///
/// # Errors
///
/// The catalog's, mapped (CX7).
pub fn judge_catalog(
    judging: CatalogJudging<'_>,
    search: &CatalogSearch<'_>,
    searched: &dyn Fn(&crate::catalog::Matched) -> String,
) -> Result<CatalogJudged, crate::http::error::ApiError> {
    let fit: &dyn FitVerifier = judging.verifier;
    let matcher = match Matcher::new(judging.document, judging.policy, judging.options, fit) {
        Ok(matcher) => matcher,
        Err(refused) => return Ok(CatalogJudged::Refused(gate_error(refused))),
    };
    let mut collected = Collected::default();
    let (original, original_problem) =
        judge_original(&matcher, judging.original, &mut collected.notes);
    let ran = crate::catalog::search(
        search.dir,
        search.selection,
        search.cap,
        |snapshot, chosen| {
            snapshot.match_catalog_observed(&matcher, chosen, search.jobs, search.watch)
        },
    )?;
    let mut found = match ran {
        crate::catalog::Searched::Ran(found) => found,
        other => return Ok(CatalogJudged::NotRun(other)),
    };
    for id in &found.bugs {
        collected.notes.push(MatchNote::JudgingFailed(id.clone()));
        collected.bugs.push(id.clone());
    }
    for id in &found.corrupt {
        collected.notes.push(MatchNote::CatalogCorrupt(id.clone()));
    }
    for (result, labels) in std::mem::take(&mut found.results) {
        let label = labels.first().cloned().unwrap_or_default();
        if reported(&label, &result, &mut collected.notes) {
            collected.shortlist |= result.shortlist_unsound();
            collected.bugs.push(label);
        }
        collected.results.push((result, labels));
    }
    collected
        .notes
        .push(MatchNote::CatalogSearched(searched(&found)));
    let mut run = finish(&matcher, collected, original.as_ref(), original_problem);
    if !found.corrupt.is_empty() {
        // QR4: a damaged row not judged fails the run, its document kept.
        run.exit = 1;
    }
    Ok(CatalogJudged::Done(Box::new(run), found))
}

/// The document and the run's facts (DC10), its exit code by core (CO2).
fn finish(
    matcher: &Matcher<'_>,
    collected: Collected,
    original: Option<&CandidateResult>,
    original_problem: Option<OriginalProblem>,
) -> MatchRun {
    let Collected {
        results,
        mut notes,
        bugs,
        shortlist,
    } = collected;
    let document = MatchDocument::with_original(matcher, results, original);
    summarize(matcher, &document, &mut notes);
    let failed = !bugs.is_empty() || original_problem.is_some();
    let outcome = document.outcome();
    let summary = document.summary();
    MatchRun {
        json: document.to_json(),
        digest: document.digest().to_owned(),
        summary,
        outcome: match outcome {
            fitment_core::Outcome::Match => "match",
            fitment_core::Outcome::InterfaceMatch => "interface_match",
            _ => "reject",
        },
        exit: if failed { 1 } else { outcome.exit_code() },
        notes,
        bugs,
        shortlist_unsound: shortlist,
        original: original_problem,
        original_judged: original.is_some(),
        distinct: summary.candidates,
        unreviewed: matcher.unreviewed().iter().map(|u| u.1).sum(),
        fit_verifier: matcher.fit_kernel().map(|(k, v)| format!("{k} {v}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JB10, JB3 (review of M6 PR 6): an original failing its own socket
    /// is noted where it fails; one whose judgment is also a fitment bug
    /// is noted as the bug only, as the CLI's short-circuit notes it, and
    /// still fails the run `original_fails_own_socket`.
    #[test]
    fn a_buggy_original_is_noted_as_the_cli_notes_it() {
        const S01: &[u8] = include_bytes!("../../fitment-core/tests/assemblies/s01_bolted.stp");
        let document = SocketDocument::extract(S01, Some("BRACKET"), None).unwrap();
        let matcher = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            &NoFitVerifier,
        )
        .unwrap();
        let original = SelectedComponent::target(S01, &document).unwrap();
        let mut result = matcher.judge_component(&original, original.source());
        assert_eq!(result.original_check().fails_at, None, "it passes as read");
        let failed = fitment_core::Failure::new(
            fitment_core::FailureCode::DiameterMin,
            fitment_core::At::default(),
            "planted",
        );
        let id = result.sockets[0].checks[0].0.as_str().to_owned();
        let passed = result.sockets[0].checks[0].1.clone();
        assert!(passed.passed());
        result.sockets[0].checks[0].1 = fitment_core::Check::of(fitment_core::Evidence::new(
            Vec::new(),
            Vec::new(),
            vec![failed],
        ));
        let at = format!("{}/{id}", result.sockets[0].socket);
        let mut notes = Vec::new();
        let problem = original_notes(&result, &mut notes);
        assert_eq!(problem, Some(OriginalProblem::Fails(at.clone())));
        assert_eq!(notes, vec![MatchNote::OriginalFails(at.clone())]);
        // A drop of a socket every check passes: SL1 broken, a fitment
        // bug (M4-17), and where the original fails too.
        result.sockets[0].checks[0].1 = passed;
        result.sockets[0].stage = fitment_core::Stage::Shortlist;
        assert!(result.is_fitment_bug());
        let at = format!("{} (the shortlist dropped it)", result.sockets[0].socket);
        let mut notes = Vec::new();
        let problem = original_notes(&result, &mut notes);
        assert_eq!(
            problem,
            Some(OriginalProblem::Fails(at)),
            "the code's choice"
        );
        assert!(
            notes.iter().all(|n| !matches!(
                n,
                MatchNote::OriginalFails(_) | MatchNote::OriginalExhausted(_)
            )),
            "{notes:?}"
        );
        assert_eq!(notes, vec![MatchNote::ShortlistUnsound("original".into())]);
    }

    /// DC7: D9's tolerances give `default`; another value `cli`, as the
    /// CLI's flags do; a zero, negative or 90-degree tolerance is refused.
    #[test]
    fn the_policy_is_the_clis() {
        let strict = policy(None, None, ClearanceName::Strict).unwrap();
        assert_eq!(policy_facts(&strict).name, "default");
        let same = policy(Some(0.01), Some(0.01), ClearanceName::Strict).unwrap();
        assert_eq!(policy_facts(&same).name, "default");
        let wide = policy(Some(0.05), None, ClearanceName::Functional).unwrap();
        let facts = policy_facts(&wide);
        assert_eq!(
            (facts.name.as_str(), facts.clearance.as_str()),
            ("cli", "functional")
        );
        for (mm, deg) in [(Some(0.0), None), (Some(-1.0), None), (None, Some(90.0))] {
            assert!(
                policy(mm, deg, ClearanceName::Strict).is_err(),
                "{mm:?} {deg:?}"
            );
        }
    }

    /// UP6, MF11: a stale document is refused by its versions before the
    /// strict reader; bytes that are not UTF-8 are refused first.
    #[test]
    fn the_gate_reads_versions_first() {
        let build = (
            fitment_core::EXTRACTOR_VERSION,
            fitment_core::SOCKET_RULES_VERSION,
        );
        assert_eq!(
            read_socket(&[0xff, 0xfe], build).unwrap_err(),
            GateError::Utf8
        );
        let stale = format!(
            r#"{{"format": "fitment.socket", "extractor_version": {}, "socket_rules_version": {}}}"#,
            build.0,
            build.1 - 1
        );
        assert!(matches!(
            read_socket(stale.as_bytes(), build),
            Err(GateError::Stale { .. })
        ));
        let current = stale.replace(&format!("{}}}", build.1 - 1), &format!("{}}}", build.1));
        assert!(matches!(
            read_socket(current.as_bytes(), build),
            Err(GateError::Unreadable(_))
        ));
    }
}
