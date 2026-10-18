//! The documents auth answers with (docs/research/m6-api.md, section 7.11,
//! DC9, M6-11): `fitment.token`, `fitment.user` and the `fitment.page` of
//! either. Each has `format` and `format_version` first and writes `null`
//! rather than omitting a field; only `fitment.token` holds times, as
//! RFC 3339 UTC.

use serde::Serialize;

use super::Role;
use crate::ids::rfc3339;
use crate::state::{TokenRow, UserRow};

/// `fitment.token`.
pub const TOKEN_FORMAT: &str = "fitment.token";

/// `fitment.user`.
pub const USER_FORMAT: &str = "fitment.user";

/// `fitment.page`.
pub const PAGE_FORMAT: &str = "fitment.page";

/// Each of the three's version (DC9: may grow within v1).
pub const FORMAT_VERSION: u32 = 1;

/// `fitment.token` v1: a token, never its hash; `secret` only in the
/// response that created it (AU3).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TokenDocument {
    /// `fitment.token`.
    pub format: &'static str,
    /// 1.
    pub format_version: u32,
    /// `t_…`.
    pub id: String,
    /// Its owner's name for it.
    pub name: String,
    /// Its own role; a request's is also capped by its user's (AU5).
    pub role: Role,
    /// The first four characters of the secret after its prefix.
    pub hint: String,
    /// The token it was minted from; null for a login's (AU10).
    pub parent: Option<String>,
    /// When it was made.
    pub created: String,
    /// When it expires (AU4).
    pub expires: String,
    /// When it was last used, as last flushed or seen (AU6).
    pub last_used: Option<String>,
    /// When it was revoked.
    pub revoked: Option<String>,
    /// The plaintext, in the creating response only.
    pub secret: Option<String>,
}

impl TokenDocument {
    /// The document of `row`, its last use the later of the stored one and
    /// `seen` (the batch not yet flushed).
    pub fn of(row: &TokenRow, seen: Option<i64>, secret: Option<&super::Secret>) -> Self {
        let last_used = match (row.last_used_ms, seen) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        Self {
            format: TOKEN_FORMAT,
            format_version: FORMAT_VERSION,
            id: row.id.clone(),
            name: row.name.clone(),
            role: row.role,
            hint: row.hint.clone(),
            parent: row.parent.clone(),
            created: rfc3339(row.created_ms),
            expires: rfc3339(row.expires_ms),
            last_used: last_used.map(rfc3339),
            revoked: row.revoked_ms.map(rfc3339),
            secret: secret.map(|s| s.expose().to_owned()),
        }
    }
}

/// The presenting token of `/v1/auth/me`: its id and the effective role.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PresentingToken {
    /// `t_…`.
    pub id: String,
    /// `min(user.role, token.role)` (AU5).
    pub role: Role,
}

/// `fitment.user` v1: never a password hash or an internal key (ER3).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UserDocument {
    /// `fitment.user`.
    pub format: &'static str,
    /// 1.
    pub format_version: u32,
    /// `u_…`.
    pub id: String,
    /// As given.
    pub username: String,
    /// The user's role.
    pub role: Role,
    /// Its tokens are refused while set (AU4).
    pub disabled: bool,
    /// For `/v1/auth/me`, the presenting token; null otherwise.
    pub token: Option<PresentingToken>,
}

impl UserDocument {
    /// The document of `row`.
    pub fn of(row: &UserRow, token: Option<PresentingToken>) -> Self {
        Self {
            format: USER_FORMAT,
            format_version: FORMAT_VERSION,
            id: row.id.clone(),
            username: row.username.clone(),
            role: row.role,
            disabled: row.disabled,
            token,
        }
    }
}

/// `fitment.page` v1 (VN3): one page of a list, `next` the next page's
/// URL or null, `generation` the catalog's for lists that depend on it
/// (null here).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Page<T: Serialize> {
    /// `fitment.page`.
    pub format: &'static str,
    /// 1.
    pub format_version: u32,
    /// The items' format.
    pub kind: &'static str,
    /// The items, in the list's one order.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<serde_json::Value>"))]
    pub items: Vec<T>,
    /// The next page, or null.
    pub next: Option<String>,
    /// The catalog's generation, for a list that depends on it.
    pub generation: Option<u64>,
}

impl<T: Serialize> Page<T> {
    /// A page of `kind`.
    pub fn new(kind: &'static str, items: Vec<T>, next: Option<String>) -> Self {
        Self {
            format: PAGE_FORMAT,
            format_version: FORMAT_VERSION,
            kind,
            items,
            next,
            generation: None,
        }
    }
}
