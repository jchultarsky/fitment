//! Authentication and roles (docs/research/m6-api.md, section 4.11, AU1 to
//! AU7, AU9, AU10), ported from Vectera's `catalog/src/auth.rs` and
//! `commons/src/auth.rs` (MIT, © 2024 Julian Chultarsky, the same author)
//! with the logic copied and the actix extractors rewritten for axum
//! (`http::extract`), and the eleven flaws of section 4.11's table fixed:
//!
//! 1. unknown usernames ran no hash: a dummy PHC string stands in (AU2);
//! 2. only admins minted tokens: every user mints and revokes their own,
//!    within the presenting token (AU3, AU5, AU10);
//! 3. the bootstrap password was logged: printed once to the command's
//!    stdout (AU9);
//! 4. expired sessions stayed: no sessions; expired and revoked tokens are
//!    swept ([`Auth::sweep`]);
//! 5. a write on every request: `last_used_ms` batched in memory (AU6);
//! 6. the pepper appended to the password: argon2's secret (AU1);
//! 7. a length in bytes with no bound: 12 to 256 characters, 1,024 bytes;
//! 8. argon2 on the executor: the blocking pool, `hash_concurrency` at once;
//! 9. default, unrecorded parameters: explicit, in the PHC string, rehashed;
//! 10. no login limit: AU7's buckets ([`limit`]);
//! 11. tokens that never expired: every token expires (AU4).

pub mod documents;
pub mod limit;
pub mod password;
pub mod requests;
pub mod roles;
pub mod token;

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

pub use password::Pepper;
pub use roles::Role;
pub use token::Secret;

use crate::config::AuthSection;
use crate::state::Db;
use limit::{LoginLimits, Rate, RequestLimits};

/// One day, in milliseconds.
pub const DAY_MS: i64 = 86_400_000;

/// How long a token stays in `server.db` after it expired or was revoked
/// (JS9).
pub const KEEP_DEAD_MS: i64 = 30 * DAY_MS;

/// The fewest bytes a pepper file holds.
pub const PEPPER_MIN_BYTES: usize = 16;

/// The most bytes a pepper file holds.
pub const PEPPER_MAX_BYTES: usize = 1024;

/// Why the pepper cannot be read (the start fails: exit 2, or 1 for I/O).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PepperError {
    /// The file cannot be read (exit 1).
    #[error("auth.pepper_file cannot be read: {0}")]
    Unreadable(io::ErrorKind),
    /// Others can read or write it (exit 2): AU1 wants mode 0600.
    #[error("auth.pepper_file must be its owner's alone (mode 0600)")]
    Mode,
    /// Too short or too long (exit 2).
    #[error("auth.pepper_file holds 16 to 1,024 bytes")]
    Size,
}

/// Reads the pepper from `path` (AU1): the owner's alone on Unix, 16 to
/// 1,024 bytes once a final line break is dropped. Its content is never
/// logged, only its path (OP1).
///
/// # Errors
///
/// [`PepperError`].
pub fn read_pepper(path: &Path) -> Result<Pepper, PepperError> {
    let meta = std::fs::metadata(path).map_err(|e| PepperError::Unreadable(e.kind()))?;
    #[cfg(unix)]
    if std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o077 != 0 {
        return Err(PepperError::Mode);
    }
    if !meta.is_file() {
        return Err(PepperError::Unreadable(io::ErrorKind::InvalidInput));
    }
    let mut bytes = std::fs::read(path).map_err(|e| PepperError::Unreadable(e.kind()))?;
    if bytes.ends_with(b"\n") {
        bytes.pop();
        if bytes.ends_with(b"\r") {
            bytes.pop();
        }
    }
    if !(PEPPER_MIN_BYTES..=PEPPER_MAX_BYTES).contains(&bytes.len()) {
        return Err(PepperError::Size);
    }
    Ok(Pepper::new(bytes))
}

/// The pepper `auth.pepper_file` names, or none when it is empty.
///
/// # Errors
///
/// [`PepperError`].
pub fn pepper_of(section: &AuthSection) -> Result<Pepper, PepperError> {
    if section.pepper_file.is_empty() {
        Ok(Pepper::none())
    } else {
        read_pepper(Path::new(&section.pepper_file))
    }
}

/// The rate limits' clock: monotonic milliseconds since the server was
/// built, which a `test-support` seam can freeze and move (OP11), so a
/// test meets a bound exactly.
#[derive(Debug)]
pub struct Clock {
    start: Instant,
    #[cfg(feature = "test-support")]
    frozen: Mutex<Option<u64>>,
}

impl Clock {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            #[cfg(feature = "test-support")]
            frozen: Mutex::new(None),
        }
    }

    /// Now, in milliseconds since the start.
    pub fn now_ms(&self) -> u64 {
        #[cfg(feature = "test-support")]
        if let Some(at) = *self.frozen.lock().unwrap_or_else(PoisonError::into_inner) {
            return at;
        }
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// OP11: stops the clock where it is.
    #[cfg(feature = "test-support")]
    pub fn freeze(&self) {
        let now = self.now_ms();
        *self.frozen.lock().unwrap_or_else(PoisonError::into_inner) = Some(now);
    }

    /// OP11: moves a frozen clock on by `by`.
    #[cfg(feature = "test-support")]
    pub fn advance(&self, by: Duration) {
        let mut frozen = self.frozen.lock().unwrap_or_else(PoisonError::into_inner);
        let at = frozen.unwrap_or(0);
        *frozen = Some(at + u64::try_from(by.as_millis()).unwrap_or(u64::MAX));
    }
}

