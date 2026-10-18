//! Uploads and objects in `server.db` (docs/research/m6-api.md, section
//! 6.2, UP3 to UP5, UP8, OB2, OB4): part of `state::db`, the one module
//! that names SQLite (SV6). An object row promises a file hashing to its
//! name (OB2); an upload row is a user's access to it (UP3). Every write
//! is one `BEGIN IMMEDIATE`, and the file calls that make an object or
//! unlink one run inside it, so a sweep never unlinks between an upload's
//! check and its row.

use rusqlite::{Connection, OptionalExtension as _, Row, TransactionBehavior, params};

use super::{Db, DbError, lock};

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

/// How a delete ended (RA6, UP5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deleted {
    /// The user's row is gone, and the object with it if nothing else held
    /// it.
    Gone,
    /// A job that is not terminal names it: `409 upload_in_use`.
    InUse,
    /// The user holds no live upload of it: `404`.
    NotFound,
}

/// The states of a job that is not terminal (JB3).
const LIVE_JOB: &str = "('queued', 'running', 'cancelling')";

/// True if a row of any table names the object `?1` (OB4): every table
/// with a foreign key to `object`, so an object row is deleted only when
/// nothing could refer to it.
const NAMED: &str = "EXISTS (SELECT 1 FROM upload WHERE sha256 = ?1)
    OR EXISTS (SELECT 1 FROM assembly WHERE sha256 = ?1)
    OR EXISTS (SELECT 1 FROM assembly_parts WHERE document_sha256 = ?1)
    OR EXISTS (SELECT 1 FROM socket_revision WHERE document_sha256 = ?1)
    OR EXISTS (SELECT 1 FROM job_input WHERE sha256 = ?1)
    OR EXISTS (SELECT 1 FROM job WHERE result_sha256 = ?1 OR index_sha256 = ?1)";

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
    /// synced, in one `BEGIN IMMEDIATE`: the object's row looked for,
    /// `place` called with whether it exists (it renames the file into
    /// place or removes it), then the object's row inserted if new and the
    /// user's upload row inserted or its lifetime extended to
    /// `expires_ms`. A sweep cannot unlink the object between the look and
    /// the rows (OB2).
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
        place: impl FnOnce(bool) -> std::io::Result<()>,
    ) -> Result<Stored, StoreError> {
        let size = i64::try_from(bytes).unwrap_or(i64::MAX);
        let mut conn = lock(&self.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM object WHERE sha256 = ?1)",
            [sha256],
            |r| r.get(0),
        )?;
        place(row).map_err(StoreError::Place)?;
        if !row {
            tx.execute(
                "INSERT INTO object (sha256, bytes, added_ms) VALUES (?1, ?2, ?3)",
                params![sha256, size, now_ms],
            )?;
        }
        let prior: Option<i64> = tx
            .query_row(
                "SELECT expires_ms FROM upload WHERE owner = ?1 AND sha256 = ?2",
                params![owner, sha256],
                |r| r.get(0),
            )
            .optional()?;
        let stored = if let Some(expires) = prior {
            tx.execute(
                "UPDATE upload SET expires_ms = max(expires_ms, ?3),
                   created_ms = CASE WHEN expires_ms > ?4 THEN created_ms ELSE ?4 END
                 WHERE owner = ?1 AND sha256 = ?2",
                params![owner, sha256, expires_ms, now_ms],
            )?;
            if expires > now_ms {
                Stored::Refreshed
            } else {
                Stored::New
            }
        } else {
            tx.execute(
                "INSERT INTO upload (owner, sha256, created_ms, expires_ms)
                 VALUES (?1, ?2, ?3, ?4)",
                params![owner, sha256, now_ms, expires_ms],
            )?;
            Stored::New
        };
        tx.commit()?;
        Ok(stored)
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

    /// UP4: the bytes `owner` holds at `now_ms`, each object once: their
    /// unexpired uploads, and the inputs and documents of their retained
    /// jobs (OB4).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn held_bytes(&self, owner: i64, now_ms: i64) -> Result<u64, DbError> {
        self.read(|conn| {
            conn.query_row(
                "SELECT coalesce(sum(bytes), 0) FROM object WHERE sha256 IN (
                   SELECT sha256 FROM upload WHERE owner = ?1 AND expires_ms > ?2
                   UNION SELECT i.sha256 FROM job_input i JOIN job j ON j.key = i.job
                         WHERE j.owner = ?1
                   UNION SELECT result_sha256 FROM job WHERE owner = ?1
                         AND result_sha256 IS NOT NULL
                   UNION SELECT index_sha256 FROM job WHERE owner = ?1
                         AND index_sha256 IS NOT NULL)",
                params![owner, now_ms],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| u64::try_from(n).unwrap_or(0))
        })
    }

    /// `DELETE /v1/uploads/sha256:{hex}` (RA6, UP5, OB2): `owner`'s live
    /// upload row deleted unless a job of theirs that is not terminal names
    /// the object; then, if no row names it, `unlink` is called and its
    /// row deleted, in the same `BEGIN IMMEDIATE` (ST5).
    ///
    /// # Errors
    ///
    /// SQLite's; an unlink that fails leaves the object and its row, for
    /// the sweep.
    pub fn delete_upload(
        &self,
        owner: i64,
        sha256: &str,
        now_ms: i64,
        unlink: impl FnOnce() -> std::io::Result<()>,
    ) -> Result<Deleted, DbError> {
        let mut conn = lock(&self.writer);
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
        let named: bool = tx.query_row(&format!("SELECT {NAMED}"), [sha256], |r| r.get(0))?;
        if !named && unlink().is_ok() {
            tx.execute("DELETE FROM object WHERE sha256 = ?1", [sha256])?;
        }
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
        let mut conn = lock(&self.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let deleted = tx.execute(
            "DELETE FROM upload WHERE key IN (
               SELECT key FROM upload WHERE expires_ms <= ?1 ORDER BY key LIMIT ?2)",
            params![now_ms, batch],
        )?;
        tx.commit()?;
        Ok(deleted)
    }

    /// OB2, OB4: up to `batch` objects no row names, each unlinked by
    /// `unlink` and its row deleted, in one `BEGIN IMMEDIATE` that has
    /// just found no row naming it (ST5). Returns those unlinked and the
    /// names whose unlink failed (left, row and all, for the next sweep).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn unlink_unnamed(
        &self,
        batch: u32,
        mut unlink: impl FnMut(&str) -> std::io::Result<()>,
    ) -> Result<(Vec<String>, Vec<String>), DbError> {
        let mut conn = lock(&self.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let unnamed: Vec<String> = {
            let mut statement = tx.prepare(&format!(
                "SELECT o.sha256 FROM object o WHERE NOT ({})
                 ORDER BY o.sha256 LIMIT ?1",
                NAMED.replace("?1", "o.sha256")
            ))?;
            statement
                .query_map([batch], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let (mut gone, mut failed) = (Vec::new(), Vec::new());
        for sha in unnamed {
            match unlink(&sha) {
                Ok(()) => {
                    tx.execute("DELETE FROM object WHERE sha256 = ?1", [&sha])?;
                    gone.push(sha);
                }
                Err(_) => failed.push(sha),
            }
        }
        tx.commit()?;
        Ok((gone, failed))
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
        let mut conn = lock(&self.writer);
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
