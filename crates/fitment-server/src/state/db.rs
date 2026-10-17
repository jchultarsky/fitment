//! `server.db` (docs/research/m6-api.md, section 6.2, OB1): the server's
//! own SQLite database, the catalog's kind of database. The only module
//! that names rusqlite (SV6); no SQL here names a catalog table.

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, OpenFlags, OptionalExtension as _, TransactionBehavior};

/// `application_id`: `0x666D7453`, 'fmtS' (OB1).
pub const APPLICATION_ID: i64 = 0x666D_7453;

/// `user_version`: the schema this build writes.
pub const SCHEMA_VERSION: i64 = 1;

/// The database's file in the state directory (section 6.1).
pub const DATABASE: &str = "server.db";

/// Readers in the pool (OB1).
pub const READERS: usize = 8;

/// A lease renewed within this long is another server's (SD6).
pub const LEASE_STALE_MS: i64 = 30_000;

/// Schema 1 (section 6.2), as one batch.
const SCHEMA: &str = include_str!("schema.sql");

/// What a `server.db` is, by its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A new, empty database: created here.
    Empty,
    /// This build's schema.
    Current,
    /// An older schema of ours: migrated at start, holding the lease (SD3).
    Older(i64),
    /// A newer schema: refused (exit 3).
    Newer(i64),
    /// Another application's database: refused (exit 2).
    Foreign,
}

/// Why `server.db` cannot be used.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DbError {
    /// Another application's database (exit 2).
    #[error("server.db belongs to another application")]
    Foreign,
    /// A newer schema (exit 3).
    #[error(
        "server.db was made by a newer fitment-server (schema {found}; this build reads {ours})"
    )]
    Newer {
        /// Its schema.
        found: i64,
        /// This build's.
        ours: i64,
    },
    /// Another server holds the lease (SD6, exit 2).
    #[error("another fitment-server holds this state directory")]
    LeaseHeld,
    /// SQLite failed.
    #[error("server.db: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// The per-connection settings (OB1): WAL (checked), full syncs, foreign
/// keys, no trusted schema, the busy timeout; `fullfsync` on Apple (ST9).
fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(std::time::Duration::from_millis(5_000))?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(rusqlite::Error::InvalidParameterName(
            "journal_mode is not wal: the state directory must be on a local file system".into(),
        ));
    }
    conn.execute_batch(
        "PRAGMA synchronous = FULL;
         PRAGMA foreign_keys = ON;
         PRAGMA trusted_schema = OFF;",
    )?;
    if cfg!(target_vendor = "apple") {
        conn.execute_batch("PRAGMA fullfsync = ON; PRAGMA checkpoint_fullfsync = ON;")?;
    }
    Ok(())
}

/// What the database at `conn` is.
fn kind(conn: &Connection) -> rusqlite::Result<Kind> {
    let id: i64 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let objects: i64 = conn.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?;
    Ok(match (id, version) {
        (0, 0) if objects == 0 => Kind::Empty,
        (APPLICATION_ID, SCHEMA_VERSION) => Kind::Current,
        (APPLICATION_ID, v) if v > SCHEMA_VERSION => Kind::Newer(v),
        (APPLICATION_ID, v) if v >= 1 => Kind::Older(v),
        _ => Kind::Foreign,
    })
}

/// Reads the header of the `server.db` at `path` without changing it
/// (`check-config`, OP4): its kind, or `None` when there is no file yet.
///
/// # Errors
///
/// SQLite's.
pub fn peek(path: &Path) -> Result<Option<Kind>, DbError> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok(Some(kind(&conn)?))
}

/// `server.db`: one writer and a pool of readers (OB1).
pub struct Db {
    writer: Mutex<Connection>,
    readers: Mutex<Vec<Connection>>,
    path: std::path::PathBuf,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db").finish_non_exhaustive()
    }
}

