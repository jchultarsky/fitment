//! The catalog in the server (docs/research/m6-api.md, section 4.10): the
//! only module naming `fitment_catalog` (SV4), read as the module with its
//! submodules under `catalog/`, as `state::db` is (PR 3's As-built note).
//! Every call here is blocking and runs inside `spawn_blocking` (SV3).
//! `fitment_catalog::Error`'s `Display` names paths and is never sent (ER3,
//! CX7): each variant is mapped to a code and logged with paths relative
//! to the catalog's root (OP1).
//!
//! CX1: one `Catalog` per concurrent user of it. A pool of reader catalogs
//! (`query_slots + read_concurrency + 1`), each checked out by one blocking
//! task and returned after it, the catalog's directory checked first so a
//! catalog that became unavailable is answered as one; exactly one job
//! writer, opened at start with `catalog.write_busy_ms` (SD3), which reads
//! or sets the catalog's instance id (CA8) for the views' entity tags
//! (VN5, M6-63); and a short-lived writer per synchronous delete (CX3,
//! M6-50), opened with `catalog.delete_busy_ms`.

mod jobs;
mod read;
mod watch;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use fitment_catalog::{Catalog, Error, Options};
use serde::Serialize;

use crate::config::CatalogSection;
use crate::http::error::{ApiError, CatalogReason, ErrorCode};

pub use jobs::{
    Done, IngestFile, JobFailure, Matched, Searched, Selection, check_id, check_reference, gc,
    generation, ingest, prune, reindex, replace, search, selection, vacuum, verify,
};
pub use read::{Deleted, Got, ListPage, StatusFilter, Step, StepBytes};
pub use watch::{Event, Watch};

/// The catalog's schema this build reads (DC11).
pub const SCHEMA_VERSION: i64 = fitment_catalog::schema::SCHEMA_VERSION;

/// The `fitment.catalog` format's version (DC11).
pub const FORMAT_VERSION: u32 = fitment_catalog::document::CATALOG_FORMAT_VERSION;

/// The `fitment.catalog` format's name.
pub const FORMAT: &str = fitment_catalog::document::CATALOG_FORMAT;

/// `fitment.verify` (CA5).
pub const VERIFY_FORMAT: &str = fitment_catalog::document::VERIFY_FORMAT;

/// `fitment.gc` (CA5).
pub const GC_FORMAT: &str = fitment_catalog::document::GC_FORMAT;

/// The versions of `fitment.verify` and `fitment.gc`, each 1 (CA5).
pub const MAINTENANCE_FORMAT_VERSION: u32 = fitment_catalog::document::VERIFY_FORMAT_VERSION;

/// The upload cap equals the catalog's (SZ1): a file the server accepts is
/// never refused by ingest for size.
pub const DEFAULT_MAX_BYTES: u64 = fitment_catalog::DEFAULT_MAX_BYTES;

/// The catalog crate's file system seam, which the server's object store
/// is written against (OB2, CA7): named here, the one module naming
/// `fitment_catalog` (SV4), so a `CountingFs` fails every file call of an
/// upload, a stored document and an object read in turn (VT11).
pub use fitment_catalog::fs::{Content, Fs, FsRead, FsWrite, StdFs};

/// The catalog the server was configured with: its directory, the reader
/// pool, the job writer and the instance id (CX1, CA8).
pub struct CatalogDir {
    root: PathBuf,
    /// The readers' options: `catalog.read_busy_ms` (CX4).
    options: Options,
    write_busy_ms: u32,
    delete_busy_ms: u32,
    readers: Mutex<Vec<Catalog>>,
    pool_size: AtomicUsize,
    writer: Mutex<Option<Catalog>>,
    instance: OnceLock<String>,
    /// CX6: the WAL barrier is up (`fitment.info`'s `wal_barrier`).
    barrier: AtomicBool,
    /// CX6: the database's page size, read once at start: what the log's
    /// pages are counted in (the review of PR 7b).
    page_bytes: AtomicU64,
    /// VN5: the first 12 hex of the SHA-256 of the build's versions.
    build: String,
}

