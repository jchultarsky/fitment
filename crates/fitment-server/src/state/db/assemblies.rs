//! Assemblies, socket lines and revisions in `server.db` (docs/research/
//! m6-api.md, section 6.2, RA3 to RA6, OB4, OB5): part of `state::db`, the
//! one module that names SQLite (SV6). An assembly is a user's row over an
//! object, with its `fitment.parts` rows by build; a socket line is a
//! user's triple (assembly, target, occurrence) under a derived id, one
//! live row beside any tombstones, and its revisions are insert-only rows
//! numbered across the id's rows (RA4, RA5).

use rusqlite::{Connection, OptionalExtension as _, Row, Transaction, TransactionBehavior, params};

use super::jobs::{AssemblyRows, SocketRows};
use super::{Db, DbError};

/// A `fitment.parts` row of an assembly (RA3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartsRow {
    /// The build's versions it was read with.
    pub extractor_version: u32,
    #[allow(missing_docs)]
    pub socket_rules_version: u32,
    /// `accepted` or `refused`.
    pub status: String,
    /// The document's object and length.
    pub document_sha256: String,
    #[allow(missing_docs)]
    pub document_bytes: u64,
    /// The root definition, as JSON.
    pub root: Option<String>,
}

/// A user's assembly (RA3, `fitment.assembly`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyRow {
    /// `assembly.key`, internal.
    pub key: i64,
    /// Its bytes' SHA-256.
    pub sha256: String,
    /// Their length.
    pub bytes: u64,
    /// The `file_name`s it was registered under, sorted.
    pub file_names: Vec<String>,
    /// Its parts row of this build, else its latest.
    pub parts: Option<PartsRow>,
    /// The owner's live socket lines naming it.
    pub sockets: u64,
}

/// A socket revision (RA5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionRow {
    /// `socket_revision.key`, internal.
    pub key: i64,
    /// Its line's id, `sk_…`.
    pub line_id: String,
    /// Its number.
    pub n: i64,
    /// `extract`, `review` or `import`.
    pub how: String,
    /// RA10.
    pub verified: bool,
    /// Its parent's number, under the same id.
    pub parent: Option<(String, i64)>,
    /// Its document's object, length and digest.
    pub document_sha256: String,
    #[allow(missing_docs)]
    pub document_bytes: u64,
    #[allow(missing_docs)]
    pub document_digest: String,
    /// Its versions.
    pub extractor_version: u32,
    #[allow(missing_docs)]
    pub socket_rules_version: u32,
    /// Its counts.
    pub unreviewed: u64,
    #[allow(missing_docs)]
    pub carried: u64,
    #[allow(missing_docs)]
    pub stale: u64,
}

/// A live socket line (RA4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineRow {
    /// `socket_line.key`, internal.
    pub key: i64,
    /// `sk_…`.
    pub id: String,
    /// The triple.
    pub assembly_sha256: String,
    #[allow(missing_docs)]
    pub definition: String,
    #[allow(missing_docs)]
    pub occurrence: String,
    /// Its current revision.
    pub current: Option<RevisionRow>,
}

/// How a delete of an assembly ended (RA6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyDeleted {
    /// Its rows are gone; its objects the sweep's, if nothing else holds
    /// them.
    Gone,
    /// A live socket line of the owner names it: `409 assembly_in_use`.
    InUse,
    /// The owner holds no such assembly: `404`.
    NotFound,
}

/// What a socket line's revision became (JS13).
pub(super) enum Revised {
    /// The revision, made or found unchanged: its href.
    Made(String),
    /// Another revision committed since the job read the current one.
    Retry,
    /// Another triple holds the id (RA4).
    Collision,
    /// The owner's assembly is gone.
    Gone,
}

const REVISION: &str = "r.key, r.line_id, r.n, r.how, r.verified,
    (SELECT p.line_id FROM socket_revision p WHERE p.key = r.parent),
    (SELECT p.n FROM socket_revision p WHERE p.key = r.parent),
    r.document_sha256, (SELECT o.bytes FROM object o WHERE o.sha256 = r.document_sha256),
    r.document_digest, r.extractor_version, r.socket_rules_version, r.unreviewed, r.carried,
    r.stale";

