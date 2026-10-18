//! The content-addressed object store (docs/research/m6-api.md, OB2, OB3,
//! UP2, UP8): the catalog's store protocol (ST1 to ST5), copied here
//! against the catalog crate's `Fs` seam (named through `crate::catalog`,
//! SV4), whose streaming calls (CA7) carry every file call of an upload
//! and an object read through the seam.
//!
//! ```text
//! <state>/objects/sha256/<2 hex>/<64 hex>   exact bytes, mode 0444
//! <state>/tmp/<32 hex>.part                 an upload in flight, mode 0600
//! ```
//!
//! Every call here is blocking (SV3). Nothing here sends a path to a
//! client; an error carries its kind alone (ER3).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use fitment_core::Sha256;

use crate::catalog::{Content, Fs, FsRead, FsWrite};

/// The largest object read into one buffer (OB3); a larger one is hashed
/// in one pass and streamed in a second.
pub const ONE_BUFFER_MAX: u64 = 64 * 1024 * 1024;

/// What a temporary file's name ends with.
pub const PART: &str = ".part";

/// Why an object could not be read and checked (OB3).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ObjectError {
    /// No regular file at its name, though a row promises one.
    #[error("missing: no file for the object")]
    Missing,
    /// A file whose SHA-256 or length is not its row's.
    #[error("damaged: the file is not the object's bytes")]
    Damaged,
    /// Larger than one buffer takes (OB3): read it through
    /// [`Objects::open_checked`].
    #[error("larger than one buffer: the object is streamed")]
    TooLargeForBuffer,
    /// The file could not be read.
    #[error("cannot read the object: {}", .0.kind())]
    Io(#[from] io::Error),
}

/// The store under a state directory.
pub struct Objects {
    root: PathBuf,
    fs: Arc<dyn Fs>,
    /// [`ONE_BUFFER_MAX`]; a `test-support` seam lowers it.
    one_buffer_max: AtomicU64,
}

impl std::fmt::Debug for Objects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Objects").finish_non_exhaustive()
    }
}

