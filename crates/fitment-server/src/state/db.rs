//! `server.db` (docs/research/m6-api.md, section 6.2, OB1): the server's
//! own SQLite database, the catalog's kind of database. The only module
//! that names rusqlite (SV6), with its submodules `auth` for users and
//! tokens, `uploads` for uploads and objects, `jobs` for jobs and their
//! events, and `assemblies` for assemblies, socket lines and revisions; no
//! SQL here names a catalog table.

mod assemblies;
mod auth;
mod jobs;
mod uploads;

pub use assemblies::{AssemblyDeleted, AssemblyRow, LineRow, PartsRow, RevisionRow};
pub use auth::{
    CreateUserError, MintError, MintFrom, NewToken, PasswordSet, Presented, SWEEP_BATCH, TokenRow,
    UpdateUserError, UserRow,
};
pub use jobs::{
    AssemblyRows, Cancelled, End, EventRow, Finished, JobRow, NewJob, NewRevision, Recovered, Rows,
    SocketRows, Stopped, Submitted,
};
pub use uploads::{CHARGE_MIN, CHARGE_ROW, Deleted, Holding, StoreError, Stored, Unnamed, charge};

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
    /// An older schema while a server holds the lease: the offline
    /// commands never migrate beside it (AU9, exit 2).
    #[error(
        "server.db is of an older schema and a fitment-server holds the state directory: \
         the server migrates it at its start"
    )]
    MigrateBesideServer,
    /// The file cannot be made (exit 1).
    #[error("server.db cannot be made: {0}")]
    Create(std::io::ErrorKind),
    /// SQLite failed.
    #[error("server.db: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// The per-connection settings (OB1): WAL (checked), full syncs, foreign
/// keys, no trusted schema, the busy timeout; `fullfsync` on Apple (ST9).
fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(std::time::Duration::from_millis(5_000))?;
    let mode = journal_mode_wal(conn)?;
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

/// Switches to WAL and answers the mode in force. SQLite answers a
/// switch that meets another connection's lock with `SQLITE_BUSY` at once,
/// without its busy handler, as concurrent first opens do (review: eight
/// bootstraps on a new state), so it is retried for up to the busy
/// timeout.
fn journal_mode_wal(conn: &Connection) -> rusqlite::Result<String> {
    let started = std::time::Instant::now();
    loop {
        match conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0)) {
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::DatabaseBusy
                    && started.elapsed() < std::time::Duration::from_millis(5_000) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            other => return other,
        }
    }
}

/// What a database with these header fields is: `objects`, whether its
/// schema holds anything.
fn kind_of(id: i64, version: i64, objects: bool) -> Kind {
    match (id, version) {
        (0, 0) if !objects => Kind::Empty,
        (APPLICATION_ID, SCHEMA_VERSION) => Kind::Current,
        (APPLICATION_ID, v) if v > SCHEMA_VERSION => Kind::Newer(v),
        (APPLICATION_ID, v) if v >= 1 => Kind::Older(v),
        _ => Kind::Foreign,
    }
}

/// What the database at `conn` is, its three fields read by one
/// statement, so from one snapshot: a concurrent first start committing
/// between separate reads would give its tables without its header, which
/// reads as foreign (CI).
fn kind(conn: &Connection) -> rusqlite::Result<Kind> {
    let (id, version, objects): (i64, i64, i64) = conn.query_row(
        "SELECT (SELECT application_id FROM pragma_application_id()),
                (SELECT user_version FROM pragma_user_version()),
                (SELECT count(*) FROM sqlite_schema)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(kind_of(id, version, objects > 0))
}

/// What the `server.db` at `path` is, from its file's first 100 bytes
/// alone, with no connection, so nothing is written (`check-config`,
/// OP4): `None` when there is no file, [`Kind::Empty`] for an empty file.
/// The fields are SQLite's header's, big-endian: the schema cookie at 40,
/// `user_version` at 60, `application_id` at 68. `open` checkpoints after
/// creating the schema, so the file's header is current.
///
/// # Errors
///
/// The file's.
pub fn header_kind(path: &Path) -> std::io::Result<Option<Kind>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut header = Vec::with_capacity(100);
    std::io::Read::read_to_end(&mut std::io::Read::take(file, 100), &mut header)?;
    if header.is_empty() {
        return Ok(Some(Kind::Empty));
    }
    let field = |at: usize| -> Option<i64> {
        let bytes: [u8; 4] = header.get(at..at + 4)?.try_into().ok()?;
        Some(i64::from(i32::from_be_bytes(bytes)))
    };
    if !header.starts_with(b"SQLite format 3\0") {
        return Ok(Some(Kind::Foreign));
    }
    Ok(Some(match (field(40), field(60), field(68)) {
        (Some(cookie), Some(version), Some(id)) => kind_of(id, version, cookie != 0),
        _ => Kind::Foreign,
    }))
}

