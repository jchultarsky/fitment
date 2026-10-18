//! The bodies section 7.10's routes read (VN6): unknown fields and
//! duplicate keys refused. Those holding a password have no `Debug`, so
//! none can be logged by mistake (OP1).

use serde::Deserialize;

use super::Role;

/// `POST /v1/auth/login`.
#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    /// The username, any case.
    pub username: String,
    /// The password.
    pub password: String,
    /// The new token's name; `login` when absent.
    #[serde(default)]
    pub token_name: Option<String>,
    /// Its lifetime in days, 1 to `auth.token_max_days`;
    /// `auth.token_default_days` when absent (AU4).
    #[serde(default)]
    pub days: Option<u32>,
}

/// `POST /v1/auth/password`.
#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PasswordRequest {
    /// The password now.
    pub current: String,
    /// The new one: 12 to 256 characters, at most 1,024 bytes (AU1).
    pub new: String,
}

/// `POST /v1/auth/tokens`.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MintRequest {
    /// Its name: 1 to 128 characters, no control character.
    pub name: String,
    /// Its role, at most the presenting token's effective role; that role
    /// when absent (AU10).
    #[serde(default)]
    pub role: Option<Role>,
    /// Its lifetime in days; its expiry at most the presenting token's.
    /// When absent, `auth.token_default_days` or the presenting token's
    /// expiry, whichever is sooner (AU10).
    #[serde(default)]
    pub days: Option<u32>,
}

/// `POST /v1/auth/users`.
#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CreateUserRequest {
    /// 1 to 64 characters of `[A-Za-z0-9._@+-]`, unique folded.
    pub username: String,
    /// The user's role.
    pub role: Role,
    /// A password (AU1); none makes a token-only user.
    #[serde(default)]
    pub password: Option<String>,
}

/// `PATCH /v1/auth/users/{id}`.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct UpdateUserRequest {
    /// A new role.
    #[serde(default)]
    pub role: Option<Role>,
    /// Disables or enables the user (AU4).
    #[serde(default)]
    pub disabled: Option<bool>,
}
