//! The catalog in the server (docs/research/m6-api.md, section 4.10): the
//! only module naming `fitment_catalog` (SV4). Every call here is blocking
//! and runs inside `spawn_blocking` (SV3). `fitment_catalog::Error`'s
//! `Display` names paths and is never sent (ER3, CX7): each variant is
//! mapped to a code and logged with paths relative to the catalog's root
//! (OP1).

use std::path::{Path, PathBuf};

use fitment_catalog::{Catalog, Error, Options};
use serde::Serialize;

use crate::config::CatalogSection;
use crate::http::error::{ApiError, CatalogReason, ErrorCode};

/// The catalog's schema this build reads (DC11).
pub const SCHEMA_VERSION: i64 = fitment_catalog::schema::SCHEMA_VERSION;

/// The `fitment.catalog` format's version (DC11).
pub const FORMAT_VERSION: u32 = fitment_catalog::document::CATALOG_FORMAT_VERSION;

/// The `fitment.catalog` format's name.
pub const FORMAT: &str = fitment_catalog::document::CATALOG_FORMAT;

/// The upload cap equals the catalog's (SZ1): a file the server accepts is
/// never refused by ingest for size.
pub const DEFAULT_MAX_BYTES: u64 = fitment_catalog::DEFAULT_MAX_BYTES;

/// The catalog crate's file system seam, which the server's object store
/// is written against (OB2, CA7): named here, the one module naming
/// `fitment_catalog` (SV4), so a `CountingFs` fails every file call of an
/// upload, a stored document and an object read in turn (VT11).
pub use fitment_catalog::fs::{Content, Fs, FsRead, FsWrite, StdFs};

/// The catalog the server was configured with.
#[derive(Debug, Clone)]
pub struct CatalogDir {
    root: PathBuf,
    options: Options,
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

/// The catalog's state, `fitment.info`'s `catalog` object (DC11).
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
    /// A reindex job `queued` or `running`, or null (M6-28; PR 7).
    pub reindex: Option<String>,
    /// The WAL barrier is up (CX6; PR 7).
    pub wal_barrier: bool,
    /// A job lane's queue is full (JS5).
    pub queues_full: bool,
}

impl CatalogDir {
    /// The catalog at `root`, opened with `section`'s reader busy timeout
    /// (CX4).
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
        }
    }

    /// Opens the catalog (SD3, SV5: a newer schema is refused).
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

    /// The catalog's generation, counts and flags (DC11), read in one
    /// snapshot (CX2), the counts in one query.
    ///
    /// # Errors
    ///
    /// `catalog_unavailable`, `catalog_busy`, `catalog_damaged` or
    /// `io_error` (CX7).
    pub fn info(&self) -> Result<CatalogInfo, ApiError> {
        let mut catalog = self.open().map_err(|unavailable| {
            tracing::warn!(error = %unavailable.log, "the catalog is unavailable");
            ApiError::from(unavailable)
        })?;
        let snapshot = catalog.snapshot().map_err(|e| self.mapped(&e))?;
        let counts = snapshot.status_counts().map_err(|e| self.mapped(&e))?;
        let mut info = CatalogInfo {
            generation: snapshot.generation(),
            entries: counts.entries,
            indexed: counts.indexed,
            quarantined: counts.quarantined,
            stale: counts.stale,
            state: CatalogState {
                stale: false,
                reindex: None,
                wal_barrier: false,
                queues_full: false,
            },
        };
        info.state.stale = info.stale > 0;
        Ok(info)
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
    fn mapped(&self, error: &Error) -> ApiError {
        let text = self.relative(error);
        match error {
            Error::Busy => {
                ApiError::new(ErrorCode::CatalogBusy, "the catalog is busy").retry_after(1)
            }
            Error::Corrupt(_) => {
                tracing::error!(error = %text, "the catalog holds a damaged row");
                ApiError::new(
                    ErrorCode::CatalogDamaged,
                    "the catalog holds a damaged row: run a verify job",
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
    fn relative(&self, error: &Error) -> String {
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
}

/// CA6: `fitment.catalog`'s schema, closed, for the OpenAPI document.
#[cfg(feature = "schema")]
pub fn schema() -> serde_json::Value {
    crate::openapi::closed_schema_of::<fitment_catalog::document::CatalogDocument>(
        FORMAT,
        FORMAT_VERSION,
    )
}
