//! The catalog's jobs (docs/research/m6-api.md, JS6, JS8, JB3, JB5, CX3,
//! CX5, CX7, section 7.13): ingest, replace, reindex, prune and gc on the
//! job writer, verify on a fresh reader, and a catalog match's search in
//! one snapshot of a pooled reader (QR2, CX2). Each makes the CLI's calls
//! (CU3) with the job's observer (CA2) and answers the document the CLI's
//! `--format json` prints (CU1, CU2), the CLI's exit code with CL2's
//! precedence across files, and for a job that did not succeed the code
//! its CLI refusal maps to (section 7.13).

use std::io;

use fitment_catalog::document::{CatalogDocument, GcDocument, VerifyDocument};
use fitment_catalog::{
    Catalog, CatalogRun, Content, Error, Id, Ingest, Ingested, Input, InputBytes, NotReplaced,
    NotRun, Outcome, Rebuilt, Reindex, Replaced, Scope, Snapshot, Status,
};
use fitment_core::{Blocked, CandidateResult};
use serde_json::{Map, Value, json};

use super::read::{reference, unnamed};
use super::{CatalogDir, GC_FORMAT, VERIFY_FORMAT, Watch};
use crate::http::error::{ApiError, ErrorCode};

/// CL2's codes.
const ERROR: u8 = 1;
const USAGE: u8 = 2;
const REFUSED: u8 = 3;

/// Why a catalog job did not succeed: its code and the error's sentence
/// and values (ER3: from codes and values, never a library's text).
#[derive(Debug, Clone, PartialEq)]
pub struct JobFailure {
    /// The code (section 7.12).
    pub code: ErrorCode,
    /// The sentence.
    pub detail: String,
    /// The values.
    pub details: Map<String, Value>,
}

impl JobFailure {
    fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
            details: Map::new(),
        }
    }

    /// A route's error as a job's (ER7: the same object without the HTTP
    /// framing).
    pub fn of(error: &ApiError) -> Self {
        Self {
            code: error.code(),
            detail: error.detail().to_owned(),
            details: error.details().clone(),
        }
    }
}

/// What a catalog write or maintenance job did (JS6): the CLI's stdout,
/// its exit code, why it did not succeed, its stderr as notes (JB10).
#[derive(Debug, Clone)]
pub struct Done {
    /// The document the CLI prints, if it prints one.
    pub document: Option<String>,
    /// Its format.
    pub format: &'static str,
    /// The CLI's exit code (CL2).
    pub exit: u8,
    /// Its flag stopped it (CA2): what it committed stands, its document
    /// lists exactly that (JB5).
    pub cancelled: bool,
    /// For a job that did not succeed: its code, where one names it.
    pub failure: Option<JobFailure>,
    /// The CLI's stderr lines: a code and a message without paths.
    pub notes: Vec<(&'static str, String)>,
    /// It changed nothing (JB3's outcome `unchanged`): no file added, the
    /// entry's own bytes, no row written, no revision pruned, no file
    /// removed.
    pub unchanged: bool,
}

impl Done {
    fn of(format: &'static str) -> Self {
        Self {
            document: None,
            format,
            exit: 0,
            cancelled: false,
            failure: None,
            notes: Vec::new(),
            unchanged: false,
        }
    }
}

/// A job's input to ingest or replace with (CA1): the verified bytes of an
/// object of the store, read when its batch reaches it, at most the cap
/// (`None` past it); its names as the client gave them (RA8), its id, and
/// the upload's SHA-256, which the catalog's own must equal (UP7).
pub struct IngestFile<'a> {
    /// Reads the bytes, `None` past `cap`.
    pub read: &'a (dyn Fn(u64) -> io::Result<Option<Vec<u8>>> + Sync),
    /// `changes[].input`: the client's label.
    pub input: String,
    /// ID3's default id and PH3's stored name.
    pub file_name: String,
    /// Its id (ID1), for this file alone.
    pub id: Option<String>,
    /// The upload's SHA-256.
    pub sha256: String,
}

struct Source<'a>(&'a (dyn Fn(u64) -> io::Result<Option<Vec<u8>>> + Sync));

impl InputBytes for Source<'_> {
    fn read(&self, cap: u64) -> io::Result<Content> {
        Ok(match (self.0)(cap)? {
            Some(bytes) => Content::Bytes(bytes),
            None => Content::TooLarge,
        })
    }
}