/// A lock that a panic elsewhere did not leave unusable: the connection
/// holds no state a panic could half-change outside a transaction.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Db {
    /// Opens `server.db` in `dir`, creating schema 1 and the `system` user
    /// when it is new (OB1, M6-56). A foreign or newer database is refused
    /// before anything is changed in it.
    ///
    /// # Errors
    ///
    /// [`DbError::Foreign`], [`DbError::Newer`], or SQLite's.
    pub fn open(dir: &Path, system_id: &str, now_ms: i64) -> Result<(Self, Kind), DbError> {
        let path = dir.join(DATABASE);
        let mut conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let found = kind(&conn)?;
        match found {
            Kind::Foreign => return Err(DbError::Foreign),
            Kind::Newer(found) => {
                return Err(DbError::Newer {
                    found,
                    ours: SCHEMA_VERSION,
                });
            }
            _ => {}
        }
        configure(&conn)?;
        if found == Kind::Empty {
            create(&mut conn, system_id, now_ms)?;
        }
        let readers = (0..READERS)
            .map(|_| -> rusqlite::Result<Connection> {
                let reader = Connection::open_with_flags(
                    &path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                reader.busy_timeout(std::time::Duration::from_millis(5_000))?;
                reader.execute_batch("PRAGMA trusted_schema = OFF;")?;
                Ok(reader)
            })
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((
            Self {
                writer: Mutex::new(conn),
                readers: Mutex::new(readers),
                path,
            },
            found,
        ))
    }

    /// Runs `read` on a pooled reader, or on a fresh one if the pool is
    /// empty (OB1).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn read<T>(
        &self,
        read: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, DbError> {
        let pooled = lock(&self.readers).pop();
        let conn = match pooled {
            Some(conn) => conn,
            None => Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?,
        };
        let result = read(&conn);
        lock(&self.readers).push(conn);
        Ok(result?)
    }

    /// Takes the lease for `holder` (SD6): refused if another holder
    /// renewed it within [`LEASE_STALE_MS`].
    ///
    /// # Errors
    ///
    /// [`DbError::LeaseHeld`], or SQLite's.
    pub fn take_lease(&self, holder: &str, now_ms: i64) -> Result<(), DbError> {
        let mut conn = lock(&self.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<(String, i64)> = tx
            .query_row(
                "SELECT holder, heartbeat_ms FROM lease WHERE one = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((other, beat)) = current {
            if other != holder && now_ms.saturating_sub(beat) < LEASE_STALE_MS {
                return Err(DbError::LeaseHeld);
            }
        }
        tx.execute(
            "INSERT INTO lease (one, holder, heartbeat_ms) VALUES (1, ?1, ?2)
             ON CONFLICT (one) DO UPDATE SET holder = excluded.holder, heartbeat_ms = excluded.heartbeat_ms",
            rusqlite::params![holder, now_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Renews the lease (SD6): false if another holder has taken it.
    ///
    /// # Errors
    ///
    /// SQLite's: the database is not writable.
    pub fn renew_lease(&self, holder: &str, now_ms: i64) -> Result<bool, DbError> {
        let conn = lock(&self.writer);
        let changed = conn.execute(
            "UPDATE lease SET heartbeat_ms = ?2 WHERE one = 1 AND holder = ?1",
            rusqlite::params![holder, now_ms],
        )?;
        Ok(changed == 1)
    }

    /// Gives the lease up at a graceful stop, so the next start need not
    /// wait for it to go stale.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn release_lease(&self, holder: &str) -> Result<(), DbError> {
        let conn = lock(&self.writer);
        conn.execute("DELETE FROM lease WHERE one = 1 AND holder = ?1", [holder])?;
        Ok(())
    }

    /// True if the lease names `holder` (OP7's readiness).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn holds_lease(&self, holder: &str) -> Result<bool, DbError> {
        self.read(|conn| {
            conn.query_row(
                "SELECT count(*) FROM lease WHERE one = 1 AND holder = ?1",
                [holder],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n == 1)
        })
    }

    /// The `system` user's id (M6-56).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn system_user(&self) -> Result<String, DbError> {
        self.read(|conn| {
            conn.query_row("SELECT id FROM user WHERE kind = 'system'", [], |r| {
                r.get(0)
            })
        })
    }
}

/// Schema 1 and the `system` user, in one exclusive transaction, the
/// header set last inside it (OB1, M6-56).
fn create(conn: &mut Connection, system_id: &str, now_ms: i64) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Exclusive)?;
    // A second process may have created it between the header's read and
    // the lock: then there is nothing to do.
    if kind(&tx)? != Kind::Empty {
        return tx.commit();
    }
    tx.execute_batch(SCHEMA)?;
    tx.execute(
        "INSERT INTO user (id, kind, username_fold, username, password_phc, role, created_ms, updated_ms)
         VALUES (?1, 'system', 'system', 'system', NULL, 'admin', ?2, ?2)",
        rusqlite::params![system_id, now_ms],
    )?;
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('created_by', ?1)",
        [concat!("fitment-server ", env!("CARGO_PKG_VERSION"))],
    )?;
    tx.pragma_update(None, "application_id", APPLICATION_ID)?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()
}
