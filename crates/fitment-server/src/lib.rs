//! fitment-server: fitment's REST API (docs/research/m6-api.md).
//!
//! The library holds the server (SV2): [`Server::build`] opens the state
//! and the catalog in SD3's order, [`Server::router`] is the routes for
//! in-process tests, and [`Server::serve`] runs the accept loop on a bound
//! listener until its shutdown future ends. `main.rs` alone reads the
//! process's arguments, installs the log subscriber and exits.
//!
//! This is M6's PR 6: PR 2's skeleton (configuration, the four server
//! routes `/v1/health`, `/v1/ready`, `/v1/info` and `/v1/openapi.json`,
//! the hardening layers, the error registry, `server.db` with its lease,
//! the OpenAPI document), PR 3's authentication (users and bearer tokens,
//! roles on every route, `/v1/auth/*` and the offline admin commands,
//! [`admin`]), PR 4's uploads into the content-addressed object store
//! ([`uploads`], [`state::objects`]), and jobs ([`jobs`]): lanes, permits
//! and fairness, events, idempotency, waits, cancellation, deadlines,
//! retention, shutdown and recovery, with `features`, `assembly_read` and
//! `socket_extract` as their first kinds, assemblies and socket lines
//! ([`assemblies`]), and metrics ([`metrics`]); then reviews, imports and
//! matches over files and components ([`matching`], the socket gate in
//! [`gate`], `fitment.run` and the candidates view in [`runs`]). The
//! catalog's routes come in the PRs of the design's section 12.
#![forbid(unsafe_code)]
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod admin;
pub mod assemblies;
pub mod auth;
pub mod catalog;
pub mod config;
pub mod gate;
#[cfg(feature = "test-support")]
pub mod hooks;
pub mod http;
pub mod ids;
pub mod info;
pub mod jobs;
pub mod matching;
pub mod metrics;
mod net;
#[cfg(feature = "schema")]
pub mod openapi;
pub mod runs;
pub mod state;
pub mod uploads;

use std::future::Future;
use std::net::IpAddr;
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
    /// The pepper cannot be used (AU1: exit 2, or 1 for I/O).
    #[error(transparent)]
    Pepper(#[from] auth::PepperError),
    /// The dummy PHC string could not be made (AU2, exit 1).
    #[error("the password hash could not be prepared")]
    Hash,
    /// The configured roots cannot be resolved (exit 1); named as
    /// `state:` and `catalog:` (OP1).
    #[error("state: or catalog: cannot be resolved: {kind}")]
    Io {
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
                state::StateError::Io { .. }
                | state::StateError::Db(state::DbError::Sqlite(_) | state::DbError::Create(_)),
            )
            | Self::Pepper(auth::PepperError::Unreadable(_))
            | Self::Hash
            | Self::Io { .. } => 1,
            Self::Catalog(unavailable) => unavailable.reason.exit_code(),
            Self::Config(_) | Self::Nested | Self::State(_) | Self::Pepper(_) => 2,
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
    let nested = state::nested(&config.paths.catalog, &config.paths.state)
        .map_err(|e| StartError::Io { kind: e.kind() })?;
    if nested {
        return Err(StartError::Nested);
    }
    state::check(&config.paths.state)?;
    auth::pepper_of(&config.auth)?;
    catalog::CatalogDir::new(&config.paths.catalog, &config.catalog)
        .inspect()
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
    /// Authentication's state (AU1 to AU7).
    pub(crate) auth: Arc<auth::Auth>,
    pub(crate) trusted_proxies: Vec<IpAddr>,
    /// The fit verifier (SV8, SN11): a lock so a `test-support` seam can
    /// set the fake one (VT31); read once per job.
    pub(crate) verifier: std::sync::RwLock<matching::Verifier>,
    /// HX8: the socket documents parsed at once by requests.
    pub(crate) parse_slots: Arc<tokio::sync::Semaphore>,
    /// OB3, DC12: the reads of stored bytes requests make at once (a
    /// document served whole or hashed before it streams, a candidate
    /// page), `limits.read_concurrency`.
    pub(crate) read_slots: Arc<tokio::sync::Semaphore>,
    /// OP11, VT32: how far the build's socket rules version is raised, so
    /// a test meets a socket stored by an older build.
    #[cfg(feature = "test-support")]
    pub(crate) rules_shift: std::sync::atomic::AtomicU32,
    /// The object store (OB2, OB3).
    pub(crate) objects: state::Objects,
    /// Uploads' slots and staged bytes (UP4).
    pub(crate) uploads: Arc<uploads::Uploads>,
    /// The job system (JS1 to JS13).
    pub(crate) jobs: Arc<jobs::Jobs>,
    /// The counters (OP6).
    pub(crate) metrics: metrics::Metrics,
    /// Set when the server drains (SD1): readiness answers 503.
    pub(crate) draining: AtomicBool,
    /// When the warning of an untrusted `X-Forwarded-For` was last logged.
    pub(crate) forwarded_warned_ms: std::sync::atomic::AtomicI64,
    /// Cleared when a lease renewal fails or finds another holder (SD6).
    pub(crate) lease_ok: AtomicBool,
    /// Set once the lease is given up.
    pub(crate) released: AtomicBool,
    /// HX8: the `limit=all` listings built at once,
    /// `limits.list_all_concurrency`.
    pub(crate) list_all_slots: Arc<tokio::sync::Semaphore>,
    /// The catalog's read slots and `limit=all` slots each user holds (the
    /// review of PR 7b): at most half the read slots, and one listing.
    pub(crate) catalog_shares: http::Shares,
    /// M6-50: the synchronous deletes run at once (the review of PR 7b).
    pub(crate) delete_slots: Arc<tokio::sync::Semaphore>,
    /// How often the lease is renewed, in milliseconds (SD6; a
    /// `test-support` seam shortens it).
    pub(crate) lease_renewal_ms: std::sync::atomic::AtomicU64,
    #[cfg(feature = "test-support")]
    pub(crate) hooks: hooks::Hooks,
}

