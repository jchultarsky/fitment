//! FL2, FL6: the fit directory and its manifest. `fitment fit setup`
//! (PR 2b) is the only step that runs uv; it writes the helper's script
//! and lock, resolves the environment and writes `manifest.json`. A run
//! and the server only read: before every spawn the manifest is read, the
//! interpreter and the site it names must exist, and the script and lock
//! on disk must hash to what this build embeds and the manifest states.
//!
//! ```text
//! <fit.dir>/fit_helper.py          the helper (FG2)
//! <fit.dir>/fit_helper.py.lock     its lock
//! <fit.dir>/manifest.json          the interpreter, the site, the versions, the hashes
//! <fit.dir>/tmp/                   each helper's own 0700 working directory
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use fitment_core::sha256_hex;
use serde::{Deserialize, Serialize};

use crate::{BINDING, KERNEL_NAME, PYTHON};

/// The helper script's name in the fit directory.
pub const SCRIPT: &str = "fit_helper.py";
/// Its lock's.
pub const LOCK: &str = "fit_helper.py.lock";
/// The manifest's.
pub const MANIFEST: &str = "manifest.json";
/// The helpers' working directories' parent.
pub const TMP: &str = "tmp";

/// The manifest's own format, and its version.
pub const MANIFEST_FORMAT: &str = "fitment.fit-manifest";
/// The manifest's format version.
pub const MANIFEST_FORMAT_VERSION: u32 = 1;

/// The sentence every refusal ends with (FL2).
const SETUP: &str = "run `fitment fit setup`";

/// The helper's script and lock as this build embeds them (FG2), with
/// their SHA-256s. PR 2b embeds the real ones; a test names its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperFiles {
    script: Vec<u8>,
    lock: Vec<u8>,
    script_sha256: String,
    lock_sha256: String,
}

impl HelperFiles {
    /// The script and lock, by their bytes.
    pub fn new(script: impl Into<Vec<u8>>, lock: impl Into<Vec<u8>>) -> Self {
        let (script, lock) = (script.into(), lock.into());
        Self {
            script_sha256: sha256_hex(&script),
            lock_sha256: sha256_hex(&lock),
            script,
            lock,
        }
    }

    /// The script's bytes.
    pub fn script(&self) -> &[u8] {
        &self.script
    }

    /// The lock's bytes.
    pub fn lock(&self) -> &[u8] {
        &self.lock
    }

    /// The script's SHA-256.
    pub fn script_sha256(&self) -> &str {
        &self.script_sha256
    }

    /// The lock's SHA-256.
    pub fn lock_sha256(&self) -> &str {
        &self.lock_sha256
    }
}

