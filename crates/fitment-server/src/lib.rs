//! fitment-server: fitment's REST API (docs/research/m6-api.md).
//!
//! The library holds the server (SV2): [`Server::build`] opens the state
//! and the catalog in SD3's order, [`Server::router`] is the routes for
//! in-process tests, and [`Server::serve`] runs the accept loop on a bound
//! listener until its shutdown future ends. `main.rs` alone reads the
//! process's arguments, installs the log subscriber and exits.
//!
//! This is M6's PR 2, the skeleton: configuration, the four server routes
//! (`/v1/health`, `/v1/ready`, `/v1/info`, `/v1/openapi.json`), the
//! hardening layers, the error registry, `server.db` with its lease, and
//! the OpenAPI document. Authentication, uploads, jobs and the catalog's
//! routes come in the PRs of the design's section 12.
#![forbid(unsafe_code)]
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod catalog;
pub mod config;
#[cfg(feature = "test-support")]
pub mod hooks;
pub mod http;
pub mod ids;
pub mod info;
pub mod matching;
mod net;
#[cfg(feature = "schema")]
pub mod openapi;
pub mod state;

use std::future::Future;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::net::TcpListener;

pub use config::{Config, ConfigError, Flags};

/// The committed OpenAPI document, served as these bytes (VN7).
pub const OPENAPI: &[u8] = include_bytes!("../openapi.json");

/// Why the server refused to start (SD3, VT33), each with its exit code.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StartError {
    /// The configuration cannot be used (exit 2).
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// One directory holds the other (SV6, exit 2).
    #[error("the catalog and state directories must not contain each other")]
    Nested,
    /// The state directory or `server.db` (exit 2, 3 for a newer schema,
    /// 1 for I/O).
    #[error(transparent)]
    State(#[from] state::StateError),
    /// The catalog cannot be opened (its reason's exit code, ER2).
    #[error("the catalog cannot be opened: {}", .0.log)]
    Catalog(catalog::Unavailable),
    /// A path cannot be resolved (exit 1).
    #[error("{}: {kind}", path.display())]
    Io {
        /// The path.
        path: PathBuf,
        /// What failed.
        kind: std::io::ErrorKind,
    },
}

impl StartError {
    /// The process's exit code: 2 for what the operator must change, 3 for
    /// a database of a newer schema (as DB5), 1 for I/O.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::State(state::StateError::Db(state::DbError::Newer { .. })) => 3,
            Self::State(
                state::StateError::Io { .. } | state::StateError::Db(state::DbError::Sqlite(_)),
            )
            | Self::Io { .. } => 1,
            Self::Catalog(unavailable) => unavailable.reason.exit_code(),
            Self::Config(_) | Self::Nested | Self::State(_) => 2,
        }
    }
}

/// OP4's check, for `check-config`: the configuration, the nesting (SV6),
/// the state directory's marker and `server.db`'s header (read, never
/// written), and the catalog opened (SV5).
///
/// # Errors
///
/// A [`StartError`], each with its exit code.
pub fn check(config: &Config) -> Result<(), StartError> {
    config.validate()?;
    let nested =
        state::nested(&config.paths.catalog, &config.paths.state).map_err(|e| StartError::Io {
            path: config.paths.state.clone(),
            kind: e.kind(),
        })?;
    if nested {
        return Err(StartError::Nested);
    }
    state::check(&config.paths.state)?;
    catalog::CatalogDir::new(&config.paths.catalog, &config.catalog)
        .open()
        .map(drop)
        .map_err(StartError::Catalog)
}

/// What every route and the accept loop share.
pub(crate) struct Shared {
    pub(crate) config: Config,
    pub(crate) db: state::Db,
    pub(crate) catalog: catalog::CatalogDir,
    /// This process's lease holder id (SD6).
    pub(crate) holder: String,
    pub(crate) ids: ids::Ids,
    pub(crate) trusted_proxies: Vec<IpAddr>,
    pub(crate) verifier: matching::Verifier,
    /// Set when the server drains (SD1): readiness answers 503.
    pub(crate) draining: AtomicBool,
    /// Cleared when a lease renewal fails or finds another holder (SD6).
    pub(crate) lease_ok: AtomicBool,
    /// Set once the lease is given up.
    pub(crate) released: AtomicBool,
    #[cfg(feature = "test-support")]
    pub(crate) hooks: hooks::Hooks,
}

