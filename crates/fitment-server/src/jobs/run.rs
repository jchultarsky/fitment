//! One function per kind (docs/research/m6-api.md, JS6, JS8, JS13, UP7,
//! OB5, RA3): each runs on the blocking pool with the job's frozen
//! request, its stop flag and its permits, makes the CLI's calls, stores
//! its document's bytes and ends the job in one transaction with what the
//! result makes durable. `socket_extract`'s carrying and revisions are in
//! [`super::socket`].
//!
//! The hook points (OP11) of a run: `job.read` (its inputs read),
//! `job.ran` (its core call returned), `job.stored` (its document written
//! and synced, its state not updated), `job.committing` (just before the
//! transaction that ends it) and `job.committed` (just after). The
//! dispatcher has `job.submitted` and `job.dispatched`.

use std::cell::Cell;

use fitment_core::{FeaturesDocument, PartsDocument, RefusalCode, Status};
use serde_json::{Map, Value};

use super::Control;
use super::documents::{self, Progress};
use super::model::{Kind, State, Stop};
use super::request::{AssemblyJob, FeaturesJob};
use crate::Shared;
use crate::http::error::ErrorCode;
use crate::state::{
    AssemblyRows, DbError, End, EventRow, Finished, JobRow, ObjectError, Rows, StoreError, Synced,
};

/// How a job's run ended, for the dispatcher.
#[derive(Debug)]
pub(crate) enum Ran {
    /// It ended: its row and the events its end wrote.
    Ended(JobRow, Vec<EventRow>),
    /// A drain stopped it: queued again, its attempt unchanged, with its
    /// `state` event (SD1).
    Requeued(JobRow, Vec<EventRow>),
}

/// An end to write because a step could not go on.
pub(crate) type Halt = Box<End>;

/// A blocking hook point (OP11); nothing without `test-support`.
pub(crate) fn hook(shared: &Shared, point: &str) {
    #[cfg(feature = "test-support")]
    shared.hooks.blocking(point);
    #[cfg(not(feature = "test-support"))]
    let _ = (shared, point);
}

/// Runs job `row` to its end (JS6). `Err` is `server.db`'s failure; the
/// dispatcher then ends the job `failed`, `internal`.
pub(crate) fn execute(shared: &Shared, row: &JobRow, control: &Control) -> Result<Ran, DbError> {
    control.progress(shared, reading(0));
    let finished = match row.kind {
        Kind::Features => features(shared, row, control),
        Kind::AssemblyRead => assembly(shared, row, control),
        Kind::SocketExtract => return super::socket::extract(shared, row, control),
        Kind::SocketImport => return super::socket::import(shared, row, control),
        Kind::Match => return super::matches::run(shared, row, control),
        _ => {
            let ending = internal(row.kind, "this build runs no job of this kind");
            end(shared, row, control, &ending, None, &Rows::None)
        }
    }?;
    conclude(shared, row, control, finished)
}

/// The dispatcher's view of a finishing transaction: a `Retry` or a
/// `Collision` (only a socket line's) ends the job `internal`.
pub(crate) fn conclude(
    shared: &Shared,
    row: &JobRow,
    control: &Control,
    finished: Finished,
) -> Result<Ran, DbError> {
    Ok(match finished {
        Finished::Ended(row, events) if row.state == State::Queued => Ran::Requeued(row, events),
        Finished::Ended(row, events) => Ran::Ended(row, events),
        Finished::Already(row) => Ran::Ended(row, Vec::new()),
        Finished::Retry | Finished::Collision | Finished::Gone => {
            let what = if matches!(finished, Finished::Gone) {
                "the job's assembly was deleted under it"
            } else {
                "a socket line's id names another triple"
            };
            tracing::error!(job = row.id, what, "a job could not commit its result");
            let ending = internal(row.kind, what);
            return conclude(
                shared,
                row,
                control,
                end(shared, row, control, &ending, None, &Rows::None)?,
            );
        }
    })
}

/// A phase's progress (JB8): counts only, never a verdict (M6-51).
pub(crate) fn progress(phase: &'static str, done: u64, total: u64) -> Progress {
    Progress {
        phase,
        done,
        total: Some(total),
    }
}

