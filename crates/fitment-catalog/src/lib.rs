//! The fitment catalog (`docs/research/m5-catalog.md`): STEP part files
//! passed through fitment-core's gate, stored by content, and indexed for
//! level 1, in one directory holding a SQLite database and a blob store.
//!
//! Internal to fitment and unstable: the CLI is its one user.
//!
//! ```text
//! <DIR>/FITMENT-CATALOG        the marker, written last by init
//! <DIR>/catalog.db             the database (WAL)
//! <DIR>/blobs/sha256/ab/ab12…  each content's exact bytes, read-only
//! <DIR>/tmp/                   blob writes in flight
//! ```
//!
//! A catalog match reads the database alone (CT6, QR11): the match path
//! is written against the snapshot's rows only, and
//! `tests/query.rs::a_match_runs_with_the_store_moved_away` shows it, every
//! run the same with every stored file moved away. Outside the crate a
//! stored file is read only through [`Catalog::read_blob`], which checks
//! its hash (ST11), and never while a snapshot, and so a match, is open:
//! the snapshot borrows the catalog.
//!
//! ```compile_fail,E0502
//! # fn run(catalog: &mut fitment_catalog::Catalog, sha: &fitment_catalog::Sha256Hex)
//! # -> Result<(), fitment_catalog::Error> {
//! let snapshot = catalog.snapshot()?;
//! let _bytes = catalog.read_blob(sha);
//! drop(snapshot);
//! # Ok(())
//! # }
//! ```

mod blobs;
mod change;
pub mod document;
pub mod fs;
mod gc;
pub mod hex;
pub mod ids;
mod ingest;
pub mod layout;
mod observe;
pub mod preimage;
mod query;
mod reindex;
pub mod schema;
mod snapshot;
mod verify;

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{Connection, OpenFlags};

pub use blobs::{BadBlob, Placed};
pub use change::{Deleted, Deletion, NotReplaced, Pruned, Released, Replaced, Unnamed};
pub use fs::{Content, Fs, FsRead, FsWrite, Point, StdFs};
pub use gc::Collected;
pub use hex::{NotHex, Sha256Hex};
pub use ids::{BadReference, Id, Reference};
pub use ingest::{Counts, Ingest, Ingested, Input, InputBytes, Outcome, Summary};
pub use observe::{NoObserver, Observer, Progress};
pub use preimage::Status;
pub use query::{CatalogRun, NotRun, Selection};
pub use reindex::{Rebuilt, Reindex, Reindexed, Scope};
pub use snapshot::{
    BadName, BadRow, EntryStatus, HistoryStep, Listed, Named, Snapshot, StatusCounts,
    StoredRefusal, has_internal,
};
pub use verify::{Kind as ProblemKind, Problem, Verified};

use layout::{INIT_NAMES, Layout, MARKER_TEXT};

/// The size cap per file (SZ1): 128 MiB.
pub const DEFAULT_MAX_BYTES: u64 = 128 * 1024 * 1024;

/// A derived document over this is refused, not stored (SZ2).
pub const MAX_DOCUMENT_BYTES: usize = 512 * 1024 * 1024;

/// How a catalog is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options {
    /// The largest file read (SZ1).
    pub max_bytes: u64,
    /// How long a writer waits for the lock (DB2, CC6).
    pub busy_timeout_ms: u32,
    /// `PRAGMA max_page_count` (SZ5): a quota, and how the disk-full tests
    /// run on every OS.
    pub max_db_pages: Option<u64>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_BYTES,
            busy_timeout_ms: 60_000,
            max_db_pages: None,
        }
    }
}

impl Options {
    /// The defaults, with `max_bytes`.
    #[must_use]
    pub fn with_max_bytes(self, max_bytes: u64) -> Self {
        Self { max_bytes, ..self }
    }

    /// The defaults, with a busy timeout.
    #[must_use]
    pub fn with_busy_timeout_ms(self, busy_timeout_ms: u32) -> Self {
        Self {
            busy_timeout_ms,
            ..self
        }
    }

