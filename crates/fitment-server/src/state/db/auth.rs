//! Users and tokens in `server.db` (docs/research/m6-api.md, section 6.2,
//! AU3 to AU6, AU9, AU10): part of `state::db`, the one module that names
//! SQLite (SV6). Reads go to a pooled reader; every write is one `BEGIN
//! IMMEDIATE`, so a mint, a revocation and a password change are each all
//! or nothing, and two of them never interleave.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension as _, Row, TransactionBehavior, params};

use super::{Db, DbError, LEASE_STALE_MS};
use crate::auth::Role;

/// A user's row: never its key or password hash outside the server (ER3);
/// its `Debug` shows whether a password is set, never its PHC string.
#[derive(Clone, PartialEq, Eq)]
pub struct UserRow {
    /// `user.key`, internal.
    pub key: i64,
    /// `u_…`.
    pub id: String,
    /// As given.
    pub username: String,
    /// Folded: lower-case ASCII.
    pub fold: String,
    /// The user's role.
    pub role: Role,
    /// A disabled user's tokens are refused (AU4).
    pub disabled: bool,
    /// The reserved `system` user (M6-56).
    pub system: bool,
    /// The PHC string; none for a token-only user.
    pub password_phc: Option<String>,
}

impl std::fmt::Debug for UserRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserRow")
            .field("key", &self.key)
            .field("id", &self.id)
            .field("role", &self.role)
            .field("disabled", &self.disabled)
            .field("system", &self.system)
            .field("password", &self.password_phc.as_ref().map(|_| "set"))
            .finish_non_exhaustive()
    }
}

/// A token's row: never its hash outside the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRow {
    /// `token.key`, internal.
    pub key: i64,
    /// `t_…`.
    pub id: String,
    /// Its user's key.
    pub user: i64,
    /// Its owner's name for it.
    pub name: String,
    /// The first four characters after the prefix.
    pub hint: String,
    /// Its own role; the effective role is also capped by its user's.
    pub role: Role,
    /// The id of the token it was minted from (AU10).
    pub parent: Option<String>,
    /// When made.
    pub created_ms: i64,
    /// When it expires (AU4).
    pub expires_ms: i64,
    /// When last used, as flushed (AU6).
    pub last_used_ms: Option<i64>,
    /// When revoked.
    pub revoked_ms: Option<i64>,
}

impl TokenRow {
    /// Neither revoked nor expired at `now_ms`.
    pub fn live(&self, now_ms: i64) -> bool {
        self.revoked_ms.is_none() && self.expires_ms > now_ms
    }
}

/// A presented token and its user, read in one indexed query (AU6).
#[derive(Debug, Clone)]
pub struct Presented {
    /// The token.
    pub token: TokenRow,
    /// Its user.
    pub user: UserRow,
}

/// A token to insert.
#[derive(Debug, Clone, Copy)]
pub struct NewToken<'a> {
    /// `t_…`.
    pub id: &'a str,
    /// Its name.
    pub name: &'a str,
    /// The SHA-256 of the secret (AU3).
    pub secret_sha256: &'a str,
    /// The secret's hint.
    pub hint: &'a str,
    /// Its role.
    pub role: Role,
    /// Now.
    pub created_ms: i64,
    /// Its expiry.
    pub expires_ms: i64,
}

/// What a token is minted from (AU10), checked again inside the
/// transaction that inserts it, so it can never outlive a revocation that
/// committed before it. Its `Debug` never shows a PHC string.
#[derive(Clone)]
pub enum MintFrom {
    /// A login: no parent; the user still enabled and its password the one
    /// verified (a password changed meanwhile wins).
    Login {
        /// The user.
        user: i64,
        /// The PHC string the password was verified against.
        phc: String,
    },
    /// `POST /v1/auth/tokens`: the presenting token, still live.
    Token {
        /// The presenting token's key.
        parent: i64,
        /// Its user.
        user: i64,
    },
    /// The offline commands (AU9): no parent.
    Offline {
        /// The user.
        user: i64,
    },
}

impl std::fmt::Debug for MintFrom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Login { user, .. } => write!(f, "Login {{ user: {user} }}"),
            Self::Token { parent, user } => write!(f, "Token {{ parent: {parent}, user: {user} }}"),
            Self::Offline { user } => write!(f, "Offline {{ user: {user} }}"),
        }
    }
}