/// A reader connection: read-only, with the busy timeout and without
/// trusted schema (OB1), however it is opened.
fn open_reader(path: &Path) -> rusqlite::Result<Connection> {
    let reader = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    reader.busy_timeout(std::time::Duration::from_millis(5_000))?;
    reader.execute_batch("PRAGMA trusted_schema = OFF;")?;
    Ok(reader)
}

/// Makes `server.db` empty, before SQLite does, with mode 0600 on Unix, so
/// its `-wal` and `-shm`, which SQLite makes with the database's mode,
/// are the owner's alone (S13 of PR 2's review).
fn create_file(path: &Path) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    match options.open(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// `server.db`: one writer and a pool of readers (OB1).
pub struct Db {
    writer: Mutex<Connection>,
    readers: Mutex<Vec<Connection>>,
    path: std::path::PathBuf,
    /// [`LEASE_STALE_MS`]; a `test-support` seam shortens it for a start
    /// after a kill (VT10).
    lease_stale_ms: std::sync::atomic::AtomicI64,
    /// Callers waiting for the writer, so a long maintenance pass can step
    /// aside between its batches ([`Db::yield_writer`]): `std`'s mutex is
    /// not fair, and a loop that unlocks and locks again at once can keep
    /// it from a waiting request for the whole pass.
    writers_waiting: std::sync::atomic::AtomicUsize,
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
    /// The writer, counted among [`Db::yield_writer`]'s waiters until it
    /// is held.
    fn writer(&self) -> MutexGuard<'_, Connection> {
        use std::sync::atomic::Ordering;
        self.writers_waiting.fetch_add(1, Ordering::SeqCst);
        let guard = lock(&self.writer);
        self.writers_waiting.fetch_sub(1, Ordering::SeqCst);
        guard
    }

    /// Between a maintenance pass's batches (JS9's sweep, ST5): while
    /// another caller waits for the writer, waits for it to take it, for
    /// at most 100 ms a batch, so a request waits at most one batch.
    pub fn yield_writer(&self) {
        use std::sync::atomic::Ordering;
        let until = std::time::Instant::now() + std::time::Duration::from_millis(100);
        while self.writers_waiting.load(Ordering::SeqCst) > 0 && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Opens `server.db` in `dir`, creating schema 1 and the `system` user
    /// when it is new (OB1, M6-56). A foreign or newer database is refused
    /// before anything is changed in it.
    ///
    /// # Errors
    ///
    /// [`DbError::Foreign`], [`DbError::Newer`], or SQLite's.
    pub fn open(dir: &Path, system_id: &str, now_ms: i64) -> Result<(Self, Kind), DbError> {
        Self::open_as(dir, system_id, now_ms, false)
    }

    /// [`Db::open`] for AU9's offline commands, which take no lease: an
    /// older schema is refused while a server holds the lease, so nothing
    /// migrates beside it ([`may_migrate`]).
    ///
    /// # Errors
    ///
    /// As [`Db::open`], and [`DbError::MigrateBesideServer`].
    pub fn open_offline(dir: &Path, system_id: &str, now_ms: i64) -> Result<(Self, Kind), DbError> {
        Self::open_as(dir, system_id, now_ms, true)
    }

    fn open_as(
        dir: &Path,
        system_id: &str,
        now_ms: i64,
        offline: bool,
    ) -> Result<(Self, Kind), DbError> {
        let path = dir.join(DATABASE);
        create_file(&path).map_err(|e| DbError::Create(e.kind()))?;
        let mut conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        // Before the first read: a first start and the offline commands
        // may open a new database at once, and its creation's locks are
        // waited for, not failed (review).
        conn.busy_timeout(std::time::Duration::from_millis(5_000))?;
        let found = kind(&conn)?;
        match found {
            Kind::Foreign => return Err(DbError::Foreign),
            Kind::Newer(found) => {
                return Err(DbError::Newer {
                    found,
                    ours: SCHEMA_VERSION,
                });
            }
            Kind::Older(_) if offline => {
                let beat: Option<i64> = conn
                    .query_row("SELECT heartbeat_ms FROM lease WHERE one = 1", [], |r| {
                        r.get(0)
                    })
                    .optional()?;
                may_migrate(
                    found,
                    beat.is_some_and(|b| now_ms.saturating_sub(b) < LEASE_STALE_MS),
                )?;
            }
            _ => {}
        }
        configure(&conn)?;
        // The header in the file itself, for `check-config`'s read; only
        // the connection that made the schema checkpoints, since two
        // checkpoints at once fail rather than wait (review: concurrent
        // first opens).
        if found == Kind::Empty && create(&mut conn, system_id, now_ms)? {
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        }
        let readers = (0..READERS)
            .map(|_| open_reader(&path))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((
            Self {
                writer: Mutex::new(conn),
                readers: Mutex::new(readers),
                path,
                lease_stale_ms: std::sync::atomic::AtomicI64::new(LEASE_STALE_MS),
                writers_waiting: std::sync::atomic::AtomicUsize::new(0),
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
            None => open_reader(&self.path)?,
        };
        let result = read(&conn);
        let mut pool = lock(&self.readers);
        if pool.len() < READERS {
            pool.push(conn);
        }
        drop(pool);
        Ok(result?)
    }

    /// Takes the lease for `holder` (SD6): refused if another holder
    /// renewed it within [`LEASE_STALE_MS`].
    ///
    /// # Errors
    ///
    /// [`DbError::LeaseHeld`], or SQLite's.
    pub fn take_lease(&self, holder: &str, now_ms: i64) -> Result<(), DbError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<(String, i64)> = tx
            .query_row(
                "SELECT holder, heartbeat_ms FROM lease WHERE one = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((other, beat)) = current {
            let stale = self
                .lease_stale_ms
                .load(std::sync::atomic::Ordering::SeqCst);
            if other != holder && now_ms.saturating_sub(beat) < stale {
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

    /// OP11: a lease renewed within `ms` is another server's, instead of
    /// 30 s, so a test restarts a killed server at once (VT10).
    #[cfg(feature = "test-support")]
    pub fn set_lease_stale(&self, ms: i64) {
        self.lease_stale_ms
            .store(ms, std::sync::atomic::Ordering::SeqCst);
    }

    /// Renews the lease (SD6): false if another holder has taken it.
    ///
    /// # Errors
    ///
    /// SQLite's: the database is not writable.
    pub fn renew_lease(&self, holder: &str, now_ms: i64) -> Result<bool, DbError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE lease SET heartbeat_ms = ?2 WHERE one = 1 AND holder = ?1",
            rusqlite::params![holder, now_ms],
        )?;
        tx.commit()?;
        Ok(changed == 1)
    }

    /// Gives the lease up at a graceful stop, so the next start need not
    /// wait for it to go stale.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn release_lease(&self, holder: &str) -> Result<(), DbError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM lease WHERE one = 1 AND holder = ?1", [holder])?;
        tx.commit()?;
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

    /// SD1's last writes: the log checkpointed (`TRUNCATE`), so a stopped
    /// server leaves `server.db` whole in its file.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn checkpoint(&self) -> Result<(), DbError> {
        let conn = self.writer();
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
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

/// AU9 and SD3: an offline command may open a database of `kind` with the
/// lease `held` by a server only if it needs no migration; a server
/// migrates an older schema at its own start, holding the lease.
///
/// # Errors
///
/// [`DbError::MigrateBesideServer`].
pub fn may_migrate(kind: Kind, held: bool) -> Result<(), DbError> {
    match kind {
        Kind::Older(_) if held => Err(DbError::MigrateBesideServer),
        _ => Ok(()),
    }
}

/// Schema 1 and the `system` user, in one exclusive transaction, the
/// header set last inside it (OB1, M6-56). False if another connection
/// made them first.
fn create(conn: &mut Connection, system_id: &str, now_ms: i64) -> rusqlite::Result<bool> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Exclusive)?;
    // A second process may have created it between the header's read and
    // the lock: then there is nothing to do.
    if kind(&tx)? != Kind::Empty {
        tx.commit()?;
        return Ok(false);
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
    tx.commit()?;
    Ok(true)
}
