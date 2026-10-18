//! The configuration (docs/research/m6-api.md, OP2, OP3): flags, then the
//! environment, then the TOML file, then the defaults. The only module
//! that reads the environment (SV7). Unknown keys are refused, so a typo
//! fails the start; secrets are read only from files it names.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The environment variable naming the configuration file (OP2).
pub const CONFIG_VAR: &str = "FITMENT_SERVER_CONFIG";

/// Each key's variable: `FITMENT_SERVER__<SECTION>__<KEY>` (OP2), the
/// separators doubled so that variables an orchestrator injects for a
/// service named `fitment-server` (Kubernetes' `FITMENT_SERVER_PORT` and
/// the like) are never read as keys.
pub const VAR_PREFIX: &str = "FITMENT_SERVER__";

/// The family of the server's variables; one of it that is neither
/// [`CONFIG_VAR`] nor a key's is ignored, with a warning.
pub const FAMILY: &str = "FITMENT_SERVER_";

/// The whole configuration (OP3).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct Config {
    /// `[server]`.
    pub server: ServerSection,
    /// `[paths]`.
    pub paths: Paths,
    /// `[limits]`.
    pub limits: Limits,
    /// `[jobs]`.
    pub jobs: Jobs,
    /// `[catalog]`.
    pub catalog: CatalogSection,
    /// `[match]`.
    #[serde(rename = "match")]
    pub matching: MatchSection,
    /// `[auth]`.
    pub auth: AuthSection,
    /// `[fit]`.
    pub fit: Fit,
    /// `[log]`.
    pub log: Log,
    /// `[metrics]`.
    pub metrics: Metrics,
    /// `[test]`: OP11's seams for the built binary under `test-support`
    /// only (VT10's aborts at hook points); a release build refuses the
    /// section as an unknown key.
    #[cfg(feature = "test-support")]
    #[serde(default)]
    pub test: TestSection,
    /// Variables of the family that named no key and were ignored, for
    /// the startup log's warning (OP2).
    #[serde(skip)]
    pub ignored: Vec<String>,
}

/// `[server]` (HX1, HX3, HX4, HX7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ServerSection {
    /// The address to bind (M6-20: loopback by default).
    pub listen: String,
    /// Open connections at most; a further one is closed at accept (HX1).
    pub max_connections: u32,
    /// Peers whose `X-Request-Id` and `X-Forwarded-For` are believed
    /// (HX7, AU7): exact addresses.
    pub trusted_proxies: Vec<String>,
    /// Exact origins allowed by CORS; none installs no CORS layer (HX4).
    pub cors_origins: Vec<String>,
    /// The time to read a request's headers (HX1).
    pub header_timeout_seconds: u32,
    /// A non-streaming handler's time (HX3).
    pub handler_timeout_seconds: u32,
    /// How long a write may make no progress before the connection is
    /// closed (review S1): a client that never reads holds no slot.
    pub write_timeout_seconds: u32,
    /// The longest a connection lives (review S1); above
    /// `limits.upload_minutes`.
    pub max_connection_seconds: u32,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8080".into(),
            max_connections: 1024,
            trusted_proxies: Vec::new(),
            cors_origins: Vec::new(),
            header_timeout_seconds: 10,
            handler_timeout_seconds: 30,
            write_timeout_seconds: 30,
            max_connection_seconds: 3600,
        }
    }
}

/// `[paths]`: both required (OP3), neither inside the other (SV6).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct Paths {
    /// The catalog's directory, an M5 catalog.
    pub catalog: PathBuf,
    /// The server's state directory (section 6.1).
    pub state: PathBuf,
}