/// Why a mint was refused.
#[derive(Debug, thiserror::Error)]
pub enum MintError {
    /// The user holds `auth.max_live_tokens` live tokens (AU10).
    #[error("the user holds the most live tokens allowed")]
    TooMany,
    /// The presenting token, or the user, changed since the request was
    /// authenticated: revoked, expired, disabled, or its password changed.
    #[error("the credential is no longer valid")]
    Gone,
    /// SQLite failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<rusqlite::Error> for MintError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Db(error.into())
    }
}

/// Why a user was not made.
#[derive(Debug, thiserror::Error)]
pub enum CreateUserError {
    /// Its folded form is another user's.
    #[error("the username is taken")]
    Taken,
    /// `bootstrap`, and an admin exists (AU9): checked in the transaction
    /// that inserts, so two bootstraps make one admin.
    #[error("an admin exists")]
    AdminExists,
    /// SQLite failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

/// Why a user was not changed.
#[derive(Debug, thiserror::Error)]
pub enum UpdateUserError {
    /// It is the last enabled admin, and the change would leave none.
    #[error("the last enabled admin cannot be disabled or demoted")]
    LastAdmin,
    /// SQLite failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<rusqlite::Error> for UpdateUserError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Db(error.into())
    }
}

/// What a password change did (AU4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordSet {
    /// Set; this many other tokens revoked.
    Done(usize),
    /// The presenting token is no longer live, or the password changed
    /// since it was verified: another change won (review).
    Gone,
}

/// Revokes `key` and every token minted from it, at any depth, inside
/// `conn`'s transaction (AU4, AU10).
fn revoke_in(conn: &Connection, key: i64, now_ms: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "WITH RECURSIVE minted(key) AS (
           SELECT ?1 UNION SELECT token.key FROM token JOIN minted ON token.parent = minted.key)
         UPDATE token SET revoked_ms = ?2
         WHERE key IN (SELECT key FROM minted) AND revoked_ms IS NULL",
        params![key, now_ms],
    )
}

/// The enabled persons of the admin role but `except`.
fn other_admins(conn: &Connection, except: i64) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT count(*) FROM user WHERE kind = 'person' AND role = 'admin' AND disabled = 0 AND key != ?1",
        [except],
        |r| r.get(0),
    )
}

/// The sweep's batch of token keys per transaction.
pub const SWEEP_BATCH: i64 = 500;

const USER_COLUMNS: &str =
    "u.key, u.id, u.username, u.username_fold, u.role, u.disabled, u.kind, u.password_phc";

const TOKEN_COLUMNS: &str = "t.key, t.id, t.user, t.name, t.hint, t.role, p.id, t.created_ms, \
     t.expires_ms, t.last_used_ms, t.revoked_ms";

fn role_at(row: &Row<'_>, at: usize) -> rusqlite::Result<Role> {
    let text: String = row.get(at)?;
    Role::parse(&text).ok_or(rusqlite::Error::IntegralValueOutOfRange(at, 0))
}

/// A user's row from `USER_COLUMNS` at `at`.
fn user_at(row: &Row<'_>, at: usize) -> rusqlite::Result<UserRow> {
    Ok(UserRow {
        key: row.get(at)?,
        id: row.get(at + 1)?,
        username: row.get(at + 2)?,
        fold: row.get(at + 3)?,
        role: role_at(row, at + 4)?,
        disabled: row.get::<_, i64>(at + 5)? != 0,
        system: row.get::<_, String>(at + 6)? == "system",
        password_phc: row.get(at + 7)?,
    })
}

/// A token's row from `TOKEN_COLUMNS` at `at`.
fn token_at(row: &Row<'_>, at: usize) -> rusqlite::Result<TokenRow> {
    Ok(TokenRow {
        key: row.get(at)?,
        id: row.get(at + 1)?,
        user: row.get(at + 2)?,
        name: row.get(at + 3)?,
        hint: row.get(at + 4)?,
        role: role_at(row, at + 5)?,
        parent: row.get(at + 6)?,
        created_ms: row.get(at + 7)?,
        expires_ms: row.get(at + 8)?,
        last_used_ms: row.get(at + 9)?,
        revoked_ms: row.get(at + 10)?,
    })
}