impl Shared {
    /// The fit verifier in force (SV8).
    pub(crate) fn verifier(&self) -> matching::Verifier {
        Arc::clone(
            &self
                .verifier
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// The auth part of JS9's sweep (AU6): blocking.
    pub(crate) fn sweep_auth(&self) -> Result<usize, state::DbError> {
        self.auth.sweep(&self.db, self.ids.now_ms())
    }

    /// The store's part of JS9's sweep (UP5, OB2, OB4): blocking.
    pub(crate) fn sweep_store(&self) -> Result<state::sweep::Swept, state::DbError> {
        let life = std::time::Duration::from_secs(
            self.config
                .limits
                .upload_minutes
                .saturating_add(1)
                .saturating_mul(60),
        );
        state::sweep::sweep(&self.db, &self.objects, self.ids.now_ms(), life)
    }

    /// The jobs' part of JS9's sweep (JB12, OB4): terminal jobs past their
    /// retention and not pinned deleted, their events and inputs with them;
    /// revisions of deleted lines no job cites deleted. Blocking. Returns
    /// the jobs and the revisions deleted.
    pub(crate) fn sweep_jobs(&self) -> Result<(usize, usize), state::DbError> {
        let now = self.ids.now_ms();
        let mut jobs = 0;
        loop {
            let deleted = self.db.expire_jobs(now, state::sweep::EXPIRE_BATCH)?;
            jobs += deleted;
            self.db.yield_writer();
            if deleted < usize::try_from(state::sweep::EXPIRE_BATCH).unwrap_or(usize::MAX) {
                break;
            }
        }
        let mut revisions = 0;
        loop {
            let deleted = self.db.sweep_revisions(state::sweep::EXPIRE_BATCH)?;
            revisions += deleted;
            self.db.yield_writer();
            if deleted == 0 {
                break;
            }
        }
        Ok((jobs, revisions))
    }

    /// How often the lease is renewed.
    pub(crate) fn lease_renewal(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.lease_renewal_ms.load(Ordering::SeqCst).max(1))
    }

    /// Gives the lease up, once (SD6): blocking, so `serve` calls it on
    /// the blocking pool; a server dropped without serving gives it up
    /// when its last handle goes.
    pub(crate) fn release(&self) {
        if !self.released.swap(true, Ordering::SeqCst) {
            // SD1's last writes: the batched last uses (AU6).
            if let Err(error) = self.sweep_auth() {
                tracing::warn!(error = %error, "the tokens' last uses could not be written");
            }
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
    /// lease (SD6), (5) the catalog's job writer, refusing a newer schema
    /// (SV5), and its instance id (CA8), (6) jobs' recovery (SD4), (7) the
    /// reader pool's size (CX1), (8) a stale catalog's reindex if
    /// configured (M6-28); `serve` starts the dispatcher and the sweep (9),
    /// and the caller binds the listener and announces it last (step 10).
    ///
    /// Blocking: call it before the runtime, or on the blocking pool.
    ///
    /// # Errors
    ///
    /// A [`StartError`], each with its exit code.
    pub fn build(config: Config) -> Result<Self, StartError> {
        Self::build_on(config, Arc::new(catalog::StdFs))
    }

    /// OP11: [`Server::build`] with the object store on `fs`, a
    /// `CountingFs` that records or fails each of its file calls (VT11).
    ///
    /// # Errors
    ///
    /// As [`Server::build`].
    #[cfg(feature = "test-support")]
    pub fn build_with_fs(config: Config, fs: Arc<dyn catalog::Fs>) -> Result<Self, StartError> {
        Self::build_on(config, fs)
    }

    fn build_on(config: Config, fs: Arc<dyn catalog::Fs>) -> Result<Self, StartError> {
        config.validate()?;
        let nested = state::nested(&config.paths.catalog, &config.paths.state)
            .map_err(|e| StartError::Io { kind: e.kind() })?;
        if nested {
            return Err(StartError::Nested);
        }
        let pepper = auth::pepper_of(&config.auth)?;
        if !pepper.is_set() {
            tracing::warn!(
                "auth.pepper_file is not set: passwords are hashed with argon2id without a pepper (AU1)"
            );
        }
        let auth = Arc::new(auth::Auth::new(&config.auth, pepper).map_err(|_| StartError::Hash)?);
        let ids = ids::Ids::default();
        let (db, _) = state::open(&config.paths.state, &ids.id("u"), ids.now_ms())?;
        let holder = ids.id("l");
        #[cfg(feature = "test-support")]
        if config.test.lease_stale_ms > 0 {
            db.set_lease_stale(config.test.lease_stale_ms);
        }
        db.take_lease(&holder, ids.now_ms())
            .map_err(|e| StartError::State(e.into()))?;
        // SD3 (5), (7): the catalog's job writer first, its instance id
        // read or set (CA8), and the reader pool's size (CX1).
        let catalog = catalog::CatalogDir::new(&config.paths.catalog, &config.catalog);
        let pool = usize::try_from(
            config
                .jobs
                .query_slots()
                .saturating_add(config.limits.read_concurrency)
                .saturating_add(1),
        )
        .unwrap_or(1);
        let mint = || ids::Ids::random_bits().to_be_bytes();
        if let Err(unavailable) = catalog.start(pool, mint) {
            // Not ours to keep: a refused start leaves the lease free.
            let _ = db.release_lease(&holder);
            return Err(StartError::Catalog(unavailable));
        }
        let trusted_proxies = config
            .trusted_proxies()?
            .into_iter()
            .map(|a| a.to_canonical())
            .collect();
        let verifier = matching::verifier(&config.fit);
        let parse_slots = usize::try_from(config.limits.parse_concurrency.max(1)).unwrap_or(1);
        let read_slots = usize::try_from(config.limits.read_concurrency.max(1)).unwrap_or(1);
        let list_all_slots =
            usize::try_from(config.limits.list_all_concurrency.max(1)).unwrap_or(1);
        let objects = state::Objects::new(&config.paths.state, fs);
        // SD2: no upload survives its connection, so a start finds only
        // abandoned temporary files.
        match objects.clear_tmp(None) {
            Ok(0) => {}
            Ok(removed) => tracing::info!(removed, "state:tmp emptied"),
            Err(error) => tracing::warn!(kind = ?error.kind(), "state:tmp could not be emptied"),
        }
        let uploads = Arc::new(uploads::Uploads::new(&config.limits));
        let jobs = Arc::new(jobs::Jobs::new(&config));
        tracing::info!("the state directory and the catalog are open");
        let server = Self {
            shared: Arc::new(Shared {
                config,
                db,
                catalog,
                holder,
                ids,
                auth,
                trusted_proxies,
                verifier: std::sync::RwLock::new(verifier),
                parse_slots: Arc::new(tokio::sync::Semaphore::new(parse_slots)),
                read_slots: Arc::new(tokio::sync::Semaphore::new(read_slots)),
                #[cfg(feature = "test-support")]
                rules_shift: std::sync::atomic::AtomicU32::new(0),
                objects,
                uploads,
                jobs,
                metrics: metrics::Metrics::default(),
                draining: AtomicBool::new(false),
                forwarded_warned_ms: std::sync::atomic::AtomicI64::new(0),
                lease_ok: AtomicBool::new(true),
                released: AtomicBool::new(false),
                list_all_slots: Arc::new(tokio::sync::Semaphore::new(list_all_slots)),
                catalog_shares: http::Shares::default(),
                delete_slots: Arc::new(tokio::sync::Semaphore::new(http::DELETE_CONCURRENCY)),
                lease_renewal_ms: std::sync::atomic::AtomicU64::new(
                    u64::try_from(net::LEASE_RENEWAL.as_millis()).unwrap_or(10_000),
                ),
                #[cfg(feature = "test-support")]
                hooks: hooks::Hooks::default(),
            }),
        };
        #[cfg(feature = "test-support")]
        server
            .shared
            .hooks
            .configure(&server.shared.config.test.hooks);
        // SD3 (6), SD4: recovery, before any request is answered.
        jobs::recover(&server.shared).map_err(|e| StartError::State(e.into()))?;
        // SD3 (8), M6-28: a stale catalog's reindex, if configured.
        if server.shared.config.jobs.reindex_on_start {
            jobs::reindex_on_start(&server.shared).map_err(|e| StartError::State(e.into()))?;
        }
        Ok(server)
    }

    /// The routes, with every layer (section 4.12), for `oneshot` tests
    /// and the accept loop.
    pub fn router(&self) -> axum::Router {
        http::router(Arc::clone(&self.shared))
    }

    /// Serves `listener` until `shutdown` ends (HX1), then drains (SD1): no
    /// new connection or job, streams told, in-flight requests given up to
    /// 10 s and running jobs `jobs.drain_seconds`, then the last writes and
    /// the lease released.
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
            auth: &self.shared.auth,
            db: &self.shared.db,
            objects: &self.shared.objects,
            uploads: &self.shared.uploads,
            jobs: &self.shared.jobs,
            catalog: &self.shared.catalog,
        }
    }

    /// Writes the batched `last_used_ms` values and deletes tokens dead
    /// for 30 days (the auth part of JS9's sweep; AU6, audit flaw 4).
    /// Blocking. Returns the tokens deleted.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn sweep_auth(&self) -> Result<usize, state::DbError> {
        self.shared.sweep_auth()
    }

