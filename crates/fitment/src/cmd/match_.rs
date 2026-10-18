//! `fitment match SOCKET CANDIDATE...`: each candidate judged against a
//! socket document, as a `fitment.match` document or a table
//! (`docs/research/m4-match.md`, sections 9.4, 11 and 13).

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::ValueEnum;
use fitment_core::{
    CandidateResult, Clearance, Grip, MatchDocument, MatchOptions, MatchPolicy, Matcher,
    NoFitVerifier, SelectedComponent, SocketDocument, SocketRefused, TolerancePolicy, sha256_hex,
};
use serde_json::Value;

use super::text::{mm, note, printable};
use super::write_atomically;
use crate::exit;

#[derive(clap::Args)]
// Each bool is a command-line flag.
#[allow(clippy::struct_excessive_bools)]
pub struct Args {
    /// The socket document (`fitment socket … -o FILE`).
    socket: PathBuf,
    /// The candidates: STEP files, or directories whose `.stp`, `.step`
    /// and `.p21` files (any case) are each one, not recursively.
    #[arg(required_unless_present = "catalog")]
    candidates: Vec<PathBuf>,
    /// Match against the catalog in DIR instead (`fitment catalog`): its
    /// index finds the candidates, read from it alone.
    #[arg(
        short = 'C',
        long = "catalog",
        value_name = "DIR",
        conflicts_with = "candidates"
    )]
    catalog: Option<PathBuf>,
    /// Judge these catalog entries only: ID, or its current ID@rN.
    #[arg(long = "id", value_name = "ID", requires = "catalog")]
    ids: Vec<String>,
    /// Judge every live catalog entry, bypassing the index: slower, and
    /// lists every entry; for when a miss is suspected.
    #[arg(long, requires = "catalog", conflicts_with = "ids")]
    exhaustive: bool,
    /// Judge at most N candidates at once (default: the machine's
    /// parallelism); the result does not depend on it.
    #[arg(long, value_name = "N")]
    jobs: Option<std::num::NonZeroUsize>,
    /// List at most N rejects in the table, all in the document: `all`, or
    /// a number (default: 10 in catalog runs, all in file runs).
    #[arg(long, value_name = "N|all")]
    rejects: Option<String>,
    /// Also judge the original: the target read from this assembly, which
    /// must be the socket's own source; it must fail exactly what no
    /// candidate can pass.
    #[arg(long, value_name = "ASSEMBLY")]
    original: Option<PathBuf>,
    /// `table` is for people and not a stable format; `json` is the
    /// `fitment.match` document.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,
    /// Write the document here (atomically); stdout gets the table unless
    /// --format json.
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,
    /// Print every candidate's every check in the table.
    #[arg(long)]
    explain: bool,
    /// Exit 5 unless some candidate's body fit is verified (no build of
    /// M4 verifies one).
    #[arg(long)]
    require_fit: bool,
    /// Refuse a socket document with requirements no one reviewed.
    #[arg(long)]
    require_reviewed: bool,
    /// D6: `strict` bounds a clearance hole by the original's too.
    #[arg(long, value_enum, default_value_t = ClearanceArg::Strict)]
    clearance: ClearanceArg,
    /// D9's linear tolerance, in mm.
    #[arg(long, value_name = "MM")]
    tolerance_mm: Option<f64>,
    /// D9's angular tolerance, in degrees.
    #[arg(long, value_name = "DEG")]
    tolerance_deg: Option<f64>,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Table,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
enum ClearanceArg {
    Strict,
    Functional,
}

/// The exit codes of VD14.
const ERROR: u8 = 1;
const USAGE: u8 = 2;

