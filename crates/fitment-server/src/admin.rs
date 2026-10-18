//! The offline commands (docs/research/m6-api.md, AU9, M6-19):
//! `fitment-server admin bootstrap|user|token … --state DIR`. They open
//! `server.db` directly, which WAL allows while the server runs; they never
//! open the catalog or take the lease (SD6), and never migrate `server.db`
//! while a server holds the lease (SD3).
//!
//! The first admin is made here, not from the environment, and its token
//! is printed once to the command's stdout and never logged (audit flaw 3,
//! Vectera's `ensure_bootstrap_admin`); `user reset-password` and `token
//! revoke` work when every admin token is lost. `main.rs` parses the
//! arguments and prints; everything else is here, blocking.

use std::path::Path;

use crate::auth::{self, Minted, Pepper, PepperError, Role, password};
use crate::config::{AuthSection, Config};
use crate::ids::Ids;
use crate::state::{
    self, CreateUserError, Db, DbError, MintError, MintFrom, StateError, TokenRow, UserRow,
};

/// Why a command failed, each with its exit code.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AdminError {
    /// The state directory or `server.db` (exit 2, 3 for a newer schema, 1
    /// for I/O).
    #[error(transparent)]
    State(#[from] StateError),
    /// The pepper (exit 2, 1 for I/O).
    #[error(transparent)]
    Pepper(#[from] PepperError),
    /// `bootstrap` when an admin exists (exit 2).
    #[error(
        "an admin exists: bootstrap makes the first one only; use `admin user` and `admin token`"
    )]
    AdminExists,
    /// No user of that name (exit 2).
    #[error("no user is named {0:?}")]
    NoSuchUser(String),
    /// No token of that id (exit 2).
    #[error("no token has the id {0:?}")]
    NoSuchToken(String),
    /// A username outside its alphabet or length (exit 2).
    #[error("a username is 1 to 64 characters of A-Z, a-z, 0-9 and . _ @ + -")]
    Username,
    /// The username is another user's (exit 2).
    #[error("another user has this username (compared without case)")]
    Taken,
    /// A password outside AU1's bounds (exit 2).
    #[error(transparent)]
    Password(#[from] password::PasswordError),
    /// A token's role above its user's, a bad name or lifetime (exit 2).
    #[error("{0}")]
    Usage(String),
    /// The user holds `auth.max_live_tokens` live tokens (exit 2).
    #[error("the user holds the most live tokens allowed: revoke one first")]
    TooManyTokens,
    /// argon2 failed (exit 1).
    #[error("the password could not be hashed")]
    Hash,
    /// SQLite (exit 1).
    #[error("server.db: {0}")]
    Db(#[from] DbError),
}

impl AdminError {
    /// The process's exit code: 2 for what the operator must change, 3 for
    /// a newer `server.db`, 1 for I/O.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::State(StateError::Db(DbError::Newer { .. })) => 3,
            Self::State(
                StateError::Io { .. } | StateError::Db(DbError::Sqlite(_) | DbError::Create(_)),
            )
            | Self::Pepper(PepperError::Unreadable(_))
            | Self::Hash
            | Self::Db(_) => 1,
            _ => 2,
        }
    }
}

impl From<MintError> for AdminError {
    fn from(error: MintError) -> Self {
        match error {
            MintError::TooMany => Self::TooManyTokens,
            MintError::Gone => Self::Usage("the user is disabled".into()),
            MintError::Db(error) => Self::Db(error),
        }
    }
}

/// `server.db` opened for the offline commands.
#[derive(Debug)]
pub struct Admin {
    db: Db,
    pepper: Pepper,
    settings: AuthSection,
    ids: Ids,
}

impl Admin {
    /// Opens the state directory `config.paths.state` (made, as a first
    /// start makes it, if it is new), with the pepper `config` names, so a
    /// password set here verifies at the server.
    ///
    /// # Errors
    ///
    /// [`AdminError`].
    pub fn open(config: &Config) -> Result<Self, AdminError> {
        let pepper = auth::pepper_of(&config.auth)?;
        let ids = Ids::default();
        let (db, _) =
            state::open_offline(Path::new(&config.paths.state), &ids.id("u"), ids.now_ms())?;
        Ok(Self {
            db,
            pepper,
            settings: config.auth.clone(),
            ids,
        })
    }

    fn hash(&self, password: &str) -> Result<String, AdminError> {
        password::check(password)?;
        password::hash(password, &self.pepper).map_err(|_| AdminError::Hash)
    }

    fn person(&self, username: &str) -> Result<UserRow, AdminError> {
        self.db
            .user_by_fold(&auth::fold(username))?
            .filter(|u| !u.system)
            .ok_or_else(|| AdminError::NoSuchUser(username.to_owned()))
    }

    /// `admin bootstrap` (AU9): refused if an admin exists; makes the user
    /// an admin, with `password` if given, and mints its first token, an
    /// admin's of `auth.token_default_days` (90), for the caller to print
    /// once.
    ///
    /// # Errors
    ///
    /// [`AdminError`].
    pub fn bootstrap(&self, username: &str, password: Option<&str>) -> Result<Minted, AdminError> {
        if self.db.admin_exists()? {
            return Err(AdminError::AdminExists);
        }
        let user = self.add_user(username, Role::Admin, password)?;
        self.mint_for(&user, "bootstrap", Role::Admin, None)
    }

