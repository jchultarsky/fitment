//! Section 7.10's routes (docs/research/m6-api.md, AU1 to AU7, AU10):
//! login, the caller, the password, the caller's tokens and the users.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::Response;

use crate::Shared;
use crate::auth::documents::{
    FORMAT_VERSION, PAGE_FORMAT, Page, PresentingToken, TOKEN_FORMAT, TokenDocument, USER_FORMAT,
    UserDocument,
};
use crate::auth::password::{self, Verified};
use crate::auth::requests::{
    CreateUserRequest, LoginRequest, MintRequest, PasswordRequest, UpdateUserRequest,
};
use crate::auth::{self, DAY_MS, Minted, Role};
use crate::http::blocking;
use crate::http::error::{ApiError, ErrorCode};
use crate::http::extract::{self, Admin, ClientAddress, Reader};
use crate::http::json::{self, Json};
use crate::http::operations::Role as Needed;
use crate::http::query;
use crate::state::{CreateUserError, MintError, MintFrom, PasswordSet, UpdateUserError, UserRow};

fn invalid(field: &str, detail: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::InvalidParameter, detail).with("parameter", field)
}

fn invalid_credentials() -> ApiError {
    ApiError::new(
        ErrorCode::InvalidCredentials,
        "the username or the password is wrong",
    )
}

fn io_error(error: &crate::state::DbError) -> ApiError {
    tracing::error!(error = %error, "server.db failed");
    ApiError::new(
        ErrorCode::IoError,
        "the server's state could not be read or written",
    )
}

/// `503 request_timeout`: no argon2 slot within `auth.hash_wait_seconds`
/// (AU2), or the hash's task failed.
fn no_slot() -> ApiError {
    ApiError::new(
        ErrorCode::RequestTimeout,
        "the server is busy checking passwords: try again later",
    )
}

/// One argon2 slot's work (AU2): `work` on the blocking pool behind the
/// fair queue.
async fn hashing<T: Send + 'static>(
    shared: &Arc<Shared>,
    work: impl FnOnce(&auth::Auth) -> T + Send + 'static,
) -> Result<T, ApiError> {
    #[cfg(feature = "test-support")]
    let work = {
        let hooked = Arc::clone(shared);
        move |a: &auth::Auth| {
            hooked.hooks.blocking("auth.hash");
            work(a)
        }
    };
    match shared.auth.hash(work).await {
        Some(Ok(out)) => Ok(out),
        Some(Err(_)) => Err(ApiError::internal()),
        None => Err(no_slot()),
    }
}

/// A document answered with `status`.
fn created<T: serde::Serialize>(format: &str, value: &T) -> Response {
    let mut response = json::document(format, FORMAT_VERSION, value);
    *response.status_mut() = StatusCode::CREATED;
    response
}

/// A token minted, or its refusal (AU10): one more than
/// `auth.max_live_tokens` is `409 too_many_tokens`.
async fn mint(
    shared: &Arc<Shared>,
    from: MintFrom,
    name: String,
    role: Role,
    expires_ms: i64,
    gone: ApiError,
) -> Result<Minted, ApiError> {
    let minting = Arc::clone(shared);
    blocking(move || {
        let now = minting.ids.now_ms();
        let max = minting.auth.settings.max_live_tokens;
        auth::mint(
            &minting.db,
            &minting.ids,
            &from,
            &name,
            role,
            now,
            expires_ms,
            max,
        )
        .map_err(|error| match error {
            MintError::TooMany => ApiError::new(
                ErrorCode::TooManyTokens,
                format!("a user holds at most {max} live tokens: revoke one first"),
            )
            .with("limit", max),
            MintError::Gone => gone,
            MintError::Db(error) => io_error(&error),
        })
    })
    .await
}