/// True if `name` is 64 lower-case hex digits: an object's name.
pub fn is_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl Objects {
    /// The store of the state directory `root`, through `fs`.
    pub fn new(root: &Path, fs: Arc<dyn Fs>) -> Self {
        Self {
            root: root.to_owned(),
            fs,
            one_buffer_max: AtomicU64::new(ONE_BUFFER_MAX),
        }
    }

    /// The seam.
    pub fn fs(&self) -> &dyn Fs {
        self.fs.as_ref()
    }

    /// OP11: reads objects above `max` bytes in two passes, so a test
    /// shows the streamed path without a 64 MiB object.
    #[cfg(feature = "test-support")]
    pub fn set_one_buffer_max(&self, max: u64) {
        self.one_buffer_max.store(max, Ordering::Relaxed);
    }

    /// `objects/sha256`.
    pub fn base(&self) -> PathBuf {
        self.root.join("objects").join("sha256")
    }

    /// The fan-out directory of `sha256`: its first two hex digits (ST1).
    pub fn fan_out(&self, sha256: &str) -> PathBuf {
        self.base().join(sha256.get(..2).unwrap_or("00"))
    }

    /// The final name of `sha256`.
    pub fn path(&self, sha256: &str) -> PathBuf {
        self.fan_out(sha256).join(sha256)
    }

    /// `tmp/`.
    pub fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// `path` relative to the state's root, as logs name it (OP1).
    pub fn shown(&self, path: &Path) -> String {
        let rest = path.strip_prefix(&self.root).unwrap_or(path);
        format!("state:{}", rest.display()).replace('\\', "/")
    }

    /// UP2: a new temporary file, `tmp/<random 128-bit hex>.part`,
    /// created new through the seam's streaming writer (mode 0600, CA7). A
    /// name already taken is passed over, never removed.
    ///
    /// # Errors
    ///
    /// The file system's.
    pub fn stage(&self) -> io::Result<Staged> {
        let mut taken = 0;
        loop {
            let name = format!("{:032x}{PART}", crate::ids::Ids::random_bits());
            let path = self.tmp().join(name);
            match self.fs.create_new_stream(&path) {
                Ok(writer) => {
                    return Ok(Staged {
                        fs: Arc::clone(&self.fs),
                        path,
                        writer: Some(writer),
                        hasher: Sha256::new(),
                        bytes: 0,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && taken < 100 => taken += 1,
                Err(e) => return Err(e),
            }
        }
    }

    /// UP8, OB2: the file half of storing `synced`, called inside the
    /// `server.db` transaction that will name it. The same calls in the
    /// same order whether or not the store holds the object (PR 4's
    /// review): the final name's metadata (never its bytes: the existing
    /// object is not read or hashed), the temporary file renamed over the
    /// final name (an atomic replacement of identical bytes, which also
    /// repairs a missing or wrong file there), the object made read-only
    /// (ST4) and its fan-out directory, made with the state (all 256),
    /// synced (ST3). Before the rename, whatever stands at the name that a
    /// rename cannot replace is cleared, as the catalog's blobs are: a
    /// directory removed, another non-regular entry unlinked, and on
    /// Windows a regular file's read-only flag cleared, which its rename
    /// needs; elsewhere a regular file is renamed over as it is, so the
    /// two branches make the same calls.
    ///
    /// # Errors
    ///
    /// The file system's: the transaction then rolls back, and the caller
    /// discards the temporary file; a file renamed into place without a
    /// row is an orphan the sweep unlinks.
    pub fn place(&self, synced: &Synced) -> io::Result<()> {
        let fan = self.fan_out(&synced.sha256);
        let path = self.path(&synced.sha256);
        let gone = |r: io::Result<()>| match r {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
        match self.fs.symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => gone(self.fs.remove_dir(&path))?,
            Ok(meta) if !meta.is_file() => gone(self.fs.remove_file(&path))?,
            Ok(_) if cfg!(windows) => gone(self.fs.set_readonly(&path, false))?,
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        self.fs.rename(&synced.path, &path)?;
        self.fs.set_readonly(&path, true)?;
        self.fs.sync_dir(&fan)
    }

    /// OB2: removes the temporary file of a write that will not be kept;
    /// one already gone is fine.
    pub fn discard(&self, path: &Path) {
        match self.fs.remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                file = %self.shown(path),
                kind = ?e.kind(),
                "a temporary file could not be removed; the sweep will"
            ),
        }
    }

    /// OB2, ST5: unlinks the object `sha256`, called inside the
    /// transaction that has just found no row naming it; a file already
    /// gone is fine. Windows refuses to delete a read-only file (ST4).
    ///
    /// # Errors
    ///
    /// The file system's.
    pub fn unlink(&self, sha256: &str) -> io::Result<()> {
        let path = self.path(sha256);
        match self.fs.symlink_metadata(&path) {
            Ok(meta) if meta.is_file() => {
                let _ = self.fs.set_readonly(&path, false);
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        }
        match self.fs.remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// The names of the objects' files, for the sweep's orphans (OB4):
    /// only names that are an object's in their own fan-out directory.
    ///
    /// # Errors
    ///
    /// The file system's.
    pub fn files(&self) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        let fans = match self.fs.read_dir(&self.base()) {
            Ok(fans) => fans,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(names),
            Err(e) => return Err(e),
        };
        for fan in fans {
            let Some(prefix) = fan.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
                continue;
            };
            let Ok(entries) = self.fs.read_dir(&fan) else {
                continue;
            };
            for entry in entries {
                if let Some(name) = entry.file_name().and_then(|n| n.to_str()) {
                    if is_name(name) && name.starts_with(&prefix) {
                        names.push(name.to_owned());
                    }
                }
            }
        }
        names.sort();
        Ok(names)
    }

    /// Removes temporary files: at a start (`older_than` `None`), every
    /// upload's (`<32 hex>.part`), since no upload survives its connection
    /// (SD2, SD4), and nothing else (a concurrent first start's marker may
    /// be on its way into place); in the sweep, every file last changed
    /// before `older_than` (no upload lives past `limits.upload_minutes`).
    /// Returns how many were removed.
    ///
    /// # Errors
    ///
    /// The directory's listing.
    pub fn clear_tmp(&self, older_than: Option<std::time::Duration>) -> io::Result<usize> {
        let entries = match self.fs.read_dir(&self.tmp()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        // The clock only judges a file's age; nothing stores it.
        let now = std::time::SystemTime::now();
        let mut removed = 0;
        for path in entries {
            let Ok(meta) = self.fs.symlink_metadata(&path) else {
                continue;
            };
            let old = match older_than {
                None => path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_suffix(PART))
                    .is_some_and(|stem| {
                        stem.len() == 32 && stem.bytes().all(|b| b.is_ascii_hexdigit())
                    }),
                Some(age) => meta
                    .modified()
                    .ok()
                    .and_then(|m| now.duration_since(m).ok())
                    .is_some_and(|elapsed| elapsed > age),
            };
            if meta.is_file() && old && self.fs.remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// OB3: the object `sha256` of `bytes` bytes, read into one buffer and
    /// returned only if it hashes to its name; above the one-buffer cap,
    /// see [`Objects::open_checked`].
    ///
    /// # Errors
    ///
    /// [`ObjectError`]; [`ObjectError::TooLargeForBuffer`] for an object
    /// above the cap, which must be streamed.
    pub fn read(&self, sha256: &str, bytes: u64) -> Result<Vec<u8>, ObjectError> {
        if bytes > self.one_buffer_max.load(Ordering::Relaxed) {
            return Err(ObjectError::TooLargeForBuffer);
        }
        let path = self.path(sha256);
        let content = self.fs.read_capped(&path, bytes).map_err(missing)?;
        match content {
            Content::Bytes(read) if fitment_core::sha256_hex(&read) == sha256 => Ok(read),
            _ => Err(ObjectError::Damaged),
        }
    }

    /// OB3: an object above the one-buffer cap, hashed in one pass of the
    /// streaming reader, then rewound on the same open file for the second,
    /// so a mismatch stops before the caller sends its first byte and the
    /// second pass reads the file the first one checked; the second pass is
    /// hashed again as it goes and fails its last read on a mismatch, so a
    /// file changed in place between the passes is never sent whole as the
    /// object (PR 4's review). An object within the cap is read whole by
    /// [`Objects::read`] instead.
    ///
    /// # Errors
    ///
    /// [`ObjectError`].
    pub fn open_checked(&self, sha256: &str, bytes: u64) -> Result<Box<dyn FsRead>, ObjectError> {
        let path = self.path(sha256);
        let mut reader = self.fs.open_stream(&path, bytes).map_err(missing)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0_u8; 256 * 1024];
        let mut seen = 0_u64;
        loop {
            let n = reader.read_chunk(&mut buf).map_err(damaged)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            seen += u64::try_from(n).unwrap_or(u64::MAX);
        }
        if seen != bytes || hasher.finish_hex() != sha256 {
            return Err(ObjectError::Damaged);
        }
        reader.rewind()?;
        Ok(Box::new(Checked {
            inner: reader,
            hasher: Sha256::new(),
            sha256: sha256.to_owned(),
            bytes,
            seen: 0,
        }))
    }

    /// The one-buffer cap in force (OB3).
    pub fn one_buffer_max(&self) -> u64 {
        self.one_buffer_max.load(Ordering::Relaxed)
    }
}

/// OB3's second pass: the bytes hashed again as they are read; the read
/// that meets the end fails, `InvalidData`, if they are not the object's,
/// so its caller aborts rather than finish sending.
struct Checked {
    inner: Box<dyn FsRead>,
    hasher: Sha256,
    sha256: String,
    bytes: u64,
    seen: u64,
}

impl FsRead for Checked {
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read_chunk(buf)?;
        if n == 0 {
            let hash = std::mem::take(&mut self.hasher).finish_hex();
            if self.seen != self.bytes || hash != self.sha256 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the object changed between its check and its sending",
                ));
            }
            return Ok(0);
        }
        self.hasher.update(&buf[..n]);
        self.seen += u64::try_from(n).unwrap_or(u64::MAX);
        Ok(n)
    }

    fn rewind(&mut self) -> io::Result<()> {
        self.inner.rewind()?;
        self.hasher = Sha256::new();
        self.seen = 0;
        Ok(())
    }
}