pub fn run(args: &Args) -> ExitCode {
    // Usage first (section 7.12): the arguments alone decide these.
    let rejects = match rejects(args) {
        Ok(rejects) => rejects,
        Err(code) => return code,
    };
    let Some(policy) = policy(args) else {
        return ExitCode::from(USAGE);
    };
    let socket_path = printable(&args.socket.display().to_string());
    let text = match super::socket_text(&args.socket, &socket_path) {
        Ok(text) => text,
        Err(code) => return code,
    };
    let document = match gate(&socket_path, &text) {
        Ok(document) => document,
        Err(code) => return code,
    };
    let options = MatchOptions::new(args.require_fit, args.require_reviewed);
    let matcher = match Matcher::new(&document, policy, options, &NoFitVerifier) {
        Ok(matcher) => matcher,
        Err(refused) => {
            refusal(&socket_path, &refused);
            return ExitCode::from(exit::REFUSED);
        }
    };
    let jobs = jobs(args);
    let mut failed = false;
    let listed = candidates(&args.candidates, &mut failed);
    if listed.is_empty() && !failed && args.catalog.is_none() {
        note("error: no candidate: the directories hold no STEP file");
        return ExitCode::from(USAGE);
    }
    if let Some(output) = &args.output {
        if let Some(problem) = clobbers(output, args, &listed) {
            note(&format!(
                "error: {}: left unchanged: {problem}",
                printable(&output.display().to_string())
            ));
            return ExitCode::from(USAGE);
        }
    }
    // VD15: judged beside the candidates, never one of them.
    let mut original = None;
    if let Some(path) = &args.original {
        match judge_original(&matcher, path) {
            Ok(Some((result, bug))) => {
                failed |= bug;
                original = Some(result);
            }
            Ok(None) => failed = true,
            Err(code) => return code,
        }
    }
    // QR1: the catalog's refusals come before any candidate is judged; an
    // M4 usage error of --original, before the catalog is read.
    let (mut results, searched) = match from_catalog(args, &matcher, jobs, &mut failed) {
        Ok(found) => found,
        Err(code) => return code,
    };
    if args.catalog.is_none() {
        results = judge_all(&matcher, listed, &mut failed, jobs);
    }
    let document_out = MatchDocument::with_original(&matcher, results, original.as_ref());
    if let Some(searched) = &searched {
        note(&format!("fitment: {socket_path}: {searched}"));
    }
    summarize(&socket_path, &matcher, &document_out, args.require_fit);
    let json = document_out.to_json();
    if let Some(output) = &args.output {
        if let Err(error) = write_atomically(output, json.as_bytes()) {
            note(&format!(
                "error: cannot write {}: {error}",
                printable(&output.display().to_string())
            ));
            return ExitCode::from(ERROR);
        }
    }
    // VD14, through core's mapping (CO2), as the server's jobs take it.
    let code = if failed {
        ExitCode::from(ERROR)
    } else {
        ExitCode::from(document_out.outcome().exit_code())
    };
    let stdout = match (args.format, &args.output) {
        (Format::Json, Some(_)) => return code,
        (Format::Json, None) => json,
        (Format::Table, _) => table(&json, args.explain, searched.as_deref(), rejects),
    };
    match io::stdout().lock().write_all(stdout.as_bytes()) {
        // A closed stdout ends output quietly and keeps the verdict's code.
        Ok(()) => code,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => code,
        Err(error) => {
            note(&format!("error: cannot write the result: {error}"));
            ExitCode::from(ERROR)
        }
    }
}

/// No more threads than this, whatever `--jobs` asks (as the catalog's).
const MAX_JOBS: usize = 256;

/// One listed file in a window: its read error, or new bytes to judge
/// with the labels of their duplicates; a duplicate leaves no entry.
enum Read {
    Failed(String),
    New(Vec<u8>, Vec<String>),
}

/// Where a SHA-256's entry is: an earlier window's result, or this
/// window's file.
#[derive(Clone, Copy)]
enum Place {
    Result(usize),
    Window(usize),
}

