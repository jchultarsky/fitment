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

pub use db::{Db, DbError, Kind};

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

/// Why the state directory cannot be used.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StateError {
    /// A directory without the marker that holds other files (exit 2).
    #[error("{} is neither empty nor a fitment-server state directory", .0.display())]
    NotAState(PathBuf),
    /// The directory or a file of it cannot be made or read (exit 1).
    #[error("{}: {kind}", path.display())]
    Io {
        /// The path.
        path: PathBuf,
        /// What failed.
        kind: io::ErrorKind,
    },
    /// `server.db` refused or failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

fn io_at(path: &Path) -> impl FnOnce(io::Error) -> StateError + '_ {
    move |e| StateError::Io {
        path: path.to_owned(),
        kind: e.kind(),
    }
}

/// Opens the state directory at `dir`, making it on the first start: the
/// directories, then `server.db` with its schema and `system` user, then
/// the marker, last (section 6.1, OB1).
///
/// # Errors
///
/// A [`StateError`].
pub fn open(dir: &Path, system_id: &str, now_ms: i64) -> Result<(Db, Kind), StateError> {
    std::fs::create_dir_all(dir).map_err(io_at(dir))?;
    let marker_path = dir.join(MARKER);
    let marked = match std::fs::read(&marker_path) {
        Ok(bytes) if bytes == MARKER_TEXT.as_bytes() => true,
        Ok(_) => return Err(StateError::NotAState(dir.to_owned())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(io_at(&marker_path)(e)),
    };
    if !marked {
        for entry in std::fs::read_dir(dir).map_err(io_at(dir))? {
            let name = entry.map_err(io_at(dir))?.file_name();
            if !NAMES.iter().any(|n| name.to_str() == Some(n)) {
                return Err(StateError::NotAState(dir.to_owned()));
            }
        }
    }
    for sub in [dir.join("objects").join("sha256"), dir.join("tmp")] {
        std::fs::create_dir_all(&sub).map_err(io_at(&sub))?;
    }
    let opened = Db::open(dir, system_id, now_ms)?;
    if !marked {
        std::fs::write(&marker_path, MARKER_TEXT).map_err(io_at(&marker_path))?;
    }
    Ok(opened)
}

/// OP4: what [`open`] would refuse, found without writing anything: a
/// directory that is not a state directory, or a `server.db` that is
/// another application's or of a newer schema. A directory that does not
/// exist yet is fine: the first start makes it.
///
/// # Errors
///
/// A [`StateError`].
pub fn check(dir: &Path) -> Result<(), StateError> {
    if !dir.exists() {
        return Ok(());
    }
    let marker = dir.join(MARKER);
    match std::fs::read(&marker) {
        Ok(bytes) if bytes == MARKER_TEXT.as_bytes() => {}
        Ok(_) => return Err(StateError::NotAState(dir.to_owned())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            for entry in std::fs::read_dir(dir).map_err(io_at(dir))? {
                let name = entry.map_err(io_at(dir))?.file_name();
                if !NAMES.iter().any(|n| name.to_str() == Some(n)) {
                    return Err(StateError::NotAState(dir.to_owned()));
                }
            }
        }
        Err(e) => return Err(io_at(&marker)(e)),
    }
    match db::peek(&dir.join(db::DATABASE))? {
        Some(Kind::Foreign) => Err(DbError::Foreign.into()),
        Some(Kind::Newer(found)) => Err(DbError::Newer {
            found,
            ours: db::SCHEMA_VERSION,
        }
        .into()),
        _ => Ok(()),
    }
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
