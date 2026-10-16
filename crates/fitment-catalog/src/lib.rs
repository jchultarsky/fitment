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

mod blobs;
pub mod fs;
pub mod hex;
pub mod ids;
mod ingest;
pub mod layout;
pub mod preimage;
pub mod schema;

use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

pub use blobs::{BadBlob, Placed};
pub use fs::{Content, Fs, Point, StdFs};
pub use hex::{NotHex, Sha256Hex};
pub use ids::{BadReference, Id, Reference};
pub use ingest::{Ingested, Outcome, Summary};
pub use preimage::Status;

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
    fs: Box<dyn Fs>,
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
        unicode(root)?;
        let layout = Layout::new(root);
        match fs.symlink_metadata(root) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_at(root)(e)),
            Ok(meta) if !meta.is_dir() => return Err(Error::NotEmpty(root.to_owned())),
            Ok(_) => {
                if fs.symlink_metadata(&layout.marker()).is_ok() {
                    return Self::open_with(root, options, fs).map(|c| (c, Init::Unchanged));
                }
                let names = fs.read_dir(root).map_err(io_at(root))?;
                let ours = |p: &PathBuf| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| INIT_NAMES.contains(&n))
                };
                if !names.iter().all(ours) {
                    return Err(Error::NotEmpty(root.to_owned()));
                }
            }
        }
        for dir in layout.directories() {
            fs.create_dir_all(&dir).map_err(io_at(&dir))?;
        }
        let database = layout.database();
        let mut conn = Connection::open(&database)?;
        schema::configure(&conn, options.busy_timeout_ms)?;
        match schema::kind(&conn)? {
            schema::Kind::Current => {}
            schema::Kind::Foreign if empty(&conn)? => schema::create(&mut conn)?,
            _ => return Err(Error::NotEmpty(root.to_owned())),
        }
        let marker = layout.marker();
        fs.write_new(&marker, MARKER_TEXT.as_bytes())
            .map_err(io_at(&marker))?;
        fs.sync_dir(root).map_err(io_at(root))?;
        drop(conn);
        Self::open_with(root, options, fs).map(|c| (c, Init::Created))
    }

    /// Opens the catalog in `root` (CL1).
    ///
    /// # Errors
    ///
    /// [`Error::NotACatalog`] without the marker or the database's
    /// identity, [`Error::Newer`] for a newer schema, [`Error::Linked`]
    /// for a planted link (PH2).
    pub fn open(root: &Path, options: Options) -> Result<Self, Error> {
        Self::open_with(root, options, Box::new(StdFs))
    }

    /// [`Catalog::open`] through `fs`.
    ///
    /// # Errors
    ///
    /// As [`Catalog::open`].
    pub fn open_with(root: &Path, options: Options, fs: Box<dyn Fs>) -> Result<Self, Error> {
        unicode(root)?;
        let layout = Layout::new(root);
        let not_a_catalog = || Error::NotACatalog(root.to_owned());
        match fs.read_capped(&layout.marker(), 64) {
            Ok(Content::Bytes(b)) if b == MARKER_TEXT.as_bytes() => {}
            _ => return Err(not_a_catalog()),
        }
        if !fs
            .symlink_metadata(&layout.database())
            .is_ok_and(|m| m.is_file())
        {
            return Err(not_a_catalog());
        }
        layout
            .check_links(fs.as_ref())
            .map_err(|linked| Error::Linked(linked.0))?;
        let mut conn = Connection::open_with_flags(
            layout.database(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        schema::configure(&conn, options.busy_timeout_ms)?;
        match schema::kind(&conn)? {
            schema::Kind::Current => {}
            schema::Kind::Foreign => return Err(not_a_catalog()),
            schema::Kind::Newer(found) => {
                return Err(Error::Newer {
                    found,
                    ours: schema::SCHEMA_VERSION,
                });
            }
            schema::Kind::Older(from) => {
                schema::migrate(&mut conn, from, schema::SCHEMA_VERSION, schema::MIGRATIONS)?;
            }
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
            fs,
            options,
        })
    }

    /// The catalog's directory.
    pub fn root(&self) -> &Path {
        self.layout.root()
    }

    /// The generation as it stands (DB8).
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn generation(&self) -> Result<i64, Error> {
        Ok(schema::generation(&self.conn)?)
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

/// Whether a database holds no table: a new file, or an `init` whose one
/// transaction did not commit.
fn empty(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?;
    Ok(n == 0)
}
