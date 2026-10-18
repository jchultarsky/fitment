//! Section 7.3's routes (docs/research/m6-api.md, UP1 to UP4, UP8, AU8,
//! VN3): `PUT` and `POST` stream bytes in ([`crate::uploads::receive`]);
//! `HEAD`, `GET`, the list and `DELETE` answer only for bytes the caller
//! may name (UP3), so another user's are `404`, the answer for bytes no
//! one holds. An admin's `?owner=` reads another user's on `GET` and
//! `HEAD` only (AU8).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::Response;

use crate::Shared;
use crate::auth::documents::Page;
use crate::http::blocking;
use crate::http::error::{ApiError, ErrorCode};
use crate::http::extract::Path;
use crate::http::extract::{Principal, Reader};
use crate::http::json;
use crate::http::query;
use crate::state::{Deleted, Holding, StoreError, Stored};
use crate::uploads::{self, Received, UPLOAD_FORMAT, UPLOAD_FORMAT_VERSION, UploadDocument};

/// HX5: `sha256:` and 64 hex digits, either case, written lower-case; the
/// segment is percent-decoded once before (axum's `Path`). Anything else
/// is `400 invalid_reference`, its value never echoed.
fn reference(segment: &str) -> Result<String, ApiError> {
    segment
        .strip_prefix("sha256:")
        .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidReference,
                "an upload is named sha256: and its 64 hex digits",
            )
        })
}

fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "no upload of these bytes is yours")
}

/// The URL of an upload.
fn location(sha256: &str) -> String {
    format!("/v1/uploads/sha256:{sha256}")
}

/// `PUT /v1/uploads/sha256:{hex}` (UP2): the bytes kept only if their
/// SHA-256 is the name given; `422 upload_digest_mismatch` otherwise,
/// nothing kept.
pub(super) async fn put(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(name): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let named = reference(&name)?;
    let (parts, body) = request.into_parts();
    let received = uploads::receive(&shared, caller.user.key, &parts.headers, Some(&named), body)
        .await
        // ER6: an upload's PUT may be sent again unchanged.
        .map_err(|e| match e.code() {
            ErrorCode::RequestTimeout => e.retryable(true).retry_after(1),
            _ => e,
        })?;
    if received.synced.sha256 != named {
        let got = received.synced.sha256.clone();
        blocking(move || {
            drop(received);
            Ok(())
        })
        .await?;
        return Err(ApiError::new(
            ErrorCode::UploadDigestMismatch,
            "the bytes' SHA-256 is not the name given: nothing was kept",
        )
        .with("expected", named)
        .with("sha256", got));
    }
    store(&shared, &caller, received).await
}

/// `POST /v1/uploads` (UP2): as `PUT`, named by the SHA-256 the server
/// computes, for a client that cannot hash first.
pub(super) async fn post(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    request: Request,
) -> Result<Response, ApiError> {
    let (parts, body) = request.into_parts();
    let received = uploads::receive(&shared, caller.user.key, &parts.headers, None, body).await?;
    store(&shared, &caller, received).await
}

/// UP8, OB2: stores what was received, with the same work whether or not
/// the object exists, and answers `201`, or `200` for the caller's own
/// bytes again, with `fitment.upload` and `Location` either way.
async fn store(
    shared: &Arc<Shared>,
    caller: &Principal,
    received: Received,
) -> Result<Response, ApiError> {
    let (writing, owner) = (Arc::clone(shared), caller.user.key);
    let (stored, holding) = blocking(move || {
        let Received {
            synced,
            slot,
            reservation,
        } = received;
        let now = writing.ids.now_ms();
        let ttl = i64::try_from(writing.config.limits.upload_ttl_hours)
            .unwrap_or(i64::MAX / 3_600_000)
            .saturating_mul(3_600_000);
        let stored = writing.db.store_upload(
            owner,
            &synced.sha256,
            synced.bytes,
            now,
            now.saturating_add(ttl),
            || writing.objects.place(&synced),
        );
        let sha256 = synced.sha256.clone();
        let stored = match stored {
            Ok(stored) => {
                synced.kept();
                stored
            }
            // The temporary file goes with `synced`, here.
            Err(StoreError::Place(error)) => return Err(uploads::storage(&error)),
            Err(StoreError::Db(error)) => return Err(crate::http::db_error(&error)),
        };
        drop((slot, reservation));
        let holding = writing
            .db
            .upload(owner, &sha256, now)
            .map_err(|e| crate::http::db_error(&e))?;
        Ok((stored, holding.ok_or_else(ApiError::internal)?))
    })
    .await?;
    let mut response = json::document(
        UPLOAD_FORMAT,
        UPLOAD_FORMAT_VERSION,
        &UploadDocument::of(&holding),
    );
    *response.status_mut() = match stored {
        Stored::New => StatusCode::CREATED,
        Stored::Refreshed => StatusCode::OK,
    };
    if let Ok(value) = HeaderValue::from_str(&location(&holding.sha256)) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    Ok(response)
}

