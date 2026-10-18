//! Uploads and objects in `server.db` (docs/research/m6-api.md, section
//! 6.2, UP3 to UP5, UP8, OB2, OB4): part of `state::db`, the one module
//! that names SQLite (SV6). An object row promises a file hashing to its
//! name (OB2); an upload row is a user's access to it (UP3). Every write
//! is one `BEGIN IMMEDIATE`, and the file calls that make an object or
//! unlink one run inside it, so a sweep never unlinks between an upload's
//! check and its row.

use rusqlite::{Connection, OptionalExtension as _, Row, TransactionBehavior, params};

use super::{Db, DbError};

/// What an object is to one user (UP3, section 7.11's `fitment.upload`):
/// what holds it among the user's rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holding {
    /// Its SHA-256, lower-case hex.
    pub sha256: String,
    /// Its length.
    pub bytes: u64,
    /// The user's rows that hold it, of `upload`, `assembly`, `socket`
    /// and `job`, in that order.
    pub held_by: Vec<&'static str>,
    /// How many of the user's jobs hold it as an input (OB4).
    pub pinned_by_jobs: u64,
}

/// How an upload ended (UP3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stored {
    /// The user held no live upload of these bytes: `201`, whether or not
    /// another user stored them (UP8).
    New,
    /// The user's own upload of them, its lifetime extended: `200`.
    Refreshed,
}

/// Why an upload could not be stored.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// The object's files could not be put in place (OB2).
    #[error("the object could not be put in place: {}", .0.kind())]
    Place(std::io::Error),
    /// SQLite failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Db(error.into())
    }
}

/// What one batch of the sweep's candidates gave (OB4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unnamed {
    /// Objects unlinked with their rows.
    pub gone: Vec<String>,
    /// Objects whose unlink failed, left candidates.
    pub failed: Vec<String>,
    /// Candidates looked at.
    pub looked: usize,
}

/// How a delete ended (RA6, UP5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deleted {
    /// The user's row is gone; the object, if nothing else holds it, is
    /// the sweep's to unlink.
    Gone,
    /// A job that is not terminal names it: `409 upload_in_use`.
    InUse,
    /// The user holds no live upload of it: `404`.
    NotFound,
}

/// The states of a job that is not terminal (JB3).
const LIVE_JOB: &str = "('queued', 'running', 'cancelling')";

/// True if a row of any table names the object `?1` (OB4): every table
/// with a foreign key to `object`, each probed through its index on the
/// column, so an object row is deleted only when nothing could refer to
/// it, in one probe a table.
const NAMED: &str = "EXISTS (SELECT 1 FROM upload WHERE sha256 = ?1)
    OR EXISTS (SELECT 1 FROM assembly WHERE sha256 = ?1)
    OR EXISTS (SELECT 1 FROM assembly_parts WHERE document_sha256 = ?1)
    OR EXISTS (SELECT 1 FROM socket_revision WHERE document_sha256 = ?1)
    OR EXISTS (SELECT 1 FROM job_input WHERE sha256 = ?1)
    OR EXISTS (SELECT 1 FROM job WHERE result_sha256 = ?1)
    OR EXISTS (SELECT 1 FROM job WHERE index_sha256 = ?1)";

/// UP4 (PR 4's review): what an object counts against its holder's quota:
/// its length, at least [`CHARGE_MIN`] (a file's least use of the disk),
/// and [`CHARGE_ROW`] for its rows, so tiny objects cannot exhaust the
/// disk's inodes within a quota of bytes.
pub const CHARGE_MIN: u64 = 4096;

/// The per-object overhead charged against a quota (UP4).
pub const CHARGE_ROW: u64 = 512;

/// The charge of an object of `bytes` bytes (UP4).
pub fn charge(bytes: u64) -> u64 {
    bytes.max(CHARGE_MIN).saturating_add(CHARGE_ROW)
}