/// `[limits]` (section 13.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
#[allow(missing_docs)] // each key is section 13.1's, documented there
pub struct Limits {
    pub max_upload_mib: u64,
    pub max_socket_mib: u64,
    pub max_json_kib: u64,
    pub max_candidates: u64,
    pub max_catalog_candidates: u64,
    pub upload_concurrency: u32,
    /// UP4: a user's uploads at once, over all their tokens.
    pub upload_concurrency_per_user: u32,
    pub read_concurrency: u32,
    pub parse_concurrency: u32,
    pub list_all_concurrency: u32,
    pub staging_gib: u64,
    pub user_quota_gib: u64,
    pub upload_ttl_hours: u64,
    pub upload_minutes: u64,
    pub upload_min_kib_s: u64,
    /// HX3: the longest wait for an upload's next chunk.
    pub upload_chunk_seconds: u64,
    /// HX3: the window `upload_min_kib_s` is averaged over; before it
    /// passes, the minimum applies pro rata.
    pub upload_window_seconds: u64,
    /// HX3 (PR 4's review): the time an upload's first bytes have.
    pub upload_first_byte_seconds: u64,
    /// UP4 (PR 4's review): a user's live uploads at most.
    pub user_max_uploads: u64,
    pub page_max: u32,
    pub wait_max_seconds: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_upload_mib: 128,
            max_socket_mib: 16,
            max_json_kib: 1024,
            max_candidates: 10_000,
            max_catalog_candidates: 100_000,
            upload_concurrency: 8,
            upload_concurrency_per_user: 2,
            read_concurrency: 16,
            parse_concurrency: 4,
            list_all_concurrency: 2,
            staging_gib: 8,
            user_quota_gib: 10,
            upload_ttl_hours: 24,
            upload_minutes: 30,
            upload_min_kib_s: 64,
            upload_chunk_seconds: 30,
            upload_window_seconds: 30,
            upload_first_byte_seconds: 5,
            user_max_uploads: 10_000,
            page_max: 1000,
            wait_max_seconds: 60,
        }
    }
}

impl Limits {
    /// The upload keys' checks (UP2, UP4, HX3): each at least 1, so no
    /// upload is refused or cut by a zero.
    ///
    /// # Errors
    ///
    /// The first value that fails.
    pub fn validate_uploads(&self) -> Result<(), ConfigError> {
        let counts = [self.upload_concurrency, self.upload_concurrency_per_user];
        let sizes = [
            self.max_upload_mib,
            self.staging_gib,
            self.user_quota_gib,
            self.upload_ttl_hours,
            self.upload_minutes,
            self.upload_min_kib_s,
            self.upload_chunk_seconds,
            self.upload_window_seconds,
            self.upload_first_byte_seconds,
            self.user_max_uploads,
        ];
        if counts.contains(&0) || sizes.contains(&0) {
            return Err(value_error(
                "limits",
                "every upload limit must be at least 1",
            ));
        }
        Ok(())
    }
}

/// `[jobs]` (JS1 to JS5, JS9, JB7, JB12, SD1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
#[allow(missing_docs)] // each key is section 4.14's OP3, documented there
pub struct Jobs {
    /// 0: `available_parallelism()`.
    pub cpu: u32,
    pub match_threads: u32,
    pub memory_mib: u64,
    pub match_entry_kib: u64,
    pub queue_query: u32,
    pub queue_write: u32,
    pub queue_maintenance: u32,
    pub per_owner_running: u32,
    pub per_owner_queued: u32,
    pub deadline_extract_minutes: u64,
    pub deadline_match_minutes: u64,
    pub deadline_verify_minutes: u64,
    pub retention_days: u64,
    pub sweep_minutes: u64,
    pub reindex_on_start: bool,
    pub catalog_gc_at: String,
    pub drain_seconds: u64,
    /// JS10: event streams a user holds at once, over all their tokens.
    pub streams_per_user: u32,
    /// JS10: event streams at once on the server.
    pub streams: u32,
}