fn count(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn version(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

fn revision_row(row: &Row<'_>) -> rusqlite::Result<RevisionRow> {
    let parent: (Option<String>, Option<i64>) = (row.get(5)?, row.get(6)?);
    let bytes: Option<i64> = row.get(8)?;
    Ok(RevisionRow {
        key: row.get(0)?,
        line_id: row.get(1)?,
        n: row.get(2)?,
        how: row.get(3)?,
        verified: row.get::<_, i64>(4)? != 0,
        parent: match parent {
            (Some(id), Some(n)) => Some((id, n)),
            _ => None,
        },
        document_sha256: row.get(7)?,
        document_bytes: count(bytes.unwrap_or(0)),
        document_digest: row.get(9)?,
        extractor_version: version(row.get(10)?),
        socket_rules_version: version(row.get(11)?),
        unreviewed: count(row.get(12)?),
        carried: count(row.get(13)?),
        stale: count(row.get(14)?),
    })
}

/// The current revision of the live line `line` (its highest number).
fn current(conn: &Connection, line: i64) -> rusqlite::Result<Option<RevisionRow>> {
    conn.query_row(
        &format!(
            "SELECT {REVISION} FROM socket_revision r WHERE r.line = ?1 ORDER BY r.n DESC LIMIT 1"
        ),
        [line],
        revision_row,
    )
    .optional()
}

fn line_row(conn: &Connection, row: &Row<'_>) -> rusqlite::Result<LineRow> {
    let key: i64 = row.get(0)?;
    Ok(LineRow {
        key,
        id: row.get(1)?,
        assembly_sha256: row.get(2)?,
        definition: row.get(3)?,
        occurrence: row.get(4)?,
        current: current(conn, key)?,
    })
}

const LINE: &str = "key, id, assembly_sha256, definition, occurrence";

/// The parts row of `assembly` for these versions, else its latest.
fn parts(
    conn: &Connection,
    assembly: i64,
    versions: (u32, u32),
) -> rusqlite::Result<Option<PartsRow>> {
    conn.query_row(
        "SELECT p.extractor_version, p.socket_rules_version, p.status, p.document_sha256,
                (SELECT o.bytes FROM object o WHERE o.sha256 = p.document_sha256), p.root
         FROM assembly_parts p WHERE p.assembly = ?1
         ORDER BY (p.extractor_version = ?2 AND p.socket_rules_version = ?3) DESC,
                  p.extractor_version DESC, p.socket_rules_version DESC LIMIT 1",
        params![assembly, versions.0, versions.1],
        |r| {
            let bytes: Option<i64> = r.get(4)?;
            Ok(PartsRow {
                extractor_version: version(r.get(0)?),
                socket_rules_version: version(r.get(1)?),
                status: r.get(2)?,
                document_sha256: r.get(3)?,
                document_bytes: count(bytes.unwrap_or(0)),
                root: r.get(5)?,
            })
        },
    )
    .optional()
}

fn assembly_row(
    conn: &Connection,
    owner: i64,
    key: i64,
    versions: (u32, u32),
) -> rusqlite::Result<AssemblyRow> {
    let (sha256, bytes): (String, i64) = conn.query_row(
        "SELECT a.sha256, o.bytes FROM assembly a JOIN object o ON o.sha256 = a.sha256
         WHERE a.key = ?1",
        [key],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let file_names = {
        let mut statement =
            conn.prepare("SELECT name FROM assembly_name WHERE assembly = ?1 ORDER BY name")?;
        statement
            .query_map([key], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?
    };
    let sockets: i64 = conn.query_row(
        "SELECT count(*) FROM socket_line WHERE owner = ?1 AND assembly_sha256 = ?2
         AND deleted_ms IS NULL",
        params![owner, sha256],
        |r| r.get(0),
    )?;
    Ok(AssemblyRow {
        key,
        sha256,
        bytes: count(bytes),
        file_names,
        parts: parts(conn, key, versions)?,
        sockets: count(sockets),
    })
}

/// The owner's live assembly of `sha256`, by key.
fn live_assembly(conn: &Connection, owner: i64, sha256: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT key FROM assembly WHERE owner = ?1 AND sha256 = ?2 AND deleted_ms IS NULL",
        params![owner, sha256],
        |r| r.get(0),
    )
    .optional()
}

/// RA3, JS13: the owner's assembly made or found, its name added, and its
/// parts row of this build inserted unless one is there (insert-only, DB7's
/// pattern), naming the job.
pub(super) fn register(
    tx: &Transaction<'_>,
    rows: &AssemblyRows<'_>,
    job: i64,
    now_ms: i64,
) -> rusqlite::Result<()> {
    let key = if let Some(key) = live_assembly(tx, rows.owner, rows.sha256)? {
        key
    } else {
        tx.execute(
            "INSERT INTO assembly (owner, sha256, created_ms) VALUES (?1, ?2, ?3)",
            params![rows.owner, rows.sha256, now_ms],
        )?;
        tx.last_insert_rowid()
    };
    if let Some(name) = rows.file_name {
        tx.execute(
            "INSERT OR IGNORE INTO assembly_name (assembly, name) VALUES (?1, ?2)",
            params![key, name],
        )?;
    }
    tx.execute(
        "INSERT OR IGNORE INTO assembly_parts (assembly, extractor_version, socket_rules_version,
                                               status, document_sha256, root, job)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            key,
            fitment_core::EXTRACTOR_VERSION,
            fitment_core::SOCKET_RULES_VERSION,
            rows.status,
            rows.document_sha256,
            rows.root,
            job
        ],
    )?;
    Ok(())
}

/// The next revision number of the owner's id (RA4): after every revision
/// any of its rows held, kept or swept.
fn next_n(tx: &Transaction<'_>, owner: i64, id: &str) -> rusqlite::Result<i64> {
    tx.query_row(
        "SELECT max(coalesce((SELECT max(n) FROM socket_revision WHERE owner = ?1 AND line_id = ?2), 0),
                    coalesce((SELECT max(last_n) FROM socket_line WHERE owner = ?1 AND id = ?2), 0)) + 1",
        params![owner, id],
        |r| r.get(0),
    )
}

