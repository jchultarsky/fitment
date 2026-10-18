//! Section 7.6's routes, as far as PR 5 builds them (docs/research/
//! m6-api.md, RA4, RA5, RA6, RA9, VN3, VN5, DC4): extract a socket (the job
//! `socket_extract`, `fitment socket FILE --part X`), list the caller's
//! lines, read a revision's exact bytes and a line's history, and delete a
//! line (a tombstone). Reviews and imports are PR 6's. An admin's
//! `?owner=` reads another user's on `GET` only (AU8).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use serde_json::Value;

use super::jobs::{self, Id, Submission};
use crate::Shared;
use crate::assemblies::{LINE_KIND, LineItem, REVISION_KIND, RevisionItem};
use crate::auth::documents::Page;
use crate::http::cache::{self, Cache, Served};
use crate::http::error::{ApiError, ErrorCode};
use crate::http::extract::Reader;
use crate::http::json::{self, Sent};
use crate::http::{blocking, db_error, query};
use crate::jobs::model::Kind;
use crate::jobs::request::{self, CarryFrom, SocketJob, SocketRef, SocketRequest};

fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "no socket of that id is yours").with("resource", "socket")
}

/// `carry_from`, resolved (RA9): an exact revision of the caller's, or a
/// held upload within `limits.max_socket_mib` (UP6's cap; read leniently in
/// the job). Its object and length are held by the job (OB4).
async fn carry_from(
    shared: &Arc<Shared>,
    owner: i64,
    carry: &CarryFrom,
) -> Result<(Value, String, u64, Option<i64>), ApiError> {
    match carry {
        CarryFrom::Revision(text) => {
            let reference = SocketRef::parse(text)?;
            let (reading, id, n) = (Arc::clone(shared), reference.id.clone(), reference.n);
            let revision =
                blocking(move || reading.db.revision(owner, &id, n).map_err(|e| db_error(&e)))
                    .await?
                    .ok_or_else(not_found)?;
            let exact = format!("{}@r{}", revision.line_id, revision.n);
            Ok((
                Value::String(exact),
                revision.document_sha256,
                revision.document_bytes,
                Some(revision.key),
            ))
        }
        CarryFrom::Upload { upload } => {
            let sha256 = request::sha256_ref(upload, "carry_from.upload")?;
            let bytes = super::nameable(shared, owner, &sha256).await?;
            let cap = shared
                .config
                .limits
                .max_socket_mib
                .saturating_mul(1024 * 1024);
            if bytes > cap {
                return Err(ApiError::new(
                    ErrorCode::TooLarge,
                    format!(
                        "a socket document is at most {} MiB",
                        shared.config.limits.max_socket_mib
                    ),
                )
                .with("limit", "max_socket_mib"));
            }
            Ok((
                serde_json::json!({ "upload": format!("sha256:{sha256}") }),
                sha256,
                bytes,
                None,
            ))
        }
    }
}

/// `POST /v1/sockets` (section 7.6): the job `socket_extract` on an
/// assembly the caller registered.
pub(super) async fn create(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    axum::Extension(request_id): Id,
    headers: HeaderMap,
    Sent(body, sent): Sent<SocketRequest>,
) -> Result<Response, ApiError> {
    // JB9 (review S4 of PR 5): a key's replay before any reference.
    let fingerprint = request::fingerprint("POST", "/v1/sockets", &sent);
    if let Some(replayed) = jobs::replay(&shared, &caller, &headers, &fingerprint).await? {
        return Ok(replayed);
    }
    let sha256 = request::sha256_ref(&body.assembly, "assembly")?;
    if body.part.is_none() && body.occurrence.is_none() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "name the part to replace with part, or one occurrence with occurrence",
        )
        .with("parameter", "part"));
    }
    for (field, value) in [("part", &body.part), ("occurrence", &body.occurrence)] {
        if let Some(value) = value {
            request::text(value, field)?;
        }
    }
    let owner = caller.user.key;
    let assembly = super::assemblies::of(&shared, owner, sha256.clone()).await?;
    let mut inputs = vec![(sha256.clone(), assembly.bytes)];
    let mut cites = Vec::new();
    let carry_from = match &body.carry_from {
        Some(carry) => {
            let (normalized, object, bytes, revision) = carry_from(&shared, owner, carry).await?;
            inputs.push((object, bytes));
            // OB4 (review S8 of PR 5): a carried revision is cited, so the
            // sweep keeps it until the job ends.
            cites.extend(revision);
            Some(normalized)
        }
        None => None,
    };
    let (request, request_sha256) = request::normalized(&SocketJob {
        assembly: format!("sha256:{sha256}"),
        part: body.part,
        occurrence: body.occurrence,
        carry: body.carry.unwrap_or(true),
        carry_from,
    });
    let submission = Submission {
        kind: Kind::SocketExtract,
        request,
        request_sha256,
        inputs,
        fingerprint,
        cites,
    };
    jobs::submit(&shared, &caller, &headers, &request_id, submission).await
}

