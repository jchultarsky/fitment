//! Authentication and the role extractors (docs/research/m6-api.md, AU3 to
//! AU7): Vectera's `CurrentUser` and its `ReadPerm`, `WritePerm` and
//! `AdminPerm` extractors (MIT, © 2024 Julian Chultarsky, the same author),
//! rewritten for axum.
//!
//! The router puts `authenticate` on every route, with the role its
//! operation names in `OPERATIONS` (AU5, XH14), so no route runs without
//! the check its table entry states: it reads the bearer token (AU3), finds
//! it by its SHA-256 in one indexed read and writes nothing (AU6), refuses
//! an expired or revoked token or a disabled user (AU4), counts the
//! request against its user's bucket (AU7), and compares the effective
//! role, `min(user.role, token.role)`, with the operation's (AU5). The
//! handler then takes [`Reader`], [`Writer`] or [`Admin`], which read what
//! the layer found and check the role again, so a handler's signature
//! states its role too.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use axum::middleware::Next;
use axum::response::{IntoResponse as _, Response};

use super::error::{ApiError, ErrorCode};
use super::operations::Role as Needed;
use crate::Shared;
use crate::auth::{Role, token};
use crate::state::{TokenRow, UserRow};

/// The authenticated caller of a request (Vectera's `CurrentUser`).
#[derive(Debug, Clone)]
pub struct Principal {
    /// The user.
    pub user: UserRow,
    /// The presenting token.
    pub token: TokenRow,
    /// `min(user.role, token.role)`, read at this request (AU5).
    pub effective: Role,
}

macro_rules! role_extractor {
    ($name:ident, $needed:expr, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone)]
        pub struct $name(pub Principal);

        impl<S: Send + Sync> FromRequestParts<S> for $name {
            type Rejection = ApiError;

            fn from_request_parts(
                parts: &mut Parts,
                _: &S,
            ) -> impl Future<Output = Result<Self, ApiError>> + Send {
                std::future::ready(principal(parts, $needed).map(Self))
            }
        }
    };
}

role_extractor!(
    Reader,
    Needed::Reader,
    "A caller of at least the reader role (AU5)."
);
role_extractor!(
    Writer,
    Needed::Writer,
    "A caller of at least the writer role (AU5)."
);
role_extractor!(Admin, Needed::Admin, "A caller of the admin role (AU5).");

/// The principal `authenticate` found, if its role is `needed`. A
/// handler reached without the layer is a bug, answered as `internal`.
fn principal(parts: &Parts, needed: Needed) -> Result<Principal, ApiError> {
    let Some(found) = parts.extensions.get::<Principal>() else {
        tracing::error!("a role extractor ran on a route without its authentication layer");
        return Err(ApiError::internal());
    };
    if found.effective.permits(needed) {
        Ok(found.clone())
    } else {
        Err(forbidden(needed, found.effective))
    }
}

/// `403 forbidden`: the role is too low (AU5).
pub fn forbidden(needed: Needed, effective: Role) -> ApiError {
    ApiError::new(
        ErrorCode::Forbidden,
        format!(
            "this needs the {} role; the token's effective role is {}",
            needed.name(),
            effective.name()
        ),
    )
    .with("required", needed.name())
    .with("role", effective.name())
}

/// `401 unauthenticated`, with one sentence for every reason, so the answer
/// says nothing of which token or user exists (AU4).
pub fn unauthenticated(missing: bool) -> ApiError {
    let detail = if missing {
        "this route needs a token in the Authorization header, scheme Bearer"
    } else {
        "the token is not valid: unknown, expired or revoked, or its user is disabled"
    };
    ApiError::new(ErrorCode::Unauthenticated, detail)
}

/// The bearer token of `headers`, if exactly one `Authorization` header
/// holds one (AU3). `Err(true)` when there is none.
fn presented(headers: &HeaderMap) -> Result<token::Secret, bool> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return Err(headers.get(header::AUTHORIZATION).is_none());
    };
    value.to_str().ok().and_then(token::bearer).ok_or(false)
}

