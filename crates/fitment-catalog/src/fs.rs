//! The file system seam (`docs/research/m5-catalog.md`, ST3, CT6, CT13):
//! every file operation the catalog makes goes through [`Fs`], so the
//! tests can count them, fail any one of them, and see the order of
//! renames, directory syncs and commits.

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
}

#[cfg(feature = "test-support")]
pub use support::{Call, CountingFs};

#[cfg(feature = "test-support")]
mod support {
    use std::fs::Metadata;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::{Content, Fs, Point, StdFs};

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
    }

    type Hook = Box<dyn Fn(&Call) + Send + Sync>;

    /// The real file system, counting and recording every call, failing
    /// the call numbered `k` (from 0) with a chosen error, and running a
    /// hook after each call that did not fail (CT13).
    #[derive(Default)]
    pub struct CountingFs {
        calls: Mutex<Vec<Call>>,
        fail_at: Option<(usize, io::ErrorKind)>,
        hook: Option<Hook>,
    }

    impl CountingFs {
        /// Counting only.
        pub fn new() -> Self {
            Self::default()
        }

        /// Failing call `k` with `kind`.
        pub fn failing_at(k: usize, kind: io::ErrorKind) -> Self {
            Self {
                fail_at: Some((k, kind)),
                ..Self::default()
            }
        }

        /// Running `hook` after each call.
        pub fn with_hook(hook: impl Fn(&Call) + Send + Sync + 'static) -> Self {
            Self {
                hook: Some(Box::new(hook)),
                ..Self::default()
            }
        }

        /// The calls so far.
        pub fn calls(&self) -> Vec<Call> {
            self.calls.lock().map(|c| c.clone()).unwrap_or_default()
        }

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
    }
}