/// The holding of the object `?2` for the user `?1` at `?3`.
const HOLDING: &str = "SELECT o.sha256, o.bytes,
      EXISTS (SELECT 1 FROM upload u WHERE u.owner = ?1 AND u.sha256 = o.sha256
              AND u.expires_ms > ?3),
      EXISTS (SELECT 1 FROM assembly a WHERE a.owner = ?1 AND a.sha256 = o.sha256
              AND a.deleted_ms IS NULL),
      EXISTS (SELECT 1 FROM socket_revision r JOIN socket_line l ON l.key = r.line
              WHERE r.owner = ?1 AND r.document_sha256 = o.sha256 AND l.deleted_ms IS NULL),
      (SELECT count(DISTINCT i.job) FROM job_input i JOIN job j ON j.key = i.job
       WHERE j.owner = ?1 AND i.sha256 = o.sha256),
      EXISTS (SELECT 1 FROM job_input i JOIN job j ON j.key = i.job
              WHERE j.owner = ?1 AND i.sha256 = o.sha256 AND j.kind = 'match')
    FROM object o WHERE o.sha256 = ?2";

fn holding(row: &Row<'_>) -> rusqlite::Result<(Holding, bool)> {
    let (upload, assembly, socket): (bool, bool, bool) = (row.get(2)?, row.get(3)?, row.get(4)?);
    let jobs: i64 = row.get(5)?;
    let mut held_by = Vec::new();
    for (holds, name) in [
        (upload, "upload"),
        (assembly, "assembly"),
        (socket, "socket"),
        (jobs > 0, "job"),
    ] {
        if holds {
            held_by.push(name);
        }
    }
    let bytes: i64 = row.get(1)?;
    let match_input: bool = row.get(6)?;
    Ok((
        Holding {
            sha256: row.get(0)?,
            bytes: u64::try_from(bytes).unwrap_or(0),
            held_by,
            pinned_by_jobs: u64::try_from(jobs).unwrap_or(0),
        },
        upload || match_input,
    ))
}

/// The object `sha256` as `owner` may name it at `now` (UP3): an
/// unexpired upload of theirs, or an input of a retained match record of
/// theirs (OB4, M6-64).
fn nameable(
    conn: &Connection,
    owner: i64,
    sha256: &str,
    now_ms: i64,
) -> rusqlite::Result<Option<Holding>> {
    let found = conn
        .query_row(HOLDING, params![owner, sha256, now_ms], holding)
        .optional()?;
    Ok(found.and_then(|(holding, may)| may.then_some(holding)))
}

impl Db {
    /// UP8, OB2: stores an upload of `owner` whose temporary file is
    /// synced, in one `BEGIN IMMEDIATE` that does the same work whether or
    /// not the object exists (PR 4's review): the caller's own row looked
    /// for (theirs alone, which tells nothing of another user's), `place`
    /// called (it renames the file over the final name), the object's row
    /// inserted unless there (`ON CONFLICT DO NOTHING`), and the caller's
    /// upload row inserted or its lifetime extended to `expires_ms`. A
    /// sweep cannot unlink the object between the rename and the rows
    /// (OB2).
    ///
    /// # Errors
    ///
    /// [`StoreError::Place`] for the files, else SQLite's; nothing is
    /// committed.
    pub fn store_upload(
        &self,
        owner: i64,
        sha256: &str,
        bytes: u64,
        now_ms: i64,
        expires_ms: i64,
        place: impl FnOnce() -> std::io::Result<()>,
    ) -> Result<Stored, StoreError> {
        let size = i64::try_from(bytes).unwrap_or(i64::MAX);
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: Option<i64> = tx
            .query_row(
                "SELECT expires_ms FROM upload WHERE owner = ?1 AND sha256 = ?2",
                params![owner, sha256],
                |r| r.get(0),
            )
            .optional()?;
        place().map_err(StoreError::Place)?;
        tx.execute(
            "INSERT INTO object (sha256, bytes, added_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT (sha256) DO NOTHING",
            params![sha256, size, now_ms],
        )?;
        tx.execute(
            "INSERT INTO upload (owner, sha256, created_ms, expires_ms) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (owner, sha256) DO UPDATE SET
               created_ms = CASE WHEN expires_ms > ?3 THEN created_ms ELSE ?3 END,
               expires_ms = max(expires_ms, ?4)",
            params![owner, sha256, now_ms, expires_ms],
        )?;
        tx.commit()?;
        Ok(match prior {
            Some(expires) if expires > now_ms => Stored::Refreshed,
            _ => Stored::New,
        })
    }