/// `POST /v1/auth/login` (AU2, AU7, AU10): usage first, then the address's
/// and the pair's buckets, then one argon2 verification whatever the
/// username, then the token. Every failure is `401 invalid_credentials`.
pub(super) async fn login(
    State(shared): State<Arc<Shared>>,
    ClientAddress(address): ClientAddress,
    Json(body): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let settings = &shared.auth.settings;
    let name = body.token_name.unwrap_or_else(|| "login".to_owned());
    if !auth::token_name_ok(&name) {
        return Err(invalid(
            "token_name",
            "a token's name is 1 to 128 characters, none a control character",
        ));
    }
    let days = auth::lifetime_days(
        body.days,
        settings.token_default_days,
        settings.token_max_days,
    )
    .ok_or_else(|| invalid("days", format!("days is 1 to {}", settings.token_max_days)))?;
    let key = auth::login_key(&body.username);
    let attempt = shared
        .auth
        .logins
        .admit(&key, address, shared.auth.clock.now_ms())
        .map_err(extract::rate_limited)?;
    let user = if auth::username_ok(&body.username) {
        let (reading, fold) = (Arc::clone(&shared), auth::fold(&body.username));
        blocking(move || reading.db.user_by_fold(&fold).map_err(|e| io_error(&e))).await?
    } else {
        None
    };
    let user = user.filter(|u| !u.disabled && !u.system && u.password_phc.is_some());
    let (verified, phc) = verify(&shared, user.as_ref(), body.password).await?;
    let Some(user) = user.filter(|_| verified) else {
        attempt.failed();
        return Err(invalid_credentials());
    };
    // Judged right: whatever follows (a mint refused, a storage error)
    // costs the pair no failure (AU7).
    drop(attempt);
    let expires = shared.ids.now_ms() + days * DAY_MS;
    let from = MintFrom::Login {
        user: user.key,
        phc,
    };
    let minted = mint(
        &shared,
        from,
        name,
        user.role,
        expires,
        invalid_credentials(),
    )
    .await?;
    let document = TokenDocument::of(&minted.row, None, Some(&minted.secret));
    Ok(created(TOKEN_FORMAT, &document))
}

/// AU2: verifies `password` against `user`'s PHC string, or the dummy's
/// for no usable user, in one slot; a match under older parameters is
/// rehashed there and stored (AU1, fix 9). Returns whether it matched and
/// the PHC string now stored. A password outside AU1's bounds matches
/// nobody's and is not hashed.
async fn verify(
    shared: &Arc<Shared>,
    user: Option<&UserRow>,
    password: String,
) -> Result<(bool, String), ApiError> {
    let real = user.and_then(|u| u.password_phc.clone());
    let phc = real.clone().unwrap_or_else(|| shared.auth.dummy.clone());
    if password::check(&password).is_err() {
        return Ok((false, phc));
    }
    let stored = phc.clone();
    let (verified, rehashed) = hashing(shared, move |a| {
        let verified = password::verify(&password, a.pepper(), &stored);
        let rehashed = match verified {
            Verified::Match { rehash: true } => password::hash(&password, a.pepper()).ok(),
            _ => None,
        };
        (verified, rehashed)
    })
    .await?;
    let matched = real.is_some() && matches!(verified, Verified::Match { .. });
    match (rehashed, user) {
        (Some(new), Some(user)) if matched => {
            let (writing, key, old) = (Arc::clone(shared), user.key, phc.clone());
            let stored = new.clone();
            let done = blocking(move || {
                writing
                    .db
                    .rehash(key, &old, &stored)
                    .map_err(|e| io_error(&e))
            })
            .await?;
            Ok((matched, if done { new } else { phc }))
        }
        _ => Ok((matched, phc)),
    }
}

/// `GET /v1/auth/me`: the caller, with the presenting token's id and
/// effective role (AU5).
pub(super) async fn me(Reader(caller): Reader) -> Response {
    let token = PresentingToken {
        id: caller.token.id.clone(),
        role: caller.effective,
    };
    json::document(
        USER_FORMAT,
        FORMAT_VERSION,
        &UserDocument::of(&caller.user, Some(token)),
    )
}

