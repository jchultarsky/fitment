//! The catalog's write and maintenance jobs (docs/research/m6-api.md, JS1,
//! JS6, JS8, JB3, JB5, JB6, JB8, CX3, CX6, UP7, SD1): `catalog_ingest`,
//! `catalog_replace`, `catalog_reindex`, `catalog_prune` and `catalog_gc`
//! on the write lane's one writer, `catalog_verify` on the maintenance
//! lane's fresh reader. Each reads its frozen request (RA9), hands the
//! catalog its inputs' verified bytes (UP7) and its observer (CA2), whose
//! stop is the job's flag (JS8) and whose progress becomes the job's
//! `change` and `batch` events (JB8); its document is the CLI's
//! `--format json` (CU1, CU2), its exit code the CLI's (JB4).
//!
//! How a write job stops (JB5, SD1): a cancel keeps what its committed
//! batches changed, its document listing exactly that, `exit_code` null;
//! a drain makes it `interrupted`, `job_interrupted`, keeping the same
//! document and stating the generation it started at (JB6). After each
//! committed batch the log is checkpointed, and the WAL barrier raised
//! when readers pin it past `catalog.wal_barrier_mib` (CX6).
//!
//! Hook points (OP11): `catalog.batch` (after a batch commits, before the
//! next is read) and `job.read`, `job.ran`, `job.stored`,
//! `job.committing`, `job.committed`.

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::Control;
use super::documents;
use super::model::{Kind, Lane, State, Stop};
use super::notes::Notes;
use super::request::{GcJob, IngestJob, PruneJob, ReindexJob, ReplaceJob, VerifyJob};
use super::run::{self, Halt, Ran, hook};
use crate::Shared;
use crate::catalog::{self, Done, Event, IngestFile, Watch};
use crate::http::error::ErrorCode;
use crate::state::{DbError, End, JobRow, ObjectError, Rows, Synced};

/// The bytes of one page of the catalog's database: what CX6's threshold
/// counts the log's pages in (SQLite's default page size).
const PAGE_BYTES: i64 = 4096;

/// An input's reader, as the catalog asks it for a file's bytes (CA1).
type Reader = Box<dyn Fn(u64) -> io::Result<Option<Vec<u8>>> + Sync>;

/// UP7, OB3: an input object's bytes, read checked, `None` past `cap`
/// without reading them; a missing or damaged object is an error of kind
/// `InvalidData`, its line an error line (exit 1).
fn object(shared: &Shared, sha256: &str, cap: u64) -> io::Result<Option<Vec<u8>>> {
    let bytes = shared
        .db
        .object_bytes(sha256)
        .map_err(|_| io::Error::other("server.db could not be read"))?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no object row"))?;
    if bytes > cap {
        return Ok(None);
    }
    let read = match shared.objects.read(sha256, bytes) {
        Err(ObjectError::TooLargeForBuffer) => run::read_streamed(shared, sha256, bytes),
        other => other,
    };
    read.map(Some).map_err(|error| {
        tracing::error!(object = sha256, error = %error, "an input could not be read");
        match error {
            ObjectError::Io(e) => e,
            _ => io::Error::new(
                io::ErrorKind::InvalidData,
                "an input's bytes are not its name's",
            ),
        }
    })
}