impl Default for Jobs {
    fn default() -> Self {
        Self {
            cpu: 0,
            match_threads: 4,
            memory_mib: 4096,
            match_entry_kib: 16,
            queue_query: 64,
            queue_write: 16,
            queue_maintenance: 4,
            per_owner_running: 2,
            per_owner_queued: 16,
            deadline_extract_minutes: 10,
            deadline_match_minutes: 60,
            deadline_verify_minutes: 60,
            retention_days: 30,
            sweep_minutes: 60,
            reindex_on_start: true,
            catalog_gc_at: String::new(),
            drain_seconds: 25,
            streams_per_user: 8,
            streams: 256,
        }
    }
}

impl Jobs {
    /// The CPU budget: `cpu`, or the machine's parallelism (JS2).
    pub fn cpu_budget(&self) -> u32 {
        if self.cpu > 0 {
            return self.cpu;
        }
        std::thread::available_parallelism()
            .ok()
            .and_then(|n| u32::try_from(n.get()).ok())
            .unwrap_or(1)
    }

    /// `query_slots = max(1, cpu / match_threads)` (JS1).
    pub fn query_slots(&self) -> u32 {
        (self.cpu_budget() / self.match_threads.max(1)).max(1)
    }
}

/// `[catalog]` (CX3, CX4, CX6, SZ5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
#[allow(missing_docs)] // each key is section 4.14's OP3, documented there
pub struct CatalogSection {
    pub write_busy_ms: u32,
    pub delete_busy_ms: u32,
    pub read_busy_ms: u32,
    pub wal_barrier_mib: u64,
    pub barrier_max_seconds: u64,
    /// 0: no page quota.
    pub max_db_pages: u64,
}

impl Default for CatalogSection {
    fn default() -> Self {
        Self {
            write_busy_ms: 60_000,
            delete_busy_ms: 10_000,
            read_busy_ms: 5_000,
            wal_barrier_mib: 256,
            barrier_max_seconds: 60,
            max_db_pages: 0,
        }
    }
}

/// `[match]` (SN3, M6-12).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct MatchSection {
    /// Forces `require_reviewed` true; never false (SN3).
    pub require_reviewed_floor: bool,
}

/// `[auth]` (AU1, AU4, AU7, AU10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
#[allow(missing_docs)] // each key is section 4.14's OP3, documented there
pub struct AuthSection {
    /// The pepper's file (mode 0600); never its content in a log (OP1).
    pub pepper_file: String,
    pub token_default_days: u32,
    pub token_max_days: u32,
    pub hash_concurrency: u32,
    /// How long a login waits for an argon2 slot (AU2), in seconds.
    pub hash_wait_seconds: u32,
    pub max_live_tokens: u32,
    pub login_buckets: u32,
    /// AU7: each user's requests a second, over all their tokens.
    pub rate_per_second: u32,
    /// AU7: and at most this many at once.
    pub rate_burst: u32,
    /// AU7: login attempts per client address per minute.
    pub login_per_minute: u32,
    /// AU7: failures per (username, address) per `login_failure_minutes`.
    pub login_failures: u32,
    pub login_failure_minutes: u32,
    /// AU10: tokens a user mints a minute through `POST /v1/auth/tokens`,
    /// so dead tokens cannot pile up faster than the sweep (review).
    pub mints_per_minute: u32,
}

impl Default for AuthSection {
    fn default() -> Self {
        Self {
            pepper_file: String::new(),
            token_default_days: 90,
            token_max_days: 365,
            hash_concurrency: 2,
            hash_wait_seconds: 10,
            max_live_tokens: 20,
            login_buckets: 10_000,
            rate_per_second: 50,
            rate_burst: 100,
            login_per_minute: 30,
            login_failures: 10,
            login_failure_minutes: 15,
            mints_per_minute: 10,
        }
    }
}

impl AuthSection {
    /// The checks a value's type cannot make (AU4, AU7, AU10).
    ///
    /// # Errors
    ///
    /// The first value that fails.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if [
            self.token_default_days,
            self.token_max_days,
            self.hash_concurrency,
            self.hash_wait_seconds,
            self.max_live_tokens,
            self.login_buckets,
            self.rate_per_second,
            self.rate_burst,
            self.login_per_minute,
            self.login_failures,
            self.login_failure_minutes,
            self.mints_per_minute,
        ]
        .contains(&0)
        {
            return Err(value_error(
                "auth",
                "every count and time must be at least 1",
            ));
        }
        if self.token_default_days > self.token_max_days {
            return Err(value_error(
                "auth.token_default_days",
                "at most auth.token_max_days",
            ));
        }
        Ok(())
    }
}