/// The layer on every route (see the module): `needed` is its operation's
/// role in `OPERATIONS`.
pub(crate) async fn authenticate(
    State((shared, needed)): State<(Arc<Shared>, Needed)>,
    mut request: Request,
    next: Next,
) -> Response {
    if needed == Needed::None {
        return next.run(request).await;
    }
    match identify(&shared, request.headers()).await {
        Ok(found) => {
            if !found.effective.permits(needed) {
                return forbidden(needed, found.effective).into_response();
            }
            // OP1: the user's id in the request's span, never the username.
            tracing::Span::current().record("user", found.user.id.as_str());
            request.extensions_mut().insert(found);
            next.run(request).await
        }
        Err(error) => error.into_response(),
    }
}

/// AU3, AU4, AU6, AU7: the presented token's principal, or the error.
async fn identify(shared: &Arc<Shared>, headers: &HeaderMap) -> Result<Principal, ApiError> {
    let secret = presented(headers).map_err(unauthenticated)?;
    let sha = secret.sha256();
    let reading = Arc::clone(shared);
    let found = super::blocking(move || {
        reading.db.presented(&sha).map_err(|error| {
            tracing::error!(error = %error, "a token could not be read");
            ApiError::new(ErrorCode::IoError, "the server's state could not be read")
        })
    })
    .await?;
    let now = shared.ids.now_ms();
    let Some(found) = found.filter(|p| p.token.live(now) && !p.user.disabled && !p.user.system)
    else {
        return Err(unauthenticated(false));
    };
    shared.auth.used(found.token.key, now);
    if let Err(wait_ms) = shared
        .auth
        .requests
        .take(found.user.key, shared.auth.clock.now_ms())
    {
        return Err(rate_limited(wait_ms));
    }
    let effective = found.user.role.min(found.token.role);
    Ok(Principal {
        user: found.user,
        token: found.token,
        effective,
    })
}

/// `429 rate_limited` with `Retry-After` in whole seconds (AU7, ER6).
pub fn rate_limited(wait_ms: u64) -> ApiError {
    let seconds = u32::try_from(wait_ms.div_ceil(1000).max(1)).unwrap_or(u32::MAX);
    ApiError::new(ErrorCode::RateLimited, "too many requests: try again later")
        .with("retry_after_seconds", seconds)
        .retry_after(seconds)
}

/// The client's address (AU7): the TCP peer's, unless the peer is a
/// trusted proxy; then the right-most address of `X-Forwarded-For` that is
/// not one. In process (`oneshot`) there is no peer: the unspecified
/// address stands in.
pub(crate) fn client_address(shared: &Shared, request: &Parts) -> IpAddr {
    let peer = request
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |c| {
            c.0.ip().to_canonical()
        });
    if !shared.trusted_proxies.contains(&peer) {
        return peer;
    }
    let forwarded: Vec<&str> = request
        .headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .collect();
    for entry in forwarded.into_iter().rev() {
        // An entry the proxies did not write well ends the walk: what lies
        // left of it is the client's own text.
        let Some(address) = forwarded_address(entry) else {
            return peer;
        };
        if !shared.trusted_proxies.contains(&address) {
            return address;
        }
    }
    peer
}

/// One `X-Forwarded-For` entry's address (AU7): `1.2.3.4`, `2001:db8::1`,
/// or either with a port, `1.2.3.4:5678` or `[::1]:5678`, as some proxies
/// write it; canonical (an IPv4-mapped address as its IPv4 one).
pub fn forwarded_address(entry: &str) -> Option<IpAddr> {
    let entry = entry.trim();
    entry
        .parse::<IpAddr>()
        .or_else(|_| entry.parse::<SocketAddr>().map(|s| s.ip()))
        .ok()
        .map(|a| a.to_canonical())
}

/// The client's address (AU7), as `client_address` reads it.
#[derive(Debug, Clone, Copy)]
pub struct ClientAddress(pub IpAddr);

impl FromRequestParts<Arc<Shared>> for ClientAddress {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        shared: &Arc<Shared>,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(Ok(Self(client_address(shared, parts))))
    }
}