    /// True if `owner` holds a live upload of `sha256` at `now_ms` (L1 of
    /// PR 4's review: a `PUT` of bytes the caller holds only extends their
    /// lifetime, and is not charged again).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn holds_upload(&self, owner: i64, sha256: &str, now_ms: i64) -> Result<bool, DbError> {
        self.read(|conn| {
            conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM upload WHERE owner = ?1 AND sha256 = ?2
                                AND expires_ms > ?3)",
                params![owner, sha256, now_ms],
                |r| r.get(0),
            )
        })
    }

    /// UP4: `owner`'s live uploads at `now_ms`.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn live_uploads(&self, owner: i64, now_ms: i64) -> Result<u64, DbError> {
        self.read(|conn| {
            conn.query_row(
                "SELECT count(*) FROM upload WHERE owner = ?1 AND expires_ms > ?2",
                params![owner, now_ms],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| u64::try_from(n).unwrap_or(0))
        })
    }

    /// The object `sha256` as `owner` may name it at `now_ms` (UP3), with
    /// what of theirs holds it; `None` for anything they may not name,
    /// another user's bytes among them.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn upload(
        &self,
        owner: i64,
        sha256: &str,
        now_ms: i64,
    ) -> Result<Option<Holding>, DbError> {
        self.read(|conn| nameable(conn, owner, sha256, now_ms))
    }

    /// A page of `owner`'s live uploads by SHA-256 (VN3): at most `limit`,
    /// after `after` (empty for the first page).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn uploads_page(
        &self,
        owner: i64,
        after: &str,
        limit: u32,
        now_ms: i64,
    ) -> Result<Vec<Holding>, DbError> {
        self.read(|conn| {
            let mut statement = conn.prepare(
                "SELECT sha256 FROM upload WHERE owner = ?1 AND expires_ms > ?2 AND sha256 > ?3
                 ORDER BY sha256 LIMIT ?4",
            )?;
            let shas = statement
                .query_map(params![owner, now_ms, after, limit], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut out = Vec::with_capacity(shas.len());
            for sha in shas {
                if let Some(holding) = nameable(conn, owner, &sha, now_ms)? {
                    out.push(holding);
                }
            }
            Ok(out)
        })
    }

    /// UP4: what `owner`'s holdings at `now_ms` count against their quota,
    /// each object once and charged as [`charge`] says: their unexpired
    /// uploads, and the inputs and documents of their retained jobs (OB4).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn charged_bytes(&self, owner: i64, now_ms: i64) -> Result<u64, DbError> {
        self.read(|conn| {
            conn.query_row(
                "SELECT coalesce(sum(max(bytes, ?3) + ?4), 0) FROM object WHERE sha256 IN (
                   SELECT sha256 FROM upload WHERE owner = ?1 AND expires_ms > ?2
                   UNION SELECT i.sha256 FROM job_input i JOIN job j ON j.key = i.job
                         WHERE j.owner = ?1
                   UNION SELECT result_sha256 FROM job WHERE owner = ?1
                         AND result_sha256 IS NOT NULL
                   UNION SELECT index_sha256 FROM job WHERE owner = ?1
                         AND index_sha256 IS NOT NULL)",
                params![
                    owner,
                    now_ms,
                    i64::try_from(CHARGE_MIN).unwrap_or(0),
                    i64::try_from(CHARGE_ROW).unwrap_or(0)
                ],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| u64::try_from(n).unwrap_or(0))
        })
    }

    /// `DELETE /v1/uploads/sha256:{hex}` (RA6, UP5, UP3): `owner`'s live
    /// upload row deleted unless a job of theirs that is not terminal names
    /// the object. Only the row: the object, if nothing else holds it, is
    /// the sweep's to unlink (PR 4's review), so a delete's time never
    /// tells whether another user holds the bytes.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn delete_upload(&self, owner: i64, sha256: &str, now_ms: i64) -> Result<Deleted, DbError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let live: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM upload WHERE owner = ?1 AND sha256 = ?2
                            AND expires_ms > ?3)",
            params![owner, sha256, now_ms],
            |r| r.get(0),
        )?;
        if !live {
            return Ok(Deleted::NotFound);
        }
        let in_use: bool = tx.query_row(
            &format!(
                "SELECT EXISTS (SELECT 1 FROM job_input i JOIN job j ON j.key = i.job
                   WHERE j.owner = ?1 AND i.sha256 = ?2 AND j.state IN {LIVE_JOB})"
            ),
            params![owner, sha256],
            |r| r.get(0),
        )?;
        if in_use {
            return Ok(Deleted::InUse);
        }
        tx.execute(
            "DELETE FROM upload WHERE owner = ?1 AND sha256 = ?2",
            params![owner, sha256],
        )?;
        tx.commit()?;
        Ok(Deleted::Gone)
    }

    /// JS9's first step for uploads (UP5): deletes up to `batch` upload
    /// rows expired at `now_ms`; returns how many.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn expire_uploads(&self, now_ms: i64, batch: u32) -> Result<usize, DbError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let deleted = tx.execute(
            "DELETE FROM upload WHERE key IN (
               SELECT key FROM upload WHERE expires_ms <= ?1 ORDER BY expires_ms LIMIT ?2)",
            params![now_ms, batch],
        )?;
        tx.commit()?;
        Ok(deleted)
    }

    /// OB2, OB4: up to `batch` of the candidates (objects whose naming
    /// row went, `object_candidate`), each looked at in one `BEGIN
    /// IMMEDIATE`: one no row names is unlinked by `unlink` and its row
    /// deleted (ST5); one a row names is no candidate any more. Returns
    /// those unlinked, those whose unlink failed (left, a candidate still,
    /// for the next sweep) and how many candidates were looked at.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn unlink_unnamed(
        &self,
        batch: u32,
        mut unlink: impl FnMut(&str) -> std::io::Result<()>,
    ) -> Result<Unnamed, DbError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidates: Vec<String> = {
            let mut statement =
                tx.prepare("SELECT sha256 FROM object_candidate ORDER BY sha256 LIMIT ?1")?;
            statement
                .query_map([batch], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut out = Unnamed {
            looked: candidates.len(),
            ..Unnamed::default()
        };
        let named_sql = format!("SELECT {NAMED}");
        for sha in candidates {
            let row: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM object WHERE sha256 = ?1)",
                [&sha],
                |r| r.get(0),
            )?;
            let named: bool = tx.query_row(&named_sql, [&sha], |r| r.get(0))?;
            if row && !named {
                if unlink(&sha).is_err() {
                    out.failed.push(sha);
                    continue;
                }
                tx.execute("DELETE FROM object WHERE sha256 = ?1", [&sha])?;
                out.gone.push(sha.clone());
            }
            tx.execute("DELETE FROM object_candidate WHERE sha256 = ?1", [&sha])?;
        }
        tx.commit()?;
        Ok(out)
    }

    /// OB2, OB4: of `names` (files found in the store), each with no
    /// object row is unlinked by `unlink`, inside one `BEGIN IMMEDIATE`
    /// that has just found no row (ST5): a crash between an upload's rename
    /// and its commit leaves such a file. Returns how many were unlinked.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn unlink_orphans(
        &self,
        names: &[String],
        mut unlink: impl FnMut(&str) -> std::io::Result<()>,
    ) -> Result<usize, DbError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut unlinked = 0;
        for name in names {
            let row: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM object WHERE sha256 = ?1)",
                [name],
                |r| r.get(0),
            )?;
            if !row && unlink(name).is_ok() {
                unlinked += 1;
            }
        }
        // Nothing was written: the transaction held the lock only.
        tx.rollback()?;
        Ok(unlinked)
    }

    /// The length of the object `sha256`, if it has a row (OB3's read).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn object_bytes(&self, sha256: &str) -> Result<Option<u64>, DbError> {
        self.read(|conn| {
            conn.query_row(
                "SELECT bytes FROM object WHERE sha256 = ?1",
                [sha256],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map(|b| b.map(|b| u64::try_from(b).unwrap_or(0)))
        })
    }

    /// The bytes of every object, summed (OP6's `fitment_state_object_bytes`).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn object_total(&self) -> Result<u64, DbError> {
        self.read(|conn| {
            conn.query_row("SELECT coalesce(sum(bytes), 0) FROM object", [], |r| {
                r.get::<_, i64>(0)
            })
            .map(|b| u64::try_from(b).unwrap_or(0))
        })
    }

    /// Every object row, with its length: for the tests' and the sweep's
    /// checks that each row's file stands (OB2).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn objects(&self) -> Result<Vec<(String, u64)>, DbError> {
        self.read(|conn| {
            let mut statement = conn.prepare("SELECT sha256, bytes FROM object ORDER BY sha256")?;
            statement
                .query_map([], |r| {
                    let bytes: i64 = r.get(1)?;
                    Ok((r.get(0)?, u64::try_from(bytes).unwrap_or(0)))
                })?
                .collect()
        })
    }
}
