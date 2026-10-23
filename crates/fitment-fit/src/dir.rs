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

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
    /// `site`, for `files`.
    pub fn new(
        interpreter: impl Into<PathBuf>,
        site: impl Into<PathBuf>,
        python: impl Into<String>,
        files: &HelperFiles,
    ) -> Self {
        Self {
            format: MANIFEST_FORMAT.to_owned(),
            format_version: MANIFEST_FORMAT_VERSION,
            interpreter: interpreter.into(),
            site: site.into(),
            python: python.into(),
            binding: BINDING.to_owned(),
            kernel: KERNEL_NAME.to_owned(),
            helper_sha256: files.script_sha256().to_owned(),
            lock_sha256: files.lock_sha256().to_owned(),
        }
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
                    found: found.clone(),
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

    /// FL2, FL6: what every spawn checks first: the directory is still
    /// FL6's, the manifest agrees with this build and `files`, its
    /// interpreter and site exist, and the script and lock on disk are
    /// `files`. The manifest, if so.
    ///
    /// # Errors
    ///
    /// [`PrepareError`]: each `helper_unavailable`.
    pub fn check(&self, files: &HelperFiles) -> Result<Manifest, PrepareError> {
        Self::open(&self.path)?;
        let manifest = self.read_manifest()?;
        manifest.agrees(&self.path.join(MANIFEST), files)?;
        if !manifest.interpreter.is_file() {
            return Err(PrepareError::Missing {
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
            match fs::read(&path) {
                Ok(bytes) if sha256_hex(&bytes) == sha => {}
                Ok(_) => return Err(PrepareError::Changed { what, path }),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(PrepareError::Missing { what, path });
                }
                Err(e) => return Err(PrepareError::io(&path, &e)),
            }
        }
        Ok(manifest)
    }

    /// FY2, FL6: a fresh working directory for one helper under `tmp/`,
    /// mode 0700 where the OS has modes; the caller removes it after the
    /// helper is reaped.
    ///
    /// # Errors
    ///
    /// [`PrepareError::TmpUnwritable`].
    pub fn helper_dir(&self) -> Result<PathBuf, PrepareError> {
        let tmp = self.path.join(TMP);
        let n = HELPER_DIRS.fetch_add(1, Ordering::Relaxed);
        let dir = tmp.join(format!("helper-{}-{n}", std::process::id()));
        private_dir(&dir, false).map_err(|e| PrepareError::TmpUnwritable {
            path: tmp,
            why: e.to_string(),
        })?;
        Ok(dir)
    }

    /// Writes `name` whole: a temporary name, synced, renamed over it.
    fn write_whole(&self, name: &str, bytes: &[u8]) -> Result<(), PrepareError> {
        let path = self.path.join(name);
        let n = HELPER_DIRS.fetch_add(1, Ordering::Relaxed);
        let temporary = self
            .path
            .join(format!(".{name}.{}-{n}.tmp", std::process::id()));
        let written = (|| {
            let mut file = fs::File::create(&temporary)?;
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