/// A reading's progress (JB8): counts only.
pub(crate) fn reading(done: u64) -> Progress {
    Progress {
        phase: "reading",
        done,
        total: Some(1),
    }
}

/// `failed`, `internal`: a fitment bug or a broken invariant (JS6).
pub(crate) fn internal(kind: Kind, detail: &str) -> End {
    documents::failed(
        State::Failed,
        Some(1),
        ErrorCode::Internal,
        detail,
        Map::new(),
        kind,
    )
}

fn blob_damaged(kind: Kind, detail: &str) -> End {
    documents::failed(
        State::Failed,
        Some(1),
        ErrorCode::BlobDamaged,
        detail,
        Map::new(),
        kind,
    )
}

/// OB3, UP7: an input object, read checked against its name; above the
/// one-buffer cap, in two checked passes. A missing or damaged one fails
/// the job `blob_damaged` (SD4).
pub(crate) fn read_input(shared: &Shared, kind: Kind, sha256: &str) -> Result<Vec<u8>, Halt> {
    let bytes = shared
        .db
        .object_bytes(sha256)
        .map_err(|_| Box::new(internal(kind, "server.db could not be read")))?
        .ok_or_else(|| Box::new(blob_damaged(kind, "an input object has no row")))?;
    let read = match shared.objects.read(sha256, bytes) {
        Err(ObjectError::TooLargeForBuffer) => read_streamed(shared, sha256, bytes),
        other => other,
    };
    read.map_err(|error| {
        tracing::error!(object = sha256, error = %error, "an input could not be read");
        Box::new(match error {
            ObjectError::Io(_) => documents::failed(
                State::Failed,
                Some(1),
                ErrorCode::IoError,
                "an input could not be read",
                Map::new(),
                kind,
            ),
            _ => blob_damaged(kind, "an input's bytes are not its name's"),
        })
    })
}

fn read_streamed(shared: &Shared, sha256: &str, bytes: u64) -> Result<Vec<u8>, ObjectError> {
    let mut reader = shared.objects.open_checked(sha256, bytes)?;
    let mut out = Vec::with_capacity(usize::try_from(bytes).unwrap_or(0));
    let mut buf = vec![0_u8; 1024 * 1024];
    loop {
        let n = reader.read_chunk(&mut buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::InvalidData {
                ObjectError::Damaged
            } else {
                ObjectError::Io(e)
            }
        })?;
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n]);
    }
}

/// OB5: `json` written to a temporary file through the seam and synced,
/// for the finishing transaction to put in place; a full disk fails the
/// job, never a short document (VT11).
pub(crate) fn stage(shared: &Shared, kind: Kind, json: &str) -> Result<Synced, Halt> {
    let staged = shared.objects.stage().and_then(|mut staged| {
        staged.write(json.as_bytes())?;
        staged.finish()
    });
    staged.map_err(|error| Box::new(storage(kind, &error)))
}

/// `storage_full` or `io_error` for a document that could not be stored.
fn storage(kind: Kind, error: &std::io::Error) -> End {
    let (code, detail) = if error.kind() == std::io::ErrorKind::StorageFull {
        (
            ErrorCode::StorageFull,
            "the server's storage is full: the job's document was not kept",
        )
    } else {
        tracing::error!(kind = ?error.kind(), "a job's document could not be stored");
        (ErrorCode::IoError, "the job's document could not be stored")
    };
    documents::failed(State::Failed, Some(1), code, detail, Map::new(), kind)
}

/// The CLI's exit code for a document of `status` and `refusals` (JB4,
/// M2's and M3's section 12): accepted 0; refused 3, but 1 for an internal
/// refusal, which is a fitment bug.
pub(crate) fn exit_of(status: Status, refusals: &[fitment_core::Refusal]) -> u8 {
    match status {
        Status::Accepted => 0,
        Status::Refused if refusals.iter().any(|r| r.code == RefusalCode::Internal) => 1,
        Status::Refused => 3,
    }
}