/// Every listed candidate judged, each file read once (MF3), in windows of
/// `jobs` new files (M5-30): a window's files are judged on up to `jobs`
/// threads and dropped before the next is read, so `--jobs 1` keeps M4's
/// memory. Notes go out in argument order: a read error, or the judged
/// file's (QR8). `failed` set by an unreadable file, a fitment bug, or a
/// judgment that panicked.
fn judge_all(
    matcher: &Matcher<'_>,
    listed: Vec<(PathBuf, String)>,
    failed: &mut bool,
    jobs: usize,
) -> Vec<(CandidateResult, Vec<String>)> {
    let jobs = jobs.clamp(1, MAX_JOBS);
    let mut results: Vec<(CandidateResult, Vec<String>)> = Vec::new();
    let mut seen: std::collections::BTreeMap<String, Place> = std::collections::BTreeMap::new();
    let mut listed = listed.into_iter().peekable();
    while listed.peek().is_some() {
        let mut window: Vec<(String, Read)> = Vec::new();
        let mut shas: Vec<String> = Vec::new();
        while shas.len() < jobs {
            let Some((path, label)) = listed.next() else {
                break;
            };
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    let message = format!("error: cannot read {}: {error}", printable(&label));
                    window.push((label, Read::Failed(message)));
                    continue;
                }
            };
            let sha = sha256_hex(&bytes);
            match seen.get(&sha).copied() {
                Some(Place::Result(at)) => {
                    if let Some((_, labels)) = results.get_mut(at) {
                        labels.push(label);
                    }
                }
                Some(Place::Window(at)) => {
                    if let Some((_, Read::New(_, labels))) = window.get_mut(at) {
                        labels.push(label);
                    }
                }
                None => {
                    seen.insert(sha.clone(), Place::Window(window.len()));
                    shas.push(sha);
                    window.push((label, Read::New(bytes, Vec::new())));
                }
            }
        }
        let new: Vec<&[u8]> = window
            .iter()
            .filter_map(|(_, r)| match r {
                Read::New(bytes, _) => Some(bytes.as_slice()),
                Read::Failed(_) => None,
            })
            .collect();
        let judged = judge_parallel(matcher, &new, jobs);
        let mut judged = judged.into_iter().zip(shas);
        for (label, read) in window {
            match read {
                Read::Failed(message) => {
                    note(&message);
                    *failed = true;
                }
                Read::New(_, more) => {
                    let Some((result, sha)) = judged.next() else {
                        continue;
                    };
                    let shown = printable(&label);
                    if let Some(result) = result {
                        *failed |= reported(&shown, &result);
                        seen.insert(sha, Place::Result(results.len()));
                        let mut labels = vec![label];
                        labels.extend(more);
                        results.push((result, labels));
                    } else {
                        seen.remove(&sha);
                        note(&format!(
                            "fitment: {shown}: judging it failed: this is a fitment bug"
                        ));
                        *failed = true;
                    }
                }
            }
        }
    }
    results
}

/// Each file's bytes judged, on up to `jobs` threads, in order; a panic
/// caught per file, the same on one thread or many, as a catalog run's is.
fn judge_parallel(
    matcher: &Matcher<'_>,
    files: &[&[u8]],
    jobs: usize,
) -> Vec<Option<CandidateResult>> {
    let one = |bytes: &&[u8]| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| matcher.judge_step(bytes))).ok()
    };
    let jobs = jobs.clamp(1, files.len().max(1));
    if jobs == 1 {
        return files.iter().map(one).collect();
    }
    let chunk = files.len().div_ceil(jobs);
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

/// Labelled results, as `MatchDocument::new` takes them.
type Labelled = Vec<(CandidateResult, Vec<String>)>;

/// `--jobs`, or the machine's parallelism (QR8, M5-30).
fn jobs(args: &Args) -> usize {
    args.jobs.map_or_else(
        || std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
        std::num::NonZeroUsize::get,
    )
}

/// CM1's combinations; then `--rejects`: `all`, or a count, the default
/// by the run's kind (CM10).
fn rejects(args: &Args) -> Result<Option<usize>, ExitCode> {
    // CL8: a build without the catalog has none to match against.
    if cfg!(not(feature = "catalog")) && args.catalog.is_some() {
        note("error: this build has no catalog (cargo feature `catalog`)");
        return Err(ExitCode::from(USAGE));
    }
    // CM1: --id and --exhaustive need --catalog, and exclude each other.
    if args.catalog.is_none() && (!args.ids.is_empty() || args.exhaustive) {
        note("error: --id and --exhaustive name catalog entries: give -C/--catalog DIR");
        return Err(ExitCode::from(USAGE));
    }
    if args.exhaustive && !args.ids.is_empty() {
        note("error: --exhaustive judges every entry; --id names some: give one");
        return Err(ExitCode::from(USAGE));
    }
    match args.rejects.as_deref() {
        None if args.catalog.is_some() => Ok(Some(10)),
        None | Some("all") => Ok(None),
        Some(n) => n.parse::<usize>().map(Some).map_err(|_| {
            note(&format!(
                "error: --rejects takes a number or `all`, not {}",
                printable(n)
            ));
            ExitCode::from(USAGE)
        }),
    }
}

