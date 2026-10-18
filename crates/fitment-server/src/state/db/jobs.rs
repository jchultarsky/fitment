//! Jobs in `server.db` (docs/research/m6-api.md, section 6.2, JB3, JB6,
//! JB8, JB9, JB12, JS11, JS13, OB4, SD4): part of `state::db`, the one
//! module that names SQLite (SV6). A job exists once its row, its inputs
//! and its idempotency key commit together (JS11); every change of state
//! is one `BEGIN IMMEDIATE` that writes the state's persisted event with
//! it (JB8), so `seq` numbers persisted events alone; a job's end commits
//! its document's object, the rows that make its result durable and its
//! terminal state in one transaction (JS13), and `server.db` decides any
//! race between a cancel and an end.

use rusqlite::{Connection, OptionalExtension as _, Row, Transaction, TransactionBehavior, params};

use super::{Db, DbError, StoreError, lock};
use crate::jobs::model::{Kind, State};

/// A job's row, as the documents need it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow {
    /// `job.key`, internal: submission order.
    pub key: i64,
    /// `j_…`.
    pub id: String,
    /// Its owner's key.
    pub owner: i64,
    /// Its owner's id, `u_…`.
    pub owner_id: String,
    /// Its kind.
    pub kind: Kind,
    /// Its state.
    pub state: State,
    /// The normalized request, as JSON (RA9).
    pub request: String,
    /// The SHA-256 of the canonical request (JB9).
    pub request_sha256: String,
    /// Its attempt, 1 or 2 (SD5).
    pub attempt: i64,
    /// The request id that made it (HX7).
    pub request_id: String,
    /// Times, Unix milliseconds.
    pub created_ms: i64,
    #[allow(missing_docs)]
    pub started_ms: Option<i64>,
    #[allow(missing_docs)]
    pub finished_ms: Option<i64>,
    #[allow(missing_docs)]
    pub deadline_ms: Option<i64>,
    /// Its owner or an admin asked it to stop (JB5).
    pub cancel_requested: bool,
    /// The CLI's exit code (JB4).
    pub exit: Option<u8>,
    /// Set only for a job that succeeded (JB3).
    pub outcome: Option<String>,
    /// The error's code, for a job that did not succeed.
    pub error_code: Option<String>,
    /// The error's sentence and values, as JSON: `{detail, details}`.
    pub error_details: Option<String>,
    /// Its document's object.
    pub result_sha256: Option<String>,
    /// That object's length.
    pub result_bytes: Option<u64>,
    /// Its document's format.
    pub result_format: Option<String>,
    /// What it made: `fitment.job`'s `result.href`.
    pub result: Option<String>,
    /// JB10's notes, as JSON.
    pub notes: String,
    /// Kept past retention (JB12).
    pub pinned: bool,
    /// When its record expires (JB12).
    pub expires_ms: Option<i64>,
    /// Its last persisted event's `seq` (JB8, VN5).
    pub last_seq: i64,
}

/// A persisted event (JB8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    /// Its job's key.
    pub job: i64,
    /// 1, 2, 3 …, persisted events alone (review F6).
    pub seq: i64,
    /// When, Unix milliseconds.
    pub at_ms: i64,
    /// `state`, `note`, `end` (and the catalog's `batch`, `change`).
    pub kind: String,
    /// Its fields beyond `format`, `job`, `seq` and `type`, as JSON.
    pub data: String,
}

/// A job to insert (JS11).
#[derive(Debug, Clone)]
pub struct NewJob<'a> {
    /// `j_…`.
    pub id: &'a str,
    /// Its owner's key.
    pub owner: i64,
    /// Its kind.
    pub kind: Kind,
    /// The normalized request (RA9).
    pub request: &'a str,
    /// Its SHA-256.
    pub request_sha256: &'a str,
    /// `Idempotency-Key` and the SHA-256 of the request as sent (JB9).
    pub idempotency: Option<(&'a str, &'a str)>,
    /// The request's id (HX7).
    pub request_id: &'a str,
    /// Now.
    pub created_ms: i64,
    /// Every object it names, held from now until it ends (OB4).
    pub inputs: &'a [String],
    /// The socket revisions it cites (OB4's `job_cites`), held as long.
    pub cites: &'a [i64],
}

/// How a submission ended (JB9).
#[derive(Debug, Clone)]
pub enum Submitted {
    /// A new job, queued, with its first event.
    New(JobRow, EventRow),
    /// The key named a job of the same request: that job.
    Existing(JobRow),
    /// The key named a job of another request: `422`.
    KeyReused,
}