impl std::fmt::Debug for CatalogDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogDir")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

/// A catalog that cannot be opened, with the reason a client is told and
/// the error logged (with paths relative to the root, OP1).
#[derive(Debug)]
pub struct Unavailable {
    /// The reason (CX7).
    pub reason: CatalogReason,
    /// For the operator's log, paths relative to the root.
    pub log: String,
}

impl From<Unavailable> for ApiError {
    fn from(unavailable: Unavailable) -> Self {
        Self::catalog_unavailable(unavailable.reason)
    }
}

/// The catalog's state, `fitment.info`'s `catalog` object (DC11), and
/// `GET /v1/catalog`'s body.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CatalogInfo {
    /// The generation (DB8).
    pub generation: i64,
    /// Live entries.
    pub entries: u64,
    /// Indexed and searched.
    pub indexed: u64,
    /// Quarantined: never searched in a full run.
    pub quarantined: u64,
    /// Read by an older build (RX2).
    pub stale: u64,
    /// The flags a client checks (DC11, OP7).
    pub state: CatalogState,
}

/// `fitment.info`'s catalog flags (DC11).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CatalogState {
    /// Some entry is stale: catalog matches are refused until a reindex
    /// (RX3, CX5).
    pub stale: bool,
    /// The newest unended reindex job's state, `queued`, `running` or
    /// `cancelling`, or null (M6-28).
    pub reindex: Option<String>,
    /// The WAL barrier is up (CX6).
    pub wal_barrier: bool,
    /// A job lane's queue is full (JS5).
    pub queues_full: bool,
}

/// VN5: the build's part of a catalog view's entity tag: the first 12 hex
/// of the SHA-256 of the server's version and every version constant, since
/// the same generation read by another build can list another status.
fn build_tag() -> String {
    let text = format!(
        "fitment-server {}\nextractor {}\nsocket_rules {}\nsignature {}\nmatch_rules {}\n\
         properties {}\ncatalog_schema {SCHEMA_VERSION}\ncatalog_format {FORMAT_VERSION}\n",
        env!("CARGO_PKG_VERSION"),
        fitment_core::EXTRACTOR_VERSION,
        fitment_core::SOCKET_RULES_VERSION,
        fitment_core::SIGNATURE_VERSION,
        fitment_core::MATCH_RULES_VERSION,
        fitment_core::PROPERTIES_VERSION,
    );
    fitment_core::sha256_hex(text.as_bytes())[..12].to_owned()
}

impl CatalogDir {
    /// The catalog at `root`, its readers opened with `section`'s busy
    /// timeout (CX4), its writers with theirs (CX3).
    pub fn new(root: &Path, section: &CatalogSection) -> Self {
        let options = Options::default().with_busy_timeout_ms(section.read_busy_ms);
        let options = if section.max_db_pages > 0 {
            options.with_max_db_pages(section.max_db_pages)
        } else {
            options
        };
        Self {
            root: root.to_owned(),
            options,
            write_busy_ms: section.write_busy_ms,
            delete_busy_ms: section.delete_busy_ms,
            readers: Mutex::new(Vec::new()),
            pool_size: AtomicUsize::new(1),
            writer: Mutex::new(None),
            instance: OnceLock::new(),
            barrier: AtomicBool::new(false),
            page_bytes: AtomicU64::new(4096),
            build: build_tag(),
        }
    }

    /// SD3 (5) and (7): the job writer opened first, so any migration
    /// happens once before readers open (a newer schema refused, SV5); the
    /// instance id read or set through it (CA8), `mint` giving 16 random
    /// bytes; the reader pool sized to `pool_size` (CX1), filled as readers
    /// are returned. A catalog the server cannot write is refused here.
    ///
    /// # Errors
    ///
    /// [`Unavailable`] with its reason.
    pub fn start(
        &self,
        pool_size: usize,
        mint: impl FnOnce() -> [u8; 16],
    ) -> Result<(), Unavailable> {
        let mut writer = Catalog::open(
            &self.root,
            self.options.with_busy_timeout_ms(self.write_busy_ms),
        )
        .map_err(|e| self.unavailable(&e))?;
        let instance = writer.instance(mint).map_err(|e| self.unavailable(&e))?;
        let _ = self.instance.set(instance);
        let page = writer.page_size().map_err(|e| self.unavailable(&e))?;
        self.page_bytes.store(page.max(1), Ordering::SeqCst);
        *lock(&self.writer) = Some(writer);
        self.pool_size.store(pool_size.max(1), Ordering::SeqCst);
        Ok(())
    }