/// The candidates a catalog run judged, with CM8's line; none for a file
/// run. `Err` is QR1's refusal, nothing on stdout.
fn from_catalog(
    args: &Args,
    matcher: &Matcher<'_>,
    jobs: usize,
    failed: &mut bool,
) -> Result<(Labelled, Option<String>), ExitCode> {
    let Some(dir) = &args.catalog else {
        return Ok((Vec::new(), None));
    };
    let found = super::match_catalog::candidates(dir, matcher, &args.ids, args.exhaustive, jobs)?;
    *failed |= found.failed;
    for (result, labels) in &found.results {
        let label = labels.first().map_or("", String::as_str);
        *failed |= reported(&printable(label), result);
    }
    Ok((found.results, Some(found.searched)))
}

/// D9 from the flags: the default unless a value differs; a zero,
/// negative or non-finite value is a usage error (MF8).
fn policy(args: &Args) -> Option<MatchPolicy> {
    let default = TolerancePolicy::default();
    let linear = args.tolerance_mm.unwrap_or(default.linear_mm());
    let angular = args.tolerance_deg.unwrap_or(default.angular_deg());
    // At 90 degrees or more every axis is parallel to every other.
    if angular >= 90.0 {
        note(&format!(
            "error: an angular tolerance of {angular} degrees compares no axis: give less than 90"
        ));
        return None;
    }
    let tolerance = if (linear, angular) == (default.linear_mm(), default.angular_deg()) {
        default
    } else {
        match TolerancePolicy::new("cli", linear, angular) {
            Ok(t) => t,
            Err(error) => {
                note(&format!("error: {error}"));
                return None;
            }
        }
    };
    let clearance = match args.clearance {
        ClearanceArg::Strict => Clearance::Strict,
        ClearanceArg::Functional => Clearance::Functional,
    };
    Some(MatchPolicy::new(tolerance, clearance, Grip::Hard))
}

/// MF2 before any candidate: a stale document is reported by its versions
/// before any strict read (MF11, VD9); then it must read back.
fn gate(path: &str, text: &str) -> Result<SocketDocument, ExitCode> {
    // CO2: the versions as the server reads them, by one core helper.
    if let Some((extractor, rules)) = SocketDocument::stated_versions(text) {
        if (extractor, rules)
            != (
                fitment_core::EXTRACTOR_VERSION,
                fitment_core::SOCKET_RULES_VERSION,
            )
        {
            refusal(path, &SocketRefused::stale(extractor, rules));
            return Err(ExitCode::from(exit::REFUSED));
        }
    }
    SocketDocument::from_json(text).map_err(|error| {
        note(&format!(
            "fitment: {path}: refused: {}",
            printable(&error.to_string())
        ));
        ExitCode::from(exit::REFUSED)
    })
}

fn refusal(path: &str, refused: &SocketRefused) {
    let line = match refused {
        SocketRefused::Stale {
            extractor,
            socket_rules,
            ..
        } => format!(
            "extracted by extractor {extractor} and socket rules {socket_rules}; this build is {} and {}: \
             extract it again (`fitment socket … -o FILE` carries decisions over)",
            fitment_core::EXTRACTOR_VERSION,
            fitment_core::SOCKET_RULES_VERSION
        ),
        SocketRefused::Unreviewed { count, first, .. } => format!(
            "{count} requirements are unreviewed, under --require-reviewed: {}",
            printable(&first.join(", "))
        ),
        SocketRefused::Refused(refusals) => format!(
            "the socket document holds no socket: {}",
            refusals
                .first()
                .map_or_else(String::new, |r| printable(&r.message))
        ),
        other => printable(&other.to_string()),
    };
    note(&format!("fitment: {path}: refused: {line}"));
}