/// `POST /v1/auth/password` (AU1, AU4): the current password verified as
/// a login's (its failures counted in the same buckets), the new one set,
/// and every other token of the caller revoked; the presenting one stays.
pub(super) async fn password(
    State(shared): State<Arc<Shared>>,
    ClientAddress(address): ClientAddress,
    Reader(caller): Reader,
    Json(body): Json<PasswordRequest>,
) -> Result<StatusCode, ApiError> {
    if let Err(error) = password::check(&body.new) {
        return Err(invalid("new", error.to_string()));
    }
    let key = auth::login_key(&caller.user.username);
    let attempt = shared
        .auth
        .logins
        .admit(&key, address, shared.auth.clock.now_ms())
        .map_err(extract::rate_limited)?;
    let (verified, verified_phc) = verify(&shared, Some(&caller.user), body.current).await?;
    if !verified {
        attempt.failed();
        return Err(invalid_credentials());
    }
    drop(attempt);
    let new = body.new;
    let phc = hashing(&shared, move |a| password::hash(&new, a.pepper())).await?;
    let phc = phc.map_err(|_| ApiError::internal())?;
    let writing = Arc::clone(&shared);
    let set = blocking(move || {
        let now = writing.ids.now_ms();
        let presenting = Some((caller.token.key, verified_phc.as_str()));
        writing
            .db
            .set_password(caller.user.key, &phc, presenting, now)
            .map_err(|e| io_error(&e))
    })
    .await?;
    match set {
        PasswordSet::Done(_) => Ok(StatusCode::NO_CONTENT),
        // Another change won meanwhile and revoked this token (review):
        // the request is answered as the next one would be.
        PasswordSet::Gone => Err(extract::unauthenticated(false)),
    }
}

/// A page's response: `fitment.page`, and `Link: <next>; rel="next"`
/// (VN3).
fn page<T: serde::Serialize>(page: &Page<T>) -> Response {
    let mut response = json::document(PAGE_FORMAT, FORMAT_VERSION, page);
    if let Some(value) = page
        .next
        .as_deref()
        .and_then(|next| HeaderValue::from_str(&format!("<{next}>; rel=\"next\"")).ok())
    {
        response.headers_mut().insert(header::LINK, value);
    }
    response
}

/// The next page's URL, if `rows` holds one more than `limit`.
fn next_url<R>(
    base: &str,
    rows: &mut Vec<R>,
    limit: u32,
    key: impl Fn(&R) -> &str,
    extra: &str,
) -> Option<String> {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    if rows.len() <= limit {
        return None;
    }
    rows.truncate(limit);
    let last = rows.last().map(key)?;
    Some(format!(
        "{base}?after={}&limit={limit}{extra}",
        query::cursor(last)
    ))
}

/// `GET /v1/auth/tokens`: the caller's tokens by id (VN3), never a secret
/// (AU3); `?owner=` names another user's, for an admin only (AU5, AU8).
pub(super) async fn tokens_list(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    uri: Uri,
) -> Result<Response, ApiError> {
    let params = query::params(uri.query(), &["after", "limit", "owner"])?;
    let limit = query::limit(&params, shared.config.limits.page_max)?;
    let after = query::after(&params)?;
    let (owner, extra) = match params.get("owner") {
        Some(id) if *id != caller.user.id => {
            if !caller.effective.permits(Needed::Admin) {
                return Err(extract::forbidden(Needed::Admin, caller.effective));
            }
            let (reading, id) = (Arc::clone(&shared), id.clone());
            let extra = format!("&owner={id}");
            let found =
                blocking(move || reading.db.person_by_id(&id).map_err(|e| io_error(&e))).await?;
            (found.ok_or_else(not_found)?, extra)
        }
        Some(id) => (caller.user.clone(), format!("&owner={id}")),
        None => (caller.user.clone(), String::new()),
    };
    // AU10 (review): a minted token sees itself and what was minted from
    // it; a root token its user's every token; an admin anyone's.
    let within = (owner.key == caller.user.key && confined(&caller)).then_some(caller.token.key);
    let reading = Arc::clone(&shared);
    let mut rows = blocking(move || {
        let fetched = match within {
            Some(root) => reading
                .db
                .tokens_page_within(root, &after, limit.saturating_add(1)),
            None => reading
                .db
                .tokens_page(owner.key, &after, limit.saturating_add(1)),
        };
        fetched.map_err(|e| io_error(&e))
    })
    .await?;
    let next = next_url(
        "/v1/auth/tokens",
        &mut rows,
        limit,
        |r| r.id.as_str(),
        &extra,
    );
    let items = rows
        .iter()
        .map(|row| TokenDocument::of(row, shared.auth.last_used(row.key), None))
        .collect();
    Ok(page(&Page::new(TOKEN_FORMAT, items, next)))
}

