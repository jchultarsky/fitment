//! `fitment match SOCKET CANDIDATE...`: each candidate judged against a
//! socket document, as a `fitment.match` document or a table
//! (`docs/research/m4-match.md`, sections 9.4, 11 and 13).

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::ValueEnum;
use fitment_core::{
    CandidateResult, CandidateSource, Clearance, ComponentRead, FailureCode, Grip, MatchDocument,
    MatchOptions, MatchPolicy, Matcher, NoFitVerifier, Outcome, RefusalCode, SocketDocument,
    SocketRefused, TolerancePolicy, read_assembly, sha256_hex,
};
use serde_json::Value;

use super::socket::write_atomically;
use super::text::{mm, note, printable};
use crate::exit;

#[derive(clap::Args)]
pub struct Args {
    /// The socket document (`fitment socket … -o FILE`).
    socket: PathBuf,
    /// The candidates: STEP files, or directories whose `.stp`, `.step`
    /// and `.p21` files (any case) are each one, not recursively.
    #[arg(required = true)]
    candidates: Vec<PathBuf>,
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
const INTERFACE_MATCH: u8 = 4;
const ALL_REJECTED: u8 = 5;

pub fn run(args: &Args) -> ExitCode {
    let Some(policy) = policy(args) else {
        return ExitCode::from(USAGE);
    };
    let socket_path = printable(&args.socket.display().to_string());
    let text = match std::fs::read_to_string(&args.socket) {
        Ok(text) => text,
        Err(error) => {
            note(&format!("error: cannot read {socket_path}: {error}"));
            return ExitCode::from(ERROR);
        }
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
    let mut failed = false;
    let listed = candidates(&args.candidates, &mut failed);
    if listed.is_empty() && !failed {
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
            Ok(Some(result)) => original = Some(result),
            Ok(None) => failed = true,
            Err(code) => return code,
        }
    }
    let results = judge_all(&matcher, listed, &mut failed);
    let document_out = MatchDocument::with_original(&matcher, results, original.as_ref());
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
    let code = if failed {
        ExitCode::from(ERROR)
    } else {
        match document_out.outcome() {
            Outcome::Match => ExitCode::SUCCESS,
            Outcome::InterfaceMatch => ExitCode::from(INTERFACE_MATCH),
            _ => ExitCode::from(ALL_REJECTED),
        }
    };
    let stdout = match (args.format, &args.output) {
        (Format::Json, Some(_)) => return code,
        (Format::Json, None) => json,
        (Format::Table, _) => table(&json, args.explain),
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

/// Every listed candidate judged, each file read once: the same bytes are
/// one entry with every label (MF3); `failed` set by an unreadable file.
fn judge_all(
    matcher: &Matcher<'_>,
    listed: Vec<(PathBuf, String)>,
    failed: &mut bool,
) -> Vec<(CandidateResult, Vec<String>)> {
    let mut results: Vec<(CandidateResult, Vec<String>)> = Vec::new();
    // MF3: the same bytes judged once, with every label.
    let mut seen: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for (path, label) in listed {
        let shown = printable(&label);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                note(&format!("error: cannot read {shown}: {error}"));
                *failed = true;
                continue;
            }
        };
        let sha = sha256_hex(&bytes);
        if let Some(at) = seen.get(&sha).copied() {
            if let Some((_, labels)) = results.get_mut(at) {
                labels.push(label);
            }
            continue;
        }
        let result = matcher.judge_step(&bytes);
        *failed |= reported(&shown, &result);
        seen.insert(sha, results.len());
        results.push((result, vec![label]));
    }
    results
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
    let value: Option<Value> = serde_json::from_str(text).ok();
    let version = |key: &str| {
        value
            .as_ref()
            .and_then(|v| v[key].as_u64())
            .and_then(|n| u32::try_from(n).ok())
    };
    if let (Some(extractor), Some(rules)) = (
        version("extractor_version"),
        version("socket_rules_version"),
    ) {
        if (extractor, rules)
            != (
                fitment_core::EXTRACTOR_VERSION,
                fitment_core::SOCKET_RULES_VERSION,
            )
        {
            refusal(
                path,
                &SocketRefused::Stale {
                    extractor,
                    socket_rules: rules,
                },
            );
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
        } => format!(
            "extracted by extractor {extractor} and socket rules {socket_rules}; this build is {} and {}: \
             extract it again (`fitment socket … -o FILE` carries decisions over)",
            fitment_core::EXTRACTOR_VERSION,
            fitment_core::SOCKET_RULES_VERSION
        ),
        SocketRefused::Unreviewed { count, first } => format!(
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
    let internal = result
        .refusals
        .iter()
        .any(|r| r.code == RefusalCode::Internal);
    // SL1: the shortlist never drops what verification passes.
    let unsound = result.sockets.iter().any(|s| {
        s.stage == fitment_core::Stage::Shortlist && s.checks.iter().all(|(_, c)| c.passed())
    });
    if unsound {
        note(&format!(
            "fitment: {label}: the shortlist dropped a candidate verification passes: this is a fitment bug"
        ));
    }
    internal || unsound
}

/// VD15: the target read from its own assembly, judged as a candidate; it
/// must fail exactly what no candidate can pass. `Ok(None)` when it fails
/// more (exit 1); `Err` on a usage error (another file than the socket's).
fn judge_original(matcher: &Matcher<'_>, path: &Path) -> Result<Option<CandidateResult>, ExitCode> {
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
    let Some(instance) = document.extracted().map(|e| e.target.definition.instance) else {
        return Ok(None);
    };
    let read = read_assembly(&bytes);
    let part = read.listing().and_then(|listing| {
        listing.definitions().iter().find_map(|d| match &d.read {
            Some(ComponentRead::Accepted(part)) if d.definition.instance == instance => {
                Some((**part).clone())
            }
            _ => None,
        })
    });
    let Some(part) = part else {
        note(&format!("fitment: {shown}: the original cannot be read"));
        return Ok(None);
    };
    let result = matcher.judge_part(
        &part,
        CandidateSource::Component {
            assembly_sha256: sha,
            definition: instance,
        },
    );
    if reported("original", &result) {
        return Ok(None);
    }
    for socket in &result.sockets {
        for (id, check) in &socket.checks {
            if check.passed() {
                continue;
            }
            let codes: Vec<FailureCode> =
                check.evidence().failures().iter().map(|f| f.code).collect();
            let at = format!("{}/{}", socket.socket, printable(id.as_str()));
            // AL11's bounds are no bug: the run names them (VR21's s55).
            if !codes.is_empty()
                && codes.iter().all(|c| {
                    matches!(
                        c,
                        FailureCode::AlignmentExhausted | FailureCode::CoverageExhausted
                    )
                })
            {
                note(&format!(
                    "fitment: the original exhausts the search at {at}: not checked"
                ));
                continue;
            }
            let expected = |c: &FailureCode| {
                matches!(
                    c,
                    FailureCode::Unverifiable | FailureCode::FloorAboveTolerance
                )
            };
            // A failure with no code is not one VD5 or VD6 explains.
            if codes.is_empty() || !codes.iter().all(expected) {
                note(&format!(
                    "fitment: the original fails its own socket at {at}: this is a fitment bug"
                ));
                return Ok(None);
            }
        }
    }
    Ok(Some(result))
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
fn table(json: &str, explain: bool) -> String {
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
    for candidate in doc["candidates"].as_array().into_iter().flatten() {
        out.push_str(&candidate_line(candidate));
        let passing = s(&candidate["verdict"]) != "reject";
        if passing || explain {
            for socket in candidate["sockets"].as_array().into_iter().flatten() {
                out.push_str(&checks(socket));
            }
        }
    }
    out
}

fn candidate_line(candidate: &Value) -> String {
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
    format!(
        "{verdict}  {}  {}  {tail}\n",
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