impl Shared {
    /// Gives the lease up, once (SD6): blocking, so `serve` calls it on
    /// the blocking pool; a server dropped without serving gives it up
    /// when its last handle goes.
    pub(crate) fn release(&self) {
        if !self.released.swap(true, Ordering::SeqCst) {
            if let Err(error) = self.db.release_lease(&self.holder) {
                tracing::warn!(error = %error, "the lease could not be released");
            }
        }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.release();
    }
}

/// The server: its state opened, its catalog checked, not yet listening.
#[derive(Clone)]
pub struct Server {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server").finish_non_exhaustive()
    }
}

impl Server {
    /// Opens what the server needs, in SD3's order: (2) the nesting check
    /// (SV6), (3) `server.db`, made on the first start (OB1), (4) the
    /// lease (SD6), (5) the catalog, refusing a newer schema (SV5). Jobs'
    /// recovery, the reader pool, a stale catalog's reindex and the
    /// dispatcher (steps 6 to 9) come with the PRs that build them; the
    /// caller binds the listener and announces it last (step 10).
    ///
    /// Blocking: call it before the runtime, or on the blocking pool.
    ///
    /// # Errors
    ///
    /// A [`StartError`], each with its exit code.
    pub fn build(config: Config) -> Result<Self, StartError> {
        config.validate()?;
        let nested = state::nested(&config.paths.catalog, &config.paths.state).map_err(|e| {
            StartError::Io {
                path: config.paths.state.clone(),
                kind: e.kind(),
            }
        })?;
        if nested {
            return Err(StartError::Nested);
        }
        let ids = ids::Ids::default();
        let (db, _) = state::open(&config.paths.state, &ids.id("u"), ids.now_ms())?;
        let holder = ids.id("l");
        db.take_lease(&holder, ids.now_ms())
            .map_err(|e| StartError::State(e.into()))?;
        let catalog = catalog::CatalogDir::new(&config.paths.catalog, &config.catalog);
        if let Err(unavailable) = catalog.open() {
            // Not ours to keep: a refused start leaves the lease free.
            let _ = db.release_lease(&holder);
            return Err(StartError::Catalog(unavailable));
        }
        let trusted_proxies = config.trusted_proxies()?;
        let verifier = matching::verifier(&config.fit);
        tracing::info!("the state directory and the catalog are open");
        Ok(Self {
            shared: Arc::new(Shared {
                config,
                db,
                catalog,
                holder,
                ids,
                trusted_proxies,
                verifier,
                draining: AtomicBool::new(false),
                lease_ok: AtomicBool::new(true),
                released: AtomicBool::new(false),
                #[cfg(feature = "test-support")]
                hooks: hooks::Hooks::default(),
            }),
        })
    }

    /// The routes, with every layer (section 4.12), for `oneshot` tests
    /// and the accept loop.
    pub fn router(&self) -> axum::Router {
        http::router(Arc::clone(&self.shared))
    }

    /// Serves `listener` until `shutdown` ends (HX1), then drains: no new
    /// connection, in-flight requests given up to 10 s, the lease released
    /// (SD1's first and last steps; the jobs' drain comes with PR 5).
    ///
    /// # Errors
    ///
    /// The listener's, if it fails for good.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()> + Send,
    ) -> std::io::Result<()> {
        net::serve(self, listener, shutdown).await
    }

    /// Starts draining (SD1): readiness turns `503 shutting_down`.
    pub fn drain(&self) {
        self.shared.draining.store(true, Ordering::SeqCst);
    }

    /// The configuration in force.
    pub fn config(&self) -> &Config {
        &self.shared.config
    }

    /// OP11: the seams, under `test-support` only.
    #[cfg(feature = "test-support")]
    pub fn seams(&self) -> Seams<'_> {
        Seams {
            ids: &self.shared.ids,
            hooks: &self.shared.hooks,
        }
    }

    pub(crate) fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }
}

/// OP11's seams: fixed ids and time, and the hook points.
#[cfg(feature = "test-support")]
#[derive(Debug)]
#[non_exhaustive]
pub struct Seams<'a> {
    /// Ids and the clock.
    pub ids: &'a ids::Ids,
    /// The hook points.
    pub hooks: &'a hooks::Hooks,
}
