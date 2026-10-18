//! The file system seam (`docs/research/m5-catalog.md`, ST3, CT6, CT13):
//! every file operation the catalog makes goes through [`Fs`], so the
//! tests can count them, fail any one of them, and see the order of
//! renames, directory syncs and commits. CA7 (`docs/research/m6-api.md`)
//! adds a streaming writer and reader, [`FsWrite`] and [`FsRead`], whose
//! every chunk, sync and read is a call of the seam too, for the server's
//! object store; the catalog's own blobs keep [`Fs::write_new`].

use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

/// A point in a write transaction the seam is told of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// Just before `COMMIT`.
    BeforeCommit,
    /// Just after it.
    AfterCommit,
}

/// What reading a file gave (SZ1, PH4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// Its bytes, at most the cap.
    Bytes(Vec<u8>),
    /// Not a regular file: a directory, FIFO, socket or device.
    NotRegular,
    /// Larger than the cap: nothing kept.
    TooLarge,
}

/// A file being written chunk by chunk ([`Fs::create_new_stream`], CA7):
/// each chunk and the sync is one call of the seam.
pub trait FsWrite: Send {
    /// Appends `bytes`, all of them.
    ///
    /// # Errors
    ///
    /// The file system's: `StorageFull` for a full disk.
    fn write_chunk(&mut self, bytes: &[u8]) -> io::Result<()>;

    /// Syncs the file's data and metadata to the disk (ST3).
    ///
    /// # Errors
    ///
    /// The file system's.
    fn sync(&mut self) -> io::Result<()>;
}

/// A file being read chunk by chunk ([`Fs::open_stream`], CA7): each read
/// is one call of the seam.
pub trait FsRead: Send {
    /// Reads into `buf` and answers how many bytes it filled, 0 at the end;
    /// never more than the stream's cap in all.
    ///
    /// # Errors
    ///
    /// The file system's; `InvalidData` once the file holds more than the
    /// cap (it grew after it was opened).
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize>;
}

/// Every file operation the catalog makes.
pub trait Fs: Send + Sync {
    /// The metadata of `path`, not following a final symbolic link.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn symlink_metadata(&self, path: &Path) -> io::Result<Metadata>;

    /// Reads a regular file once, whole, keeping at most `cap` bytes: its
    /// type checked before it is opened (a FIFO is never opened) and again
    /// on the open handle, its length before and while reading.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn read_capped(&self, path: &Path, cap: u64) -> io::Result<Content>;

    /// The entries of a directory, in no order.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>>;

    /// Creates a directory, and its parents.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;

    /// Creates a new file (never an existing one), writes `bytes` and syncs
    /// it (ST3).
    ///
    /// # Errors
    ///
    /// The file system's; `AlreadyExists` if `path` does.
    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;

    /// Renames `from` over `to`.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;

    /// Makes a file read-only (mode 0444, ST4) or writable by its owner.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn set_readonly(&self, path: &Path, readonly: bool) -> io::Result<()>;

    /// Removes a file.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn remove_file(&self, path: &Path) -> io::Result<()>;

    /// Syncs a directory, so the names in it are durable (ST3). Windows
    /// cannot open a directory to sync it; there it does nothing (R7).
    ///
    /// # Errors
    ///
    /// The file system's.
    fn sync_dir(&self, path: &Path) -> io::Result<()>;

    /// Creates a new file (never an existing one) for writing in chunks,
    /// with mode 0600 on Unix: the owner's alone while it is written (CA7).
    ///
    /// # Errors
    ///
    /// The file system's; `AlreadyExists` if `path` does.
    fn create_new_stream(&self, path: &Path) -> io::Result<Box<dyn FsWrite>>;

    /// Opens a regular file for reading in chunks, at most `cap` bytes:
    /// its type checked before it is opened (a FIFO is never opened) and
    /// again on the open handle, its length before (CA7).
    ///
    /// # Errors
    ///
    /// The file system's; `InvalidInput` for a file that is not regular,
    /// `InvalidData` for one longer than `cap`.
    fn open_stream(&self, path: &Path, cap: u64) -> io::Result<Box<dyn FsRead>>;

    /// Told of each point of a write transaction; the tests' hook.
    ///
    /// # Errors
    ///
    /// A fault a test injects; the transaction then does not commit.
    fn point(&self, at: Point) -> io::Result<()> {
        let _ = at;
        Ok(())
    }
}

/// The real file system.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdFs;

/// Reads at most `cap` bytes of an open regular file.
fn read_open(mut file: File, cap: u64) -> io::Result<Content> {
    if !file.metadata()?.is_file() {
        return Ok(Content::NotRegular);
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(cap.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).map_or(true, |n| n > cap) {
        return Ok(Content::TooLarge);
    }
    Ok(Content::Bytes(bytes))
}

/// `O_NONBLOCK`, so a FIFO put in a file's place between the type check
/// and the open cannot block the open; a regular file reads as before.
#[cfg(any(target_os = "linux", target_os = "android"))]
const NONBLOCK: i32 = 0o4000;
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
const NONBLOCK: i32 = 0x4;

/// Opens `path` for reading, without blocking on a FIFO where the platform
/// allows it; the open handle's type is checked after (`read_open`).
fn open_nonblocking(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(NONBLOCK);
    }
    options.open(path)
}