    /// Opens a reader (SD3, SV5: a newer schema is refused).
    ///
    /// # Errors
    ///
    /// [`Unavailable`] with its reason.
    pub fn open(&self) -> Result<Catalog, Unavailable> {
        Catalog::open(&self.root, self.options).map_err(|e| self.unavailable(&e))
    }

    /// OP4: checks the catalog writing nothing (`Catalog::inspect`: no
    /// connection, no pragma, no migration).
    ///
    /// # Errors
    ///
    /// [`Unavailable`] with its reason.
    pub fn inspect(&self) -> Result<(), Unavailable> {
        Catalog::inspect(&self.root)
            .map(drop)
            .map_err(|e| self.unavailable(&e))
    }

    /// The instance id (CA8), once started.
    pub fn instance(&self) -> &str {
        self.instance.get().map_or("", String::as_str)
    }

    /// VN5, M6-63: a catalog view's strong entity tag at `generation`:
    /// `"<instance>.g<generation>.<build>"`, so two catalogs at one
    /// generation never share one, nor two builds.
    pub fn etag(&self, generation: i64) -> String {
        format!("\"{}.g{generation}.{}\"", self.instance(), self.build)
    }

    /// RA7: the generation a view's entity tag `tag` names, if it is one
    /// of this catalog's and this build's (a strong tag, quoted); `None`
    /// for any other, `GET /v1/catalog`'s included.
    pub fn tag_generation(&self, tag: &str) -> Option<i64> {
        let inner = tag.strip_prefix('"')?.strip_suffix('"')?;
        let mut parts = inner.split('.');
        let (instance, generation, build) = (parts.next()?, parts.next()?, parts.next()?);
        let ours = parts.next().is_none() && instance == self.instance() && build == self.build;
        ours.then(|| generation.strip_prefix('g')?.parse().ok())?
    }

    /// VN5, M6-63: `GET /v1/catalog`'s entity tag: the views' at
    /// `info`'s generation, with 8 hex of the SHA-256 of its state's flags
    /// (a reindex's state, the barrier, a full queue, stale entries), so a
    /// change of state is a new tag at one generation (the review of PR
    /// 7b).
    pub fn info_etag(&self, info: &CatalogInfo) -> String {
        let state = &info.state;
        let text = format!(
            "stale {}\nreindex {}\nwal_barrier {}\nqueues_full {}\n",
            state.stale,
            state.reindex.as_deref().unwrap_or("-"),
            state.wal_barrier,
            state.queues_full
        );
        let hash = fitment_core::sha256_hex(text.as_bytes());
        format!(
            "\"{}.g{}.{}.{}\"",
            self.instance(),
            info.generation,
            self.build,
            &hash[..8]
        )
    }

    /// The generation as a reader reads it now (RA7).
    ///
    /// # Errors
    ///
    /// The catalog's, mapped (CX7).
    pub fn generation_now(&self) -> Result<i64, ApiError> {
        self.read(|catalog| catalog.generation())
    }

    /// CX6: the bytes of one of the database's pages.
    pub fn page_bytes(&self) -> u64 {
        self.page_bytes.load(Ordering::SeqCst)
    }

    /// JS3 (the review of PR 7b): the largest stored file's length, what a
    /// reindex or a deep verify holds of one file at a time.
    ///
    /// # Errors
    ///
    /// The catalog's, mapped (CX7).
    pub fn largest_blob(&self) -> Result<u64, ApiError> {
        self.read(|catalog| catalog.snapshot()?.largest_blob())
    }

    /// CX6: whether the WAL barrier is up.
    pub fn barrier(&self) -> bool {
        self.barrier.load(Ordering::SeqCst)
    }

