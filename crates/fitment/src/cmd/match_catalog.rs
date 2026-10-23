//! `fitment match SOCKET --catalog DIR` (`docs/research/m5-catalog.md`,
//! section 7.12, QR1, CM1 to CM10): the candidates come from a catalog's
//! index, and everything after them is a file run's.

use std::path::Path;
use std::process::ExitCode;

use fitment_catalog::{Catalog, Error, NoObserver, NotRun, Options, Reference, Selection};
use fitment_core::{Blocked, CandidateResult, Matcher};

use super::text::{note, printable};

/// The candidates of a catalog run, and what its header says (CM8).
pub(super) struct Candidates {
    pub results: Vec<(CandidateResult, Vec<String>)>,
    /// CM8's line: what was searched and what was not.
    pub searched: String,
    /// True if a stored row was damaged (exit 1).
    pub failed: bool,
    /// With a fit verifier, how many stored files its fits read (FU6).
    pub fit_reads: Option<usize>,
}

/// CL2's code for an error opening the catalog.
fn code_of(error: &Error) -> u8 {
    match error {
        Error::Newer { .. } => crate::exit::REFUSED,
        Error::NotACatalog(_) | Error::Linked(_) | Error::NotUnicode(_) => 2,
        _ => 1,
    }
}

/// QR1's refusals, after the socket gate: the catalog, its schema, its
/// staleness, an empty or blocked full run. `Err` is the exit code, with
/// nothing on stdout.
pub(super) fn candidates(
    dir: &Path,
    matcher: &Matcher<'_>,
    ids: &[String],
    exhaustive: bool,
    jobs: usize,
) -> Result<Candidates, ExitCode> {
    let shown = printable(&dir.display().to_string());
    let fail = |error: &Error| {
        note(&format!(
            "fitment: {shown}: {}",
            printable(&error.to_string())
        ));
        ExitCode::from(code_of(error))
    };
    let selection = if exhaustive {
        Selection::Exhaustive
    } else if ids.is_empty() {
        Selection::Full
    } else {
        let mut references = Vec::new();
        for id in ids {
            match Reference::parse(id) {
                Ok(r) => references.push(r),
                Err(error) => {
                    note(&format!("error: {}", printable(&error.to_string())));
                    return Err(ExitCode::from(2));
                }
            }
        }
        Selection::Named(references)
    };
    let mut catalog = Catalog::open(dir, Options::default()).map_err(|e| fail(&e))?;
    // FC6: with a fit verifier, a passing candidate's file is read through
    // this handle, hash-checked, when its fit is asked.
    let blobs = matcher.fit_identity().map(|_| catalog.fit_blobs());
    let run = {
        let snapshot = catalog.snapshot().map_err(|e| fail(&e))?;
        match &blobs {
            Some(blobs) => {
                snapshot.match_catalog_fitted(matcher, &selection, jobs, &NoObserver, blobs)
            }
            None => snapshot.match_catalog(matcher, &selection, jobs),
        }
        .map_err(|e| fail(&e))?
    };
    // CC3: the snapshot is over; the WAL may be folded back.
    catalog.checkpoint();
    let run = match run {
        Ok(run) => run,
        Err(refused) => return Err(not_run(&shown, &refused)),
    };
    let mut failed = false;
    for id in &run.bugs {
        note(&format!(
            "fitment: {shown}: {}: judging it failed: this is a fitment bug",
            printable(id)
        ));
        failed = true;
    }
    for id in &run.corrupt {
        note(&format!(
            "fitment: {shown}: {}: its stored rows are damaged: not judged: run fitment catalog verify -C {shown}",
            printable(id)
        ));
        failed = true;
    }
    let searched = match &selection {
        Selection::Full => {
            // CR8: what is not searched is counted, and where to see it.
            let see = if run.quarantined > 0 {
                format!(" (fitment catalog list -C {shown} --status quarantined)")
            } else {
                String::new()
            };
            format!(
                "catalog {shown}  generation {}  entries {}: {} searched, {} quarantined, not searched{see}  level 1 kept {}",
                run.generation,
                run.entries,
                run.indexed,
                run.quarantined,
                run.level_one.unwrap_or_default()
            )
        }
        Selection::Named(_) => format!(
            "catalog {shown}  generation {}  {} named entries (level 1 not used)",
            run.generation, run.selected
        ),
        _ => format!(
            "catalog {shown}  generation {}  entries {}: exhaustive: level 1 bypassed",
            run.generation, run.entries
        ),
    };
    Ok(Candidates {
        results: run.results,
        searched,
        failed,
        fit_reads: blobs.as_ref().map(fitment_catalog::FitBlobs::reads),
    })
}

/// A refusal of QR1, on stderr, and its code.
fn not_run(shown: &str, refused: &NotRun) -> ExitCode {
    let (line, code) = match refused {
        NotRun::Stale {
            count,
            extractor,
            signature,
            ..
        } => {
            let by = match (extractor, signature) {
                (Some(e), _) => format!(
                    "extractor {e}, this build {}",
                    fitment_core::EXTRACTOR_VERSION
                ),
                (None, Some(s)) => format!(
                    "signature {s}, this build {}",
                    fitment_core::SIGNATURE_VERSION
                ),
                (None, None) => "no current rows".to_owned(),
            };
            (
                format!(
                    "refused: {count} parts were read by another fitment ({by}): run fitment catalog reindex -C {shown}"
                ),
                crate::exit::REFUSED,
            )
        }
        // After the stale gate, an entry not indexed is quarantined.
        NotRun::Empty { entries: 0 } => (
            "refused: the catalog holds no part".to_owned(),
            crate::exit::REFUSED,
        ),
        NotRun::Empty { entries } => (
            format!(
                "refused: the catalog holds no searchable part: {entries} parts, all quarantined (fitment catalog list -C {shown} --status quarantined)"
            ),
            crate::exit::REFUSED,
        ),
        NotRun::Blocked { socket, why, .. } => {
            let why = match why {
                Blocked::FailsEvery { requirements, .. } => {
                    let ids: Vec<&str> = requirements
                        .iter()
                        .map(fitment_core::RequirementId::as_str)
                        .collect();
                    format!("{} fail every candidate", ids.join(", "))
                }
                _ => "it demands nothing a candidate can show".to_owned(),
            };
            (
                format!(
                    "refused: {}: {}: remove it in review, or no catalog part can be found",
                    printable(socket),
                    printable(&why)
                ),
                crate::exit::REFUSED,
            )
        }
        NotRun::Unnamed(unnamed) => {
            let why: Vec<String> = unnamed.iter().map(|u| printable(&u.to_string())).collect();
            (why.join("; "), 2)
        }
        other => (format!("refused: {other:?}"), 1),
    };
    note(&format!("fitment: {shown}: {line}"));
    ExitCode::from(code)
}