fn id_of(text: &str) -> Result<Id, ApiError> {
    Id::parse(text).map_err(|_| {
        ApiError::new(
            ErrorCode::InvalidReference,
            "an id is ID1's: letters, digits and . _ -",
        )
    })
}

/// ID1: an id as a request names one.
///
/// # Errors
///
/// `invalid_reference`.
pub fn check_id(text: &str) -> Result<(), ApiError> {
    id_of(text).map(drop)
}

/// RA2: a reference as a request names one.
///
/// # Errors
///
/// `invalid_reference`.
pub fn check_reference(text: &str) -> Result<(), ApiError> {
    reference(text).map(drop)
}

/// CL2's code for one ingested file (`fitment catalog ingest`).
fn ingest_code(line: &Ingested) -> u8 {
    let summary = line.summary.as_ref();
    let quarantined = summary.is_some_and(|s| s.status == Status::Refused);
    let internal = summary.is_some_and(|s| s.internal);
    match &line.outcome {
        Outcome::Added | Outcome::Unchanged if internal => ERROR,
        Outcome::Added | Outcome::Unchanged if quarantined => REFUSED,
        Outcome::Added | Outcome::Unchanged | Outcome::Duplicate { .. } => 0,
        Outcome::DuplicateUnderId { .. } | Outcome::Conflict { .. } => USAGE,
        Outcome::Refused { .. } => REFUSED,
        _ => ERROR,
    }
}

/// CL2: across several, the first of 1, 2, 3 that any gave, else 0.
fn worst(codes: impl IntoIterator<Item = u8>) -> u8 {
    let codes: Vec<u8> = codes.into_iter().collect();
    [ERROR, USAGE, REFUSED]
        .into_iter()
        .find(|c| codes.contains(c))
        .unwrap_or(0)
}

/// Why a run stopped: its code (CX3: `catalog_busy` retryable), the
/// error's kind alone (CA4).
fn stopped_failure(dir: &CatalogDir, error: &Error) -> JobFailure {
    match error {
        Error::Busy => JobFailure::new(
            ErrorCode::CatalogBusy,
            "the catalog stayed busy past catalog.write_busy_ms: submit the job again",
        ),
        other => {
            tracing::error!(error = %dir.relative(other), "a catalog job stopped");
            let code = if other.kind_name() == "storage full" {
                ErrorCode::StorageFull
            } else {
                ErrorCode::IoError
            };
            let mut failure = JobFailure::new(code, "the catalog could not be written");
            failure
                .details
                .insert("kind".into(), other.kind_name().into());
            failure
        }
    }
}

/// The notes of an ingest (JB10): `ingest`'s count line, then a line per
/// quarantined file.
fn ingest_notes(lines: &[Ingested]) -> Vec<(&'static str, String)> {
    let mut counts = Vec::new();
    for word in [
        "added",
        "unchanged",
        "quarantined",
        "duplicate",
        "conflict",
        "refused",
        "error",
    ] {
        let n = lines
            .iter()
            .filter(|l| fitment_catalog::document::result(l) == word)
            .count();
        if n > 0 {
            counts.push(format!("{n} {word}"));
        }
    }
    let files = if lines.len() == 1 { "file" } else { "files" };
    let mut notes = vec![(
        "summary",
        format!("{} {files}: {}", lines.len(), counts.join(", ")),
    )];
    for line in lines {
        let quarantined = fitment_catalog::document::result(line) == "quarantined";
        if let (true, Some(reason)) = (
            quarantined,
            line.summary.as_ref().and_then(|s| s.first_refusal.as_ref()),
        ) {
            notes.push((
                "quarantined",
                format!(
                    "{}: quarantined: {}",
                    crate::jobs::notes::printable(&line.input.to_string_lossy()),
                    crate::jobs::notes::printable(reason)
                ),
            ));
        }
    }
    notes
}