    /// Deletes expired uploads, then unlinks the objects and files no row
    /// names and abandoned temporary files (the store's part of JS9's
    /// sweep; UP5, OB2, OB4). Blocking.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn sweep_store(&self) -> Result<state::sweep::Swept, state::DbError> {
        self.shared.sweep_store()
    }

    /// The jobs' part of JS9's sweep (JB12, OB4): expired job records and
    /// uncited revisions of deleted lines. Blocking. Returns how many of
    /// each were deleted.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn sweep_jobs(&self) -> Result<(usize, usize), state::DbError> {
        self.shared.sweep_jobs()
    }

    /// OP6's exposition, as the metrics listener serves it. Blocking.
    pub fn metrics_text(&self) -> String {
        metrics::render(&self.shared)
    }

    /// OP11: holds the dispatcher, so submitted jobs stay queued.
    #[cfg(feature = "test-support")]
    pub fn hold_jobs(&self) {
        self.shared.jobs.hold(true);
    }

    /// OP11, SN11, VT31: judges with `verifier` from now on, the fake that
    /// verifies every fit among them.
    #[cfg(feature = "test-support")]
    pub fn set_fit_verifier(&self, verifier: matching::Verifier) {
        *self
            .shared
            .verifier
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = verifier;
    }

    /// OP11, VT32: raises the build's socket rules version by `shift`, as
    /// an upgraded build would see every stored socket (MF11, JB2).
    #[cfg(feature = "test-support")]
    pub fn set_rules_shift(&self, shift: u32) {
        self.shared.rules_shift.store(shift, Ordering::SeqCst);
    }

    /// OP11, JS9: queues the daily catalog gc now, as `jobs.catalog_gc_at`
    /// does at its time: its job, or `None` when one is queued or running.
    /// Blocking.
    ///
    /// # Errors
    ///
    /// `server.db`'s.
    #[cfg(feature = "test-support")]
    pub fn queue_daily_gc(&self) -> Result<Option<String>, state::DbError> {
        jobs::queue_daily_gc(&self.shared).map(|row| row.map(|r| r.id))
    }

    /// OP11: lets a held dispatcher go on, starting what may run.
    #[cfg(feature = "test-support")]
    pub fn resume_jobs(&self) {
        self.shared.jobs.hold(false);
        self.shared.jobs.kick(&self.shared);
    }

    /// OP11: renews the lease every `interval` instead of 10 s, so a test
    /// sees a lost lease quickly. Set before [`Server::serve`].
    #[cfg(feature = "test-support")]
    pub fn set_lease_renewal(&self, interval: std::time::Duration) {
        self.shared.lease_renewal_ms.store(
            u64::try_from(interval.as_millis()).unwrap_or(u64::MAX),
            Ordering::SeqCst,
        );
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
    /// Authentication's counters and the limits' clock.
    pub auth: &'a auth::Auth,
    /// `server.db`, for seeding users and tokens with the fixed ids.
    pub db: &'a state::Db,
    /// The object store (OB2, OB3).
    pub objects: &'a state::Objects,
    /// Uploads' slots and staged bytes (UP4).
    pub uploads: &'a uploads::Uploads,
    /// The job system: its deadline and heartbeat seams, its counts.
    pub jobs: &'a jobs::Jobs,
    /// The catalog: its instance id (CA8), its barrier's flag (CX6).
    pub catalog: &'a catalog::CatalogDir,
}