/// Why no helper can be spawned from a fit directory (FL2, FL6): each
/// `helper_unavailable`. [`PrepareError::sentence`] names no path, for a
/// document (FV9); `Display` names it, for a person.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PrepareError {
    /// The fit directory has no manifest.
    #[error("{dir} holds no helper manifest: {SETUP}")]
    NoManifest {
        /// The fit directory.
        dir: PathBuf,
    },
    /// The manifest cannot be read, or is not one.
    #[error("the helper manifest {path} cannot be read ({why}): {SETUP}")]
    BadManifest {
        /// The manifest.
        path: PathBuf,
        /// Why.
        why: String,
    },
    /// A file the manifest or the directory needs is missing.
    #[error("the helper's {what} {path} is missing: {SETUP}")]
    Missing {
        /// What: the interpreter, the site, the script or the lock.
        what: &'static str,
        /// Where it should be.
        path: PathBuf,
    },
    /// The script or lock on disk, or the manifest's hash of one, is not
    /// what this build embeds.
    #[error("the helper's {what} {path} is not the one this build embeds: {SETUP}")]
    Changed {
        /// The script or the lock.
        what: &'static str,
        /// The file.
        path: PathBuf,
    },
    /// The manifest names another binding, kernel or Python than this
    /// build needs.
    #[error("the helper manifest names {what} {found}, and this build needs {needed}: {SETUP}")]
    Disagrees {
        /// The binding, the kernel or Python.
        what: &'static str,
        /// What the manifest names.
        found: String,
        /// What this build needs.
        needed: String,
    },
    /// The fit directory, or something inside it, is a symbolic link.
    #[error("{path} is a symbolic link, which the fit directory refuses")]
    SymbolicLink {
        /// The link.
        path: PathBuf,
    },
    /// The fit directory, its `tmp/`, a file in it or the interpreter
    /// another user could write (Unix): group- or world-writable, or owned
    /// by another user than this one or root (the review of PR 2a, P4).
    #[error("{path} is {why}, which the fit directory refuses: {SETUP}")]
    NotPrivate {
        /// The path.
        path: PathBuf,
        /// Why: group- or world-writable, or owned by another user.
        why: &'static str,
    },
    /// The fit directory is not a directory.
    #[error("{path} is not a directory")]
    NotADirectory {
        /// The path.
        path: PathBuf,
    },
    /// The helpers' working directories cannot be made.
    #[error("the helpers' payload directory {path} is not writable ({why})")]
    TmpUnwritable {
        /// `tmp/`.
        path: PathBuf,
        /// Why.
        why: String,
    },
    /// Another I/O failure.
    #[error("{path}: {why}")]
    Io {
        /// The path.
        path: PathBuf,
        /// Why.
        why: String,
    },
}

impl PrepareError {
    /// The refusal as a fixed sentence with no path in it (FV9), for a
    /// fit's `helper_unavailable`.
    pub fn sentence(&self) -> String {
        match self {
            Self::NoManifest { .. } => {
                format!("the fit directory holds no helper manifest: {SETUP}")
            }
            Self::BadManifest { .. } => format!("the helper manifest cannot be read: {SETUP}"),
            Self::Missing { what, .. } => format!("the helper's {what} is missing: {SETUP}"),
            Self::Changed { what, .. } => {
                format!("the helper's {what} is not the one this build embeds: {SETUP}")
            }
            Self::Disagrees {
                what,
                found,
                needed,
            } => format!(
                "the helper manifest names {what} {found}, and this build needs {needed}: {SETUP}"
            ),
            Self::SymbolicLink { .. } => {
                "the fit directory holds a symbolic link, which it refuses".to_owned()
            }
            Self::NotPrivate { why, .. } => {
                format!("the fit directory holds a file that is {why}, which it refuses: {SETUP}")
            }
            Self::NotADirectory { .. } => "the fit directory is not a directory".to_owned(),
            Self::TmpUnwritable { .. } => {
                "the helpers' payload directory is not writable".to_owned()
            }
            Self::Io { .. } => "the fit directory cannot be read".to_owned(),
        }
    }

    fn io(path: &Path, e: &std::io::Error) -> Self {
        Self::Io {
            path: path.to_owned(),
            why: e.to_string(),
        }
    }
}

/// The manifest `fitment fit setup` writes (FL2): what to spawn, and what
/// the helper must state in its handshake (FH3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Manifest {
    /// [`MANIFEST_FORMAT`].
    pub format: String,
    /// [`MANIFEST_FORMAT_VERSION`].
    pub format_version: u32,
    /// The base interpreter, `sys._base_executable`, never a launcher.
    pub interpreter: PathBuf,
    /// Its SHA-256, checked before a spawn (the review of PR 2a, P4).
    pub interpreter_sha256: String,
    /// The environment's site-packages directory, which the helper adds
    /// with `site.addsitedir`.
    pub site: PathBuf,
    /// The interpreter's version, such as `3.13.5`.
    pub python: String,
    /// The binding the environment holds, [`BINDING`].
    pub binding: String,
    /// The kernel, [`KERNEL_NAME`].
    pub kernel: String,
    /// The script's SHA-256.
    pub helper_sha256: String,
    /// The lock's SHA-256.
    pub lock_sha256: String,
}