/// [`StdFs`]'s streaming writer: a file.
struct StdWrite(File);

impl FsWrite for StdWrite {
    fn write_chunk(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.write_all(bytes)
    }

    fn sync(&mut self) -> io::Result<()> {
        self.0.sync_all()
    }
}

/// [`StdFs`]'s streaming reader: a file, and what it may still give.
struct StdRead {
    file: File,
    left: u64,
}

impl FsRead for StdRead {
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            // One byte more than the cap is a file that grew: refused, so a
            // reader never takes it for the whole file.
            let mut probe = [0_u8; 1];
            return match self.file.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the file is longer than its cap",
                )),
            };
        }
        let room = usize::try_from(self.left)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let n = self.file.read(&mut buf[..room])?;
        self.left -= u64::try_from(n).unwrap_or(self.left);
        Ok(n)
    }
}

impl Fs for StdFs {
    fn symlink_metadata(&self, path: &Path) -> io::Result<Metadata> {
        std::fs::symlink_metadata(path)
    }

    fn read_capped(&self, path: &Path, cap: u64) -> io::Result<Content> {
        let meta = std::fs::metadata(path)?;
        if !meta.is_file() {
            return Ok(Content::NotRegular);
        }
        if meta.len() > cap {
            return Ok(Content::TooLarge);
        }
        read_open(open_nonblocking(path)?, cap)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        std::fs::read_dir(path)?
            .map(|entry| entry.map(|e| e.path()))
            .collect()
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    fn set_readonly(&self, path: &Path, readonly: bool) -> io::Result<()> {
        let mut permissions = std::fs::metadata(path)?.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            permissions.set_mode(if readonly { 0o444 } else { 0o644 });
        }
        #[cfg(not(unix))]
        {
            // Off Unix the flag is all there is, and clearing it gives no
            // one else a write.
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(readonly);
        }
        std::fs::set_permissions(path, permissions)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        if cfg!(windows) {
            return Ok(());
        }
        File::open(path)?.sync_all()
    }

    fn create_new_stream(&self, path: &Path) -> io::Result<Box<dyn FsWrite>> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        Ok(Box::new(StdWrite(options.open(path)?)))
    }

    fn open_stream(&self, path: &Path, cap: u64) -> io::Result<Box<dyn FsRead>> {
        let meta = std::fs::metadata(path)?;
        if !meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        if meta.len() > cap {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the file is longer than its cap",
            ));
        }
        let file = open_nonblocking(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        Ok(Box::new(StdRead { file, left: cap }))
    }
}

#[cfg(feature = "test-support")]
pub use support::{Call, CountingFs};

#[cfg(feature = "test-support")]
mod support {
    use std::fs::Metadata;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use super::{Content, Fs, FsRead, FsWrite, Point, StdFs};

    /// One call the seam saw.
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum Call {
        /// `symlink_metadata`.
        Metadata(PathBuf),
        /// `read_capped`.
        Read(PathBuf),
        /// `read_dir`.
        ReadDir(PathBuf),
        /// `create_dir_all`.
        CreateDir(PathBuf),
        /// `write_new`.
        WriteNew(PathBuf),
        /// `rename`, from and to.
        Rename(PathBuf, PathBuf),
        /// `set_readonly`.
        SetReadonly(PathBuf, bool),
        /// `remove_file`.
        Remove(PathBuf),
        /// `sync_dir`.
        SyncDir(PathBuf),
        /// `point`.
        Point(Point),
        /// `create_new_stream` (CA7).
        CreateStream(PathBuf),
        /// A stream's `write_chunk`, with the chunk's length.
        WriteChunk(PathBuf, usize),
        /// A stream's `sync`.
        SyncStream(PathBuf),
        /// `open_stream` (CA7).
        OpenStream(PathBuf),
        /// A stream's `read_chunk`, with the room it was given.
        ReadChunk(PathBuf, usize),
    }

    type Hook = Box<dyn Fn(&Call) + Send + Sync>;

    /// What a [`CountingFs`] and its streams share.
    #[derive(Default)]
    struct Inner {
        calls: Mutex<Vec<Call>>,
        fail_at: Option<(usize, io::ErrorKind)>,
        hook: Option<Hook>,
    }

    impl Inner {
        /// Records `call`, fails it if it is the one, else runs it and then
        /// the hook.
        fn run<T>(&self, call: &Call, op: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
            let k = {
                let Ok(mut calls) = self.calls.lock() else {
                    return Err(io::Error::other("a poisoned seam"));
                };
                calls.push(call.clone());
                calls.len() - 1
            };
            if let Some((at, kind)) = self.fail_at {
                if at == k {
                    return Err(io::Error::new(kind, "injected"));
                }
            }
            let out = op();
            if let Some(hook) = &self.hook {
                hook(call);
            }
            out
        }
    }