/// Why OUT must not be written: it is the socket file, the original or a
/// candidate, as given or found in a directory.
fn clobbers(output: &Path, args: &Args, listed: &[(PathBuf, String)]) -> Option<&'static str> {
    let same = |p: &Path| match (output.canonicalize(), p.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if same(&args.socket) {
        return Some("it is the socket document");
    }
    if let Some(dir) = &args.catalog {
        // ST10: never inside the catalog's directory.
        let parent = match output.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        if let (Ok(dir), Ok(parent)) = (dir.canonicalize(), parent.canonicalize()) {
            let target = output
                .file_name()
                .map_or(parent.clone(), |n| parent.join(n));
            if target.starts_with(dir) {
                return Some("it is inside the catalog");
            }
        }
    }
    if args.candidates.iter().any(|c| same(c))
        || listed.iter().any(|(p, _)| same(p))
        || args.original.as_deref().is_some_and(same)
    {
        return Some("it is an input");
    }
    None
}

/// The candidate files and their labels (MF1, MF4): a file as given; a
/// directory's STEP files, sorted, labelled `dir/name`.
fn candidates(arguments: &[PathBuf], failed: &mut bool) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for argument in arguments {
        if !argument.is_dir() {
            out.push((argument.clone(), argument.display().to_string()));
            continue;
        }
        let entries = match std::fs::read_dir(argument) {
            Ok(entries) => entries,
            Err(error) => {
                note(&format!(
                    "error: cannot read {}: {error}",
                    printable(&argument.display().to_string())
                ));
                *failed = true;
                continue;
            }
        };
        let mut files: Vec<PathBuf> = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => files.push(entry.path()),
                Err(error) => {
                    note(&format!(
                        "error: cannot list {}: {error}",
                        printable(&argument.display().to_string())
                    ));
                    *failed = true;
                }
            }
        }
        // Anything named as STEP but a directory is a candidate: a link
        // that leads nowhere fails its read rather than vanishing.
        let mut files: Vec<PathBuf> = files
            .into_iter()
            .filter(|p| !p.is_dir())
            .filter(|p| {
                p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    ["stp", "step", "p21"].contains(&e.to_ascii_lowercase().as_str())
                })
            })
            .collect();
        files.sort();
        let shown = argument.display().to_string();
        let dir = shown.trim_end_matches(['/', std::path::MAIN_SEPARATOR]);
        out.extend(files.into_iter().map(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
            let label = format!("{dir}/{}", name.unwrap_or_default());
            (p, label)
        }));
    }
    out
}

/// stderr for one candidate: a refusal's first reason; true if the run
/// must exit 1 (an internal refusal, or a pass after a shortlist drop).
fn reported(label: &str, result: &CandidateResult) -> bool {
    if let Some(first) = result.refusals.first() {
        note(&format!(
            "fitment: {label}: refused: {}: {}",
            first.code.name(),
            printable(&first.message)
        ));
    }
    // SL1: the shortlist never drops what verification passes (CO2).
    if result.shortlist_unsound() {
        note(&format!(
            "fitment: {label}: the shortlist dropped a candidate verification passes: this is a fitment bug"
        ));
    }
    result.is_fitment_bug()
}

/// VD15: the target read from its own assembly, judged as a candidate; it
/// must fail exactly what no candidate can pass. With true when it fails
/// more (exit 1, the evidence kept in the document); `Ok(None)` when it
/// cannot be read (exit 1); `Err` on a usage error (another file than the
/// socket's).
fn judge_original(
    matcher: &Matcher<'_>,
    path: &Path,
) -> Result<Option<(CandidateResult, bool)>, ExitCode> {
    let shown = printable(&path.display().to_string());
    let bytes = std::fs::read(path).map_err(|error| {
        note(&format!("error: cannot read {shown}: {error}"));
        ExitCode::from(ERROR)
    })?;
    let document = matcher.document();
    let sha = sha256_hex(&bytes);
    if sha != document.source().sha256 {
        note(&format!(
            "error: {shown} is not the assembly the socket was read from"
        ));
        return Err(ExitCode::from(USAGE));
    }
    // CO5: the target read and chosen by core, as the server reads it.
    let Ok(original) = SelectedComponent::target(&bytes, document) else {
        note(&format!("fitment: {shown}: the original cannot be read"));
        return Ok(None);
    };
    let result = matcher.judge_component(&original, original.source());
    let bug = reported("original", &result) || original_bug(&result);
    Ok(Some((result, bug)))
}