/// RA4, RA5, JS13: the line's new revision, in the job's finishing
/// transaction; a revived line is a new live row under the same id, its
/// revisions numbered on from the id's last.
pub(super) fn revise(
    tx: &Transaction<'_>,
    rows: &SocketRows<'_>,
    job: i64,
    now_ms: i64,
) -> rusqlite::Result<Revised> {
    let live: Option<LineRow> = tx
        .query_row(
            &format!(
                "SELECT {LINE} FROM socket_line WHERE owner = ?1 AND id = ?2 AND deleted_ms IS NULL"
            ),
            params![rows.owner, rows.line_id],
            |r| line_row(tx, r),
        )
        .optional()?;
    if let Some(line) = &live {
        if (
            line.assembly_sha256.as_str(),
            line.definition.as_str(),
            line.occurrence.as_str(),
        ) != (rows.assembly_sha256, rows.definition, rows.occurrence)
        {
            return Ok(Revised::Collision);
        }
    }
    let now = live.as_ref().and_then(|l| l.current.as_ref());
    if now.map(|r| r.key) != rows.base {
        return Ok(Revised::Retry);
    }
    // Review S3 of PR 5, defence in depth: no revision of an assembly the
    // owner no longer holds.
    if live_assembly(tx, rows.owner, rows.assembly_sha256)?.is_none() {
        return Ok(Revised::Gone);
    }
    let Some(made) = &rows.revision else {
        return Ok(match now {
            Some(r) => Revised::Made(format!("/v1/sockets/{}@r{}", rows.line_id, r.n)),
            None => Revised::Retry,
        });
    };
    let row = if let Some(line) = &live {
        line.key
    } else {
        tx.execute(
            "INSERT INTO socket_line (owner, id, assembly_sha256, definition, occurrence, created_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                rows.owner,
                rows.line_id,
                rows.assembly_sha256,
                rows.definition,
                rows.occurrence,
                now_ms
            ],
        )?;
        tx.last_insert_rowid()
    };
    let n = next_n(tx, rows.owner, rows.line_id)?;
    let clamp = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
    let verified = i64::from(made.verified);
    tx.execute(
        "INSERT INTO socket_revision (line, owner, line_id, n, how, verified, parent,
            document_sha256, document_digest, extractor_version, socket_rules_version,
            unreviewed, carried, stale, job, created_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            row,
            rows.owner,
            rows.line_id,
            n,
            made.how,
            verified,
            made.parent,
            made.document_sha256,
            made.document_digest,
            made.extractor_version,
            made.socket_rules_version,
            clamp(made.unreviewed),
            clamp(made.carried),
            clamp(made.stale),
            job,
            now_ms
        ],
    )?;
    Ok(Revised::Made(format!("/v1/sockets/{}@r{n}", rows.line_id)))
}