/// The holding of `sha256` the owner may name (UP3), or `404`.
async fn named(shared: &Arc<Shared>, owner: i64, sha256: String) -> Result<Holding, ApiError> {
    let reading = Arc::clone(shared);
    blocking(move || {
        let now = reading.ids.now_ms();
        reading
            .db
            .upload(owner, &sha256, now)
            .map_err(|e| crate::http::db_error(&e))
    })
    .await?
    .ok_or_else(not_found)
}

/// `GET /v1/uploads/sha256:{hex}`: `fitment.upload`, metadata and never
/// the bytes (UP3).
pub(super) async fn get(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(name): Path<String>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let sha256 = reference(&name)?;
    let params = query::params(uri.query(), &["owner"])?;
    let (owner, _) = super::owner(&shared, &caller, &params).await?;
    let holding = named(&shared, owner, sha256).await?;
    Ok(json::document(
        UPLOAD_FORMAT,
        UPLOAD_FORMAT_VERSION,
        &UploadDocument::of(&holding),
    ))
}

/// `HEAD /v1/uploads/sha256:{hex}` (UP3): `200` only for bytes the caller
/// may name, else `404`, so a client that hashed locally skips sending
/// its own bytes only; the headers `GET` would send, no body.
pub(super) async fn head(
    shared: State<Arc<Shared>>,
    caller: Reader,
    name: Path<String>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let response = get(shared, caller, name, uri).await?;
    let (parts, _) = response.into_parts();
    Ok(Response::from_parts(parts, Body::empty()))
}

/// `GET /v1/uploads`: a page of the caller's live uploads by SHA-256
/// (VN3), with `Link` to the next.
pub(super) async fn list(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    uri: Uri,
) -> Result<Response, ApiError> {
    let params = query::params(uri.query(), &["after", "limit", "owner"])?;
    let limit = query::limit(&params, shared.config.limits.page_max)?;
    let after = query::after(&params)?;
    if !after.is_empty() && !crate::state::objects::is_name(&after) {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "after is not a cursor of this list",
        )
        .with("parameter", "after"));
    }
    let (owner, extra) = super::owner(&shared, &caller, &params).await?;
    let reading = Arc::clone(&shared);
    let mut rows = blocking(move || {
        let now = reading.ids.now_ms();
        reading
            .db
            .uploads_page(owner, &after, limit.saturating_add(1), now)
            .map_err(|e| crate::http::db_error(&e))
    })
    .await?;
    let next = super::next_url(
        "/v1/uploads",
        &mut rows,
        limit,
        |r| r.sha256.as_str(),
        &extra,
    );
    let items = rows.iter().map(UploadDocument::of).collect();
    Ok(super::page(&Page::new(UPLOAD_FORMAT, items, next)))
}

/// `DELETE /v1/uploads/sha256:{hex}` (RA6, UP5, UP3): `204`, the
/// caller's row gone; the object, if nothing else holds it, the sweep's to
/// unlink, so the answer's time never tells whether another user holds the
/// bytes (PR 4's review); `409 upload_in_use` while a job that is not
/// terminal names it.
pub(super) async fn delete(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let sha256 = reference(&name)?;
    let (writing, owner) = (Arc::clone(&shared), caller.user.key);
    let deleted = blocking(move || {
        let now = writing.ids.now_ms();
        writing
            .db
            .delete_upload(owner, &sha256, now)
            .map_err(|e| crate::http::db_error(&e))
    })
    .await?;
    match deleted {
        Deleted::Gone => Ok(StatusCode::NO_CONTENT),
        Deleted::InUse => Err(ApiError::new(
            ErrorCode::UploadInUse,
            "a job that has not ended names these bytes: cancel it or wait for it",
        )),
        Deleted::NotFound => Err(not_found()),
    }
}