/// The failure of an ingest that refused or failed (JB3, section 7.12): a
/// conflict under an id the request gave is `id_conflict`, the bytes of
/// another entry under one `duplicate_content`; an internal refusal a
/// fitment bug; an unreadable file `io_error`. Other refusals are the
/// document's lines (ER7): no code. `given[k]`: the `k`th line's input
/// gave its id (each line tied to its own input by ID4's order, never by
/// its label, which several may share: the review of PR 7b).
fn ingest_failure(run: &Ingest, given: &[bool]) -> Option<JobFailure> {
    let lines = &run.lines;
    if lines
        .iter()
        .any(|l| l.summary.as_ref().is_some_and(|s| s.internal))
    {
        return Some(JobFailure::new(
            ErrorCode::Internal,
            "the gate refused a file as internal: this is a fitment bug",
        ));
    }
    if lines
        .iter()
        .any(|l| matches!(l.outcome, Outcome::Error { .. }))
    {
        return Some(JobFailure::new(
            ErrorCode::IoError,
            "a file could not be read or stored: see its line",
        ));
    }
    if lines
        .iter()
        .zip(given)
        .any(|(l, given)| matches!(l.outcome, Outcome::Conflict { .. }) && *given)
    {
        return Some(JobFailure::new(
            ErrorCode::IdConflict,
            "an id the request gave is another content's: see its line",
        ));
    }
    if lines
        .iter()
        .any(|l| matches!(l.outcome, Outcome::DuplicateUnderId { .. }))
    {
        return Some(JobFailure::new(
            ErrorCode::DuplicateContent,
            "a file's bytes are another entry's, under an id the request gave: see its line",
        ));
    }
    None
}