/// VD15's test of the original's result (CO2): true, with a note, if it
/// fails a requirement VD5 and VD6 do not explain, or the shortlist drops
/// it where every requirement verification ran on passes or fails as
/// expected (SL1). AL11's bounds are noted and excuse their socket.
fn original_bug(result: &CandidateResult) -> bool {
    let check = result.original_check();
    for at in &check.exhausted {
        // AL11's bounds are no bug: the run names them (VR21's s55).
        note(&format!(
            "fitment: the original exhausts the search at {}: not checked",
            printable(at)
        ));
    }
    let Some(at) = check.fails_at else {
        return false;
    };
    note(&format!(
        "fitment: the original fails its own socket at {}: this is a fitment bug",
        printable(&at)
    ));
    true
}

/// MF10's stderr: the summary; a warning per socket every candidate fails
/// for its own requirements; the unreviewed count; under --require-fit,
/// that nothing can pass.
fn summarize(path: &str, matcher: &Matcher<'_>, document: &MatchDocument, require_fit: bool) {
    let s = document.summary();
    note(&format!(
        "fitment: {path}: {} candidates: {} match, {} interface match, {} reject; fit not checked ({})",
        s.candidates,
        s.matched,
        s.interface_match,
        s.reject,
        fitment_core::NO_FIT_VERIFIER
    ));
    let doc = matcher.document();
    for (socket, failing) in matcher.failing_all() {
        if doc.active(socket).next().is_none() {
            note(&format!(
                "fitment: warning: {}: no requirement is active, so every candidate is rejected",
                printable(socket)
            ));
        } else if !failing.is_empty() {
            note(&format!(
                "fitment: warning: {}: {} fail every candidate until removed in review (`fitment review --remove ID --note …`)",
                printable(socket),
                printable(&failing.join(", "))
            ));
        }
    }
    let unreviewed: usize = matcher.unreviewed().iter().map(|u| u.1).sum();
    if unreviewed > 0 {
        note(&format!(
            "fitment: {unreviewed} requirements are unreviewed"
        ));
    }
    if require_fit {
        note("fitment: --require-fit: no candidate can pass without a fit verifier in this build");
    }
}

/// MF9's table, from the document itself: a header, a line per candidate,
/// and each passing candidate's checks (every candidate's with
/// --explain).
fn table(json: &str, explain: bool, searched: Option<&str>, rejects: Option<usize>) -> String {
    let Ok(doc) = serde_json::from_str::<Value>(json) else {
        return String::new();
    };
    let s = |v: &Value| v.as_str().unwrap_or("").to_owned();
    let policy = &doc["policy"];
    let digest = s(&doc["socket"]["digest"]);
    let target = s(&doc["socket"]["target"]["product"]["id"]);
    let mut out = format!(
        "socket {}  target '{}'  {} socket(s)  policy {} mm / {} deg, clearance {}, grip {}{}  fit: not checked ({})\n",
        digest.get(..19).unwrap_or(&digest),
        printable(&target),
        doc["socket"]["sockets"].as_array().map_or(0, Vec::len),
        mm(policy["linear_mm"].as_f64().unwrap_or(0.0)),
        mm(policy["angular_deg"].as_f64().unwrap_or(0.0)),
        s(&policy["clearance"]),
        s(&policy["grip"]),
        if s(&policy["name"]) == "default" {
            ""
        } else {
            " (not the default)"
        },
        fitment_core::NO_FIT_VERIFIER
    );
    if let Some(searched) = searched {
        // CM8: what the catalog run searched, and what it did not.
        let _ = writeln!(out, "{searched}");
    }
    let mut listed_rejects = 0;
    let mut more = 0;
    for candidate in doc["candidates"].as_array().into_iter().flatten() {
        if s(&candidate["verdict"]) == "reject" {
            if rejects.is_some_and(|n| listed_rejects >= n) {
                more += 1;
                continue;
            }
            listed_rejects += 1;
        }
        out.push_str(&candidate_line(candidate, searched.is_some()));
        let passing = s(&candidate["verdict"]) != "reject";
        if passing || explain {
            for socket in candidate["sockets"].as_array().into_iter().flatten() {
                out.push_str(&checks(socket));
            }
        }
    }
    if more > 0 {
        let _ = writeln!(out, "  … {more} more rejects (in the document)");
    }
    out
}