    /// The real file system, counting and recording every call, failing
    /// the call numbered `k` (from 0) with a chosen error, and running a
    /// hook after each call that did not fail (CT13). A clone shares the
    /// count, and so does every stream it opens (CA7).
    #[derive(Default, Clone)]
    pub struct CountingFs {
        inner: Arc<Inner>,
    }

    impl CountingFs {
        /// Counting only.
        pub fn new() -> Self {
            Self::default()
        }

        /// Failing call `k` with `kind`.
        pub fn failing_at(k: usize, kind: io::ErrorKind) -> Self {
            Self {
                inner: Arc::new(Inner {
                    fail_at: Some((k, kind)),
                    ..Inner::default()
                }),
            }
        }

        /// Running `hook` after each call.
        pub fn with_hook(hook: impl Fn(&Call) + Send + Sync + 'static) -> Self {
            Self {
                inner: Arc::new(Inner {
                    hook: Some(Box::new(hook)),
                    ..Inner::default()
                }),
            }
        }

        /// The calls so far.
        pub fn calls(&self) -> Vec<Call> {
            self.inner
                .calls
                .lock()
                .map(|c| c.clone())
                .unwrap_or_default()
        }

        fn run<T>(&self, call: &Call, op: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
            self.inner.run(call, op)
        }
    }

    /// A [`CountingFs`] stream's writer: each chunk and the sync a call.
    struct CountedWrite {
        inner: Arc<Inner>,
        path: PathBuf,
        file: Box<dyn FsWrite>,
    }

    impl FsWrite for CountedWrite {
        fn write_chunk(&mut self, bytes: &[u8]) -> io::Result<()> {
            let call = Call::WriteChunk(self.path.clone(), bytes.len());
            let file = &mut self.file;
            self.inner.run(&call, || file.write_chunk(bytes))
        }

        fn sync(&mut self) -> io::Result<()> {
            let call = Call::SyncStream(self.path.clone());
            let file = &mut self.file;
            self.inner.run(&call, || file.sync())
        }
    }

    /// A [`CountingFs`] stream's reader: each read a call.
    struct CountedRead {
        inner: Arc<Inner>,
        path: PathBuf,
        file: Box<dyn FsRead>,
    }

    impl FsRead for CountedRead {
        fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let call = Call::ReadChunk(self.path.clone(), buf.len());
            let file = &mut self.file;
            self.inner.run(&call, || file.read_chunk(buf))
        }
    }

    impl Fs for CountingFs {
        fn symlink_metadata(&self, path: &Path) -> io::Result<Metadata> {
            self.run(&Call::Metadata(path.to_owned()), || {
                StdFs.symlink_metadata(path)
            })
        }

        fn read_capped(&self, path: &Path, cap: u64) -> io::Result<Content> {
            self.run(&Call::Read(path.to_owned()), || {
                StdFs.read_capped(path, cap)
            })
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
            self.run(&Call::ReadDir(path.to_owned()), || StdFs.read_dir(path))
        }

        fn create_dir_all(&self, path: &Path) -> io::Result<()> {
            self.run(&Call::CreateDir(path.to_owned()), || {
                StdFs.create_dir_all(path)
            })
        }

        fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
            self.run(&Call::WriteNew(path.to_owned()), || {
                StdFs.write_new(path, bytes)
            })
        }

        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            self.run(&Call::Rename(from.to_owned(), to.to_owned()), || {
                StdFs.rename(from, to)
            })
        }

        fn set_readonly(&self, path: &Path, readonly: bool) -> io::Result<()> {
            self.run(&Call::SetReadonly(path.to_owned(), readonly), || {
                StdFs.set_readonly(path, readonly)
            })
        }

        fn remove_file(&self, path: &Path) -> io::Result<()> {
            self.run(&Call::Remove(path.to_owned()), || StdFs.remove_file(path))
        }

        fn sync_dir(&self, path: &Path) -> io::Result<()> {
            self.run(&Call::SyncDir(path.to_owned()), || StdFs.sync_dir(path))
        }

        fn point(&self, at: Point) -> io::Result<()> {
            self.run(&Call::Point(at), || Ok(()))
        }

        fn create_new_stream(&self, path: &Path) -> io::Result<Box<dyn FsWrite>> {
            let file = self.run(&Call::CreateStream(path.to_owned()), || {
                StdFs.create_new_stream(path)
            })?;
            Ok(Box::new(CountedWrite {
                inner: Arc::clone(&self.inner),
                path: path.to_owned(),
                file,
            }))
        }

        fn open_stream(&self, path: &Path, cap: u64) -> io::Result<Box<dyn FsRead>> {
            let file = self.run(&Call::OpenStream(path.to_owned()), || {
                StdFs.open_stream(path, cap)
            })?;
            Ok(Box::new(CountedRead {
                inner: Arc::clone(&self.inner),
                path: path.to_owned(),
                file,
            }))
        }
    }
}