    /// `admin user add`: a user, with a password or token-only.
    ///
    /// # Errors
    ///
    /// [`AdminError`].
    pub fn add_user(
        &self,
        username: &str,
        role: Role,
        password: Option<&str>,
    ) -> Result<UserRow, AdminError> {
        if !auth::username_ok(username) {
            return Err(AdminError::Username);
        }
        let phc = password.map(|p| self.hash(p)).transpose()?;
        let id = self.ids.id("u");
        self.db
            .create_user(
                &id,
                username,
                &auth::fold(username),
                role,
                phc.as_deref(),
                self.ids.now_ms(),
            )
            .map_err(|error| match error {
                CreateUserError::Taken => AdminError::Taken,
                CreateUserError::Db(error) => AdminError::Db(error),
            })
    }

    /// `admin user set`: a role or `disabled`, in force at the server's
    /// next request (AU4).
    ///
    /// # Errors
    ///
    /// [`AdminError`].
    pub fn set_user(
        &self,
        username: &str,
        role: Option<Role>,
        disabled: Option<bool>,
    ) -> Result<UserRow, AdminError> {
        let user = self.person(username)?;
        self.db
            .update_user(&user.id, role, disabled, self.ids.now_ms())?
            .ok_or_else(|| AdminError::NoSuchUser(username.to_owned()))
    }

    /// `admin user reset-password` (AU4, AU9): a new password, and every
    /// token of the user revoked. Returns how many.
    ///
    /// # Errors
    ///
    /// [`AdminError`].
    pub fn reset_password(&self, username: &str, password: &str) -> Result<usize, AdminError> {
        let user = self.person(username)?;
        let phc = self.hash(password)?;
        Ok(self
            .db
            .set_password(user.key, &phc, None, self.ids.now_ms())?)
    }

    /// `admin user list`: every person, by folded username.
    ///
    /// # Errors
    ///
    /// [`AdminError`].
    pub fn users(&self) -> Result<Vec<UserRow>, AdminError> {
        let mut all = Vec::new();
        loop {
            let after = all
                .last()
                .map_or(String::new(), |u: &UserRow| u.fold.clone());
            let page = self.db.users_page(&after, 1000)?;
            if page.is_empty() {
                return Ok(all);
            }
            all.extend(page);
        }
    }

    /// `admin token mint`: a token for `username`, no parent, its role at
    /// most the user's (the user's when not given) and its lifetime
    /// `auth.token_default_days` unless given (AU4, AU10).
    ///
    /// # Errors
    ///
    /// [`AdminError`].
    pub fn mint(
        &self,
        username: &str,
        name: &str,
        role: Option<Role>,
        days: Option<u32>,
    ) -> Result<Minted, AdminError> {
        let user = self.person(username)?;
        let role = role.unwrap_or(user.role);
        if role > user.role {
            return Err(AdminError::Usage(format!(
                "a token's role is at most its user's, {}",
                user.role.name()
            )));
        }
        self.mint_for(&user, name, role, days)
    }

    fn mint_for(
        &self,
        user: &UserRow,
        name: &str,
        role: Role,
        days: Option<u32>,
    ) -> Result<Minted, AdminError> {
        if !auth::token_name_ok(name) {
            return Err(AdminError::Usage(
                "a token's name is 1 to 128 characters, none a control character".into(),
            ));
        }
        let settings = &self.settings;
        let days = auth::lifetime_days(days, settings.token_default_days, settings.token_max_days)
            .ok_or_else(|| {
                AdminError::Usage(format!("days is 1 to {}", settings.token_max_days))
            })?;
        let now = self.ids.now_ms();
        Ok(auth::mint(
            &self.db,
            &self.ids,
            &MintFrom::Offline { user: user.key },
            name,
            role,
            now,
            now + days * auth::DAY_MS,
            settings.max_live_tokens,
        )?)
    }

    /// `admin token list`: the user's tokens, by id, never a secret.
    ///
    /// # Errors
    ///
    /// [`AdminError`].
    pub fn tokens(&self, username: &str) -> Result<Vec<TokenRow>, AdminError> {
        let user = self.person(username)?;
        let mut all = Vec::new();
        loop {
            let after = all
                .last()
                .map_or(String::new(), |t: &TokenRow| t.id.clone());
            let page = self.db.tokens_page(user.key, &after, 1000)?;
            if page.is_empty() {
                return Ok(all);
            }
            all.extend(page);
        }
    }

    /// `admin token revoke` (AU4): the token and every token minted from
    /// it. Returns how many it revoked.
    ///
    /// # Errors
    ///
    /// [`AdminError`].
    pub fn revoke(&self, id: &str) -> Result<usize, AdminError> {
        let token = self
            .db
            .token_by_id(id)?
            .ok_or_else(|| AdminError::NoSuchToken(id.to_owned()))?;
        Ok(self.db.revoke(token.key, self.ids.now_ms())?)
    }

    /// `server.db`, for tests that read what a command wrote.
    pub fn db(&self) -> &Db {
        &self.db
    }
}