/// How a job ends (JB3): what its finishing transaction writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct End {
    /// A terminal state.
    pub state: State,
    /// The CLI's exit code, or null.
    pub exit: Option<u8>,
    /// Set only when `state` is `succeeded`.
    pub outcome: Option<String>,
    /// The error's code and `{detail, details}`.
    pub error: Option<(String, String)>,
    /// The key is released (JB9): `interrupted`, `cancelled`, or `failed`
    /// with a retryable code.
    pub release_key: bool,
    /// Its document: object name, length and format, placed by the
    /// transaction (OB5).
    pub document: Option<(String, u64, String)>,
    /// What it made, if known before the transaction.
    pub result: Option<String>,
}

/// What a job's end makes durable with it (JS13).
#[derive(Debug, Clone)]
pub enum Rows<'a> {
    /// Nothing but the job's own row.
    None,
    /// `assembly_read`: the caller's assembly row, its name and its
    /// `fitment.parts` row of this build.
    Assembly(AssemblyRows<'a>),
    /// `socket_extract`: the line and its new revision, or nothing new
    /// for an unchanged extraction.
    Socket(SocketRows<'a>),
}

/// `assembly_read`'s rows (RA3).
#[derive(Debug, Clone)]
pub struct AssemblyRows<'a> {
    /// The owner's key.
    pub owner: i64,
    /// The assembly's SHA-256.
    pub sha256: &'a str,
    /// RA8's `file_name`, if given.
    pub file_name: Option<&'a str>,
    /// `accepted` or `refused`.
    pub status: &'a str,
    /// The root definition, as JSON.
    pub root: Option<&'a str>,
    /// The `fitment.parts` document's object.
    pub document_sha256: &'a str,
}

/// `socket_extract`'s rows (RA4, RA5).
#[derive(Debug, Clone)]
pub struct SocketRows<'a> {
    /// The owner's key.
    pub owner: i64,
    /// `sk_…`.
    pub line_id: &'a str,
    /// The triple (RA4).
    pub assembly_sha256: &'a str,
    #[allow(missing_docs)]
    pub definition: &'a str,
    #[allow(missing_docs)]
    pub occurrence: &'a str,
    /// The revision the job carried from, or that it found current: the
    /// transaction refuses to commit over another (a review or an
    /// extraction that committed meanwhile), answering [`Finished::Retry`].
    pub base: Option<i64>,
    /// The new revision, or `None`: the bytes equal the current verified
    /// revision's, so nothing is written (`unchanged`).
    pub revision: Option<NewRevision<'a>>,
}

/// A socket revision to insert (RA5, OB5).
#[derive(Debug, Clone)]
pub struct NewRevision<'a> {
    /// `extract`, `review` or `import`.
    pub how: &'a str,
    /// RA10.
    pub verified: bool,
    /// The revision it carries decisions from, by key.
    pub parent: Option<i64>,
    /// Its document's object and digest.
    pub document_sha256: &'a str,
    #[allow(missing_docs)]
    pub document_digest: &'a str,
    /// Its versions.
    pub extractor_version: u32,
    #[allow(missing_docs)]
    pub socket_rules_version: u32,
    /// Its unreviewed, carried and stale counts.
    pub unreviewed: u64,
    #[allow(missing_docs)]
    pub carried: u64,
    #[allow(missing_docs)]
    pub stale: u64,
}

/// What a finishing transaction did.
#[derive(Debug, Clone)]
pub enum Finished {
    /// The job ended as asked, or as a stop that committed first says
    /// (JB5): its row and its two events, `state` and `end`.
    Ended(JobRow, Vec<EventRow>),
    /// It had ended already: nothing written.
    Already(JobRow),
    /// The line's current revision is no longer the one the job carried
    /// from: carry again and finish again.
    Retry,
    /// Another triple holds the line's id in the owner's namespace (RA4):
    /// a collision, `internal`.
    Collision,
    /// The owner's assembly was deleted under the job: nothing written.
    Gone,
}

/// What a cancel did (JB5).
#[derive(Debug, Clone)]
pub enum Cancelled {
    /// A queued job: `cancelled` at once.
    Ended(JobRow, Vec<EventRow>),
    /// A running one: `cancelling` until its call returns.
    Cancelling(JobRow, Option<EventRow>),
    /// It has ended: `409 job_finished`.
    Finished(JobRow),
}