fn user_where(
    conn: &Connection,
    clause: &str,
    value: &dyn rusqlite::ToSql,
) -> rusqlite::Result<Option<UserRow>> {
    conn.query_row(
        &format!("SELECT {USER_COLUMNS} FROM user AS u WHERE {clause}"),
        [value],
        |r| user_at(r, 0),
    )
    .optional()
}

fn token_where(
    conn: &Connection,
    clause: &str,
    value: &dyn rusqlite::ToSql,
) -> rusqlite::Result<Option<TokenRow>> {
    conn.query_row(
        &format!(
            "SELECT {TOKEN_COLUMNS} FROM token AS t LEFT JOIN token AS p ON p.key = t.parent WHERE {clause}"
        ),
        [value],
        |r| token_at(r, 0),
    )
    .optional()
}

/// The live tokens of `user` at `now_ms`.
fn live_count(conn: &Connection, user: i64, now_ms: i64) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT count(*) FROM token WHERE user = ?1 AND revoked_ms IS NULL AND expires_ms > ?2",
        params![user, now_ms],
        |r| r.get(0),
    )
}

/// Whether `from` still allows a mint, inside the mint's transaction.
fn mint_allowed(conn: &Connection, from: &MintFrom, now_ms: i64) -> rusqlite::Result<bool> {
    let enabled = |user: i64| -> rusqlite::Result<Option<UserRow>> {
        Ok(user_where(conn, "u.key = ?1", &user)?.filter(|u| !u.disabled && !u.system))
    };
    Ok(match from {
        MintFrom::Login { user, phc } => {
            enabled(*user)?.is_some_and(|u| u.password_phc.as_deref() == Some(phc.as_str()))
        }
        MintFrom::Token { parent, user } => {
            enabled(*user)?.is_some()
                && token_where(conn, "t.key = ?1", parent)?
                    .is_some_and(|t| t.user == *user && t.live(now_ms))
        }
        MintFrom::Offline { user } => enabled(*user)?.is_some(),
    })
}