/// `[fit]` (OP9, SN11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct Fit {
    /// The fit verifier: `"none"` in M6 (`NoFitVerifier`).
    pub verifier: String,
}

impl Default for Fit {
    fn default() -> Self {
        Self {
            verifier: "none".into(),
        }
    }
}

/// `[log]` (OP1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct Log {
    /// `json` or `text`.
    pub format: String,
    /// The filter when `RUST_LOG` is unset.
    pub filter: String,
}

impl Default for Log {
    fn default() -> Self {
        Self {
            format: "json".into(),
            filter: "info".into(),
        }
    }
}

/// `[test]` (OP11, `test-support` only): what each named hook point does
/// in the process, as `"abort"`, `"panic"` or `"delay:<ms>"`.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct TestSection {
    /// Hook point to action.
    pub hooks: BTreeMap<String, String>,
    /// A lease renewed within this many milliseconds is another server's;
    /// 0: SD6's 30 s. A test restarting a killed child sets it small.
    pub lease_stale_ms: i64,
}

/// `[metrics]` (OP6): off when empty.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct Metrics {
    /// The metrics listener's address; empty: off.
    pub listen: String,
}

/// The flags that override a key (OP2): the only ones.
#[derive(Debug, Clone, Default)]
pub struct Flags {
    /// `--config PATH`.
    pub config: Option<PathBuf>,
    /// `--listen ADDR`.
    pub listen: Option<String>,
    /// `--catalog DIR`.
    pub catalog: Option<PathBuf>,
    /// `--state DIR`.
    pub state: Option<PathBuf>,
    /// `--log-format json|text`.
    pub log_format: Option<String>,
}

/// A configuration that cannot be used: the start fails, exit 2 (OP2,
/// VT33). Messages name the operator's own file, keys and variables.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The file cannot be read.
    #[error("cannot read the configuration file {path}: {kind}")]
    Unreadable {
        /// The file.
        path: PathBuf,
        /// What failed.
        kind: std::io::ErrorKind,
    },
    /// The file is not TOML, or a key is unknown or of the wrong type.
    #[error("the configuration is invalid: {0}")]
    Invalid(String),
    /// A value is out of range or a required key is missing.
    #[error("{key}: {message}")]
    Value {
        /// The key, as `paths.catalog`.
        key: String,
        /// Why.
        message: String,
    },
}

fn value_error(key: &str, message: impl Into<String>) -> ConfigError {
    ConfigError::Value {
        key: key.to_owned(),
        message: message.into(),
    }
}

impl Config {
    /// Reads the configuration from the process's environment and `flags`
    /// (OP2): flags, then `FITMENT_SERVER__<SECTION>__<KEY>`, then the file
    /// (`--config` or `FITMENT_SERVER_CONFIG`), then the defaults.
    ///
    /// # Errors
    ///
    /// A [`ConfigError`] for an unreadable file, an unknown key or
    /// variable, a wrong type or a value out of range.
    #[allow(clippy::disallowed_methods)] // SV7: the one module that reads the environment
    pub fn load(flags: &Flags) -> Result<Self, ConfigError> {
        let vars: BTreeMap<String, String> = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .filter(|(k, _)| k.starts_with(FAMILY))
            .collect();
        Self::from_sources(flags, &vars)
    }

    /// [`Config::load`] over the variables given, for tests.
    ///
    /// # Errors
    ///
    /// As [`Config::load`].
    pub fn from_sources(
        flags: &Flags,
        vars: &BTreeMap<String, String>,
    ) -> Result<Self, ConfigError> {
        Self::checked(flags, vars)
    }