impl Manifest {
    /// The manifest of an environment whose base interpreter is
    /// `interpreter`, at Python `python`, with its site-packages at
    /// `site`, for `files`; the interpreter is read for its SHA-256.
    ///
    /// # Errors
    ///
    /// [`PrepareError::Missing`] or [`PrepareError::Io`] if the
    /// interpreter cannot be read.
    pub fn new(
        interpreter: impl Into<PathBuf>,
        site: impl Into<PathBuf>,
        python: impl Into<String>,
        files: &HelperFiles,
    ) -> Result<Self, PrepareError> {
        let interpreter = interpreter.into();
        let interpreter_sha256 = interpreter_sha256(&interpreter)?;
        Ok(Self {
            format: MANIFEST_FORMAT.to_owned(),
            format_version: MANIFEST_FORMAT_VERSION,
            interpreter,
            interpreter_sha256,
            site: site.into(),
            python: python.into(),
            binding: BINDING.to_owned(),
            kernel: KERNEL_NAME.to_owned(),
            helper_sha256: files.script_sha256().to_owned(),
            lock_sha256: files.lock_sha256().to_owned(),
        })
    }

    /// FL2: the manifest agrees with this build: its format, binding,
    /// kernel and Python series, and the hashes of `files`.
    fn agrees(&self, path: &Path, files: &HelperFiles) -> Result<(), PrepareError> {
        if self.format != MANIFEST_FORMAT || self.format_version != MANIFEST_FORMAT_VERSION {
            return Err(PrepareError::BadManifest {
                path: path.to_owned(),
                why: format!("{} {}", self.format, self.format_version),
            });
        }
        let python_series = self
            .python
            .strip_prefix(PYTHON)
            .is_some_and(|rest| rest.starts_with('.'));
        for (what, found, needed, fine) in [
            ("binding", &self.binding, BINDING, self.binding == BINDING),
            (
                "kernel",
                &self.kernel,
                KERNEL_NAME,
                self.kernel == KERNEL_NAME,
            ),
            ("Python", &self.python, PYTHON, python_series),
        ] {
            if !fine {
                return Err(PrepareError::Disagrees {
                    what,
                    found: crate::protocol::quoted(found),
                    needed: needed.to_owned(),
                });
            }
        }
        if self.helper_sha256 != files.script_sha256() {
            return Err(PrepareError::Changed {
                what: "script",
                path: path.to_owned(),
            });
        }
        if self.lock_sha256 != files.lock_sha256() {
            return Err(PrepareError::Changed {
                what: "lock",
                path: path.to_owned(),
            });
        }
        if !self.interpreter.is_absolute() || !self.site.is_absolute() {
            return Err(PrepareError::BadManifest {
                path: path.to_owned(),
                why: "a relative path".to_owned(),
            });
        }
        Ok(())
    }
}

/// A fit directory (FL6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FitDir {
    path: PathBuf,
}

/// Each helper's working directory gets a fresh name.
static HELPER_DIRS: AtomicU64 = AtomicU64::new(0);