    /// CX6: raises or lowers the barrier's flag.
    pub(crate) fn set_barrier(&self, up: bool) {
        self.barrier.store(up, Ordering::SeqCst);
    }

    /// CX1, CX2: `work` on a reader of the pool, the catalog's directory
    /// checked first (a catalog that became unavailable is answered with
    /// its reason, VT33); a reader that fails is dropped, and a fresh one
    /// opened next time.
    ///
    /// # Errors
    ///
    /// `work`'s, mapped (CX7), or `catalog_unavailable`.
    pub fn read<T>(
        &self,
        work: impl FnOnce(&mut Catalog) -> Result<T, Error>,
    ) -> Result<T, ApiError> {
        self.inspect().map_err(|unavailable| {
            tracing::warn!(error = %unavailable.log, "the catalog is unavailable");
            ApiError::from(unavailable)
        })?;
        let pooled = lock(&self.readers).pop();
        let mut catalog = match pooled {
            Some(catalog) => catalog,
            None => self.open().map_err(ApiError::from)?,
        };
        // A pooled reader sees a schema another build set meanwhile only
        // through its connection (VT33).
        if let Err(error) = catalog.check_schema() {
            return Err(self.mapped(&error));
        }
        let result = work(&mut catalog);
        match result {
            Ok(value) => {
                let mut readers = lock(&self.readers);
                if readers.len() < self.pool_size.load(Ordering::SeqCst) {
                    readers.push(catalog);
                }
                Ok(value)
            }
            Err(error) => Err(self.mapped(&error)),
        }
    }

    /// CX3: `work` on the job writer, the write lane's alone, opened again
    /// if a failure dropped it.
    ///
    /// # Errors
    ///
    /// `catalog_unavailable` when it cannot be opened.
    pub(crate) fn write<T>(&self, work: impl FnOnce(&mut Catalog) -> T) -> Result<T, ApiError> {
        self.inspect().map_err(|unavailable| {
            tracing::warn!(error = %unavailable.log, "the catalog is unavailable");
            ApiError::from(unavailable)
        })?;
        let mut writer = lock(&self.writer);
        if writer.is_none() {
            let opened = Catalog::open(
                &self.root,
                self.options.with_busy_timeout_ms(self.write_busy_ms),
            )
            .map_err(|e| ApiError::from(self.unavailable(&e)))?;
            *writer = Some(opened);
        }
        let catalog = writer.as_mut().ok_or_else(ApiError::internal)?;
        if let Err(error) = catalog.check_schema() {
            *writer = None;
            return Err(self.mapped(&error));
        }
        Ok(work(catalog))
    }

    /// CA3, CX6: a `PASSIVE` checkpoint on a reader: `(busy, log pages,
    /// pages checkpointed)`; readers pinning the log show as the second
    /// below the first.
    ///
    /// # Errors
    ///
    /// The catalog's, mapped (CX7).
    pub fn checkpoint_passive(&self) -> Result<(bool, i64, i64), ApiError> {
        self.read(|catalog| {
            let c = catalog.checkpoint_passive()?;
            Ok((c.busy, c.log_pages, c.checkpointed_pages))
        })
    }

    /// CA3, CX6: a `TRUNCATE` checkpoint on a reader: true when readers
    /// still held the log past the busy timeout.
    ///
    /// # Errors
    ///
    /// The catalog's, mapped (CX7).
    pub fn checkpoint_truncate(&self) -> Result<bool, ApiError> {
        self.read(|catalog| catalog.checkpoint_truncate().map(|c| c.busy))
    }

    /// The catalog's generation, counts and flags (DC11), read in one
    /// snapshot (CX2), the counts in one query.
    ///
    /// # Errors
    ///
    /// `catalog_unavailable`, `catalog_busy`, `catalog_damaged` or
    /// `io_error` (CX7).
    pub fn info(&self) -> Result<CatalogInfo, ApiError> {
        let (generation, counts) = self.read(|catalog| {
            let snapshot = catalog.snapshot()?;
            let counts = snapshot.status_counts()?;
            Ok((snapshot.generation(), counts))
        })?;
        Ok(CatalogInfo {
            generation,
            entries: counts.entries,
            indexed: counts.indexed,
            quarantined: counts.quarantined,
            stale: counts.stale,
            state: CatalogState {
                stale: counts.stale > 0,
                reindex: None,
                wal_barrier: self.barrier(),
                queues_full: false,
            },
        })
    }