/// The job (JS6): its kind's call, then its end.
pub(crate) fn run(shared: &Arc<Shared>, row: &JobRow, control: &Control) -> Result<Ran, DbError> {
    if !row.kind.pure() {
        // JB6: the generation it starts at, for an interrupted job's
        // details.
        if let Ok(generation) = catalog::generation(&shared.catalog) {
            shared.db.set_catalog_facts(row.key, generation, None)?;
        }
    }
    let tell = |event: Event| told(shared, row, control, event);
    let watch = Watch::new(&control.flag, &tell);
    hook(shared, "job.read");
    let done = match row.kind {
        Kind::CatalogIngest => ingest(shared, row, &watch),
        Kind::CatalogReplace => replace(shared, row, &watch),
        Kind::CatalogReindex => run::request::<ReindexJob>(row)
            .map(|job| catalog::reindex(&shared.catalog, &job.refs, job.all, &watch)),
        Kind::CatalogVerify => run::request::<VerifyJob>(row)
            .map(|job| catalog::verify(&shared.catalog, job.deep, job.full, &watch)),
        Kind::CatalogGc => gc(shared, row, &watch),
        Kind::CatalogPrune => {
            run::request::<PruneJob>(row).map(|_| catalog::prune(&shared.catalog))
        }
        _ => Err(Box::new(run::internal(row.kind, "not a catalog job"))),
    };
    hook(shared, "job.ran");
    if !row.kind.pure() {
        pressure(shared);
    }
    let done = match done {
        Ok(Ok(done)) => done,
        Ok(Err(error)) => {
            if error.code() == ErrorCode::CatalogBusy {
                shared.metrics.catalog_busy();
            }
            let ending = documents::failed(
                State::Failed,
                error.exit_code(),
                error.code(),
                error.detail(),
                error.details().clone(),
                row.kind,
            );
            return finish(shared, row, control, &ending, None);
        }
        Err(halt) => return finish(shared, row, control, &halt, None),
    };
    ended(shared, row, control, &done)
}

/// JB8: what the catalog told, as the job's events: each committed file's
/// change and each batch persisted, counts transient; and CX6's check
/// after each batch.
fn told(shared: &Arc<Shared>, row: &JobRow, control: &Control, event: Event) {
    let persist = |kind: &str, data: &str| match shared.db.append_event(
        row.key,
        kind,
        data,
        shared.ids.now_ms(),
    ) {
        Ok(event) => shared.jobs.publish(&row.id, std::slice::from_ref(&event)),
        Err(error) => tracing::warn!(job = row.id, error = %error, "an event was not kept"),
    };
    let count = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
    match event {
        Event::Change(change) => persist("change", &change),
        Event::Batch {
            generation,
            lines,
            done,
            total,
        } => {
            persist(
                "batch",
                &json!({ "generation": generation, "lines": lines }).to_string(),
            );
            control.progress(shared, run::progress("batch", count(done), count(total)));
            hook(shared, "catalog.batch");
            pressure(shared);
        }
        Event::Counts { phase, done, total } => {
            let phase = if phase == "verifying" {
                "reading"
            } else {
                "batch"
            };
            control.progress(shared, run::progress(phase, count(done), count(total)));
        }
    }
}

/// `catalog_ingest` (CA1): each file an object of the store, read when its
/// batch reaches it (ST2), named as the request named it (RA8).
fn ingest(
    shared: &Arc<Shared>,
    row: &JobRow,
    watch: &Watch<'_>,
) -> Result<Result<Done, crate::http::error::ApiError>, Halt> {
    let job: IngestJob = run::request(row)?;
    let readers: Vec<Reader> = job
        .files
        .iter()
        .map(|f| {
            let sha256 = f.upload.trim_start_matches("sha256:").to_owned();
            let shared = Arc::clone(shared);
            let read: Reader = Box::new(move |cap| object(&shared, &sha256, cap));
            read
        })
        .collect();
    let files: Vec<IngestFile<'_>> = job
        .files
        .iter()
        .zip(&readers)
        .map(|(f, read)| IngestFile {
            read: read.as_ref(),
            input: f.input.clone(),
            file_name: f.file_name.clone(),
            id: f.id.clone(),
            sha256: f.upload.trim_start_matches("sha256:").to_owned(),
        })
        .collect();
    let cap = job.max_mib.map(|m| m.saturating_mul(1024 * 1024));
    Ok(catalog::ingest(&shared.catalog, &files, cap, watch))
}