impl FitDir {
    /// FL6's default: `XDG_CACHE_HOME/fitment/fit`, else
    /// `HOME/.cache/fitment/fit`, else `LOCALAPPDATA/fitment/fit`.
    pub fn default_path() -> Option<PathBuf> {
        let under = |var: &str, rest: &[&str]| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(|v| rest.iter().fold(PathBuf::from(v), |p, r| p.join(r)))
        };
        under("XDG_CACHE_HOME", &["fitment", "fit"])
            .or_else(|| under("HOME", &[".cache", "fitment", "fit"]))
            .or_else(|| under("LOCALAPPDATA", &["fitment", "fit"]))
    }

    /// FL6: makes the fit directory and its `tmp/` (mode 0700 where the
    /// OS has modes), then opens it.
    ///
    /// # Errors
    ///
    /// [`PrepareError`] if it cannot be made, or [`FitDir::open`] refuses it.
    pub fn create(path: &Path) -> Result<Self, PrepareError> {
        for dir in [path.to_owned(), path.join(TMP)] {
            if fs::symlink_metadata(&dir).is_err() {
                private_dir(&dir, true).map_err(|e| PrepareError::io(&dir, &e))?;
            }
        }
        Self::open(path)
    }

    /// FL6: an existing fit directory, refused if it, its `tmp/` or a file
    /// in it is a symbolic link.
    ///
    /// # Errors
    ///
    /// [`PrepareError`].
    pub fn open(path: &Path) -> Result<Self, PrepareError> {
        let meta = fs::symlink_metadata(path).map_err(|e| PrepareError::io(path, &e))?;
        if meta.file_type().is_symlink() {
            return Err(PrepareError::SymbolicLink {
                path: path.to_owned(),
            });
        }
        if !meta.is_dir() {
            return Err(PrepareError::NotADirectory {
                path: path.to_owned(),
            });
        }
        let dir = Self {
            path: path.to_owned(),
        };
        for name in [TMP, SCRIPT, LOCK, MANIFEST] {
            let inside = dir.path.join(name);
            if fs::symlink_metadata(&inside).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(PrepareError::SymbolicLink { path: inside });
            }
        }
        Ok(dir)
    }

    /// Its path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// FL6: writes the script and the lock, each whole (a temporary name,
    /// synced, then renamed).
    ///
    /// # Errors
    ///
    /// [`PrepareError::Io`].
    pub fn install(&self, files: &HelperFiles) -> Result<(), PrepareError> {
        self.write_whole(SCRIPT, files.script())?;
        self.write_whole(LOCK, files.lock())
    }

    /// FL2: writes the manifest, whole.
    ///
    /// # Errors
    ///
    /// [`PrepareError::Io`].
    pub fn write_manifest(&self, manifest: &Manifest) -> Result<(), PrepareError> {
        let mut bytes = serde_json::to_vec_pretty(manifest).map_err(|e| PrepareError::Io {
            path: self.path.join(MANIFEST),
            why: e.to_string(),
        })?;
        bytes.push(b'\n');
        self.write_whole(MANIFEST, &bytes)
    }

    /// The manifest, as written.
    ///
    /// # Errors
    ///
    /// [`PrepareError::NoManifest`] or [`PrepareError::BadManifest`].
    pub fn read_manifest(&self) -> Result<Manifest, PrepareError> {
        let path = self.path.join(MANIFEST);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(PrepareError::NoManifest {
                    dir: self.path.clone(),
                });
            }
            Err(e) => return Err(PrepareError::io(&path, &e)),
        };
        serde_json::from_slice(&bytes).map_err(|e| PrepareError::BadManifest {
            path,
            why: e.to_string(),
        })
    }

    /// FL2, FL6, FY5: what every spawn checks first: the directory is
    /// still FL6's, and on Unix no other user can write it, its `tmp/`, its
    /// files or the interpreter; the manifest agrees with this build and
    /// `files`; its interpreter (by its SHA-256) and site exist; and the
    /// script and lock on disk are `files`. The manifest, if so.
    ///
    /// # Errors
    ///
    /// [`PrepareError`]: each `helper_unavailable`.
    pub fn check(&self, files: &HelperFiles) -> Result<Manifest, PrepareError> {
        Self::open(&self.path)?;
        let tmp = self.path.join(TMP);
        private(&self.path, None)?;
        private(&tmp, None)?;
        let me = self.owner()?;
        for name in [TMP, MANIFEST, SCRIPT, LOCK] {
            private(&self.path.join(name), Some(me))?;
        }
        private(&self.path, Some(me))?;
        let manifest = self.read_manifest()?;
        manifest.agrees(&self.path.join(MANIFEST), files)?;
        if !manifest.interpreter.is_file() {
            return Err(PrepareError::Missing {
                what: "interpreter",
                path: manifest.interpreter.clone(),
            });
        }
        private(&manifest.interpreter, Some(me))?;
        if interpreter_sha256(&manifest.interpreter)? != manifest.interpreter_sha256 {
            return Err(PrepareError::Changed {
                what: "interpreter",
                path: manifest.interpreter.clone(),
            });
        }
        if !manifest.site.is_dir() {
            return Err(PrepareError::Missing {
                what: "site",
                path: manifest.site.clone(),
            });
        }
        for (what, name, sha) in [
            ("script", SCRIPT, files.script_sha256()),
            ("lock", LOCK, files.lock_sha256()),
        ] {
            let path = self.path.join(name);
            if hash_file(what, &path)? != sha {
                return Err(PrepareError::Changed { what, path });
            }
        }
        Ok(manifest)
    }

    /// The current user's id, without `libc`: the owner of a directory
    /// just made under `tmp/` (Unix; 0 elsewhere, where nothing reads it).
    fn owner(&self) -> Result<u32, PrepareError> {
        let probe = self.helper_dir()?;
        let owner = owner_of(&probe);
        remove_probe(&probe);
        owner.map_err(|e| PrepareError::io(&probe, &e))
    }

    /// FY2, FL6: a fresh working directory for one helper under `tmp/`,
    /// mode 0700 where the OS has modes; the caller removes it after the
    /// helper is reaped. A name a stale directory holds is passed over
    /// (the review of PR 2a, P3).
    ///
    /// # Errors
    ///
    /// [`PrepareError::TmpUnwritable`].
    pub fn helper_dir(&self) -> Result<PathBuf, PrepareError> {
        let tmp = self.path.join(TMP);
        let mut last = None;
        for _ in 0..HELPER_DIR_TRIES {
            let n = HELPER_DIRS.fetch_add(1, Ordering::Relaxed);
            let dir = tmp.join(format!("helper-{}-{n}", std::process::id()));
            match private_dir(&dir, false) {
                Ok(()) => return Ok(dir),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => {
                    return Err(PrepareError::TmpUnwritable {
                        path: tmp,
                        why: e.to_string(),
                    });
                }
            }
        }
        Err(PrepareError::TmpUnwritable {
            path: tmp,
            why: last.map_or_else(String::new, |e| e.to_string()),
        })
    }

    /// The review of PR 2a (P3): removes every helper's directory under
    /// `tmp/` older than a day, which no helper with a deadline can still
    /// hold. Best effort.
    pub fn sweep(&self) {
        let Ok(entries) = fs::read_dir(self.path.join(TMP)) else {
            return;
        };
        let day = std::time::Duration::from_secs(24 * 3600);
        for entry in entries.flatten() {
            let stale = entry.file_name().to_string_lossy().starts_with("helper-")
                && entry.metadata().is_ok_and(|m| {
                    m.is_dir()
                        && m.modified()
                            .ok()
                            .and_then(|t| t.elapsed().ok())
                            .is_some_and(|age| age > day)
                });
            if stale {
                crate::process::remove_dir(&entry.path());
            }
        }
    }

    /// Writes `name` whole: a temporary name, synced, renamed over it.
    fn write_whole(&self, name: &str, bytes: &[u8]) -> Result<(), PrepareError> {
        let path = self.path.join(name);
        let n = HELPER_DIRS.fetch_add(1, Ordering::Relaxed);
        let temporary = self
            .path
            .join(format!(".{name}.{}-{n}.tmp", std::process::id()));
        let written = (|| {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                // Only its owner writes it (P4), whatever the umask.
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, &path)
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&temporary);
            return Err(PrepareError::io(&path, &e));
        }
        sync_dir(&self.path);
        Ok(())
    }
}