/// The ends a job is held to when a stop committed first (JB5, JB7).
#[derive(Debug, Clone)]
pub struct Stopped {
    /// A cancel: `cancelled`.
    pub cancelled: End,
    /// Its deadline: `failed`, `deadline_exceeded`.
    pub deadline: End,
}

/// What recovery did with one job (SD4).
#[derive(Debug, Clone)]
pub struct Recovered {
    /// Its row afterwards.
    pub row: JobRow,
    /// The events it wrote.
    pub events: Vec<EventRow>,
}

/// The columns [`job_row`] reads, from `job j JOIN user u`.
const COLUMNS: &str = "j.key, j.id, j.owner, u.id, j.kind, j.state, CAST(j.request AS TEXT),
    j.request_sha256, j.attempt, j.request_id, j.created_ms, j.started_ms, j.finished_ms,
    j.deadline_ms, j.cancel_requested, j.exit, j.outcome, j.error_code, j.error_details,
    j.result_sha256, (SELECT o.bytes FROM object o WHERE o.sha256 = j.result_sha256),
    j.result_format, j.result, j.notes, j.pinned, j.expires_ms,
    (SELECT coalesce(max(e.seq), 0) FROM job_event e WHERE e.job = j.key)";

fn job_row(row: &Row<'_>) -> rusqlite::Result<JobRow> {
    let kind: String = row.get(4)?;
    let state: String = row.get(5)?;
    let bad = |what: &str| {
        rusqlite::Error::InvalidColumnType(0, what.to_owned(), rusqlite::types::Type::Text)
    };
    let exit: Option<i64> = row.get(15)?;
    let bytes: Option<i64> = row.get(20)?;
    Ok(JobRow {
        key: row.get(0)?,
        id: row.get(1)?,
        owner: row.get(2)?,
        owner_id: row.get(3)?,
        kind: Kind::parse(&kind).ok_or_else(|| bad("kind"))?,
        state: State::parse(&state).ok_or_else(|| bad("state"))?,
        request: row.get(6)?,
        request_sha256: row.get(7)?,
        attempt: row.get(8)?,
        request_id: row.get(9)?,
        created_ms: row.get(10)?,
        started_ms: row.get(11)?,
        finished_ms: row.get(12)?,
        deadline_ms: row.get(13)?,
        cancel_requested: row.get::<_, i64>(14)? != 0,
        exit: exit.and_then(|e| u8::try_from(e).ok()),
        outcome: row.get(16)?,
        error_code: row.get(17)?,
        error_details: row.get(18)?,
        result_sha256: row.get(19)?,
        result_bytes: bytes.and_then(|b| u64::try_from(b).ok()),
        result_format: row.get(21)?,
        result: row.get(22)?,
        notes: row.get(23)?,
        pinned: row.get::<_, i64>(24)? != 0,
        expires_ms: row.get(25)?,
        last_seq: row.get(26)?,
    })
}

fn event_row(row: &Row<'_>) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        job: row.get(0)?,
        seq: row.get(1)?,
        at_ms: row.get(2)?,
        kind: row.get(3)?,
        data: row.get(4)?,
    })
}

/// The job of `key`, in `conn`.
fn by_key(conn: &Connection, key: i64) -> rusqlite::Result<JobRow> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM job j JOIN user u ON u.key = j.owner WHERE j.key = ?1"),
        [key],
        job_row,
    )
}

/// Appends a persisted event to job `key` (JB8): its `seq` the next.
fn append(
    tx: &Transaction<'_>,
    key: i64,
    now_ms: i64,
    kind: &str,
    data: &str,
) -> rusqlite::Result<EventRow> {
    let seq: i64 = tx.query_row(
        "SELECT coalesce(max(seq), 0) + 1 FROM job_event WHERE job = ?1",
        [key],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT INTO job_event (job, seq, at_ms, type, data) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![key, seq, now_ms, kind, data],
    )?;
    Ok(EventRow {
        job: key,
        seq,
        at_ms: now_ms,
        kind: kind.to_owned(),
        data: data.to_owned(),
    })
}