/// True if the caller's token is confined to its own subtree (AU10): a
/// minted token, unless its effective role is admin.
fn confined(caller: &extract::Principal) -> bool {
    caller.token.parent.is_some() && !caller.effective.permits(Needed::Admin)
}

fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "nothing of that id is yours to see")
}

/// `POST /v1/auth/tokens` (AU10): a token within the presenting one, its
/// role at most the effective role and its expiry at most the presenting
/// token's, recorded as its parent; asking more is `400`, never capped.
pub(super) async fn tokens_mint(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Json(body): Json<MintRequest>,
) -> Result<Response, ApiError> {
    let settings = &shared.auth.settings;
    if !auth::token_name_ok(&body.name) {
        return Err(invalid(
            "name",
            "a token's name is 1 to 128 characters, none a control character",
        ));
    }
    let role = body.role.unwrap_or(caller.effective);
    if role > caller.effective {
        return Err(invalid(
            "role",
            format!(
                "a minted token's role is at most the presenting token's effective role, {}",
                caller.effective.name()
            ),
        ));
    }
    let now = shared.ids.now_ms();
    let ceiling = caller.token.expires_ms;
    let expires = match body.days {
        Some(_) => {
            let days = auth::lifetime_days(
                body.days,
                settings.token_default_days,
                settings.token_max_days,
            )
            .ok_or_else(|| invalid("days", format!("days is 1 to {}", settings.token_max_days)))?;
            now + days * DAY_MS
        }
        None => (now + i64::from(settings.token_default_days) * DAY_MS).min(ceiling),
    };
    if expires > ceiling {
        return Err(invalid(
            "days",
            "a minted token expires at the latest when the presenting token does",
        )
        .with("expires_at_most", crate::ids::rfc3339(ceiling)));
    }
    // AU10 (review): at most `auth.mints_per_minute` a user, so dead
    // tokens never pile up faster than the sweep removes them.
    shared
        .auth
        .mints
        .take(caller.user.key, shared.auth.clock.now_ms())
        .map_err(extract::rate_limited)?;
    let from = MintFrom::Token {
        parent: caller.token.key,
        user: caller.user.key,
    };
    let minted = mint(
        &shared,
        from,
        body.name,
        role,
        expires,
        extract::unauthenticated(false),
    )
    .await?;
    let document = TokenDocument::of(&minted.row, None, Some(&minted.secret));
    Ok(created(TOKEN_FORMAT, &document))
}