    /// The defaults, with a page quota.
    #[must_use]
    pub fn with_max_db_pages(self, pages: u64) -> Self {
        Self {
            max_db_pages: Some(pages),
            ..self
        }
    }
}

/// Why a command could not run on the catalog.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The directory is not a catalog (CL1, exit 2).
    #[error("not a catalog: run fitment catalog init -C {}", .0.display())]
    NotACatalog(PathBuf),
    /// `init` on a directory that is neither empty nor a catalog (ST10).
    #[error("{} is not empty and is not a catalog", .0.display())]
    NotEmpty(PathBuf),
    /// A catalog of a newer schema (DB5, exit 3).
    #[error("this catalog was made by a newer fitment (schema {found}; this build reads {ours})")]
    Newer {
        /// Its schema.
        found: i64,
        /// This build's.
        ours: i64,
    },
    /// A directory of the store that is a symbolic link, or missing (PH2).
    #[error("{} is a symbolic link or not a directory: refused", .0.display())]
    Linked(PathBuf),
    /// A path that is not Unicode where SQLite needs it (PH5).
    #[error("{} is not a Unicode path", .0.display())]
    NotUnicode(PathBuf),
    /// An input inside the catalog (ST10, exit 2).
    #[error("{} is inside the catalog", .0.display())]
    InsideCatalog(PathBuf),
    /// `--id` with other than one file (exit 2).
    #[error("--id names one file; these arguments give {0}")]
    IdNeedsOneFile(usize),
    /// The write lock was not had within the busy timeout (CC6, exit 1).
    #[error("the catalog is busy")]
    Busy,
    /// A file of the store could not be made or read (exit 1).
    #[error("{}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// What failed.
        source: io::Error,
    },
    /// The database failed (exit 1).
    #[error("the catalog's database: {0}")]
    Database(rusqlite::Error),
    /// A row holds what no build writes (exit 1): run `verify`.
    #[error("the catalog's database holds a damaged row ({0}): run fitment catalog verify")]
    Corrupt(&'static str),
    /// The [`Observer`] asked the work to stop (CA2,
    /// `docs/research/m6-api.md`): what was committed before stays.
    #[error("cancelled")]
    Cancelled,
}

impl Error {
    /// CA4 (`docs/research/m6-api.md`): the error's kind, the only text of
    /// it a catalog document or a line holds (`io: not found`, `database:
    /// busy`, `storage full`), never its `Display`, which for an I/O or a
    /// SQLite error can carry a path or a statement.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Busy => "database: busy",
            Self::Database(error) => database_kind(error),
            Self::Io { source, .. } => io_kind(source),
            Self::Corrupt(_) => "database: corrupt",
            Self::Cancelled => "cancelled",
            Self::Newer { .. } => "catalog: newer schema",
            Self::NotACatalog(_) | Self::NotEmpty(_) | Self::Linked(_) | Self::NotUnicode(_) => {
                "catalog: unavailable"
            }
            Self::InsideCatalog(_) | Self::IdNeedsOneFile(_) => "usage",
        }
    }
}

/// CA4: an I/O error's kind, as a line writes it.
pub(crate) fn io_kind(error: &io::Error) -> &'static str {
    use io::ErrorKind as K;
    match error.kind() {
        K::NotFound => "io: not found",
        K::PermissionDenied => "io: permission denied",
        K::AlreadyExists => "io: already exists",
        K::InvalidInput => "io: invalid input",
        K::InvalidData => "io: invalid data",
        K::UnexpectedEof => "io: unexpected end of file",
        K::TimedOut => "io: timed out",
        K::Interrupted => "io: interrupted",
        K::WriteZero => "io: write zero",
        K::Unsupported => "io: unsupported",
        K::OutOfMemory => "io: out of memory",
        K::StorageFull => "storage full",
        K::ReadOnlyFilesystem => "io: read-only file system",
        K::IsADirectory => "io: is a directory",
        K::NotADirectory => "io: not a directory",
        K::FileTooLarge => "io: file too large",
        K::ResourceBusy => "io: resource busy",
        _ => "io: other",
    }
}