/// What authentication shares across requests. Its `Debug` shows the
/// settings alone: never the pepper or a PHC string (OP1).
pub struct Auth {
    /// The `[auth]` section in force.
    pub settings: AuthSection,
    /// The pepper (AU1).
    pub(crate) pepper: Pepper,
    /// AU2's stand-in PHC string.
    pub(crate) dummy: String,
    /// At most `hash_concurrency` argon2 calls at once, a fair queue (AU2);
    /// a permit lives with its blocking task, so a request given up never
    /// lets a third hash run.
    pub(crate) hashing: std::sync::Arc<tokio::sync::Semaphore>,
    /// AU7's per-user requests.
    pub(crate) requests: RequestLimits,
    /// AU7's logins.
    pub(crate) logins: LoginLimits,
    /// `last_used_ms` per token key, flushed by [`Auth::sweep`] (AU6).
    used: Mutex<BTreeMap<i64, i64>>,
    /// The limits' clock.
    pub clock: Clock,
    #[cfg(feature = "test-support")]
    hashes: std::sync::atomic::AtomicU64,
    #[cfg(feature = "test-support")]
    hashing_now: std::sync::atomic::AtomicU64,
    #[cfg(feature = "test-support")]
    hashing_most: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Auth")
            .field("settings", &self.settings)
            .field("pepper", &self.pepper)
            .finish_non_exhaustive()
    }
}