    fn from_sources_unchecked(
        flags: &Flags,
        vars: &BTreeMap<String, String>,
    ) -> Result<Self, ConfigError> {
        let file = flags
            .config
            .clone()
            .or_else(|| vars.get(CONFIG_VAR).map(PathBuf::from));
        let mut table = match &file {
            Some(path) => read_file(path)?,
            None => toml::Table::new(),
        };
        let ignored = overlay_vars(&mut table, vars)?;
        let mut config: Self = table
            .try_into()
            .map_err(|e: toml::de::Error| ConfigError::Invalid(e.message().to_owned()))?;
        config.apply(flags);
        config.ignored = ignored;
        Ok(config)
    }

    /// [`Config::from_sources`], then [`Config::validate`].
    fn checked(flags: &Flags, vars: &BTreeMap<String, String>) -> Result<Self, ConfigError> {
        let config = Self::from_sources_unchecked(flags, vars)?;
        config.validate()?;
        Ok(config)
    }

    /// The configuration of AU9's offline commands: the same sources, but
    /// only `paths.state` is required, since they never open the catalog.
    ///
    /// # Errors
    ///
    /// As [`Config::load`], `paths.catalog` excepted.
    #[allow(clippy::disallowed_methods)] // SV7: the one module that reads the environment
    pub fn load_offline(flags: &Flags) -> Result<Self, ConfigError> {
        let vars: BTreeMap<String, String> = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .filter(|(k, _)| k.starts_with(FAMILY))
            .collect();
        Self::offline_from_sources(flags, &vars)
    }

    /// [`Config::load_offline`] over the variables given, for tests.
    ///
    /// # Errors
    ///
    /// As [`Config::load_offline`].
    pub fn offline_from_sources(
        flags: &Flags,
        vars: &BTreeMap<String, String>,
    ) -> Result<Self, ConfigError> {
        let config = Self::from_sources_unchecked(flags, vars)?;
        if config.paths.state.as_os_str().is_empty() {
            return Err(value_error(
                "paths.state",
                "required: the server's state directory",
            ));
        }
        if crate::state::has_parent_component(&config.paths.state) {
            return Err(value_error("paths.state", "no `..`"));
        }
        config.auth.validate()?;
        Ok(config)
    }

    fn apply(&mut self, flags: &Flags) {
        if let Some(listen) = &flags.listen {
            self.server.listen.clone_from(listen);
        }
        if let Some(catalog) = &flags.catalog {
            self.paths.catalog.clone_from(catalog);
        }
        if let Some(state) = &flags.state {
            self.paths.state.clone_from(state);
        }
        if let Some(format) = &flags.log_format {
            self.log.format.clone_from(format);
        }
    }