/// CA4: a SQLite error's kind, as a line writes it.
fn database_kind(error: &rusqlite::Error) -> &'static str {
    use rusqlite::ErrorCode as C;
    match error.sqlite_error_code() {
        Some(C::DatabaseBusy | C::DatabaseLocked) => "database: busy",
        Some(C::DatabaseCorrupt | C::NotADatabase) => "database: corrupt",
        Some(C::DiskFull) => "storage full",
        Some(C::ReadOnly) => "database: read only",
        Some(C::PermissionDenied) => "database: permission denied",
        Some(C::SystemIoFailure) => "database: io",
        Some(C::CannotOpen) => "database: cannot open",
        Some(C::OutOfMemory) => "database: out of memory",
        Some(C::ConstraintViolation) => "database: constraint",
        _ => "database: error",
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                Self::Busy
            }
            _ => Self::Database(error),
        }
    }
}

fn io_at(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}

/// What a WAL checkpoint did (CA3): SQLite's `wal_checkpoint` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Checkpoint {
    /// True when readers or a writer kept it from completing.
    pub busy: bool,
    /// Pages in the WAL, -1 when the database is not in WAL mode.
    pub log_pages: i64,
    /// Pages of it folded back into the database.
    pub checkpointed_pages: i64,
}

/// CA8's form of an instance id: 32 lower-case hex digits.
fn is_instance(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// What `init` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Init {
    /// Made a new catalog.
    Created,
    /// The directory was already a catalog of this schema.
    Unchanged,
}

/// An open catalog.
pub struct Catalog {
    layout: Layout,
    conn: Connection,
    /// Shared with the inputs read through it ([`Catalog::ingest`]).
    fs: Arc<dyn Fs>,
    options: Options,
}

impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Catalog")
            .field("root", &self.layout.root())
            .finish_non_exhaustive()
    }
}

fn unicode(root: &Path) -> Result<(), Error> {
    if cfg!(windows) && root.to_str().is_none() {
        return Err(Error::NotUnicode(root.to_owned()));
    }
    Ok(())
}

impl Catalog {
    /// `fitment catalog init`: makes a catalog in `root`, creating it if
    /// needed, the marker last; or opens the catalog already there.
    ///
    /// # Errors
    ///
    /// [`Error::NotEmpty`] for a directory holding anything else, and
    /// [`Catalog::open`]'s errors.
    pub fn init(root: &Path, options: Options) -> Result<(Self, Init), Error> {
        Self::init_with(root, options, Box::new(StdFs))
    }