    /// Why `error` makes the catalog unavailable (CX7), logged with paths
    /// relative to the root.
    fn unavailable(&self, error: &Error) -> Unavailable {
        let reason = match error {
            Error::Newer { .. } => CatalogReason::NewerSchema,
            Error::NotACatalog(_) | Error::NotEmpty(_) => CatalogReason::NotACatalog,
            Error::Linked(_) => CatalogReason::Linked,
            Error::NotUnicode(_) => CatalogReason::NotUnicode,
            _ => CatalogReason::Unreadable,
        };
        Unavailable {
            reason,
            log: self.relative(error),
        }
    }

    /// `error` as a code (CX7), logged first.
    pub(crate) fn mapped(&self, error: &Error) -> ApiError {
        let text = self.relative(error);
        match error {
            Error::Busy => {
                ApiError::new(ErrorCode::CatalogBusy, "the catalog is busy").retry_after(1)
            }
            Error::Corrupt(_) => {
                tracing::error!(error = %text, "the catalog holds a damaged row");
                ApiError::new(
                    ErrorCode::CatalogDamaged,
                    "the catalog holds a damaged row: run a verify job (POST /v1/catalog/verify)",
                )
            }
            Error::Io { .. } | Error::Database(_) => {
                tracing::error!(error = %text, "the catalog failed");
                ApiError::new(ErrorCode::IoError, "the catalog could not be read")
            }
            _ => {
                let unavailable = self.unavailable(error);
                tracing::warn!(error = %unavailable.log, "the catalog is unavailable");
                unavailable.into()
            }
        }
    }

    /// The error's text with the catalog's root written `catalog:` (OP1).
    pub(crate) fn relative(&self, error: &Error) -> String {
        let text = error.to_string();
        let root = self.root.display().to_string();
        let absolute = std::path::absolute(&self.root)
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let mut out = text;
        for prefix in [absolute, root] {
            if !prefix.is_empty() {
                let separated = format!("{prefix}{}", std::path::MAIN_SEPARATOR);
                out = out
                    .replace(&separated, "catalog:")
                    .replace(&prefix, "catalog:");
            }
        }
        out
    }

    /// The readers' options, with another busy timeout.
    fn options_with(&self, busy_ms: u32) -> Options {
        self.options.with_busy_timeout_ms(busy_ms)
    }

    /// The root, for the submodules.
    fn root(&self) -> &Path {
        &self.root
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// CA6: `fitment.catalog`'s schema, closed, for the OpenAPI document.
#[cfg(feature = "schema")]
pub fn schema() -> serde_json::Value {
    crate::openapi::closed_schema_of::<fitment_catalog::document::CatalogDocument>(
        FORMAT,
        FORMAT_VERSION,
    )
}

/// CA5: `fitment.verify`'s schema, closed.
#[cfg(feature = "schema")]
pub fn verify_schema() -> serde_json::Value {
    crate::openapi::closed_schema_of::<fitment_catalog::document::VerifyDocument>(
        VERIFY_FORMAT,
        MAINTENANCE_FORMAT_VERSION,
    )
}

/// CA5: `fitment.gc`'s schema, closed.
#[cfg(feature = "schema")]
pub fn gc_schema() -> serde_json::Value {
    crate::openapi::closed_schema_of::<fitment_catalog::document::GcDocument>(
        GC_FORMAT,
        MAINTENANCE_FORMAT_VERSION,
    )
}

/// OP11: the catalog's test connection, for the tests' raw edits (a busy
/// holder, a stale row): opened on the server's catalog directory.
///
/// # Errors
///
/// [`Unavailable`].
#[cfg(feature = "test-support")]
pub fn test_catalog(dir: &CatalogDir) -> Result<Catalog, Unavailable> {
    dir.open()
}