/// `catalog ingest FILE… --format json` (CA1, CU3): `files` in ID4's order,
/// each at most `max_bytes` (`None`: the catalog's 128 MiB), on the job
/// writer, `watch` asked and told (CA2); the document of the lines its
/// committed batches gave, the CLI's exit code across them (CL2).
///
/// # Errors
///
/// The catalog's, mapped (CX7), when nothing could be read or written.
pub fn ingest(
    dir: &CatalogDir,
    files: &[IngestFile<'_>],
    max_bytes: Option<u64>,
    watch: &Watch<'_>,
) -> Result<Done, ApiError> {
    let sources: Vec<Source<'_>> = files.iter().map(|f| Source(f.read)).collect();
    let mut inputs = Vec::with_capacity(files.len());
    for (file, source) in files.iter().zip(&sources) {
        let input = Input::new(source, &file.input, &file.file_name);
        inputs.push(match &file.id {
            Some(id) => input.with_id(id_of(id)?),
            None => input,
        });
    }
    // The `k`th line is the line of `files[order[k]]` (ID4, RA8).
    let order = fitment_catalog::ingest_order(&inputs);
    let given: Vec<bool> = order.iter().map(|&at| files[at].id.is_some()).collect();
    dir.write(|catalog| {
        let run = catalog
            .ingest_inputs(&inputs, max_bytes, watch)
            .map_err(|e| dir.mapped(&e))?;
        let mut done = Done::of(super::FORMAT);
        done.document = Some(change_document(dir, catalog, |s| {
            CatalogDocument::ingested(s, &run.lines)
        })?);
        done.notes = ingest_notes(&run.lines);
        done.exit = worst(run.lines.iter().map(ingest_code));
        done.unchanged = !run
            .lines
            .iter()
            .any(|l| matches!(l.outcome, Outcome::Added));
        done.failure = ingest_failure(&run, &given);
        // UP7, ST2: the catalog hashed its own buffer of each upload's
        // verified bytes; another SHA-256 is a broken invariant.
        let differs = run.lines.iter().zip(&order).any(|(line, &at)| {
            line.sha256
                .as_ref()
                .is_some_and(|sha| files[at].sha256 != sha.as_str())
        });
        if differs {
            done.exit = ERROR;
            done.failure = Some(JobFailure::new(
                ErrorCode::Internal,
                "an upload's bytes hashed to another SHA-256 in the catalog",
            ));
        }
        match &run.stopped {
            Some(Error::Cancelled) => done.cancelled = true,
            Some(error) => {
                done.exit = ERROR;
                done.failure = Some(stopped_failure(dir, error));
            }
            None => {}
        }
        Ok(done)
    })?
}

/// A change's `fitment.catalog`, read in a snapshot of the writer after it.
fn change_document(
    dir: &CatalogDir,
    catalog: &mut Catalog,
    build: impl FnOnce(&Snapshot<'_>) -> Result<CatalogDocument, Error>,
) -> Result<String, ApiError> {
    let snapshot = catalog.snapshot().map_err(|e| dir.mapped(&e))?;
    build(&snapshot)
        .map(|d| d.to_json())
        .map_err(|e| dir.mapped(&e))
}

/// `catalog replace REF FILE --format json` (CA1, CR3, CR4): one
/// transaction, so not cancellable once running (JB5); the CLI's refusals
/// as codes: a reference naming no entry it may replace (RA2), another
/// entry's bytes (`duplicate_content`), a refused file (`file_refused`,
/// exit 3) or an unreadable one (exit 1); nothing changed then, and no
/// document, as the CLI prints none. `at`: `If-Match`'s generation (RA7),
/// checked in its transaction; another is `precondition_failed`, exit 2.
///
/// # Errors
///
/// The catalog's, mapped (CX7).
pub fn replace(
    dir: &CatalogDir,
    named: &str,
    file: &IngestFile<'_>,
    (max_bytes, at): (Option<u64>, Option<i64>),
    watch: &Watch<'_>,
) -> Result<Done, ApiError> {
    let reference = reference(named)?;
    let source = Source(file.read);
    let input = Input::new(&source, &file.input, &file.file_name);
    dir.write(|catalog| {
        let mut done = Done::of(super::FORMAT);
        let replaced = match catalog.replace_input_at(&reference, &input, max_bytes, at, watch) {
            Ok(Ok(replaced)) => replaced,
            Ok(Err(refused)) => {
                not_replaced(&mut done, &refused);
                return Ok(done);
            }
            Err(Error::Cancelled) => {
                done.cancelled = true;
                return Ok(done);
            }
            Err(error) => {
                done.exit = ERROR;
                done.failure = Some(stopped_failure(dir, &error));
                return Ok(done);
            }
        };
        if let Replaced::Replaced { to, .. } = &replaced {
            if to.1.as_str() != file.sha256 {
                done.exit = ERROR;
                done.failure = Some(JobFailure::new(
                    ErrorCode::Internal,
                    "an upload's bytes hashed to another SHA-256 in the catalog",
                ));
            }
        }
        done.notes.push(("summary", replace_line(&replaced)));
        done.unchanged = matches!(replaced, Replaced::Unchanged { .. });
        done.document = Some(change_document(dir, catalog, |s| {
            CatalogDocument::replaced(s, &file.input, &replaced)
        })?);
        Ok(done)
    })?
}

/// `replace`'s stderr line.
fn replace_line(replaced: &Replaced) -> String {
    match replaced {
        Replaced::Replaced { id, from, to, .. } => {
            format!("{id} replaced: r{} is now r{}", from.0, to.0)
        }
        Replaced::Unchanged { id, revision, .. } => {
            format!("{id} unchanged: the file is r{revision}'s bytes")
        }
        _ => String::new(),
    }
}

/// CR4, section 7.13: a replace that changed nothing, as its job's end.
fn not_replaced(done: &mut Done, refused: &NotReplaced) {
    match refused {
        NotReplaced::Unnamed(why) => {
            done.exit = USAGE;
            done.failure = Some(JobFailure::of(&unnamed(why)));
        }
        NotReplaced::Duplicate { of } => {
            done.exit = USAGE;
            let mut failure = JobFailure::new(
                ErrorCode::DuplicateContent,
                format!("the file's bytes are {of}'s: not replaced"),
            );
            failure.details.insert("of".into(), of.clone().into());
            done.failure = Some(failure);
        }
        NotReplaced::File(line) => {
            let internal = line.summary.as_ref().is_some_and(|s| s.internal);
            let (exit, code, detail) = match &line.outcome {
                Outcome::Refused { .. } if internal => (
                    ERROR,
                    ErrorCode::Internal,
                    "the gate refused the file as internal: this is a fitment bug",
                ),
                Outcome::Refused { .. } => (
                    REFUSED,
                    ErrorCode::FileRefused,
                    "the file was refused: the entry is unchanged",
                ),
                _ => (
                    ERROR,
                    ErrorCode::IoError,
                    "the file could not be read: the entry is unchanged",
                ),
            };
            done.exit = exit;
            let mut failure = JobFailure::new(code, detail);
            let reason = match &line.outcome {
                Outcome::Refused { reason } => reason.trim_end_matches(": not added").to_owned(),
                Outcome::Error { message } => message.clone(),
                _ => String::new(),
            };
            failure.details.insert("reason".into(), reason.into());
            if let Some(first) = line.summary.as_ref().and_then(|s| s.first_refusal.clone()) {
                failure.details.insert("first_refusal".into(), first.into());
            }
            done.failure = Some(failure);
        }
        _ => {
            done.exit = ERROR;
            done.failure = Some(JobFailure::new(
                ErrorCode::Internal,
                "an outcome this build cannot report",
            ));
        }
    }
}

/// CL2's code of a reindex (`fitment catalog reindex`).
fn reindex_code(run: &Reindex) -> u8 {
    let codes = run.lines.iter().map(|line| match &line.rebuilt {
        Rebuilt::Damaged(_) | Rebuilt::Failed(_) => ERROR,
        Rebuilt::TooLarge => REFUSED,
        _ if line.left_search() => REFUSED,
        _ => 0,
    });
    let stopped = run
        .stopped
        .as_ref()
        .filter(|e| !matches!(e, Error::Cancelled))
        .map(|_| ERROR);
    worst(codes.chain(stopped))
}

/// `reindex`'s stderr (JB10): its summary, and a line per part that left
/// the search or stays stale.
fn reindex_notes(run: &Reindex) -> Vec<(&'static str, String)> {
    let written = run
        .lines
        .iter()
        .filter(|l| matches!(l.rebuilt, Rebuilt::Written { .. }));
    let (mut indexed, mut quarantined, mut n) = (0, 0, 0);
    for line in written {
        n += 1;
        match line.after.map(fitment_catalog::EntryStatus::name) {
            Some("quarantined") => quarantined += 1,
            Some("indexed" | "stale") => indexed += 1,
            _ => {}
        }
    }
    let joined = run.lines.iter().filter(|l| l.joined_search()).count();
    let left = run.lines.iter().filter(|l| l.left_search()).count();
    let mut notes = vec![(
        "summary",
        format!(
            "{n} reindexed: {indexed} indexed, {quarantined} quarantined; {joined} newly searched, {left} no longer searched"
        ),
    )];
    for line in run.lines.iter().filter(|l| l.left_search()) {
        notes.push((
            "left_search",
            format!(
                "{}: no longer searched: {}",
                line.id,
                crate::jobs::notes::printable(line.first_refusal.as_deref().unwrap_or_default())
            ),
        ));
    }
    notes
}

/// `catalog reindex [REF…] [--all] --format json` (RX4 to RX9): neither
/// field, every stale entry (RX9); a reference naming no live entry's
/// current revision is refused before anything is written (RA2).
///
/// # Errors
///
/// The catalog's, mapped (CX7).
pub fn reindex(
    dir: &CatalogDir,
    refs: &[String],
    all: bool,
    watch: &Watch<'_>,
) -> Result<Done, ApiError> {
    let scope = if all {
        Scope::All
    } else if refs.is_empty() {
        Scope::Stale
    } else {
        let mut references = Vec::with_capacity(refs.len());
        for text in refs {
            references.push(reference(text)?);
        }
        Scope::Named(references)
    };
    dir.write(|catalog| {
        let mut done = Done::of(super::FORMAT);
        let run = match catalog.reindex_observed(&scope, watch) {
            Ok(Ok(run)) => run,
            Ok(Err(why)) => {
                done.exit = USAGE;
                done.failure = Some(JobFailure::of(&unnamed(&why)));
                return Ok(done);
            }
            Err(error) => return Err(dir.mapped(&error)),
        };
        done.exit = reindex_code(&run);
        done.notes = reindex_notes(&run);
        done.unchanged = !run
            .lines
            .iter()
            .any(|l| matches!(l.rebuilt, Rebuilt::Written { .. }));
        done.document = Some(change_document(dir, catalog, |s| {
            CatalogDocument::reindexed(s, &run)
        })?);
        match &run.stopped {
            Some(Error::Cancelled) => done.cancelled = true,
            Some(error) => done.failure = Some(stopped_failure(dir, error)),
            None if done.exit == ERROR => {
                done.failure = Some(JobFailure::new(
                    ErrorCode::BlobDamaged,
                    "an entry's file is missing or damaged: it stays stale; run a verify job",
                ));
            }
            None => {}
        }
        Ok(done)
    })?
}

/// `catalog prune --revisions --format json` (CR11, CU1): one transaction,
/// not cancellable once running (JB5).
///
/// # Errors
///
/// The catalog's, mapped (CX7).
pub fn prune(dir: &CatalogDir) -> Result<Done, ApiError> {
    dir.write(|catalog| {
        let mut done = Done::of(super::FORMAT);
        let (pruned, freed) = match catalog.prune_revisions() {
            Ok(done) => done,
            Err(error) => {
                let mut failed = Done::of(super::FORMAT);
                failed.exit = ERROR;
                failed.failure = Some(stopped_failure(dir, &error));
                return Ok(failed);
            }
        };
        let total: u64 = freed.contents.iter().map(|(_, b)| *b).sum();
        let n = pruned.len();
        done.unchanged = n == 0;
        done.notes.push((
            "summary",
            format!(
                "{n} {} pruned, {total} bytes released",
                if n == 1 { "revision" } else { "revisions" }
            ),
        ));
        done.document = Some(change_document(dir, catalog, |s| {
            CatalogDocument::pruned(s, &pruned)
        })?);
        Ok(done)
    })?
}

/// `catalog gc [--versions] [--vacuum] --format json` (CU2): exit 1 for a
/// file it could not remove; a cancel keeps what it unlinked, the document
/// counting exactly that (JB5). Its collection runs on the job writer; the
/// log's `TRUNCATE` on a reader, whose wait for readers is
/// `catalog.read_busy_ms`, so it never holds the writer that long (CX6,
/// the review of PR 7b); `VACUUM` is the caller's, under the barrier
/// ([`vacuum`]).
///
/// # Errors
///
/// The catalog's, mapped (CX7).
pub fn gc(
    dir: &CatalogDir,
    versions: bool,
    vacuum: bool,
    watch: &Watch<'_>,
) -> Result<Done, ApiError> {
    let done = dir.write(|catalog| -> Result<Done, ApiError> {
        let mut done = Done::of(GC_FORMAT);
        let collected = match catalog.gc_collect_observed(versions, watch) {
            Ok(collected) => collected,
            Err(error) => {
                done.exit = ERROR;
                done.failure = Some(stopped_failure(dir, &error));
                return Ok(done);
            }
        };
        let old = if versions {
            format!(", {} rows of older versions removed", collected.old_rows)
        } else {
            String::new()
        };
        done.notes.push((
            "summary",
            format!(
                "{} orphan files removed, {} temporary files removed{old}",
                collected.unlinked.len(),
                collected.temporaries.len()
            ),
        ));
        if !collected.failed.is_empty() {
            done.exit = ERROR;
            let mut failure = JobFailure::new(ErrorCode::IoError, "gc could not remove some files");
            failure
                .details
                .insert("failed".into(), collected.failed.clone().into());
            done.failure = Some(failure);
        }
        done.cancelled = collected.cancelled;
        done.unchanged = collected.unlinked.is_empty()
            && collected.temporaries.is_empty()
            && collected.old_rows == 0;
        done.document = Some(GcDocument::new(&collected, versions, vacuum).to_json());
        Ok(done)
    })??;
    if !done.cancelled && done.document.is_some() {
        truncate(dir);
    }
    Ok(done)
}

/// CC3, CX6: the log truncated on a reader within `catalog.read_busy_ms`;
/// readers that still pin it leave it, as the barrier would (logged).
fn truncate(dir: &CatalogDir) {
    match dir.checkpoint_truncate() {
        Ok(false) => {}
        Ok(true) => tracing::info!("the log was not truncated: readers pin it"),
        Err(error) => tracing::warn!(code = error.code().name(), "the log was not truncated"),
    }
}

/// SZ4: `gc --vacuum`'s `VACUUM`, on the job writer, then the log
/// truncated; the caller holds the WAL barrier (CX6).
///
/// # Errors
///
/// `catalog_busy` past `catalog.write_busy_ms`; the catalog's, mapped
/// (CX7), a full disk `storage_full`.
pub fn vacuum(dir: &CatalogDir) -> Result<(), JobFailure> {
    let vacuumed = dir
        .write(|catalog| catalog.vacuum())
        .map_err(|e| JobFailure::of(&e))?;
    vacuumed.map_err(|error| stopped_failure(dir, &error))?;
    truncate(dir);
    Ok(())
}

/// `catalog verify [--deep] [--full] --format json` (CU2, M5-25) on a fresh
/// reader (JS6): exit 3 for a problem found, its document listing it; a
/// cancel leaves nothing (JB5).
///
/// # Errors
///
/// The catalog's, mapped (CX7).
pub fn verify(
    dir: &CatalogDir,
    deep: bool,
    full: bool,
    watch: &Watch<'_>,
) -> Result<Done, ApiError> {
    dir.inspect().map_err(ApiError::from)?;
    let mut catalog = dir.open().map_err(ApiError::from)?;
    let mut done = Done::of(VERIFY_FORMAT);
    let verified = match catalog.verify_observed(deep, full, watch) {
        Ok(verified) => verified,
        Err(Error::Cancelled) => {
            done.cancelled = true;
            return Ok(done);
        }
        Err(error) => return Err(dir.mapped(&error)),
    };
    let n = verified.problems.len();
    done.notes.push((
        "summary",
        if n == 0 {
            "consistent".to_owned()
        } else {
            format!("{n} {}", if n == 1 { "problem" } else { "problems" })
        },
    ));
    done.exit = if n == 0 { 0 } else { REFUSED };
    done.document = Some(VerifyDocument::new(&verified, deep, full).to_json());
    Ok(done)
}

/// What a catalog match judges (CM2, CM3): level 1's survivors, the
/// entries named (their references as given), or every live entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// `full`.
    Full,
    /// `exhaustive` (JS12).
    Exhaustive,
    /// `{"ids": […]}`.
    Named(Vec<String>),
}

impl Selection {
    /// Its name, as `fitment.run` states it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Exhaustive => "exhaustive",
            Self::Named(_) => "named",
        }
    }
}