/// `catalog_replace` (CA1, CR3): one transaction, not cancellable once
/// running (JB5).
fn replace(
    shared: &Arc<Shared>,
    row: &JobRow,
    watch: &Watch<'_>,
) -> Result<Result<Done, crate::http::error::ApiError>, Halt> {
    let job: ReplaceJob = run::request(row)?;
    let sha256 = job.upload.trim_start_matches("sha256:").to_owned();
    let reading = Arc::clone(shared);
    let source = sha256.clone();
    let read = move |cap| object(&reading, &source, cap);
    let file = IngestFile {
        read: &read,
        input: job.input.clone(),
        file_name: job.file_name.clone(),
        id: None,
        sha256,
    };
    let cap = job.max_mib.map(|m| m.saturating_mul(1024 * 1024));
    Ok(catalog::replace(
        &shared.catalog,
        &job.entry,
        &file,
        cap,
        watch,
    ))
}

/// `catalog_gc` (CU2): with `vacuum`, the WAL barrier taken first (CX6);
/// one it could not take within `catalog.barrier_max_seconds` fails the
/// job `catalog_busy`, retryable, nothing done.
fn gc(
    shared: &Arc<Shared>,
    row: &JobRow,
    watch: &Watch<'_>,
) -> Result<Result<Done, crate::http::error::ApiError>, Halt> {
    let job: GcJob = run::request(row)?;
    if job.vacuum && !barrier(shared, true) {
        return Ok(Err(crate::http::error::ApiError::new(
            ErrorCode::CatalogBusy,
            "VACUUM could not take the WAL barrier: running queries pin the log; submit it again",
        )
        .with("limit", "barrier_max_seconds")));
    }
    let done = catalog::gc(&shared.catalog, job.versions, job.vacuum, watch);
    if job.vacuum {
        lower(shared, true);
    }
    Ok(done)
}

/// The end of a catalog job that ran (JB3, JB5, SD1): its document kept,
/// its state the CLI's exit code's, or the stop's.
fn ended(
    shared: &Arc<Shared>,
    row: &JobRow,
    control: &Control,
    done: &Done,
) -> Result<Ran, DbError> {
    let staged = match done
        .document
        .as_deref()
        .map(|json| run::stage(shared, row.kind, json))
        .transpose()
    {
        Ok(staged) => staged,
        Err(halt) => return finish(shared, row, control, &halt, None),
    };
    hook(shared, "job.stored");
    let document = staged
        .as_ref()
        .map(|s| (s.sha256.clone(), s.bytes, done.format.to_owned()));
    let mut notes = Notes::default();
    for (code, message) in &done.notes {
        notes.push(code, message.clone(), Map::new());
    }
    let notes = notes.to_json();
    // A write job its flag stopped, its committed changes listed (JB5).
    if done.cancelled && !row.kind.pure() {
        let ending = match control.stop() {
            Some(Stop::Shutdown) => {
                interrupted(shared, row, "the server drained while the job ran")
            }
            _ => End {
                release_key: true,
                ..End::bare(State::Cancelled)
            },
        };
        let ending = End {
            document,
            notes,
            ..ending
        };
        return finish(shared, row, control, &ending, staged);
    }
    let state = State::of_exit(done.exit);
    let ending = match (&done.failure, state) {
        (Some(failure), State::Refused | State::Failed) => End {
            document,
            notes,
            ..documents::failed(
                state,
                Some(done.exit),
                failure.code,
                &failure.detail,
                failure.details.clone(),
                row.kind,
            )
        },
        _ => End {
            exit: Some(done.exit),
            outcome: (state == State::Succeeded && done.unchanged).then(|| "unchanged".to_owned()),
            document,
            notes,
            ..End::bare(state)
        },
    };
    finish(shared, row, control, &ending, staged)
}

/// JB6, SD1: `interrupted`, `job_interrupted`, with the generation the job
/// started at and the catalog's now.
pub(crate) fn interrupted(shared: &Shared, row: &JobRow, why: &str) -> End {
    let mut details = Map::new();
    details.insert("attempt".into(), row.attempt.into());
    details.insert(
        "generation_at_start".into(),
        shared
            .db
            .job_by_key(row.key)
            .ok()
            .flatten()
            .and_then(|r| r.catalog_generation)
            .map_or(Value::Null, Value::from),
    );
    details.insert(
        "generation_at_stop".into(),
        catalog::generation(&shared.catalog).map_or(Value::Null, Value::from),
    );
    documents::failed(
        State::Interrupted,
        None,
        ErrorCode::JobInterrupted,
        why,
        details,
        row.kind,
    )
}