/// JB10: the CLI's line per refusal of the document, as notes.
pub(crate) fn refusal_notes(refusals: &[fitment_core::Refusal]) -> Option<String> {
    let mut notes = super::notes::Notes::default();
    notes.refusals(refusals);
    notes.to_json()
}

/// The end of a job whose document is the CLI's answer with `exit`
/// (JB3): `succeeded`, `refused` or `failed`, the document kept in each.
/// A refusal is stated by the document itself (ER4); a fitment bug is
/// `internal`, the refusals in its details.
pub(crate) fn answered(
    kind: Kind,
    exit: u8,
    staged: &Synced,
    refusals: &[fitment_core::Refusal],
) -> End {
    let state = State::of_exit(exit);
    let error = (state == State::Failed).then(|| {
        let list = refusals
            .iter()
            .map(|r| serde_json::json!({"code": r.code.name(), "message": r.message, "instance": r.instance}))
            .collect();
        let mut details = Map::new();
        details.insert("refusals".into(), Value::Array(list));
        (
            ErrorCode::Internal.name().to_owned(),
            documents::error_details("the reading refused the input as a fitment bug", details),
        )
    });
    End {
        exit: Some(exit),
        error,
        document: Some((
            staged.sha256.clone(),
            staged.bytes,
            kind.document_format().to_owned(),
        )),
        ..End::bare(state)
    }
}

/// JB5, JS8, SD1: ends the job with `ending`, unless its flag is set: a
/// drain then queues it again, its attempt unchanged; a cancel or a
/// deadline, which `server.db` recorded before the flag, ends it as that
/// stop says, with nothing kept.
pub(crate) fn end(
    shared: &Shared,
    row: &JobRow,
    control: &Control,
    ending: &End,
    staged: Option<Synced>,
    rows: &Rows<'_>,
) -> Result<Finished, DbError> {
    end_staged(
        shared,
        row,
        control,
        ending,
        staged.into_iter().collect(),
        rows,
    )
}

/// As [`end`], with every object `staged` put in place by the finishing
/// transaction: a match's document and its candidates index (DC12).
pub(crate) fn end_staged(
    shared: &Shared,
    row: &JobRow,
    control: &Control,
    ending: &End,
    staged: Vec<Synced>,
    rows: &Rows<'_>,
) -> Result<Finished, DbError> {
    let now = shared.ids.now_ms();
    let retention = shared.jobs.retention_ms();
    let stopped = documents::stopped(row.kind);
    match control.stop() {
        Some(Stop::Shutdown) => {
            shared
                .db
                .requeue_job(row.key, &stopped, now, retention, &documents::end_data)
        }
        Some(Stop::Cancel | Stop::Deadline) => {
            let mut none = || Ok(());
            shared
                .db
                .finish_job(
                    row.key,
                    &stopped.cancelled,
                    &stopped,
                    &mut none,
                    &Rows::None,
                    now,
                    retention,
                    &documents::end_data,
                )
                .map_err(db_error)
        }
        None => finish(shared, row, ending, staged, rows),
    }
}

fn db_error(error: StoreError) -> DbError {
    match error {
        StoreError::Db(error) => error,
        StoreError::Place(error) => DbError::Create(error.kind()),
    }
}

/// The finishing transaction with its hook points (JS13). A document
/// that cannot be placed ends the job `failed` with the storage's code,
/// nothing of its result kept.
fn finish(
    shared: &Shared,
    row: &JobRow,
    ending: &End,
    staged: Vec<Synced>,
    rows: &Rows<'_>,
) -> Result<Finished, DbError> {
    hook(shared, "job.committing");
    let placed = Cell::new(false);
    let mut place = || {
        for synced in &staged {
            shared.objects.place(synced)?;
        }
        placed.set(!staged.is_empty());
        Ok(())
    };
    let stopped = documents::stopped(row.kind);
    let (now, retention) = (shared.ids.now_ms(), shared.jobs.retention_ms());
    let finished = shared.db.finish_job(
        row.key,
        ending,
        &stopped,
        &mut place,
        rows,
        now,
        retention,
        &documents::end_data,
    );
    match finished {
        Ok(finished) => {
            if placed.get() {
                for synced in staged {
                    synced.kept();
                }
            }
            hook(shared, "job.committed");
            Ok(finished)
        }
        Err(StoreError::Place(error)) => {
            drop(staged);
            let ending = storage(row.kind, &error);
            let mut none = || Ok(());
            shared
                .db
                .finish_job(
                    row.key,
                    &ending,
                    &stopped,
                    &mut none,
                    &Rows::None,
                    now,
                    retention,
                    &documents::end_data,
                )
                .map_err(db_error)
        }
        Err(StoreError::Db(error)) => Err(error),
    }
}