/// A page of the owner's live lines (VN3), of one assembly if given.
pub(super) async fn page(
    shared: &Arc<Shared>,
    owner: i64,
    assembly: Option<String>,
    params: &BTreeMap<String, String>,
    base: &str,
    extra: &str,
) -> Result<Response, ApiError> {
    let limit = query::limit(params, shared.config.limits.page_max)?;
    let after = query::after(params)?;
    if !after.is_empty() && SocketRef::parse(&after).is_err() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "after is not a cursor of this list",
        )
        .with("parameter", "after"));
    }
    let reading = Arc::clone(shared);
    let mut rows = blocking(move || {
        reading
            .db
            .lines_page(owner, assembly.as_deref(), &after, limit.saturating_add(1))
            .map_err(|e| db_error(&e))
    })
    .await?;
    let next = super::next_url(base, &mut rows, limit, |r| r.id.as_str(), extra);
    let items: Vec<LineItem> = rows.iter().map(LineItem::of).collect();
    Ok(super::page(&Page::new(LINE_KIND, items, next)))
}

/// `GET /v1/sockets?assembly=&after=&limit=` (VN3).
pub(super) async fn list(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    uri: Uri,
) -> Result<Response, ApiError> {
    let params = query::params(uri.query(), &["assembly", "after", "limit", "owner"])?;
    let (owner, mut extra) = super::owner(&shared, &caller, &params).await?;
    let assembly = match params.get("assembly") {
        Some(text) => {
            let sha256 = request::sha256_ref(text, "assembly")?;
            let _ = write!(extra, "&assembly=sha256:{sha256}");
            Some(sha256)
        }
        None => None,
    };
    page(&shared, owner, assembly, &params, "/v1/sockets", &extra).await
}

/// `GET /v1/sockets/{ref}` (RA5, VN5, DC4): a revision's exact
/// `fitment.socket` bytes, a bare id its line's current one; `immutable`
/// for `@rN`, `no-cache` otherwise; `Fitment-Digest` its digest.
pub(super) async fn get(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(name): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let reference = SocketRef::parse(&name)?;
    let params = query::params(uri.query(), &["owner"])?;
    let (owner, _) = super::owner(&shared, &caller, &params).await?;
    let (reading, id, n) = (Arc::clone(&shared), reference.id.clone(), reference.n);
    let revision = blocking(move || reading.db.revision(owner, &id, n).map_err(|e| db_error(&e)))
        .await?
        .ok_or_else(not_found)?;
    let served = Served {
        sha256: revision.document_sha256,
        bytes: revision.document_bytes,
        content_type: json::JSON,
        format: Some((
            fitment_core::SOCKET_FORMAT.to_owned(),
            fitment_core::SOCKET_FORMAT_VERSION,
        )),
        exit_code: Some(0),
        digest: Some(revision.document_digest).filter(|d| !d.is_empty()),
        cache: if reference.n.is_some() {
            Cache::Immutable
        } else {
            Cache::NoCache
        },
        disposition: None,
    };
    cache::object(&shared, &headers, served).await
}

/// `GET /v1/sockets/{ref}/revisions?after=&limit=` (RA5, VN3): the line's
/// history, by number.
pub(super) async fn revisions(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(name): Path<String>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let reference = SocketRef::parse(&name)?;
    let params = query::params(uri.query(), &["after", "limit", "owner"])?;
    let limit = query::limit(&params, shared.config.limits.page_max)?;
    let after = query::after(&params)?;
    let after_n = if after.is_empty() {
        0
    } else {
        after.parse::<i64>().map_err(|_| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                "after is not a cursor of this list",
            )
            .with("parameter", "after")
        })?
    };
    let (owner, extra) = super::owner(&shared, &caller, &params).await?;
    let (reading, id) = (Arc::clone(&shared), reference.id.clone());
    let (line, mut rows) = blocking(move || {
        let line = reading.db.line(owner, &id).map_err(|e| db_error(&e))?;
        let rows = reading
            .db
            .revisions_page(owner, &id, after_n, limit.saturating_add(1))
            .map_err(|e| db_error(&e))?;
        Ok((line, rows))
    })
    .await?;
    if line.is_none() {
        return Err(not_found());
    }
    let keys: Vec<String> = rows.iter().map(|r| r.n.to_string()).collect();
    let mut keyed: Vec<(String, crate::state::RevisionRow)> =
        keys.into_iter().zip(rows.drain(..)).collect();
    let base = format!("/v1/sockets/{}/revisions", reference.id);
    let next = super::next_url(&base, &mut keyed, limit, |(k, _)| k.as_str(), &extra);
    let items: Vec<RevisionItem> = keyed.iter().map(|(_, r)| RevisionItem::of(r)).collect();
    Ok(super::page(&Page::new(REVISION_KIND, items, next)))
}

/// `DELETE /v1/sockets/{line}` (RA6): `204`, a tombstone; a later
/// extraction of its triple revives the id (RA4). A revision is not
/// deleted alone: `@rN` is `400 invalid_reference`.
pub(super) async fn delete(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let reference = SocketRef::parse(&name)?;
    if reference.n.is_some() {
        return Err(ApiError::new(
            ErrorCode::InvalidReference,
            "a line is deleted by its id, sk_ and 20 hex digits, without @rN",
        ));
    }
    let (writing, owner) = (Arc::clone(&shared), caller.user.key);
    let deleted = blocking(move || {
        writing
            .db
            .delete_line(owner, &reference.id, writing.ids.now_ms())
            .map_err(|e| db_error(&e))
    })
    .await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found())
    }
}