    /// [`Catalog::init`] through `fs`.
    ///
    /// # Errors
    ///
    /// As [`Catalog::init`].
    pub fn init_with(
        root: &Path,
        options: Options,
        fs: Box<dyn Fs>,
    ) -> Result<(Self, Init), Error> {
        let given = root;
        let root = &absolute(root)?;
        let layout = Layout::new(root);
        match fs.symlink_metadata(root) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_at(given)(e)),
            Ok(meta) if !meta.is_dir() => return Err(Error::NotEmpty(given.to_owned())),
            Ok(_) => match fs.read_capped(&layout.marker(), 64) {
                Ok(Content::Bytes(b)) if b == MARKER_TEXT.as_bytes() => {
                    return Self::open_with(given, options, fs).map(|c| (c, Init::Unchanged));
                }
                // A marker cut short is an init that stopped; anything else
                // there is not ours.
                Ok(Content::Bytes(b)) if MARKER_TEXT.as_bytes().starts_with(&b) => {
                    unfinished(fs.as_ref(), &layout, given)?;
                }
                Ok(_) => return Err(Error::NotEmpty(given.to_owned())),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    unfinished(fs.as_ref(), &layout, given)?;
                }
                Err(e) => return Err(io_at(&layout.marker())(e)),
            },
        }
        for dir in layout.directories() {
            fs.create_dir_all(&dir).map_err(io_at(&dir))?;
        }
        let database = layout.database();
        let mut conn = Connection::open(&database)?;
        match schema::kind(&conn)? {
            schema::Kind::Current => {}
            schema::Kind::Foreign if empty(&conn)? => {
                schema::configure(&conn, options.busy_timeout_ms)?;
                schema::create(&mut conn)?;
            }
            _ => return Err(Error::NotEmpty(given.to_owned())),
        }
        drop(conn);
        // The new directories' names, then the marker, by rename, last.
        for dir in layout.containers() {
            fs.sync_dir(&dir).map_err(io_at(&dir))?;
        }
        let temporary = layout.temporary();
        let marker = layout.marker();
        fs.write_new(&temporary, MARKER_TEXT.as_bytes())
            .and_then(|()| fs.rename(&temporary, &marker))
            .map_err(|e| {
                let _ = fs.remove_file(&temporary);
                io_at(&marker)(e)
            })?;
        fs.sync_dir(root).map_err(io_at(given))?;
        Self::open_with(given, options, fs).map(|c| (c, Init::Created))
    }

    /// Opens the catalog in `root` (CL1).
    ///
    /// # Errors
    ///
    /// [`Error::NotACatalog`] without the marker or the database's
    /// identity, [`Error::Newer`] for a newer schema, [`Error::Linked`]
    /// for a planted link (PH2), [`Error::Io`] when the marker or the
    /// database cannot be read.
    pub fn open(root: &Path, options: Options) -> Result<Self, Error> {
        Self::open_with(root, options, Box::new(StdFs))
    }

    /// [`Catalog::open`] through `fs`.
    ///
    /// # Errors
    ///
    /// As [`Catalog::open`].
    pub fn open_with(root: &Path, options: Options, fs: Box<dyn Fs>) -> Result<Self, Error> {
        let given = root;
        let root = &absolute(root)?;
        let layout = Layout::new(root);
        let not_a_catalog = || Error::NotACatalog(given.to_owned());
        let marker = layout.marker();
        match fs.read_capped(&marker, 64) {
            Ok(Content::Bytes(b)) if b == MARKER_TEXT.as_bytes() => {}
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(io_at(&marker)(e)),
            _ => return Err(not_a_catalog()),
        }
        let database = layout.database();
        match fs.symlink_metadata(&database) {
            Ok(meta) if meta.is_file() => {}
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(io_at(&database)(e)),
            _ => return Err(not_a_catalog()),
        }
        layout
            .check_links(fs.as_ref())
            .map_err(|linked| Error::Linked(linked.0))?;
        let mut conn = Connection::open_with_flags(
            &database,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        // Its identity first: nothing is changed in a database not ours.
        let kind = schema::kind(&conn)?;
        match kind {
            schema::Kind::Foreign => return Err(not_a_catalog()),
            schema::Kind::Newer(found) => {
                return Err(Error::Newer {
                    found,
                    ours: schema::SCHEMA_VERSION,
                });
            }
            _ => {}
        }
        schema::configure(&conn, options.busy_timeout_ms)?;
        if let schema::Kind::Older(from) = kind {
            schema::migrate(&mut conn, from, schema::SCHEMA_VERSION, schema::MIGRATIONS)?;
        }
        if let Some(pages) = options.max_db_pages {
            conn.pragma_update(
                None,
                "max_page_count",
                i64::try_from(pages).unwrap_or(i64::MAX),
            )?;
        }
        Ok(Self {
            layout,
            conn,
            fs: Arc::from(fs),
            options,
        })
    }

    /// Checks that `root` is a catalog this build can open, writing
    /// nothing (docs/research/m6-api.md, OP4's `check-config`): the
    /// marker, the store's directories (PH2), and the database's header,
    /// read as bytes with no connection, so no `-wal` or `-shm` file is
    /// made, no pragma set and no migration run. An older schema passes:
    /// [`Catalog::open`] migrates it (DB5).
    ///
    /// # Errors
    ///
    /// As [`Catalog::open`]: [`Error::NotACatalog`], [`Error::Newer`],
    /// [`Error::Linked`] or [`Error::Io`].
    pub fn inspect(root: &Path) -> Result<schema::Kind, Error> {
        let fs = StdFs;
        let given = root;
        let root = &absolute(root)?;
        let layout = Layout::new(root);
        let not_a_catalog = || Error::NotACatalog(given.to_owned());
        let marker = layout.marker();
        match fs.read_capped(&marker, 64) {
            Ok(Content::Bytes(b)) if b == MARKER_TEXT.as_bytes() => {}
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(io_at(&marker)(e)),
            _ => return Err(not_a_catalog()),
        }
        layout
            .check_links(&fs)
            .map_err(|linked| Error::Linked(linked.0))?;
        let database = layout.database();
        match fs.symlink_metadata(&database) {
            Ok(meta) if meta.is_file() => {}
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(io_at(&database)(e)),
            _ => return Err(not_a_catalog()),
        }
        let mut header = Vec::with_capacity(100);
        let file = std::fs::File::open(&database).map_err(io_at(&database))?;
        io::Read::read_to_end(&mut io::Read::take(file, 100), &mut header)
            .map_err(io_at(&database))?;
        match schema::kind_of_header(&header) {
            schema::Kind::Foreign => Err(not_a_catalog()),
            schema::Kind::Newer(found) => Err(Error::Newer {
                found,
                ours: schema::SCHEMA_VERSION,
            }),
            kind => Ok(kind),
        }
    }

    /// The catalog's directory.
    pub fn root(&self) -> &Path {
        self.layout.root()
    }

    /// The largest file read from now on (SZ1): a long-lived catalog, the
    /// server's one writer (CX1), takes each ingest's or replace's own cap
    /// (`max_mib`) without opening another.
    pub fn set_max_bytes(&mut self, max_bytes: u64) {
        self.options.max_bytes = max_bytes;
    }

    /// The generation as it stands (DB8).
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn generation(&self) -> Result<i64, Error> {
        Ok(schema::generation(&self.conn)?)
    }

    /// CC3: folds the WAL back into the database where no reader still
    /// needs it; a catalog match calls it once its snapshot is over.
    pub fn checkpoint(&self) {
        let _ = self
            .conn
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()));
    }

    /// CA3 (`docs/research/m6-api.md`): [`Catalog::checkpoint`]'s
    /// `wal_checkpoint(PASSIVE)`, with what SQLite says of it, so the
    /// server sees readers pinning the WAL (CX6) without reading the
    /// catalog's files.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn checkpoint_passive(&self) -> Result<Checkpoint, Error> {
        self.wal_checkpoint("PRAGMA wal_checkpoint(PASSIVE)")
    }

    /// CA3: `wal_checkpoint(TRUNCATE)`, which waits within the busy
    /// timeout for the readers pinning the WAL, then empties it; `busy`
    /// when they still held it. The barrier's (CX6) and `gc`'s.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn checkpoint_truncate(&self) -> Result<Checkpoint, Error> {
        self.wal_checkpoint("PRAGMA wal_checkpoint(TRUNCATE)")
    }

    fn wal_checkpoint(&self, pragma: &str) -> Result<Checkpoint, Error> {
        let (busy, log, done): (i64, i64, i64) = self
            .conn
            .query_row(pragma, [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(Checkpoint {
            busy: busy != 0,
            log_pages: log,
            checkpointed_pages: done,
        })
    }

    /// CA8 (`docs/research/m6-api.md`): the catalog's instance id, 32
    /// lower-case hex digits, read from the `meta` row `instance`; or,
    /// when there is none, made from `mint`'s 16 bytes and inserted, in
    /// one `BEGIN IMMEDIATE`, so two callers at once agree on one. The row
    /// is not an `op` and raises no generation; schema 1 is unchanged, and
    /// `verify` does not read it. The catalog has no random source: the
    /// server mints, and keeps the value for VN5's entity tags.
    ///
    /// # Errors
    ///
    /// [`Error::Corrupt`] for a stored value that is not 32 lower-case hex
    /// digits; the database's or the lock's.
    pub fn instance(&mut self, mint: impl FnOnce() -> [u8; 16]) -> Result<String, Error> {
        let read = |conn: &Connection| -> Result<Option<String>, Error> {
            let value: Option<String> = rusqlite::OptionalExtension::optional(conn.query_row(
                "SELECT value FROM meta WHERE key = 'instance'",
                [],
                |r| r.get(0),
            ))?;
            match value {
                Some(v) if is_instance(&v) => Ok(Some(v)),
                Some(_) => Err(Error::Corrupt("the catalog's instance id")),
                None => Ok(None),
            }
        };
        if let Some(found) = read(&self.conn)? {
            return Ok(found);
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(found) = read(&tx)? {
            return Ok(found);
        }
        let made = mint()
            .iter()
            .fold(String::with_capacity(32), |mut text, b| {
                let _ = write!(text, "{b:02x}");
                text
            });
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('instance', ?1)",
            [&made],
        )?;
        tx.commit()?;
        Ok(made)
    }

    /// The database connection, for the tests' raw edits.
    #[cfg(feature = "test-support")]
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// A blob's path, for the tests.
    #[cfg(feature = "test-support")]
    pub fn blob_path(&self, sha256: &Sha256Hex) -> PathBuf {
        self.layout.blob(sha256)
    }

    fn blobs(&self) -> blobs::Blobs<'_> {
        blobs::Blobs {
            layout: &self.layout,
            fs: self.fs.as_ref(),
        }
    }
}