    /// The checks a value's type cannot make (OP3, HX4, OP9).
    ///
    /// # Errors
    ///
    /// The first value that fails.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.paths.catalog.as_os_str().is_empty() {
            return Err(value_error(
                "paths.catalog",
                "required: the catalog's directory",
            ));
        }
        if self.paths.state.as_os_str().is_empty() {
            return Err(value_error(
                "paths.state",
                "required: the server's state directory",
            ));
        }
        for (key, path) in [
            ("paths.catalog", &self.paths.catalog),
            ("paths.state", &self.paths.state),
        ] {
            if crate::state::has_parent_component(path) {
                return Err(value_error(
                    key,
                    "no `..`: write the directory without it, so the nesting check (SV6) is sound",
                ));
            }
        }
        self.listen_addr()?;
        if self.server.max_connections == 0 {
            return Err(value_error("server.max_connections", "must be at least 1"));
        }
        let server = &self.server;
        if [
            server.header_timeout_seconds,
            server.handler_timeout_seconds,
            server.write_timeout_seconds,
            server.max_connection_seconds,
        ]
        .contains(&0)
        {
            return Err(value_error("server", "timeouts must be at least 1 second"));
        }
        self.trusted_proxies()?;
        for origin in &self.server.cors_origins {
            if origin == "*" || !(origin.starts_with("http://") || origin.starts_with("https://")) {
                return Err(value_error(
                    "server.cors_origins",
                    "exact origins only, such as https://app.example.org; never a wildcard",
                ));
            }
        }
        if !matches!(self.log.format.as_str(), "json" | "text") {
            return Err(value_error("log.format", "json or text"));
        }
        if self.fit.verifier != "none" {
            return Err(value_error(
                "fit.verifier",
                "this build knows only \"none\" (OP9)",
            ));
        }
        if [
            self.jobs.queue_query,
            self.jobs.queue_write,
            self.jobs.queue_maintenance,
            self.jobs.per_owner_running,
            self.jobs.per_owner_queued,
            self.jobs.streams_per_user,
            self.jobs.streams,
            self.jobs.match_threads,
        ]
        .contains(&0)
            || self.jobs.memory_mib == 0
            || self.jobs.deadline_extract_minutes == 0
            || self.jobs.sweep_minutes == 0
        {
            return Err(value_error(
                "jobs",
                "every count and time must be at least 1",
            ));
        }
        if !self.metrics.listen.is_empty() && self.metrics.listen.parse::<SocketAddr>().is_err() {
            return Err(value_error(
                "metrics.listen",
                "an address such as 127.0.0.1:9464, or empty for none",
            ));
        }
        if self.limits.max_json_kib == 0 || self.limits.page_max == 0 {
            return Err(value_error("limits", "sizes must be at least 1"));
        }
        self.limits.validate_uploads()?;
        self.auth.validate()
    }

    /// `server.listen` as an address.
    ///
    /// # Errors
    ///
    /// A [`ConfigError::Value`] if it is not `IP:PORT`.
    pub fn listen_addr(&self) -> Result<SocketAddr, ConfigError> {
        self.server
            .listen
            .parse()
            .map_err(|_| value_error("server.listen", "an address such as 127.0.0.1:8080"))
    }

    /// `server.trusted_proxies` as addresses (HX7).
    ///
    /// # Errors
    ///
    /// A [`ConfigError::Value`] for an entry that is not an IP address.
    pub fn trusted_proxies(&self) -> Result<Vec<IpAddr>, ConfigError> {
        self.server
            .trusted_proxies
            .iter()
            .map(|p| {
                p.parse()
                    .map_err(|_| value_error("server.trusted_proxies", "IP addresses only"))
            })
            .collect()
    }

    /// The effective configuration as JSON, for `check-config` and the
    /// startup log line (OP1, OP4): the pepper's path, never its content.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }
}

fn read_file(path: &Path) -> Result<toml::Table, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Unreadable {
        path: path.to_owned(),
        kind: e.kind(),
    })?;
    text.parse::<toml::Table>()
        .map_err(|e| ConfigError::Invalid(e.message().to_owned()))
}

/// Sets each `FITMENT_SERVER__<SECTION>__<KEY>` over the file's table.
/// The value is read as its key's default's type: text for a text key,
/// TOML otherwise (`64`, `true`, `["10.0.0.1"]`). A variable of that form
/// naming no key is refused, as an unknown key in the file is; any other
/// `FITMENT_SERVER_*` but [`CONFIG_VAR`] is returned, to be ignored with a
/// warning.
fn overlay_vars(
    table: &mut toml::Table,
    vars: &BTreeMap<String, String>,
) -> Result<Vec<String>, ConfigError> {
    let defaults = serde_json::to_value(Config::default()).unwrap_or(Value::Null);
    let mut ignored = Vec::new();
    for (name, raw) in vars {
        if name == CONFIG_VAR || !name.starts_with(FAMILY) {
            continue;
        }
        if !name.starts_with(VAR_PREFIX) {
            ignored.push(name.clone());
            continue;
        }
        let Some((section, key, default)) = key_of(&defaults, name) else {
            return Err(value_error(name, "names no configuration key"));
        };
        let value = if default.is_string() {
            toml::Value::String(raw.clone())
        } else {
            let parsed = format!("v = {raw}")
                .parse::<toml::Table>()
                .ok()
                .and_then(|mut t| t.remove("v"));
            parsed.ok_or_else(|| value_error(name, "not a value of the key's type"))?
        };
        let entry = table
            .entry(section.clone())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        match entry {
            toml::Value::Table(section_table) => {
                section_table.insert(key, value);
            }
            _ => return Err(value_error(&section, "is not a table")),
        }
    }
    Ok(ignored)
}