/// The catalog's own selection of `selection`.
///
/// # Errors
///
/// `invalid_reference` for a malformed reference (RA2).
pub fn selection(selection: &Selection) -> Result<fitment_catalog::Selection, ApiError> {
    Ok(match selection {
        Selection::Full => fitment_catalog::Selection::Full,
        Selection::Exhaustive => fitment_catalog::Selection::Exhaustive,
        Selection::Named(texts) => {
            let mut references = Vec::with_capacity(texts.len());
            for text in texts {
                references.push(reference(text)?);
            }
            fitment_catalog::Selection::Named(references)
        }
    })
}

/// What a catalog run judged (QR2, CM8, DC10): its snapshot's generation
/// and counts, and each candidate judged with its label `catalog:<id>@r<n>`.
#[derive(Debug, Clone)]
pub struct Matched {
    /// The snapshot's generation.
    pub generation: i64,
    /// Live entries, indexed (searched) and quarantined.
    pub entries: usize,
    #[allow(missing_docs)]
    pub indexed: usize,
    #[allow(missing_docs)]
    pub quarantined: usize,
    /// Level 1's survivors, in a full run.
    pub level_one: Option<usize>,
    /// The distinct entries selected, before QR4.
    pub selected: usize,
    /// The judgments made, one per group of equal parts (QR6).
    pub judged: usize,
    /// The candidates judged, however many results the matcher's module
    /// has taken from `results` since.
    pub candidates: usize,
    /// The candidates judged, labelled.
    pub results: Vec<(CandidateResult, Vec<String>)>,
    /// Entries whose stored rows failed QR4.
    pub corrupt: Vec<String>,
    /// Entries whose judgment panicked.
    pub bugs: Vec<String>,
}