/// `root` made absolute, so SQLite never reads it as a `file:` URI; and
/// Unicode on Windows, where SQLite needs it (PH5).
fn absolute(root: &Path) -> Result<PathBuf, Error> {
    unicode(root)?;
    std::path::absolute(root).map_err(io_at(root))
}

/// ST10 and the review's F3 of PR 2a: a directory without a whole marker
/// is taken as an `init` that stopped only if it holds nothing but what
/// `init` makes: its database ours or empty (read only, so nothing is
/// changed in one that is not), `tmp/` holding only temporary files, and
/// `blobs/` only the empty fan-out directories.
fn unfinished(fs: &dyn Fs, layout: &Layout, given: &Path) -> Result<(), Error> {
    let not_ours = || Error::NotEmpty(given.to_owned());
    let root = layout.root();
    let names = fs.read_dir(root).map_err(io_at(given))?;
    let name = |p: &PathBuf| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
            .unwrap_or_default()
    };
    for path in &names {
        let n = name(path);
        if n != layout::MARKER && !INIT_NAMES.contains(&n.as_str()) {
            return Err(not_ours());
        }
    }
    let tmp = layout.tmp();
    if fs.symlink_metadata(&tmp).is_ok() {
        let entries = fs.read_dir(&tmp).map_err(io_at(&tmp))?;
        if !entries
            .iter()
            .all(|p| p.extension().is_some_and(|e| e == "part"))
        {
            return Err(not_ours());
        }
    }
    let blobs = root.join("blobs");
    if fs.symlink_metadata(&blobs).is_ok() {
        let inside = fs.read_dir(&blobs).map_err(io_at(&blobs))?;
        if inside.iter().any(|p| name(p) != "sha256") {
            return Err(not_ours());
        }
        let sha256 = blobs.join("sha256");
        if fs.symlink_metadata(&sha256).is_ok() {
            for fan in fs.read_dir(&sha256).map_err(io_at(&sha256))? {
                let n = name(&fan);
                let hex = n.len() == 2 && n.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
                if !hex || !fs.read_dir(&fan).map_err(io_at(&fan))?.is_empty() {
                    return Err(not_ours());
                }
            }
        }
    }
    let database = layout.database();
    if fs.symlink_metadata(&database).is_ok() {
        let conn = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| not_ours())?;
        let ours = match schema::kind(&conn) {
            Ok(schema::Kind::Current) => true,
            Ok(schema::Kind::Newer(found)) => {
                return Err(Error::Newer {
                    found,
                    ours: schema::SCHEMA_VERSION,
                });
            }
            Ok(schema::Kind::Foreign) => empty(&conn).unwrap_or(false),
            _ => false,
        };
        if !ours {
            return Err(not_ours());
        }
    }
    Ok(())
}

/// Whether a database holds no table: a new file, or an `init` whose one
/// transaction did not commit.
fn empty(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?;
    Ok(n == 0)
}