/// The number the next helper directory of this process tries.
#[cfg(feature = "test-support")]
pub(crate) fn next_helper_dir() -> u64 {
    HELPER_DIRS.load(Ordering::Relaxed)
}

/// How many names [`FitDir::helper_dir`] tries.
const HELPER_DIR_TRIES: usize = 1024;

/// What identifies a file's contents without reading them: its length,
/// modification time and, on Unix, its inode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    inode: u64,
}

impl Stamp {
    fn of(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        #[cfg(unix)]
        let inode = std::os::unix::fs::MetadataExt::ino(&meta);
        #[cfg(not(unix))]
        let inode = 0;
        Some(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            inode,
        })
    }
}

/// Every interpreter this process hashed, with the stamp it had then.
static HASHED: Mutex<BTreeMap<PathBuf, (Stamp, String)>> = Mutex::new(BTreeMap::new());

/// P4: the interpreter's SHA-256, read again only when its file is not the
/// one this process last hashed (its length, time and inode): a file only
/// its owner or root can write (checked before) changes no other way.
fn interpreter_sha256(path: &Path) -> Result<String, PrepareError> {
    let stamp = Stamp::of(path);
    let mut hashed = HASHED.lock().unwrap_or_else(PoisonError::into_inner);
    if let (Some(stamp), Some((known, sha))) = (stamp, hashed.get(path)) {
        if stamp == *known {
            return Ok(sha.clone());
        }
    }
    let sha = hash_file("interpreter", path)?;
    if let Some(stamp) = stamp {
        hashed.insert(path.to_owned(), (stamp, sha.clone()));
    }
    Ok(sha)
}