impl Auth {
    /// The state for `section` and `pepper`, the dummy PHC string made
    /// with AU1's parameters (one hash: blocking).
    ///
    /// # Errors
    ///
    /// The hash's, which a working random source never gives.
    pub fn new(section: &AuthSection, pepper: Pepper) -> Result<Self, password::HashError> {
        let dummy = password::dummy(&pepper)?;
        let rate = Rate::burst_and_rate(section.rate_burst, section.rate_per_second);
        Ok(Self {
            settings: section.clone(),
            pepper,
            dummy,
            hashing: std::sync::Arc::new(tokio::sync::Semaphore::new(
                usize::try_from(section.hash_concurrency.max(1)).unwrap_or(1),
            )),
            requests: RequestLimits::new(rate),
            logins: LoginLimits::new(
                Rate::per(section.login_per_minute, 60_000),
                Rate::per(
                    section.login_failures,
                    u64::from(section.login_failure_minutes) * 60_000,
                ),
                usize::try_from(section.login_buckets).unwrap_or(usize::MAX),
            ),
            used: Mutex::new(BTreeMap::new()),
            clock: Clock::new(),
            #[cfg(feature = "test-support")]
            hashes: std::sync::atomic::AtomicU64::new(0),
            #[cfg(feature = "test-support")]
            hashing_now: std::sync::atomic::AtomicU64::new(0),
            #[cfg(feature = "test-support")]
            hashing_most: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// The pepper.
    pub fn pepper(&self) -> &Pepper {
        &self.pepper
    }

    /// Records that the token `key` was used at `now_ms` (AU6): memory
    /// only; the sweep writes it.
    pub(crate) fn used(&self, key: i64, now_ms: i64) {
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        let at = used.entry(key).or_insert(now_ms);
        *at = (*at).max(now_ms);
    }

    /// The batched `last_used_ms` of `key`, if newer than the stored one.
    pub(crate) fn last_used(&self, key: i64) -> Option<i64> {
        self.used
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .copied()
    }

    /// Writes the batched `last_used_ms` values (AU6) and deletes tokens
    /// expired or revoked 30 days before `now_ms` (JS9, audit flaw 4):
    /// the auth part of JS9's sweep, and the last step of a graceful stop
    /// (SD1). Blocking. Returns the tokens deleted.
    ///
    /// # Errors
    ///
    /// SQLite's; the batch is kept for the next try.
    pub fn sweep(&self, db: &Db, now_ms: i64) -> Result<usize, crate::state::DbError> {
        let batch = std::mem::take(&mut *self.used.lock().unwrap_or_else(PoisonError::into_inner));
        if let Err(error) = db.flush_last_used(&batch) {
            let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
            for (key, at) in batch {
                let kept = used.entry(key).or_insert(at);
                *kept = (*kept).max(at);
            }
            return Err(error);
        }
        db.sweep_tokens(now_ms - KEEP_DEAD_MS)
    }

    /// Runs one argon2 call (AU2): a permit of the fair queue within
    /// `auth.hash_wait_seconds`, then the blocking pool (SV3, fix 8).
    /// `None` when no permit came in time.
    pub(crate) async fn hash<T: Send + 'static>(
        self: &std::sync::Arc<Self>,
        work: impl FnOnce(&Self) -> T + Send + 'static,
    ) -> Option<Result<T, tokio::task::JoinError>> {
        let wait = Duration::from_secs(u64::from(self.settings.hash_wait_seconds));
        let queue = std::sync::Arc::clone(&self.hashing);
        let permit = tokio::time::timeout(wait, queue.acquire_owned())
            .await
            .ok()?
            .ok()?;
        let auth = std::sync::Arc::clone(self);
        let result = tokio::task::spawn_blocking(move || {
            #[cfg(feature = "test-support")]
            auth.hashing_started();
            let out = work(&auth);
            #[cfg(feature = "test-support")]
            auth.hashing_now
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            drop(permit);
            out
        })
        .await;
        Some(result)
    }

    #[cfg(feature = "test-support")]
    fn hashing_started(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        self.hashes.fetch_add(1, SeqCst);
        let now = self.hashing_now.fetch_add(1, SeqCst) + 1;
        self.hashing_most.fetch_max(now, SeqCst);
    }

    /// OP11: the argon2 calls made so far (VT24's count, not a timing).
    #[cfg(feature = "test-support")]
    pub fn hashes(&self) -> u64 {
        self.hashes.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// OP11: the most argon2 calls that ran at once (AU2).
    #[cfg(feature = "test-support")]
    pub fn most_hashing_at_once(&self) -> u64 {
        self.hashing_most.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// OP11: the login buckets kept (AU7's bound).
    #[cfg(feature = "test-support")]
    pub fn login_buckets(&self) -> usize {
        self.logins.len()
    }
}

/// A token just minted: its row and its secret, shown once.
#[derive(Debug)]
pub struct Minted {
    /// The row.
    pub row: crate::state::TokenRow,
    /// The plaintext (AU3).
    pub secret: Secret,
}

/// Mints a token for the user `from` names (AU3, AU10): a new secret, its
/// SHA-256 and hint stored, never the secret. Blocking.
///
/// # Errors
///
/// [`crate::state::MintError`].
#[allow(clippy::too_many_arguments)] // each is a column of the new row
pub fn mint(
    db: &Db,
    ids: &crate::ids::Ids,
    from: &crate::state::MintFrom,
    name: &str,
    role: Role,
    now_ms: i64,
    expires_ms: i64,
    max_live: u32,
) -> Result<Minted, crate::state::MintError> {
    let secret = Secret::from_bytes(&ids.secret_bytes());
    let id = ids.id("t");
    let (sha, hint) = (secret.sha256(), secret.hint());
    let row = db.mint(
        from,
        &crate::state::NewToken {
            id: &id,
            name,
            secret_sha256: &sha,
            hint: &hint,
            role,
            created_ms: now_ms,
            expires_ms,
        },
        max_live,
    )?;
    Ok(Minted { row, secret })
}

/// A token's lifetime in days (AU4): `days`, 1 to `max`, or `default`.
/// `None` when out of bounds.
pub fn lifetime_days(days: Option<u32>, default: u32, max: u32) -> Option<i64> {
    let days = days.unwrap_or(default);
    (1..=max).contains(&days).then(|| i64::from(days))
}

/// A username's folded form: lower-case ASCII (usernames are ASCII).
pub fn fold(username: &str) -> String {
    username.to_ascii_lowercase()
}

/// A username: 1 to 64 characters of `[A-Za-z0-9._@+-]`, so its folded
/// form is its lower-case and it is safe in any table or log (PH6).
pub fn username_ok(username: &str) -> bool {
    (1..=64).contains(&username.len())
        && username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'@' | b'+' | b'-'))
}

/// A token's name: 1 to 128 characters, none a control character.
pub fn token_name_ok(name: &str) -> bool {
    (1..=128).contains(&name.chars().count()) && !name.chars().any(char::is_control)
}

/// The key a login bucket keeps for any username string: the SHA-256 of
/// its ASCII-folded bytes, so the bucket map is bounded per entry and
/// holds no username.
pub fn login_key(username: &str) -> [u8; 32] {
    fitment_core::sha256(username.to_ascii_lowercase().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_and_names_are_checked() {
        for good in ["alice", "a", "ci.bot+1@example.org", &"x".repeat(64)] {
            assert!(username_ok(good), "{good}");
        }
        for bad in ["", "a b", "a/b", "é", "a\u{0}", &"x".repeat(65)] {
            assert!(!username_ok(bad), "{bad:?}");
        }
        assert_eq!(fold("Alice"), "alice");
        assert!(token_name_ok("ci token ✓"));
        assert!(!token_name_ok(""));
        assert!(!token_name_ok("a\nb"));
        assert!(!token_name_ok(&"x".repeat(129)));
    }

    #[cfg(unix)]
    #[test]
    fn a_pepper_file_is_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("fitment-pepper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pepper");
        std::fs::write(&path, "PEPPER-0123456789abcdef\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(read_pepper(&path), Err(PepperError::Mode)));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(read_pepper(&path).unwrap().is_set());
        std::fs::write(&path, "short\n").unwrap();
        assert!(matches!(read_pepper(&path), Err(PepperError::Size)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