/// How a catalog run's search ended.
#[derive(Debug, Clone)]
pub enum Searched {
    /// It judged.
    Ran(Box<Matched>),
    /// QR1 refused it (section 7.13): stale, empty or blocked (exit 3), or
    /// a named entry that is not a live entry's current revision (exit 2).
    Refused(u8, JobFailure),
    /// JS12: an exhaustive run over more live entries than the cap.
    TooLarge {
        /// Live entries in its snapshot.
        live: u64,
    },
    /// Its flag stopped it (CA2): nothing is kept (SN7).
    Cancelled,
}

/// QR1's refusals as codes (CX7).
fn not_run(refused: &NotRun) -> Option<(u8, JobFailure)> {
    Some(match refused {
        NotRun::Stale {
            count,
            extractor,
            signature,
        } => {
            let mut failure = JobFailure::new(
                ErrorCode::CatalogStale,
                format!(
                    "{count} parts were read by another fitment: reindex the catalog (POST /v1/catalog/reindex)"
                ),
            );
            failure.details.insert("count".into(), (*count).into());
            failure
                .details
                .insert("extractor_version".into(), json!(extractor));
            failure
                .details
                .insert("signature_version".into(), json!(signature));
            (REFUSED, failure)
        }
        NotRun::Empty { entries } => {
            let mut failure = JobFailure::new(
                ErrorCode::CatalogEmpty,
                if *entries == 0 {
                    "the catalog holds no part".to_owned()
                } else {
                    format!(
                        "the catalog holds no searchable part: {entries} parts, all quarantined"
                    )
                },
            );
            failure.details.insert("entries".into(), (*entries).into());
            (REFUSED, failure)
        }
        NotRun::Blocked { socket, why } => {
            let requirements: Vec<String> = match why {
                Blocked::FailsEvery { requirements, .. } => {
                    requirements.iter().map(|r| r.as_str().to_owned()).collect()
                }
                _ => Vec::new(),
            };
            let mut failure = JobFailure::new(
                ErrorCode::SocketBlocked,
                format!(
                    "{socket}: a requirement fails every candidate: remove it in review, or no catalog part can be found"
                ),
            );
            failure
                .details
                .insert("socket".into(), socket.clone().into());
            failure
                .details
                .insert("requirements".into(), requirements.into());
            (REFUSED, failure)
        }
        NotRun::Unnamed(list) => (USAGE, JobFailure::of(&unnamed(list.first()?))),
        _ => return None,
    })
}