/// A candidate's line; in a catalog run, with its PRODUCT.id (CM10).
fn candidate_line(candidate: &Value, product: bool) -> String {
    let s = |v: &Value| v.as_str().unwrap_or("").to_owned();
    let verdict = match s(&candidate["verdict"]).as_str() {
        "match" => "MATCH    ",
        "interface_match" => "INTERFACE",
        _ => "reject   ",
    };
    let labels: Vec<String> = candidate["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| printable(&s(l)))
        .collect();
    let sha = s(&candidate["source"]["sha256"]);
    let failed: Vec<String> = candidate["sockets"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|socket| {
            let id = s(&socket["socket"]);
            socket["checks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| c["result"] == "failed")
                .map(move |c| {
                    let mut codes: Vec<String> = c["failures"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|f| s(&f["code"]))
                        .collect();
                    codes.sort();
                    codes.dedup();
                    format!(
                        "{id}/{} {}",
                        printable(&s(&c["requirement"])),
                        codes.join(",")
                    )
                })
        })
        .collect();
    let tail = if s(&candidate["status"]) == "refused" {
        let first = &candidate["refusals"][0];
        format!("refused: {}", printable(&s(&first["code"])))
    } else if failed.is_empty() {
        let flags: Vec<String> = candidate["flags"]
            .as_array()
            .into_iter()
            .flatten()
            .map(s)
            .collect();
        if s(&candidate["verdict"]) == "reject" {
            // Rejected with every check passed: the first reason says why.
            let first = &candidate["reasons"][0];
            let socket = s(&first["socket"]);
            format!(
                "rejected: {}{}",
                printable(&s(&first["reason"])),
                if socket.is_empty() {
                    String::new()
                } else {
                    format!(" at {}", printable(&socket))
                }
            )
        } else {
            format!("all requirements verified; {}", flags.join(", "))
        }
    } else {
        format!(
            "{} failed: {}",
            failed.len(),
            failed.first().cloned().unwrap_or_default()
        )
    };
    let product = if product {
        let id = candidate["product"]["id"].as_str();
        id.map_or_else(|| "  -".to_owned(), |id| format!("  '{}'", printable(id)))
    } else {
        String::new()
    };
    format!(
        "{verdict}  {}  {}{product}  {tail}\n",
        labels.join(", "),
        sha.get(..8).unwrap_or("")
    )
}

/// A socket's checks, one line each: requirement, result, feature, the
/// tightest margin; removed ones say so.
fn checks(socket: &Value) -> String {
    let s = |v: &Value| v.as_str().unwrap_or("").to_owned();
    let mut out = String::new();
    let all = socket["checks"].as_array().cloned().unwrap_or_default();
    let removed = all.iter().filter(|c| c["result"] == "not_checked").count();
    if removed > 0 {
        let _ = writeln!(
            out,
            "    {}: {} of {} checked, {removed} removed in review",
            s(&socket["socket"]),
            all.len() - removed,
            all.len()
        );
    }
    for check in &all {
        let margin = check["measures"]
            .as_array()
            .into_iter()
            .flatten()
            // Lengths only: an angle's margin is in radians.
            .filter(|m| m.get("band_mm").is_some())
            .filter_map(|m| m["margin"].as_f64())
            .reduce(f64::min);
        let _ = writeln!(
            out,
            "    {}/{}  {}  {}{}",
            s(&socket["socket"]),
            printable(&s(&check["requirement"])),
            s(&check["result"]),
            printable(check["feature"].as_str().unwrap_or("-")),
            margin.map_or_else(String::new, |m| format!("  margin {} mm", mm(m)))
        );
    }
    out
}