impl Db {
    /// The token whose secret hashes to `secret_sha256`, with its user:
    /// one indexed read on a reader, no write (AU6).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn presented(&self, secret_sha256: &str) -> Result<Option<Presented>, DbError> {
        self.read(|conn| {
            conn.query_row(
                &format!(
                    "SELECT {TOKEN_COLUMNS}, {USER_COLUMNS} FROM token AS t \
                     LEFT JOIN token AS p ON p.key = t.parent JOIN user AS u ON u.key = t.user \
                     WHERE t.secret_sha256 = ?1"
                ),
                [secret_sha256],
                |r| {
                    Ok(Presented {
                        token: token_at(r, 0)?,
                        user: user_at(r, 11)?,
                    })
                },
            )
            .optional()
        })
    }

    /// The user whose folded username is `fold`.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn user_by_fold(&self, fold: &str) -> Result<Option<UserRow>, DbError> {
        self.read(|conn| user_where(conn, "u.username_fold = ?1", &fold))
    }

    /// The person whose id is `id` (never the `system` user).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn person_by_id(&self, id: &str) -> Result<Option<UserRow>, DbError> {
        self.read(|conn| user_where(conn, "u.id = ?1 AND u.kind = 'person'", &id))
    }

    /// The token whose id is `id`.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn token_by_id(&self, id: &str) -> Result<Option<TokenRow>, DbError> {
        self.read(|conn| token_where(conn, "t.id = ?1", &id))
    }

    /// Up to `limit` persons after the folded username `after`, in its
    /// order (VN3).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn users_page(&self, after: &str, limit: u32) -> Result<Vec<UserRow>, DbError> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {USER_COLUMNS} FROM user AS u WHERE u.kind = 'person' \
                 AND u.username_fold > ?1 ORDER BY u.username_fold LIMIT ?2"
            ))?;
            let rows = statement.query_map(params![after, limit], |r| user_at(r, 0))?;
            rows.collect()
        })
    }

    /// Up to `limit` tokens of `user` after the id `after`, by id (VN3).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn tokens_page(
        &self,
        user: i64,
        after: &str,
        limit: u32,
    ) -> Result<Vec<TokenRow>, DbError> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {TOKEN_COLUMNS} FROM token AS t LEFT JOIN token AS p ON p.key = t.parent \
                 WHERE t.user = ?1 AND t.id > ?2 ORDER BY t.id LIMIT ?3"
            ))?;
            let rows = statement.query_map(params![user, after, limit], |r| token_at(r, 0))?;
            rows.collect()
        })
    }

    /// True if a person holds the admin role (AU9's bootstrap refuses then).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn admin_exists(&self) -> Result<bool, DbError> {
        self.read(|conn| {
            conn.query_row(
                "SELECT count(*) FROM user WHERE kind = 'person' AND role = 'admin'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
        })
    }

    /// Makes a person (AU9, `POST /v1/auth/users`).
    ///
    /// # Errors
    ///
    /// [`CreateUserError::Taken`] when the folded username is another's.
    pub fn create_user(
        &self,
        id: &str,
        username: &str,
        fold: &str,
        role: Role,
        phc: Option<&str>,
        now_ms: i64,
    ) -> Result<UserRow, CreateUserError> {
        self.create_person(id, username, fold, role, phc, now_ms, false)
    }

    /// Makes the first admin (AU9's `bootstrap`): refused if an admin
    /// exists, checked in the transaction that inserts it (review).
    ///
    /// # Errors
    ///
    /// [`CreateUserError::AdminExists`], or as [`Db::create_user`].
    pub fn create_first_admin(
        &self,
        id: &str,
        username: &str,
        fold: &str,
        phc: Option<&str>,
        now_ms: i64,
    ) -> Result<UserRow, CreateUserError> {
        self.create_person(id, username, fold, Role::Admin, phc, now_ms, true)
    }

    #[allow(clippy::too_many_arguments)] // the row's columns, and the first-admin rule
    fn create_person(
        &self,
        id: &str,
        username: &str,
        fold: &str,
        role: Role,
        phc: Option<&str>,
        now_ms: i64,
        first_admin: bool,
    ) -> Result<UserRow, CreateUserError> {
        let mut conn = self.writer();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DbError::from)?;
        if first_admin {
            let admins: i64 = tx
                .query_row(
                    "SELECT count(*) FROM user WHERE kind = 'person' AND role = 'admin'",
                    [],
                    |r| r.get(0),
                )
                .map_err(DbError::from)?;
            if admins > 0 {
                return Err(CreateUserError::AdminExists);
            }
        }
        let made = tx
            .execute(
                "INSERT INTO user (id, kind, username_fold, username, password_phc, role, created_ms, updated_ms)
                 VALUES (?1, 'person', ?2, ?3, ?4, ?5, ?6, ?6) ON CONFLICT (username_fold) DO NOTHING",
                params![id, fold, username, phc, role.name(), now_ms],
            )
            .map_err(DbError::from)?;
        if made == 0 {
            return Err(CreateUserError::Taken);
        }
        let row = user_where(&tx, "u.id = ?1", &id)
            .map_err(DbError::from)?
            .ok_or(DbError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;
        tx.commit().map_err(DbError::from)?;
        Ok(row)
    }

    /// Sets a person's role or `disabled` (`PATCH /v1/auth/users/{id}`,
    /// AU9): in force at the next request, since authentication reads both
    /// each time (AU4). Disabling revokes every token of the user;
    /// enabling restores none (review). With `keep_an_admin` (the API),
    /// disabling or demoting the last enabled admin is refused.
    ///
    /// # Errors
    ///
    /// [`UpdateUserError`].
    pub fn update_user(
        &self,
        id: &str,
        role: Option<Role>,
        disabled: Option<bool>,
        keep_an_admin: bool,
        now_ms: i64,
    ) -> Result<Option<UserRow>, UpdateUserError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(before) = user_where(&tx, "u.id = ?1 AND u.kind = 'person'", &id)? else {
            return Ok(None);
        };
        let loses = disabled == Some(true) || role.is_some_and(|r| r != Role::Admin);
        if keep_an_admin
            && before.role == Role::Admin
            && !before.disabled
            && loses
            && other_admins(&tx, before.key)? == 0
        {
            return Err(UpdateUserError::LastAdmin);
        }
        tx.execute(
            "UPDATE user SET role = coalesce(?2, role), disabled = coalesce(?3, disabled), updated_ms = ?4
             WHERE key = ?1",
            params![before.key, role.map(Role::name), disabled.map(i64::from), now_ms],
        )?;
        if disabled == Some(true) {
            tx.execute(
                "UPDATE token SET revoked_ms = ?2 WHERE user = ?1 AND revoked_ms IS NULL",
                params![before.key, now_ms],
            )?;
        }
        let row = user_where(&tx, "u.key = ?1", &before.key)?;
        tx.commit()?;
        Ok(row)
    }

    /// Sets `user`'s password and revokes every token of theirs but the
    /// presenting one (AU4): `POST /v1/auth/password` passes `presenting`,
    /// the token's key and the PHC string its `current` was verified
    /// against, both checked again here (the token still live, the
    /// password unchanged), so of two concurrent changes one wins and the
    /// other is [`PasswordSet::Gone`]; an offline reset passes none and
    /// revokes every token.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn set_password(
        &self,
        user: i64,
        phc: &str,
        presenting: Option<(i64, &str)>,
        now_ms: i64,
    ) -> Result<PasswordSet, DbError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some((token, verified)) = presenting {
            let live = token_where(&tx, "t.key = ?1", &token)?
                .is_some_and(|t| t.user == user && t.live(now_ms));
            let unchanged = user_where(&tx, "u.key = ?1", &user)?
                .is_some_and(|u| u.password_phc.as_deref() == Some(verified));
            if !(live && unchanged) {
                return Ok(PasswordSet::Gone);
            }
        }
        tx.execute(
            "UPDATE user SET password_phc = ?2, updated_ms = ?3 WHERE key = ?1",
            params![user, phc, now_ms],
        )?;
        let keep = presenting.map(|(token, _)| token);
        let revoked = tx.execute(
            "UPDATE token SET revoked_ms = ?2 WHERE user = ?1 AND revoked_ms IS NULL AND key IS NOT ?3",
            params![user, now_ms, keep],
        )?;
        tx.commit()?;
        Ok(PasswordSet::Done(revoked))
    }

    /// Replaces `user`'s PHC string `old` by `new`, a rehash under this
    /// build's parameters (AU1), unless it changed meanwhile.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn rehash(&self, user: i64, old: &str, new: &str) -> Result<bool, DbError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE user SET password_phc = ?3 WHERE key = ?1 AND password_phc = ?2",
            params![user, old, new],
        )?;
        tx.commit()?;
        Ok(changed == 1)
    }

    /// Inserts `token` for the user `from` names (AU10): refused when its
    /// source is no longer valid, or when the user holds `max_live` live
    /// tokens; checked and inserted in one transaction.
    ///
    /// # Errors
    ///
    /// [`MintError`].
    pub fn mint(
        &self,
        from: &MintFrom,
        token: &NewToken<'_>,
        max_live: u32,
    ) -> Result<TokenRow, MintError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now_ms = token.created_ms;
        if !mint_allowed(&tx, from, now_ms)? {
            return Err(MintError::Gone);
        }
        let (user, parent) = match from {
            MintFrom::Login { user, .. } | MintFrom::Offline { user } => (*user, None),
            MintFrom::Token { parent, user } => (*user, Some(*parent)),
        };
        if live_count(&tx, user, now_ms)? >= i64::from(max_live) {
            // At the cap a login evicts the user's oldest live root token
            // (and what was minted from it), so a full cap never locks its
            // user out; a mint is refused (AU10, review).
            let oldest: Option<i64> = match from {
                MintFrom::Login { .. } => tx
                    .query_row(
                        "SELECT key FROM token WHERE user = ?1 AND parent IS NULL
                         AND revoked_ms IS NULL AND expires_ms > ?2 ORDER BY created_ms, key LIMIT 1",
                        params![user, now_ms],
                        |r| r.get(0),
                    )
                    .optional()?,
                _ => None,
            };
            match oldest {
                Some(key) => {
                    revoke_in(&tx, key, now_ms)?;
                }
                None => return Err(MintError::TooMany),
            }
            if live_count(&tx, user, now_ms)? >= i64::from(max_live) {
                return Err(MintError::TooMany);
            }
        }
        tx.execute(
            "INSERT INTO token (id, user, name, secret_sha256, hint, role, parent, created_ms, expires_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                token.id,
                user,
                token.name,
                token.secret_sha256,
                token.hint,
                token.role.name(),
                parent,
                token.created_ms,
                token.expires_ms
            ],
        )?;
        let row = token_where(&tx, "t.id = ?1", &token.id)?
            .ok_or(DbError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;
        tx.commit()?;
        Ok(row)
    }

    /// Revokes the token `key` and every token minted from it, at any
    /// depth, in one transaction (AU4, AU10); the ones already revoked keep
    /// their time. Returns how many it revoked.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn revoke(&self, key: i64, now_ms: i64) -> Result<usize, DbError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revoked = revoke_in(&tx, key, now_ms)?;
        tx.commit()?;
        Ok(revoked)
    }

    /// True if the token `key` is `ancestor` or was minted from it, at any
    /// depth (AU10: what a minted token may see and revoke).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn is_within(&self, ancestor: i64, key: i64) -> Result<bool, DbError> {
        self.read(|conn| {
            conn.query_row(
                "WITH RECURSIVE up(key, parent) AS (
                   SELECT key, parent FROM token WHERE key = ?2
                   UNION ALL SELECT token.key, token.parent FROM token JOIN up ON token.key = up.parent)
                 SELECT count(*) FROM up WHERE key = ?1",
                params![ancestor, key],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
        })
    }

    /// As [`Db::tokens_page`], the tokens `root` is or was minted into
    /// alone (AU10: a minted token's own subtree).
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn tokens_page_within(
        &self,
        root: i64,
        after: &str,
        limit: u32,
    ) -> Result<Vec<TokenRow>, DbError> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "WITH RECURSIVE minted(key) AS (
                   SELECT ?1 UNION SELECT token.key FROM token JOIN minted ON token.parent = minted.key)
                 SELECT {TOKEN_COLUMNS} FROM token AS t LEFT JOIN token AS p ON p.key = t.parent
                 WHERE t.key IN (SELECT key FROM minted) AND t.id > ?2 ORDER BY t.id LIMIT ?3"
            ))?;
            let rows = statement.query_map(params![root, after, limit], |r| token_at(r, 0))?;
            rows.collect()
        })
    }

    /// Writes the batched `last_used_ms` values (AU6): the sweep's and the
    /// last step of a graceful stop (SD1), never a request's.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn flush_last_used(&self, used: &BTreeMap<i64, i64>) -> Result<(), DbError> {
        if used.is_empty() {
            return Ok(());
        }
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut statement = tx.prepare(
                "UPDATE token SET last_used_ms = max(coalesce(last_used_ms, 0), ?2) WHERE key = ?1",
            )?;
            for (key, at) in used {
                statement.execute(params![key, at])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Deletes the tokens expired or revoked before `before_ms` (JS9: 30
    /// days after either), in batches of [`SWEEP_BATCH`] keys, each its own
    /// short transaction, so the writer is never held long (review). Keys
    /// are walked downwards: a token minted from another has the larger
    /// key, so a dead child goes before its parent is looked at, and one
    /// pass is enough; a token another still names as its parent stays
    /// until that one goes. Linear in the tokens, each child test an
    /// indexed read (`token_parent`). Returns how many it deleted.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn sweep_tokens(&self, before_ms: i64) -> Result<usize, DbError> {
        let mut cursor = i64::MAX;
        let mut deleted = 0;
        loop {
            let mut conn = self.writer();
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let keys: Vec<i64> = {
                let mut statement =
                    tx.prepare("SELECT key FROM token WHERE key < ?1 ORDER BY key DESC LIMIT ?2")?;
                let rows = statement.query_map(params![cursor, SWEEP_BATCH], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let Some(&last) = keys.last() else {
                return Ok(deleted);
            };
            {
                let mut delete = tx.prepare(
                    "DELETE FROM token WHERE key = ?1 AND (expires_ms < ?2 OR revoked_ms < ?2)
                     AND NOT EXISTS (SELECT 1 FROM token AS c WHERE c.parent = ?1)",
                )?;
                for key in &keys {
                    deleted += delete.execute(params![key, before_ms])?;
                }
            }
            tx.commit()?;
            drop(conn);
            self.yield_writer();
            cursor = last;
        }
    }

    /// True if another holder renewed the lease within 30 s (SD6): AU9's
    /// commands then refuse to migrate.
    ///
    /// # Errors
    ///
    /// SQLite's.
    pub fn lease_is_held(&self, now_ms: i64) -> Result<bool, DbError> {
        self.read(|conn| {
            conn.query_row("SELECT heartbeat_ms FROM lease WHERE one = 1", [], |r| {
                r.get::<_, i64>(0)
            })
            .optional()
            .map(|beat| beat.is_some_and(|b| now_ms.saturating_sub(b) < LEASE_STALE_MS))
        })
    }
}