/// A missing file is [`ObjectError::Missing`]; a file that is not regular
/// or longer than its row is damaged.
fn missing(error: io::Error) -> ObjectError {
    match error.kind() {
        io::ErrorKind::NotFound => ObjectError::Missing,
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => ObjectError::Damaged,
        _ => ObjectError::Io(error),
    }
}

/// A read that meets more bytes than the row's is damaged.
fn damaged(error: io::Error) -> ObjectError {
    if error.kind() == io::ErrorKind::InvalidData {
        ObjectError::Damaged
    } else {
        ObjectError::Io(error)
    }
}

/// An upload being written to its temporary file (UP2): each chunk fed to
/// CO1's hasher and written through the seam. Dropped before
/// [`Staged::finish`], its temporary file is removed.
pub struct Staged {
    fs: Arc<dyn Fs>,
    path: PathBuf,
    writer: Option<Box<dyn FsWrite>>,
    hasher: Sha256,
    bytes: u64,
}

impl std::fmt::Debug for Staged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Staged")
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

impl Staged {
    /// Hashes and writes `chunk`.
    ///
    /// # Errors
    ///
    /// The file system's.
    pub fn write(&mut self, chunk: &[u8]) -> io::Result<()> {
        let Some(writer) = self.writer.as_mut() else {
            return Err(io::Error::other("the upload is finished"));
        };
        self.hasher.update(chunk);
        writer.write_chunk(chunk)?;
        self.bytes += u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        Ok(())
    }