impl Db {
    /// The owner's live assembly of `sha256` (RA3), its parts row of these
    /// versions or else its latest.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn assembly(
        &self,
        owner: i64,
        sha256: &str,
        versions: (u32, u32),
    ) -> Result<Option<AssemblyRow>, DbError> {
        self.read(|conn| {
            live_assembly(conn, owner, sha256)?
                .map(|key| assembly_row(conn, owner, key, versions))
                .transpose()
        })
    }

    /// A page of the owner's live assemblies by SHA-256 (VN3).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn assemblies_page(
        &self,
        owner: i64,
        after: &str,
        limit: u32,
        versions: (u32, u32),
    ) -> Result<Vec<AssemblyRow>, DbError> {
        self.read(|conn| {
            let keys: Vec<i64> = {
                let mut statement = conn.prepare(
                    "SELECT key FROM assembly WHERE owner = ?1 AND deleted_ms IS NULL AND sha256 > ?2
                     ORDER BY sha256 LIMIT ?3",
                )?;
                statement
                    .query_map(params![owner, after, limit], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            keys.into_iter()
                .map(|key| assembly_row(conn, owner, key, versions))
                .collect()
        })
    }

    /// RA3: adds `name` to the owner's live assembly of `sha256`, which is
    /// already current at this build (`POST /v1/assemblies` answering
    /// without a job). False if it is not there any more.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn name_assembly(&self, owner: i64, sha256: &str, name: &str) -> Result<bool, DbError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(key) = live_assembly(&tx, owner, sha256)? else {
            return Ok(false);
        };
        tx.execute(
            "INSERT OR IGNORE INTO assembly_name (assembly, name) VALUES (?1, ?2)",
            params![key, name],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// RA6: deletes the owner's assembly of `sha256` and its rows, unless a
    /// live socket line of theirs names it; its objects become the sweep's
    /// candidates (OB4).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn delete_assembly(&self, owner: i64, sha256: &str) -> Result<AssemblyDeleted, DbError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(key) = live_assembly(&tx, owner, sha256)? else {
            return Ok(AssemblyDeleted::NotFound);
        };
        // RA6, and review S3 of PR 5: a live line, or a job of the owner's
        // that has not ended and names the bytes (an extraction queued on
        // it), keeps the assembly.
        let used: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM socket_line WHERE owner = ?1 AND assembly_sha256 = ?2
                            AND deleted_ms IS NULL)
                 OR EXISTS (SELECT 1 FROM job_input i JOIN job j ON j.key = i.job
                            WHERE j.owner = ?1 AND i.sha256 = ?2
                              AND j.state IN ('queued', 'running', 'cancelling'))",
            params![owner, sha256],
            |r| r.get(0),
        )?;
        if used {
            return Ok(AssemblyDeleted::InUse);
        }
        tx.execute("DELETE FROM assembly_parts WHERE assembly = ?1", [key])?;
        tx.execute("DELETE FROM assembly_name WHERE assembly = ?1", [key])?;
        tx.execute("DELETE FROM assembly WHERE key = ?1", [key])?;
        tx.commit()?;
        Ok(AssemblyDeleted::Gone)
    }

    /// The owner's live line `id` (RA4), with its current revision.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn line(&self, owner: i64, id: &str) -> Result<Option<LineRow>, DbError> {
        self.read(|conn| {
            conn.query_row(
                &format!("SELECT {LINE} FROM socket_line WHERE owner = ?1 AND id = ?2 AND deleted_ms IS NULL"),
                params![owner, id],
                |r| line_row(conn, r),
            )
            .optional()
        })
    }

    /// The owner's revision `n` of `id`, across the id's rows, kept or
    /// tombstoned (RA4, RA6), or for `None` the live line's current one.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn revision(
        &self,
        owner: i64,
        id: &str,
        n: Option<i64>,
    ) -> Result<Option<RevisionRow>, DbError> {
        let Some(n) = n else {
            return Ok(self.line(owner, id)?.and_then(|line| line.current));
        };
        self.read(|conn| {
            conn.query_row(
                &format!("SELECT {REVISION} FROM socket_revision r WHERE r.owner = ?1 AND r.line_id = ?2 AND r.n = ?3"),
                params![owner, id, n],
                revision_row,
            )
            .optional()
        })
    }

    /// A page of the owner's live lines by id (VN3), of one assembly if
    /// given.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn lines_page(
        &self,
        owner: i64,
        assembly: Option<&str>,
        after: &str,
        limit: u32,
    ) -> Result<Vec<LineRow>, DbError> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {LINE} FROM socket_line WHERE owner = ?1 AND deleted_ms IS NULL
                   AND (?2 IS NULL OR assembly_sha256 = ?2) AND id > ?3 ORDER BY id LIMIT ?4"
            ))?;
            statement
                .query_map(params![owner, assembly, after, limit], |r| {
                    line_row(conn, r)
                })?
                .collect()
        })
    }

    /// A page of the revisions of the owner's id by number (VN3), after
    /// `after`.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn revisions_page(
        &self,
        owner: i64,
        id: &str,
        after: i64,
        limit: u32,
    ) -> Result<Vec<RevisionRow>, DbError> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {REVISION} FROM socket_revision r WHERE r.owner = ?1 AND r.line_id = ?2
                   AND r.n > ?3 ORDER BY r.n LIMIT ?4"
            ))?;
            statement
                .query_map(params![owner, id, after, limit], revision_row)?
                .collect()
        })
    }

    /// RA6: the owner's live line `id` becomes a tombstone, its highest
    /// revision kept with it (RA4's numbering). False if none is live.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn delete_line(&self, owner: i64, id: &str, now_ms: i64) -> Result<bool, DbError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE socket_line SET deleted_ms = ?3,
               last_n = coalesce((SELECT max(n) FROM socket_revision WHERE line = socket_line.key), 0)
             WHERE owner = ?1 AND id = ?2 AND deleted_ms IS NULL",
            params![owner, id, now_ms],
        )?;
        tx.commit()?;
        Ok(changed == 1)
    }

    /// OB4, JS9: deletes up to `batch` revisions of tombstoned lines that
    /// no job cites (`job_cites`) and no revision names as its parent, so
    /// their objects become the sweep's candidates; returns how many.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn sweep_revisions(&self, batch: u32) -> Result<usize, DbError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let deleted = tx.execute(
            "DELETE FROM socket_revision WHERE key IN (
               SELECT r.key FROM socket_revision r JOIN socket_line l ON l.key = r.line
               WHERE l.deleted_ms IS NOT NULL
                 AND NOT EXISTS (SELECT 1 FROM job_cites c WHERE c.revision = r.key)
                 AND NOT EXISTS (SELECT 1 FROM socket_revision c WHERE c.parent = r.key)
               LIMIT ?1)",
            [batch],
        )?;
        tx.commit()?;
        Ok(deleted)
    }
}