/// The SHA-256 of the file `what` at `path`.
fn hash_file(what: &'static str, path: &Path) -> Result<String, PrepareError> {
    match fs::read(path) {
        Ok(bytes) => Ok(sha256_hex(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(PrepareError::Missing {
            what,
            path: path.to_owned(),
        }),
        Err(e) => Err(PrepareError::io(path, &e)),
    }
}

/// P4's rule for one file: why another user could write it, if one could:
/// group- or world-writable (`mode & 0o022`), or owned by a user other
/// than `me` and root (who can run anything as anyone already).
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn not_private(mode: u32, owner: u32, me: Option<u32>) -> Option<&'static str> {
    if mode & 0o022 != 0 {
        Some("group- or world-writable")
    } else if me.is_some_and(|me| owner != me && owner != 0) {
        Some("owned by another user")
    } else {
        None
    }
}

/// P4 on Unix: `path` is not writable by another user, and owned by `me`
/// or root when `me` is given. On Windows the fit directory's ACLs are
/// not read: the trust boundary is the user's account (FY5).
fn private(path: &Path, me: Option<u32>) -> Result<(), PrepareError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            // A file not there is refused as missing, by what reads it.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(PrepareError::io(path, &e)),
        };
        if let Some(why) = not_private(meta.mode(), meta.uid(), me) {
            return Err(PrepareError::NotPrivate {
                path: path.to_owned(),
                why,
            });
        }
    }
    #[cfg(not(unix))]
    let _ = (path, me);
    Ok(())
}

/// The owner of `path` (Unix; 0 elsewhere).
fn owner_of(path: &Path) -> std::io::Result<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path).map(|m| m.uid())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(0)
    }
}

fn remove_probe(path: &Path) {
    let _ = fs::remove_dir(path);
}

/// A directory only its owner can enter, where the OS has modes.
fn private_dir(path: &Path, parents: bool) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(parents);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// A rename is durable once its directory is synced (Unix only; Windows
/// cannot open a directory as a file).
fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = fs::File::open(path) {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The review of PR 2a (P4): a file another user could write is
    /// refused, whatever the user's umask made of it.
    #[test]
    fn only_a_file_no_other_user_writes_is_private() {
        let me = 501;
        for (mode, owner, expected) in [
            (0o40_700, me, None),
            (0o100_600, me, None),
            (0o100_755, 0, None),
            (0o40_777, me, Some("group- or world-writable")),
            (0o100_666, me, Some("group- or world-writable")),
            (0o100_620, me, Some("group- or world-writable")),
            (0o41_777, 0, Some("group- or world-writable")),
            (0o100_600, me + 1, Some("owned by another user")),
        ] {
            assert_eq!(
                not_private(mode, owner, Some(me)),
                expected,
                "{mode:o} {owner}"
            );
        }
        assert_eq!(
            not_private(0o40_700, me + 1, None),
            None,
            "no owner checked"
        );
    }
}