    /// The bytes written so far.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Syncs the file (ST3) and names it by its SHA-256 (CO1).
    ///
    /// # Errors
    ///
    /// The file system's; the temporary file is then removed.
    pub fn finish(mut self) -> io::Result<Synced> {
        let Some(mut writer) = self.writer.take() else {
            return Err(io::Error::other("the upload is finished"));
        };
        let synced = writer.sync();
        drop(writer);
        if let Err(error) = synced {
            self.writer = None;
            remove_quietly(self.fs.as_ref(), &self.path);
            return Err(error);
        }
        let hasher = std::mem::take(&mut self.hasher);
        let path = std::mem::take(&mut self.path);
        Ok(Synced {
            fs: Arc::clone(&self.fs),
            path,
            sha256: hasher.finish_hex(),
            bytes: self.bytes,
            armed: true,
        })
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if self.writer.take().is_some() {
            remove_quietly(self.fs.as_ref(), &self.path);
        }
    }
}

fn remove_quietly(fs: &dyn Fs, path: &Path) {
    if path.as_os_str().is_empty() {
        return;
    }
    let _ = fs.remove_file(path);
}

/// An upload's temporary file, synced and named by its SHA-256: ready for
/// [`Objects::place`]. Dropped before [`Synced::kept`], its temporary file
/// is removed, so a refused or failed upload leaves nothing (UP2).
pub struct Synced {
    fs: Arc<dyn Fs>,
    /// The temporary file.
    pub path: PathBuf,
    /// The bytes' SHA-256, lower-case hex.
    pub sha256: String,
    /// Their count.
    pub bytes: u64,
    armed: bool,
}

impl std::fmt::Debug for Synced {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Synced")
            .field("sha256", &self.sha256)
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

impl Synced {
    /// The rows that name it committed: its temporary file is the store's
    /// now, renamed or removed by [`Objects::place`].
    pub fn kept(mut self) {
        self.armed = false;
    }
}

impl Drop for Synced {
    fn drop(&mut self) {
        if self.armed {
            remove_quietly(self.fs.as_ref(), &self.path);
        }
    }
}