/// A `state` event's data (JB8): the new state and the attempt.
fn state_data(state: State, attempt: i64) -> String {
    format!(r#"{{"state":"{}","attempt":{attempt}}}"#, state.name())
}

/// Moves job `key` to `state`, writing its `state` event.
fn set_state(
    tx: &Transaction<'_>,
    key: i64,
    state: State,
    attempt: i64,
    now_ms: i64,
) -> rusqlite::Result<EventRow> {
    tx.execute(
        "UPDATE job SET state = ?2, attempt = ?3 WHERE key = ?1",
        params![key, state.name(), attempt],
    )?;
    append(tx, key, now_ms, "state", &state_data(state, attempt))
}

impl Db {
    /// JS11, JB9: inserts a queued job, its inputs (OB4's hold, from now)
    /// and its idempotency key in one `BEGIN IMMEDIATE`, with its first
    /// event. A key already used by the owner answers the job it named,
    /// or [`Submitted::KeyReused`] for another request; nothing is written
    /// then.
    ///
    /// # Errors
    ///
    /// SQLite's; nothing is committed.
    pub fn submit_job(&self, new: &NewJob<'_>) -> Result<Submitted, DbError> {
        let mut conn = lock(&self.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some((key, sha)) = new.idempotency {
            let found: Option<(i64, Option<String>)> = tx
                .query_row(
                    "SELECT key, idempotency_sha256 FROM job WHERE owner = ?1 AND idempotency_key = ?2",
                    params![new.owner, key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((job, fingerprint)) = found {
                return Ok(if fingerprint.as_deref() == Some(sha) {
                    Submitted::Existing(by_key(&tx, job)?)
                } else {
                    Submitted::KeyReused
                });
            }
        }
        tx.execute(
            "INSERT INTO job (id, owner, kind, lane, state, request, request_sha256,
                              idempotency_key, idempotency_sha256, request_id, created_ms)
             VALUES (?1, ?2, ?3, ?4, 'queued', CAST(?5 AS BLOB), ?6, ?7, ?8, ?9, ?10)",
            params![
                new.id,
                new.owner,
                new.kind.name(),
                new.kind.lane().name(),
                new.request,
                new.request_sha256,
                new.idempotency.map(|(k, _)| k),
                new.idempotency.map(|(_, s)| s),
                new.request_id,
                new.created_ms
            ],
        )?;
        let key = tx.last_insert_rowid();
        for (n, sha256) in new.inputs.iter().enumerate() {
            tx.execute(
                "INSERT INTO job_input (job, n, sha256) VALUES (?1, ?2, ?3)",
                params![key, i64::try_from(n).unwrap_or(i64::MAX), sha256],
            )?;
        }
        for revision in new.cites {
            tx.execute(
                "INSERT OR IGNORE INTO job_cites (job, revision) VALUES (?1, ?2)",
                params![key, revision],
            )?;
        }
        let event = append(
            &tx,
            key,
            new.created_ms,
            "state",
            &state_data(State::Queued, 1),
        )?;
        let row = by_key(&tx, key)?;
        tx.commit()?;
        Ok(Submitted::New(row, event))
    }

    /// The job `id`, if `owner` (or anyone, for `None`: an admin) may see
    /// it (AU8).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn job(&self, id: &str, owner: Option<i64>) -> Result<Option<JobRow>, DbError> {
        self.read(|conn| {
            conn.query_row(
                &format!(
                    "SELECT {COLUMNS} FROM job j JOIN user u ON u.key = j.owner
                     WHERE j.id = ?1 AND (?2 IS NULL OR j.owner = ?2)"
                ),
                params![id, owner],
                job_row,
            )
            .optional()
        })
    }

    /// JB9: the owner's job of `key`, with the fingerprint it was made
    /// under; a released key names nothing.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn job_by_idempotency(
        &self,
        owner: i64,
        key: &str,
    ) -> Result<Option<(JobRow, String)>, DbError> {
        self.read(|conn| {
            conn.query_row(
                &format!(
                    "SELECT {COLUMNS}, coalesce(j.idempotency_sha256, '') FROM job j
                     JOIN user u ON u.key = j.owner WHERE j.owner = ?1 AND j.idempotency_key = ?2"
                ),
                params![owner, key],
                |r| Ok((job_row(r)?, r.get(27)?)),
            )
            .optional()
        })
    }

    /// The job of `key`.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn job_by_key(&self, key: i64) -> Result<Option<JobRow>, DbError> {
        self.read(|conn| {
            conn.query_row(
                &format!(
                    "SELECT {COLUMNS} FROM job j JOIN user u ON u.key = j.owner WHERE j.key = ?1"
                ),
                [key],
                job_row,
            )
            .optional()
        })
    }

    /// A page of `owner`'s jobs by (creation time, id) (VN3, M6-55), after
    /// `after`, of `kind` and `state` if given: at most `limit`.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn jobs_page(
        &self,
        owner: i64,
        kind: Option<Kind>,
        state: Option<State>,
        after: Option<(i64, &str)>,
        limit: u32,
    ) -> Result<Vec<JobRow>, DbError> {
        let (after_ms, after_id) = after.unwrap_or((i64::MIN, ""));
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {COLUMNS} FROM job j JOIN user u ON u.key = j.owner
                 WHERE j.owner = ?1 AND (?2 IS NULL OR j.kind = ?2) AND (?3 IS NULL OR j.state = ?3)
                   AND (j.created_ms > ?4 OR (j.created_ms = ?4 AND j.id > ?5))
                 ORDER BY j.created_ms, j.id LIMIT ?6"
            ))?;
            statement
                .query_map(
                    params![
                        owner,
                        kind.map(Kind::name),
                        state.map(State::name),
                        after_ms,
                        after_id,
                        limit
                    ],
                    job_row,
                )?
                .collect()
        })
    }

    /// Job `key`'s persisted events after `seq`, in order (JB8).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn events_after(&self, key: i64, seq: i64) -> Result<Vec<EventRow>, DbError> {
        self.read(|conn| {
            let mut statement = conn.prepare(
                "SELECT job, seq, at_ms, type, data FROM job_event WHERE job = ?1 AND seq > ?2
                 ORDER BY seq",
            )?;
            statement.query_map(params![key, seq], event_row)?.collect()
        })
    }

    /// A job's queue position (section 7.11): its rank, from 1, among its
    /// lane's queued jobs in submission order; `None` once it is not
    /// queued.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn queue_position(&self, row: &JobRow) -> Result<Option<u64>, DbError> {
        if row.state != State::Queued {
            return Ok(None);
        }
        self.read(|conn| {
            conn.query_row(
                "SELECT count(*) FROM job WHERE lane = ?1 AND state = 'queued' AND key <= ?2",
                params![row.kind.lane().name(), row.key],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| u64::try_from(n).ok())
        })
    }

    /// Dispatch (JS4): a queued job becomes `running`, its start and its
    /// deadline (JB7) recorded, with its `state` event. `None` if it is not
    /// queued any more (cancelled while it waited for its permits).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn start_job(
        &self,
        key: i64,
        now_ms: i64,
        deadline_ms: Option<i64>,
    ) -> Result<Option<(JobRow, EventRow)>, DbError> {
        let mut conn = lock(&self.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = by_key(&tx, key)?;
        if row.state != State::Queued {
            return Ok(None);
        }
        tx.execute(
            "UPDATE job SET started_ms = ?2, deadline_ms = ?3 WHERE key = ?1",
            params![key, now_ms, deadline_ms],
        )?;
        let event = set_state(&tx, key, State::Running, row.attempt, now_ms)?;
        let row = by_key(&tx, key)?;
        tx.commit()?;
        Ok(Some((row, event)))
    }

    /// JB5: a cancel of job `key`. A queued job ends `cancelled` at once
    /// (`end` gives its end event's data); a running one becomes
    /// `cancelling`, `cancel_requested` set; one `cancelling` already, by a
    /// cancel or its deadline, and one that ended are left.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn cancel_job(
        &self,
        key: i64,
        now_ms: i64,
        retention_ms: i64,
        end: &dyn Fn(&JobRow) -> String,
    ) -> Result<Cancelled, DbError> {
        let mut conn = lock(&self.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = by_key(&tx, key)?;
        match row.state {
            State::Queued => {
                let ending = End {
                    state: State::Cancelled,
                    exit: None,
                    outcome: None,
                    error: None,
                    release_key: true,
                    document: None,
                    result: None,
                };
                let events = write_end(&tx, &row, &ending, now_ms, retention_ms, end)?;
                let row = by_key(&tx, key)?;
                tx.commit()?;
                Ok(Cancelled::Ended(row, events))
            }
            State::Running => {
                tx.execute("UPDATE job SET cancel_requested = 1 WHERE key = ?1", [key])?;
                let event = set_state(&tx, key, State::Cancelling, row.attempt, now_ms)?;
                let row = by_key(&tx, key)?;
                tx.commit()?;
                Ok(Cancelled::Cancelling(row, Some(event)))
            }
            // JB5, JB7 (review S7 of PR 5): the first reason stands, so a
            // deadline's `cancelling` stays a deadline's.
            State::Cancelling => Ok(Cancelled::Cancelling(row, None)),
            _ => Ok(Cancelled::Finished(row)),
        }
    }

    /// JB7: a running job past its deadline becomes `cancelling`, without
    /// `cancel_requested`, so its end is `failed`, `deadline_exceeded`.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn deadline_job(&self, key: i64, now_ms: i64) -> Result<Option<EventRow>, DbError> {
        let mut conn = lock(&self.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = by_key(&tx, key)?;
        if row.state != State::Running {
            return Ok(None);
        }
        let event = set_state(&tx, key, State::Cancelling, row.attempt, now_ms)?;
        tx.commit()?;
        Ok(Some(event))
    }

    /// SD1: a pure job stopped by a drain goes back to `queued`, its
    /// attempt unchanged, since a drain is not the job's doing, with its
    /// `state` event (decided in PR 5's review); a stop that committed first
    /// ends it as JB5 says.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn requeue_job(
        &self,
        key: i64,
        stopped: &Stopped,
        now_ms: i64,
        retention_ms: i64,
        end: &dyn Fn(&JobRow) -> String,
    ) -> Result<Finished, DbError> {
        let mut conn = lock(&self.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = by_key(&tx, key)?;
        let ending = match row.state {
            State::Running => {
                tx.execute(
                    "UPDATE job SET started_ms = NULL, deadline_ms = NULL WHERE key = ?1",
                    [key],
                )?;
                let event = set_state(&tx, key, State::Queued, row.attempt, now_ms)?;
                let row = by_key(&tx, key)?;
                tx.commit()?;
                return Ok(Finished::Ended(row, vec![event]));
            }
            State::Cancelling if row.cancel_requested => &stopped.cancelled,
            State::Cancelling => &stopped.deadline,
            _ => return Ok(Finished::Already(row)),
        };
        let events = write_end(&tx, &row, ending, now_ms, retention_ms, end)?;
        let row = by_key(&tx, key)?;
        tx.commit()?;
        Ok(Finished::Ended(row, events))
    }

    /// JS13, OB5: ends job `key` as `end` says, in one `BEGIN IMMEDIATE`:
    /// its document's object put in place by `place` and its row inserted,
    /// the rows that make its result durable (`rows`), its terminal state,
    /// its events, its inputs' hold released (OB4) and, where JB9 says,
    /// its key. A stop that committed first wins (JB5): then `stopped`'s
    /// end is written, and nothing of the result. `end` gives the end
    /// event's data from the row as it ends.
    ///
    /// # Errors
    ///
    /// [`StoreError::Place`] for the document's files, else SQLite's;
    /// nothing is committed.
    #[allow(clippy::too_many_arguments)] // one transaction's inputs
    pub fn finish_job(
        &self,
        key: i64,
        ending: &End,
        stopped: &Stopped,
        place: &mut dyn FnMut() -> std::io::Result<()>,
        rows: &Rows<'_>,
        now_ms: i64,
        retention_ms: i64,
        end: &dyn Fn(&JobRow) -> String,
    ) -> Result<Finished, StoreError> {
        let mut conn = lock(&self.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = by_key(&tx, key)?;
        let ending = match row.state {
            s if s.terminal() => return Ok(Finished::Already(row)),
            State::Cancelling if row.cancel_requested => stopped.cancelled.clone(),
            State::Cancelling => stopped.deadline.clone(),
            _ => {
                let mut ending = ending.clone();
                // The object before the rows that name it (OB5); a rename
                // a rollback leaves behind is an orphan the sweep unlinks.
                if let Some((sha256, bytes, _)) = &ending.document {
                    place().map_err(StoreError::Place)?;
                    tx.execute(
                        "INSERT INTO object (sha256, bytes, added_ms) VALUES (?1, ?2, ?3)
                         ON CONFLICT (sha256) DO NOTHING",
                        params![sha256, i64::try_from(*bytes).unwrap_or(i64::MAX), now_ms],
                    )?;
                }
                match rows {
                    Rows::None => {}
                    Rows::Assembly(a) => super::assemblies::register(&tx, a, row.key, now_ms)?,
                    Rows::Socket(s) => match super::assemblies::revise(&tx, s, row.key, now_ms)? {
                        super::assemblies::Revised::Retry => return Ok(Finished::Retry),
                        super::assemblies::Revised::Collision => return Ok(Finished::Collision),
                        super::assemblies::Revised::Gone => return Ok(Finished::Gone),
                        super::assemblies::Revised::Made(href) => ending.result = Some(href),
                    },
                }
                ending
            }
        };
        let events = write_end(&tx, &row, &ending, now_ms, retention_ms, end)?;
        let row = by_key(&tx, key)?;
        tx.commit()?;
        Ok(Finished::Ended(row, events))
    }

    /// SD4, JB6, JS13: settles the jobs a stopped process left `running`
    /// or `cancelling`, oldest first. A cancelled one is `cancelled`; one
    /// past its deadline `failed`; a pure one whose result committed ends
    /// with it; another pure one is queued again with its next attempt, or
    /// `interrupted` after its second (SD5); a write job is `interrupted`.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn recover_jobs(
        &self,
        stopped: &Stopped,
        interrupted: &dyn Fn(&JobRow) -> End,
        now_ms: i64,
        retention_ms: i64,
        end: &dyn Fn(&JobRow) -> String,
    ) -> Result<Vec<Recovered>, DbError> {
        let mut conn = lock(&self.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let keys: Vec<i64> = {
            let mut statement = tx.prepare(
                "SELECT key FROM job WHERE state IN ('running', 'cancelling') ORDER BY key",
            )?;
            statement
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            let row = by_key(&tx, key)?;
            let ending = match row.state {
                State::Cancelling if row.cancel_requested => Some(stopped.cancelled.clone()),
                State::Cancelling => Some(stopped.deadline.clone()),
                _ if row.kind.pure() => committed(&tx, &row)?,
                _ => Some(interrupted(&row)),
            };
            let events = match ending {
                Some(ending) => write_end(&tx, &row, &ending, now_ms, retention_ms, end)?,
                None if row.attempt < 2 => {
                    tx.execute(
                        "UPDATE job SET started_ms = NULL, deadline_ms = NULL WHERE key = ?1",
                        [key],
                    )?;
                    vec![set_state(&tx, key, State::Queued, row.attempt + 1, now_ms)?]
                }
                None => write_end(&tx, &row, &interrupted(&row), now_ms, retention_ms, end)?,
            };
            out.push(Recovered {
                row: by_key(&tx, key)?,
                events,
            });
        }
        tx.commit()?;
        Ok(out)
    }

    /// SD4: every queued job, in submission order, with the bytes of the
    /// objects it names (JS3's estimate).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn queued_jobs(&self) -> Result<Vec<(JobRow, u64)>, DbError> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {COLUMNS}, (SELECT coalesce(max(o.bytes), 0) FROM job_input i
                                    JOIN object o ON o.sha256 = i.sha256 WHERE i.job = j.key)
                 FROM job j JOIN user u ON u.key = j.owner WHERE j.state = 'queued' ORDER BY j.key"
            ))?;
            statement
                .query_map([], |r| {
                    let bytes: i64 = r.get(27)?;
                    Ok((job_row(r)?, u64::try_from(bytes).unwrap_or(0)))
                })?
                .collect()
        })
    }

    /// JB12: pins job `id` past retention, or unpins it.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn pin_job(&self, key: i64, pinned: bool) -> Result<(), DbError> {
        let mut conn = lock(&self.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE job SET pinned = ?2 WHERE key = ?1",
            params![key, i64::from(pinned)],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// JB12: deletes a finished job's record, its events and inputs
    /// cascading; what it made durable stays. False if it has not ended.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn delete_job(&self, key: i64) -> Result<bool, DbError> {
        let mut conn = lock(&self.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = by_key(&tx, key)?;
        if !row.state.terminal() {
            return Ok(false);
        }
        tx.execute("DELETE FROM job WHERE key = ?1", [key])?;
        tx.commit()?;
        Ok(true)
    }

    /// JS9, JB12: deletes up to `batch` terminal jobs whose record expired
    /// at `now_ms` and is not pinned; returns how many.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn expire_jobs(&self, now_ms: i64, batch: u32) -> Result<usize, DbError> {
        let mut conn = lock(&self.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let deleted = tx.execute(
            "DELETE FROM job WHERE key IN (
               SELECT key FROM job WHERE expires_ms IS NOT NULL AND expires_ms <= ?1 AND pinned = 0
               ORDER BY expires_ms LIMIT ?2)",
            params![now_ms, batch],
        )?;
        tx.commit()?;
        Ok(deleted)
    }

    /// The objects job `key` holds as its inputs (OB4), in order.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn job_inputs(&self, key: i64) -> Result<Vec<String>, DbError> {
        self.read(|conn| {
            let mut statement =
                conn.prepare("SELECT sha256 FROM job_input WHERE job = ?1 ORDER BY n")?;
            statement.query_map([key], |r| r.get(0))?.collect()
        })
    }
}