/// The section, key and default a variable names, matched against the
/// default configuration's keys.
fn key_of<'d>(defaults: &'d Value, var: &str) -> Option<(String, String, &'d Value)> {
    let rest = var.strip_prefix(VAR_PREFIX)?.to_ascii_lowercase();
    let (section, key) = rest.split_once("__")?;
    let default = defaults.as_object()?.get(section)?.as_object()?.get(key)?;
    Some((section.to_owned(), key.to_owned(), default))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    /// OP2: `FITMENT_SERVER__<SECTION>__<KEY>` sets a key as its type;
    /// other `FITMENT_SERVER_*` variables are ignored.
    fn variables_are_read_as_their_keys_type() {
        let flags = Flags {
            catalog: Some("/c".into()),
            state: Some("/s".into()),
            ..Flags::default()
        };
        let config = Config::from_sources(
            &flags,
            &vars(&[
                ("FITMENT_SERVER__LIMITS__MAX_UPLOAD_MIB", "64"),
                ("FITMENT_SERVER__LOG__FILTER", "debug"),
                ("FITMENT_SERVER__SERVER__TRUSTED_PROXIES", "[\"10.0.0.1\"]"),
                ("FITMENT_SERVER__JOBS__REINDEX_ON_START", "false"),
                ("FITMENT_SERVER_PORT", "tcp://10.0.0.9:8080"),
                ("FITMENT_SERVER_SERVICE_HOST", "10.0.0.9"),
            ]),
        )
        .unwrap();
        assert_eq!(config.limits.max_upload_mib, 64);
        assert_eq!(config.log.filter, "debug");
        assert_eq!(config.server.trusted_proxies, ["10.0.0.1"]);
        assert!(!config.jobs.reindex_on_start);
        // An orchestrator's variables for a service of this name are not
        // keys: ignored, and named for the warning.
        assert_eq!(
            config.ignored,
            ["FITMENT_SERVER_PORT", "FITMENT_SERVER_SERVICE_HOST"]
        );
    }

    #[test]
    /// OP2: a key's variable naming no key, or of the wrong type, refuses
    /// the start, as an unknown key in the file does.
    fn an_unknown_variable_or_wrong_type_is_refused() {
        let flags = Flags {
            catalog: Some("/c".into()),
            state: Some("/s".into()),
            ..Flags::default()
        };
        for (k, v) in [
            ("FITMENT_SERVER__LIMITS__MAX_UPLOAD_MEB", "64"),
            ("FITMENT_SERVER__LIMITS__MAX_UPLOAD_MIB", "lots"),
            ("FITMENT_SERVER__NOPE", "1"),
            ("FITMENT_SERVER__NOPE__MAX_UPLOAD_MIB", "1"),
        ] {
            assert!(
                Config::from_sources(&flags, &vars(&[(k, v)])).is_err(),
                "{k}"
            );
        }
    }

    /// SV6, review S3: a `..` in either directory is refused.
    #[test]
    fn parent_components_are_refused() {
        for (catalog, state) in [("/c/../s/c", "/s"), ("/c", "/c/x/../.."), ("c", "../s")] {
            let flags = Flags {
                catalog: Some(catalog.into()),
                state: Some(state.into()),
                ..Flags::default()
            };
            assert!(
                Config::from_sources(&flags, &BTreeMap::new()).is_err(),
                "{catalog} {state}"
            );
        }
    }
}