/// The finishing transaction (JS13): `ending`, or a stop that committed
/// first (JB5). A pure job (verify) ends as any: its stop leaves nothing,
/// a drain queues it again (SD1). A write job is never queued again: a
/// cancel keeps `ending`'s document, its committed changes; a drain made
/// `ending` its interruption already.
fn finish(
    shared: &Arc<Shared>,
    row: &JobRow,
    control: &Control,
    ending: &End,
    staged: Option<Synced>,
) -> Result<Ran, DbError> {
    let finished = if row.kind.pure() {
        run::end(shared, row, control, ending, staged, &Rows::None)?
    } else {
        let mut stopped = documents::stopped(row.kind);
        stopped.cancelled = End {
            document: ending.document.clone(),
            notes: ending.notes.clone(),
            ..stopped.cancelled
        };
        run::finish_with(shared, row, ending, &stopped, staged.into_iter().collect())?
    };
    run::conclude(shared, row, control, finished)
}

/// CX6: after a write, the log checkpointed; one above
/// `catalog.wal_barrier_mib` that readers keep from completing raises the
/// barrier.
fn pressure(shared: &Arc<Shared>) {
    let Ok(checkpoint) = shared.catalog.checkpoint_passive() else {
        return;
    };
    let limit = i64::try_from(shared.config.catalog.wal_barrier_mib)
        .unwrap_or(i64::MAX)
        .saturating_mul(1024 * 1024);
    let pinned = checkpoint.1 > 0 && checkpoint.2 < checkpoint.1;
    if !pinned {
        // The log folded back whole: no reader pins it.
        shared.catalog.set_barrier(false);
        return;
    }
    if checkpoint.1.saturating_mul(PAGE_BYTES) > limit && barrier(shared, false) {
        lower(shared, false);
    }
}

/// CX6: the barrier: query dispatch stops, running queries are given up to
/// `catalog.barrier_max_seconds` to finish, then the log is truncated and
/// dispatch resumes. If they still pin it, dispatch resumes without the
/// truncate, `fitment.info`'s flag stays set, and the barrier is tried no
/// sooner than the bound again; `vacuum`'s barrier is taken whatever the
/// last try. True when it was taken (and dispatch stays stopped until
/// [`lower`]).
fn barrier(shared: &Arc<Shared>, vacuum: bool) -> bool {
    let bound = Duration::from_secs(shared.config.catalog.barrier_max_seconds);
    if !vacuum && !shared.jobs.barrier_due() {
        return false;
    }
    shared.jobs.set_barrier(true);
    shared.catalog.set_barrier(true);
    let started = Instant::now();
    while shared.jobs.counts(Lane::Query).1 > 0 && started.elapsed() < bound {
        std::thread::sleep(Duration::from_millis(10));
    }
    let taken = shared.jobs.counts(Lane::Query).1 == 0
        && (vacuum || shared.catalog.checkpoint_truncate().is_ok_and(|busy| !busy));
    if !taken {
        tracing::warn!("the WAL barrier was not taken: running queries pin the log");
        shared.jobs.barrier_missed(bound);
        shared.jobs.set_barrier(false);
        shared.jobs.kick(shared);
    }
    taken
}

/// CX6: the barrier lowered, dispatch resumed; after a vacuum, the log
/// truncated as gc left it.
fn lower(shared: &Arc<Shared>, vacuum: bool) {
    if !vacuum || shared.catalog.checkpoint_truncate().is_ok_and(|busy| !busy) {
        shared.catalog.set_barrier(false);
    }
    shared.jobs.set_barrier(false);
    shared.jobs.kick(shared);
}