/// JS13: the result a pure job's own row names, if its end committed it:
/// a socket revision or a parts row naming the job. Its finishing
/// transaction commits those with its terminal state, so a job found
/// running has none; the check stands for a crash between the two, which
/// one transaction rules out (VT10).
fn committed(tx: &Transaction<'_>, row: &JobRow) -> rusqlite::Result<Option<End>> {
    let revision: Option<(String, i64, String)> = tx
        .query_row(
            "SELECT line_id, n, document_sha256 FROM socket_revision WHERE job = ?1",
            [row.key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((line, n, sha256)) = revision {
        let bytes = object_bytes(tx, &sha256)?;
        return Ok(Some(succeeded(
            Some(format!("/v1/sockets/{line}@r{n}")),
            sha256,
            bytes,
            row.kind,
        )));
    }
    let parts: Option<(String, String, String)> = tx
        .query_row(
            "SELECT a.sha256, p.status, p.document_sha256 FROM assembly_parts p
             JOIN assembly a ON a.key = p.assembly WHERE p.job = ?1",
            [row.key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(parts.map(|(assembly, status, sha256)| {
        let bytes = object_bytes(tx, &sha256).unwrap_or(0);
        let mut end = succeeded(
            Some(format!("/v1/assemblies/sha256:{assembly}")),
            sha256,
            bytes,
            row.kind,
        );
        if status == "refused" {
            end.state = State::Refused;
            end.exit = Some(3);
        }
        end
    }))
}

fn object_bytes(tx: &Transaction<'_>, sha256: &str) -> rusqlite::Result<u64> {
    tx.query_row(
        "SELECT bytes FROM object WHERE sha256 = ?1",
        [sha256],
        |r| r.get::<_, i64>(0),
    )
    .map(|b| u64::try_from(b).unwrap_or(0))
}

fn succeeded(result: Option<String>, sha256: String, bytes: u64, kind: Kind) -> End {
    End {
        state: State::Succeeded,
        exit: Some(0),
        outcome: None,
        error: None,
        release_key: false,
        document: Some((sha256, bytes, kind.document_format().to_owned())),
        result,
    }
}

/// Writes a job's end: its terminal row, its record's expiry (JB12), its
/// `state` and `end` events, its key released where JB9 says, and a
/// non-match job's inputs' hold released (OB4).
fn write_end(
    tx: &Transaction<'_>,
    row: &JobRow,
    ending: &End,
    now_ms: i64,
    retention_ms: i64,
    end: &dyn Fn(&JobRow) -> String,
) -> rusqlite::Result<Vec<EventRow>> {
    let (sha256, format) = match &ending.document {
        Some((sha, _, format)) => (Some(sha.as_str()), Some(format.as_str())),
        None => (None, None),
    };
    tx.execute(
        "UPDATE job SET state = ?2, exit = ?3, outcome = ?4, error_code = ?5, error_details = ?6,
                        result_sha256 = ?7, result_format = ?8, result = ?9, finished_ms = ?10,
                        expires_ms = ?11
         WHERE key = ?1",
        params![
            row.key,
            ending.state.name(),
            ending.exit.map(i64::from),
            ending.outcome,
            ending.error.as_ref().map(|(c, _)| c.as_str()),
            ending.error.as_ref().map(|(_, d)| d.as_str()),
            sha256,
            format,
            ending.result,
            now_ms,
            now_ms.saturating_add(retention_ms)
        ],
    )?;
    if ending.release_key {
        tx.execute(
            "UPDATE job SET released_key = idempotency_key, idempotency_key = NULL
             WHERE key = ?1 AND idempotency_key IS NOT NULL",
            [row.key],
        )?;
    }
    if row.kind != Kind::Match {
        tx.execute("DELETE FROM job_input WHERE job = ?1", [row.key])?;
        tx.execute("DELETE FROM job_cites WHERE job = ?1", [row.key])?;
    }
    let state = append(
        tx,
        row.key,
        now_ms,
        "state",
        &state_data(ending.state, row.attempt),
    )?;
    let ended = by_key(tx, row.key)?;
    let data = end(&ended);
    let last = append(tx, row.key, now_ms, "end", &data)?;
    Ok(vec![state, last])
}
