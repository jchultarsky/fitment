//! The catalog on disk (`docs/research/m5-catalog.md`, section 6.1, ST1,
//! PH1, PH2): the marker, the database and the blob store, every path
//! built from the root and a [`Sha256Hex`] or a temporary name the writer
//! made.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::fs::Fs;
use crate::hex::Sha256Hex;

/// The marker's name, written last by `init`.
pub const MARKER: &str = "FITMENT-CATALOG";

/// The marker's content.
pub const MARKER_TEXT: &str = "fitment.catalog 1\n";

/// The database's name.
pub const DATABASE: &str = "catalog.db";

/// The names `init` makes in the root: a directory holding only these and
/// no marker is an `init` that stopped, and may be finished.
pub const INIT_NAMES: [&str; 6] = [
    DATABASE,
    "catalog.db-wal",
    "catalog.db-shm",
    "catalog.db-journal",
    "blobs",
    "tmp",
];

/// Temporary names in this process (ST3): `<pid>-<n>.part`.
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// The paths of one catalog.
#[derive(Debug, Clone)]
pub struct Layout {
    root: PathBuf,
}

/// A path under the root that is a symbolic link where a directory must be
/// (PH2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked(pub PathBuf);

impl Layout {
    /// The layout under `root`.
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
        }
    }

    /// The root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The marker.
    pub fn marker(&self) -> PathBuf {
        self.root.join(MARKER)
    }

    /// The database.
    pub fn database(&self) -> PathBuf {
        self.root.join(DATABASE)
    }

    /// `blobs/`.
    fn blobs(&self) -> PathBuf {
        self.root.join("blobs")
    }

    /// `blobs/sha256/`: the algorithm; a later hash is a sibling.
    fn sha256(&self) -> PathBuf {
        self.blobs().join("sha256")
    }

    /// `tmp/`.
    pub fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// The fan-out directory `blobs/sha256/<2 hex>/` of `sha256`.
    pub fn fan_out(&self, sha256: &Sha256Hex) -> PathBuf {
        self.sha256().join(sha256.fan_out())
    }

    /// A blob's path (ST1).
    pub fn blob(&self, sha256: &Sha256Hex) -> PathBuf {
        self.fan_out(sha256).join(sha256.as_str())
    }

    /// A fresh temporary file's path under `tmp/`.
    pub fn temporary(&self) -> PathBuf {
        let n = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
        // The process id only names a temporary file, so two processes
        // never pick one name; it decides nothing.
        #[allow(clippy::disallowed_methods)]
        let pid = std::process::id();
        self.tmp().join(format!("{pid}-{n}.part"))
    }

    /// Every directory `init` makes: `blobs/sha256/00` to `ff`, then `tmp`.
    pub fn directories(&self) -> Vec<PathBuf> {
        let sha256 = self.sha256();
        let mut out: Vec<PathBuf> = (0..=255_u8)
            .map(|b| sha256.join(format!("{b:02x}")))
            .collect();
        out.push(self.tmp());
        out
    }

    /// PH2: `blobs`, `blobs/sha256`, each fan-out directory and `tmp` are
    /// directories, none a symbolic link.
    ///
    /// # Errors
    ///
    /// The first that is a link, or missing or not a directory.
    pub fn check_links(&self, fs: &dyn Fs) -> Result<(), Linked> {
        let mut paths = vec![self.blobs(), self.sha256()];
        paths.extend(self.directories());
        for path in paths {
            match fs.symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => {}
                _ => return Err(Linked(path)),
            }
        }
        Ok(())
    }
}