/// The job's recorded request (RA9).
pub(crate) fn request<T: serde::de::DeserializeOwned>(row: &JobRow) -> Result<T, Halt> {
    serde_json::from_str(&row.request).map_err(|_| {
        Box::new(internal(
            row.kind,
            "the job's recorded request cannot be read",
        ))
    })
}

/// `features` (JS6): `FeaturesDocument::read` on the upload, the bytes of
/// `to_json()` stored as the CLI prints them (DC1).
fn features(shared: &Shared, row: &JobRow, control: &Control) -> Result<Finished, DbError> {
    let work = || -> Result<(End, Synced), Halt> {
        let job: FeaturesJob = request(row)?;
        let bytes = read_input(shared, row.kind, job.upload.trim_start_matches("sha256:"))?;
        hook(shared, "job.read");
        let document = FeaturesDocument::read(&bytes);
        drop(bytes);
        hook(shared, "job.ran");
        let exit = exit_of(document.status(), document.refusals());
        let staged = stage(shared, row.kind, &document.to_json())?;
        let mut ending = answered(row.kind, exit, &staged, document.refusals());
        ending.notes = refusal_notes(document.refusals());
        Ok((ending, staged))
    };
    match work() {
        Ok((ending, staged)) => {
            control.progress(shared, reading(1));
            hook(shared, "job.stored");
            end(shared, row, control, &ending, Some(staged), &Rows::None)
        }
        Err(halt) => end(shared, row, control, &halt, None, &Rows::None),
    }
}

/// `assembly_read` (JS6, RA3): `PartsDocument::read`, the `fitment.parts`
/// bytes stored, then the caller's assembly, its name and its parts row of
/// this build committed with the job's end (JS13). A refused assembly is
/// registered too, its document saying why; one the reading calls a
/// fitment bug is not.
fn assembly(shared: &Shared, row: &JobRow, control: &Control) -> Result<Finished, DbError> {
    let work = || -> Result<(End, Synced, AssemblyJob, Option<String>), Halt> {
        let job: AssemblyJob = request(row)?;
        let bytes = read_input(shared, row.kind, job.upload.trim_start_matches("sha256:"))?;
        hook(shared, "job.read");
        let document = PartsDocument::read(&bytes);
        drop(bytes);
        hook(shared, "job.ran");
        let exit = exit_of(document.status(), document.refusals());
        let root = document.root().and_then(|r| serde_json::to_string(r).ok());
        let staged = stage(shared, row.kind, &document.to_json())?;
        let mut ending = answered(row.kind, exit, &staged, document.refusals());
        ending.notes = refusal_notes(document.refusals());
        if exit != 1 {
            ending.result = Some(format!("/v1/assemblies/{}", job.upload));
        }
        Ok((ending, staged, job, root))
    };
    let (ending, staged, job, root) = match work() {
        Ok(done) => done,
        Err(halt) => return end(shared, row, control, &halt, None, &Rows::None),
    };
    control.progress(shared, reading(1));
    hook(shared, "job.stored");
    let document_sha256 = staged.sha256.clone();
    let rows = if ending.exit == Some(1) {
        Rows::None
    } else {
        Rows::Assembly(AssemblyRows {
            owner: row.owner,
            sha256: job.upload.trim_start_matches("sha256:"),
            file_name: job.file_name.as_deref(),
            status: if ending.exit == Some(0) {
                "accepted"
            } else {
                "refused"
            },
            root: root.as_deref(),
            document_sha256: &document_sha256,
        })
    };
    end(shared, row, control, &ending, Some(staged), &rows)
}
