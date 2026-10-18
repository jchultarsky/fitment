//! The server's state directory (docs/research/m6-api.md, section 6.1):
//! its marker, `server.db` and the object store's directories.
//!
//! ```text
//! <state>/
//!   FITMENT-SERVER-STATE      marker, "fitment.server 1\n", written last by the first start
//!   server.db                 SQLite, WAL (+ -wal, -shm)
//!   objects/sha256/           uploads, assemblies and stored documents (PR 4)
//!   tmp/                      uploads and documents in flight
//! ```

pub mod db;

use std::io;
use std::path::{Path, PathBuf};

pub use db::{
    CreateUserError, Db, DbError, Kind, MintError, MintFrom, NewToken, Presented, TokenRow, UserRow,
};

/// The marker's name.
pub const MARKER: &str = "FITMENT-SERVER-STATE";

/// The marker's exact text.
pub const MARKER_TEXT: &str = "fitment.server 1\n";

/// The names a state directory holds: anything else in a directory without
/// the marker means it is not one (a typo in `paths.state`).
const NAMES: [&str; 6] = [
    MARKER,
    db::DATABASE,
    "server.db-wal",
    "server.db-shm",
    "objects",
    "tmp",
];

/// Why the state directory cannot be used. Messages name paths relative
/// to the configured root, `state:` (OP1).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StateError {
    /// A directory without the marker that holds other files (exit 2).
    #[error("state: is neither empty nor a fitment-server state directory")]
    NotAState,
    /// The marker is there, but `server.db` is missing or empty (exit 2),
    /// as a catalog without its database is not a catalog.
    #[error("state: has its marker but no server.db: restore it, or start from an empty directory")]
    NoDatabase,
    /// The directory or a file of it cannot be made or read (exit 1).
    #[error("{what}: {kind}")]
    Io {
        /// The path, relative to the configured root (`state:…`).
        what: String,
        /// What failed.
        kind: io::ErrorKind,
    },
    /// `server.db` refused or failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

/// `path` written relative to the state root `dir`, as `state:<rest>`.
fn shown(dir: &Path, path: &Path) -> String {
    let rest = path.strip_prefix(dir).unwrap_or(path);
    format!("state:{}", rest.display()).replace('\\', "/")
}

fn io_at<'a>(dir: &'a Path, path: &'a Path) -> impl FnOnce(io::Error) -> StateError + 'a {
    move |e| StateError::Io {
        what: shown(dir, path),
        kind: e.kind(),
    }
}

/// Whether `dir` is a state directory (its marker, and then a non-empty
/// `server.db`), or a directory a first start may make one of (missing,
/// empty, or holding only the state's own names).
fn marked(dir: &Path) -> Result<bool, StateError> {
    let marker = dir.join(MARKER);
    match std::fs::read(&marker) {
        Ok(bytes) if bytes == MARKER_TEXT.as_bytes() => {
            let database = dir.join(db::DATABASE);
            match std::fs::metadata(&database) {
                Ok(meta) if meta.is_file() && meta.len() > 0 => Ok(true),
                Ok(_) => Err(StateError::NoDatabase),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Err(StateError::NoDatabase),
                Err(e) => Err(io_at(dir, &database)(e)),
            }
        }
        Ok(_) => Err(StateError::NotAState),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(e) => return Err(io_at(dir, dir)(e)),
            };
            for entry in entries {
                let name = entry.map_err(io_at(dir, dir))?.file_name();
                if !NAMES.iter().any(|n| name.to_str() == Some(n)) {
                    return Err(StateError::NotAState);
                }
            }
            Ok(false)
        }
        Err(e) => Err(io_at(dir, &marker)(e)),
    }
}

/// Makes `dir` and its parents; on Unix the state's own directories are
/// the owner's alone (0700).
fn make_dir(dir: &Path, path: &Path) -> Result<(), StateError> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path).map_err(io_at(dir, path))
}

/// Opens the state directory at `dir`, making it on the first start: the
/// directories (0700 on Unix), then `server.db` (0600) with its schema and
/// `system` user, then the marker, last (section 6.1, OB1). A marker
/// without its database is refused.
///
/// # Errors
///
/// A [`StateError`].
pub fn open(dir: &Path, system_id: &str, now_ms: i64) -> Result<(Db, Kind), StateError> {
    open_as(dir, system_id, now_ms, false)
}

/// [`open`] for AU9's offline commands: a new state directory is made as a
/// first start makes it, and an older `server.db` is refused while a
/// server holds the lease ([`Db::open_offline`]).
///
/// # Errors
///
/// A [`StateError`].
pub fn open_offline(dir: &Path, system_id: &str, now_ms: i64) -> Result<(Db, Kind), StateError> {
    open_as(dir, system_id, now_ms, true)
}

fn open_as(
    dir: &Path,
    system_id: &str,
    now_ms: i64,
    offline: bool,
) -> Result<(Db, Kind), StateError> {
    let was = marked(dir)?;
    make_dir(dir, dir)?;
    for sub in [dir.join("objects").join("sha256"), dir.join("tmp")] {
        make_dir(dir, &sub)?;
    }
    let opened = if offline {
        Db::open_offline(dir, system_id, now_ms)?
    } else {
        Db::open(dir, system_id, now_ms)?
    };
    if !was {
        let marker = dir.join(MARKER);
        std::fs::write(&marker, MARKER_TEXT).map_err(io_at(dir, &marker))?;
    }
    Ok(opened)
}

/// OP4: what [`open`] would refuse, found without writing anything: a
/// directory that is not a state directory, a marker without its
/// database, or a `server.db` that is another application's or of a
/// newer schema, read from its header's bytes with no connection (so no
/// `-wal` or `-shm` is made). A directory that does not exist yet is
/// fine: the first start makes it.
///
/// # Errors
///
/// A [`StateError`].
pub fn check(dir: &Path) -> Result<(), StateError> {
    marked(dir)?;
    let database = dir.join(db::DATABASE);
    match db::header_kind(&database).map_err(io_at(dir, &database))? {
        Some(Kind::Foreign) => Err(DbError::Foreign.into()),
        Some(Kind::Newer(found)) => Err(DbError::Newer {
            found,
            ours: db::SCHEMA_VERSION,
        }
        .into()),
        _ => Ok(()),
    }
}

/// True if a path names a parent directory (`..`): refused in
/// `paths.catalog` and `paths.state`, since `..` through a directory that
/// does not exist yet would defeat SV6's comparison (review S3).
pub fn has_parent_component(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// True if either directory contains the other (SV6), after both are made
/// absolute and their links resolved; a directory that does not exist yet
/// is resolved through its nearest existing ancestor.
///
/// # Errors
///
/// An I/O error resolving either path.
pub fn nested(catalog: &Path, state: &Path) -> io::Result<bool> {
    let (a, b) = (resolve(catalog)?, resolve(state)?);
    Ok(a.starts_with(&b) || b.starts_with(&a))
}

fn resolve(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut existing = absolute.as_path();
    let mut rest = Vec::new();
    loop {
        match std::fs::canonicalize(existing) {
            Ok(mut resolved) => {
                resolved.extend(rest.iter().rev());
                return Ok(resolved);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let Some(parent) = existing.parent() else {
                    return Ok(absolute);
                };
                if let Some(name) = existing.file_name() {
                    rest.push(name.to_owned());
                }
                existing = parent;
            }
            Err(e) => return Err(e),
        }
    }
}