/// `DELETE /v1/auth/tokens/{id}` (AU4): revokes the token and every token
/// minted from it; `current` is the presenting one (a logout). Another
/// user's is `404` but to an admin (AU8).
pub(super) async fn tokens_revoke(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let target = if id == "current" {
        caller.token.clone()
    } else if crate::ids::well_formed("t", &id) {
        let reading = Arc::clone(&shared);
        blocking(move || reading.db.token_by_id(&id).map_err(|e| io_error(&e)))
            .await?
            .ok_or_else(not_found)?
    } else {
        return Err(not_found());
    };
    if target.user != caller.user.key && !caller.effective.permits(Needed::Admin) {
        return Err(not_found());
    }
    if target.user == caller.user.key && target.key != caller.token.key && confined(&caller) {
        let (reading, (root, key)) = (Arc::clone(&shared), (caller.token.key, target.key));
        let inside =
            blocking(move || reading.db.is_within(root, key).map_err(|e| io_error(&e))).await?;
        if !inside {
            return Err(not_found());
        }
    }
    let writing = Arc::clone(&shared);
    blocking(move || {
        let now = writing.ids.now_ms();
        writing.db.revoke(target.key, now).map_err(|e| io_error(&e))
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/auth/users` (admin): the users by folded username (VN3); the
/// `system` user is no one's to manage and is not listed.
pub(super) async fn users_list(
    State(shared): State<Arc<Shared>>,
    Admin(_): Admin,
    uri: Uri,
) -> Result<Response, ApiError> {
    let params = query::params(uri.query(), &["after", "limit"])?;
    let limit = query::limit(&params, shared.config.limits.page_max)?;
    let after = query::after(&params)?;
    let reading = Arc::clone(&shared);
    let mut rows = blocking(move || {
        reading
            .db
            .users_page(&after, limit.saturating_add(1))
            .map_err(|e| io_error(&e))
    })
    .await?;
    let next = next_url("/v1/auth/users", &mut rows, limit, |r| r.fold.as_str(), "");
    let items = rows.iter().map(|row| UserDocument::of(row, None)).collect();
    Ok(page(&Page::new(USER_FORMAT, items, next)))
}

/// `POST /v1/auth/users` (admin): a user, with a password (hashed in an
/// argon2 slot, AU1) or token-only.
pub(super) async fn users_create(
    State(shared): State<Arc<Shared>>,
    Admin(_): Admin,
    Json(body): Json<CreateUserRequest>,
) -> Result<Response, ApiError> {
    if !auth::username_ok(&body.username) {
        return Err(invalid(
            "username",
            "a username is 1 to 64 characters of A-Z, a-z, 0-9 and . _ @ + -",
        ));
    }
    let phc = match body.password {
        Some(password) => {
            if let Err(error) = password::check(&password) {
                return Err(invalid("password", error.to_string()));
            }
            let phc = hashing(&shared, move |a| password::hash(&password, a.pepper())).await?;
            Some(phc.map_err(|_| ApiError::internal())?)
        }
        None => None,
    };
    let writing = Arc::clone(&shared);
    let (username, role) = (body.username, body.role);
    let row = blocking(move || {
        let (id, now) = (writing.ids.id("u"), writing.ids.now_ms());
        let fold = auth::fold(&username);
        writing
            .db
            .create_user(&id, &username, &fold, role, phc.as_deref(), now)
            .map_err(|error| match error {
                CreateUserError::Taken => ApiError::new(
                    ErrorCode::UsernameTaken,
                    "another user has this username (compared without case)",
                ),
                CreateUserError::Db(error) => io_error(&error),
                // Only a bootstrap asks for the first admin.
                CreateUserError::AdminExists => ApiError::internal(),
            })
    })
    .await?;
    Ok(created(USER_FORMAT, &UserDocument::of(&row, None)))
}

/// `PATCH /v1/auth/users/{id}` (admin): a role or `disabled`, in force at
/// the next request (AU4), since authentication reads both every time.
pub(super) async fn users_update(
    State(shared): State<Arc<Shared>>,
    Admin(_): Admin,
    Path(id): Path<String>,
    Json(body): Json<UpdateUserRequest>,
) -> Result<Response, ApiError> {
    if !crate::ids::well_formed("u", &id) {
        return Err(not_found());
    }
    let writing = Arc::clone(&shared);
    let row = blocking(move || {
        let now = writing.ids.now_ms();
        writing
            .db
            .update_user(&id, body.role, body.disabled, true, now)
            .map_err(|error| match error {
                UpdateUserError::LastAdmin => ApiError::new(
                    ErrorCode::LastAdmin,
                    "the last enabled admin cannot be disabled or demoted: make another admin first",
                ),
                UpdateUserError::Db(error) => io_error(&error),
            })
    })
    .await?
    .ok_or_else(not_found)?;
    Ok(json::document(
        USER_FORMAT,
        FORMAT_VERSION,
        &UserDocument::of(&row, None),
    ))
}