type Run = Result<Result<CatalogRun, NotRun>, Error>;

/// QR2, CX2, JS12, CC3: a catalog match's search in one snapshot of a
/// pooled reader: an exhaustive run over more live entries than `cap`
/// refused before anything is judged; `run` the judging, which the
/// matcher's module calls (SV4: `matching.rs` alone holds a matcher); the
/// snapshot dropped, then the log checkpointed (CC3).
///
/// # Errors
///
/// The catalog's, mapped (CX7).
pub fn search(
    dir: &CatalogDir,
    chosen: &Selection,
    cap: u64,
    run: impl FnOnce(&Snapshot<'_>, &fitment_catalog::Selection) -> Run,
) -> Result<Searched, ApiError> {
    let selection = selection(chosen)?;
    let ran = dir.read(|catalog| {
        let ran = {
            let snapshot = catalog.snapshot()?;
            if *chosen == Selection::Exhaustive {
                let live = snapshot.status_counts()?.entries;
                if live > cap {
                    return Ok(Searched::TooLarge { live });
                }
            }
            run(&snapshot, &selection)?
        };
        catalog.checkpoint();
        Ok(match ran {
            Ok(run) => Searched::Ran(Box::new(Matched {
                generation: run.generation,
                entries: run.entries,
                indexed: run.indexed,
                quarantined: run.quarantined,
                level_one: run.level_one,
                selected: run.selected,
                judged: run.judged,
                candidates: run.results.len(),
                results: run.results,
                corrupt: run.corrupt,
                bugs: run.bugs,
            })),
            Err(NotRun::Cancelled) => Searched::Cancelled,
            Err(refused) => match not_run(&refused) {
                Some((exit, failure)) => Searched::Refused(exit, failure),
                None => Searched::Refused(
                    ERROR,
                    JobFailure::new(ErrorCode::Internal, "a refusal this build cannot report"),
                ),
            },
        })
    })?;
    Ok(ran)
}

/// What a write job changed, for its `job_interrupted` details (JB6): the
/// catalog's generation as the writer reads it now.
///
/// # Errors
///
/// The catalog's, mapped (CX7).
pub fn generation(dir: &CatalogDir) -> Result<i64, ApiError> {
    dir.write(|catalog| catalog.generation().map_err(|e| dir.mapped(&e)))?
}
