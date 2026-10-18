//! Section 7.5's routes (docs/research/m6-api.md, RA3, RA6, RA8, VN3,
//! VN5, DC4): register an uploaded assembly (the job `assembly_read`,
//! `fitment parts FILE`), list and read the caller's assemblies, their
//! `fitment.parts` (derived: `no-cache`, `409 derived_stale` when no
//! reading of this build exists), their bytes and their socket lines, and
//! delete one no live socket line names. An admin's `?owner=` reads
//! another user's on `GET` only (AU8).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;

use super::jobs::{self, Id, Submission};
use crate::Shared;
use crate::assemblies::{self, ASSEMBLY_FORMAT, ASSEMBLY_FORMAT_VERSION, AssemblyDocument};
use crate::auth::documents::Page;
use crate::http::cache::{self, Cache, Served};
use crate::http::error::{ApiError, ErrorCode};
use crate::http::extract::Reader;
use crate::http::json::{self, Sent};
use crate::http::{blocking, db_error, query};
use crate::jobs::model::Kind;
use crate::jobs::request::{self, AssemblyJob, AssemblyRequest};
use crate::state::{AssemblyDeleted, AssemblyRow};

/// HX5: `sha256:` and 64 hex digits, either case.
fn reference(segment: &str) -> Result<String, ApiError> {
    request::sha256_ref(segment, "assembly")
}

pub(super) fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "no assembly of these bytes is yours")
        .with("resource", "assembly")
}

/// The owner's assembly of `sha256`, or `404`.
pub(super) async fn of(
    shared: &Arc<Shared>,
    owner: i64,
    sha256: String,
) -> Result<AssemblyRow, ApiError> {
    let reading = Arc::clone(shared);
    blocking(move || {
        reading
            .db
            .assembly(owner, &sha256, assemblies::versions())
            .map_err(|e| db_error(&e))
    })
    .await?
    .ok_or_else(not_found)
}

/// `POST /v1/assemblies` (RA3): the job `assembly_read`; an assembly the
/// caller registered already and that is current at this build is `200`
/// with its `fitment.assembly`, no job, its name added.
pub(super) async fn create(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    axum::Extension(request_id): Id,
    headers: HeaderMap,
    Sent(body, sent): Sent<AssemblyRequest>,
) -> Result<Response, ApiError> {
    let sha256 = request::sha256_ref(&body.upload, "upload")?;
    if let Some(name) = &body.file_name {
        request::file_name(name)?;
    }
    let bytes = super::nameable(&shared, caller.user.key, &sha256).await?;
    let fingerprint = request::fingerprint("POST", "/v1/assemblies", &sent);
    if let Some(replayed) = jobs::replay(&shared, &caller, &headers, &fingerprint).await? {
        return Ok(replayed);
    }
    let owner = caller.user.key;
    let found = of(&shared, owner, sha256.clone()).await.ok();
    if found
        .as_ref()
        .and_then(|a| a.parts.as_ref())
        .is_some_and(assemblies::current)
    {
        if let Some(name) = body.file_name.clone() {
            let (writing, sha) = (Arc::clone(&shared), sha256.clone());
            blocking(move || {
                writing
                    .db
                    .name_assembly(owner, &sha, &name)
                    .map_err(|e| db_error(&e))
            })
            .await?;
        }
        let row = of(&shared, owner, sha256).await?;
        return Ok(json::document(
            ASSEMBLY_FORMAT,
            ASSEMBLY_FORMAT_VERSION,
            &AssemblyDocument::of(&row),
        ));
    }
    let (request, request_sha256) = request::normalized(&AssemblyJob {
        upload: format!("sha256:{sha256}"),
        file_name: body.file_name,
    });
    let submission = Submission {
        kind: Kind::AssemblyRead,
        request,
        request_sha256,
        inputs: vec![(sha256, bytes)],
        fingerprint,
    };
    jobs::submit(&shared, &caller, &headers, &request_id, submission).await
}

/// `GET /v1/assemblies?after=&limit=` (VN3): the caller's, by SHA-256.
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
        reading
            .db
            .assemblies_page(
                owner,
                &after,
                limit.saturating_add(1),
                assemblies::versions(),
            )
            .map_err(|e| db_error(&e))
    })
    .await?;
    let next = super::next_url(
        "/v1/assemblies",
        &mut rows,
        limit,
        |r| r.sha256.as_str(),
        &extra,
    );
    let items = rows.iter().map(AssemblyDocument::of).collect();
    Ok(super::page(&Page::new(ASSEMBLY_FORMAT, items, next)))
}

/// `GET /v1/assemblies/sha256:{hex}`: `fitment.assembly`.
pub(super) async fn get(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(name): Path<String>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let sha256 = reference(&name)?;
    let params = query::params(uri.query(), &["owner"])?;
    let (owner, _) = super::owner(&shared, &caller, &params).await?;
    let row = of(&shared, owner, sha256).await?;
    Ok(json::document(
        ASSEMBLY_FORMAT,
        ASSEMBLY_FORMAT_VERSION,
        &AssemblyDocument::of(&row),
    ))
}

/// `GET /v1/assemblies/sha256:{hex}/parts` (RA3, VN5): this build's
/// `fitment.parts` bytes, `no-cache`, `Fitment-Exit-Code` 0 or 3; read by
/// another build only, `409 derived_stale`.
pub(super) async fn parts(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(name): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let sha256 = reference(&name)?;
    let params = query::params(uri.query(), &["owner"])?;
    let (owner, _) = super::owner(&shared, &caller, &params).await?;
    let row = of(&shared, owner, sha256).await?;
    let Some(parts) = row.parts.filter(assemblies::current) else {
        return Err(ApiError::new(
            ErrorCode::DerivedStale,
            "the assembly was read by another build: register it again (POST /v1/assemblies)",
        )
        .with("extractor_version", fitment_core::EXTRACTOR_VERSION)
        .with("socket_rules_version", fitment_core::SOCKET_RULES_VERSION));
    };
    let served = Served {
        sha256: parts.document_sha256,
        bytes: parts.document_bytes,
        content_type: json::JSON,
        format: Some((
            fitment_core::PARTS_FORMAT.to_owned(),
            fitment_core::PARTS_FORMAT_VERSION,
        )),
        exit_code: Some(if parts.status == "accepted" { 0 } else { 3 }),
        digest: None,
        cache: Cache::NoCache,
        disposition: None,
    };
    cache::object(&shared, &headers, served).await
}

/// `GET /v1/assemblies/sha256:{hex}/step` (OB3, HX6): the stored bytes,
/// re-hashed first; `immutable`, since the URL names them exactly.
pub(super) async fn step(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(name): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let sha256 = reference(&name)?;
    let params = query::params(uri.query(), &["owner"])?;
    let (owner, _) = super::owner(&shared, &caller, &params).await?;
    let row = of(&shared, owner, sha256).await?;
    let served = Served {
        disposition: Some(format!("{}.stp", &row.sha256[..12])),
        sha256: row.sha256,
        bytes: row.bytes,
        content_type: "application/octet-stream",
        format: None,
        exit_code: None,
        digest: None,
        cache: Cache::Immutable,
    };
    cache::object(&shared, &headers, served).await
}

/// `GET /v1/assemblies/sha256:{hex}/sockets?after=&limit=` (VN3): the
/// caller's live socket lines of the assembly, by id.
pub(super) async fn sockets(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(name): Path<String>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let sha256 = reference(&name)?;
    let params = query::params(uri.query(), &["after", "limit", "owner"])?;
    let (owner, extra) = super::owner(&shared, &caller, &params).await?;
    let row = of(&shared, owner, sha256).await?;
    let base = format!("/v1/assemblies/sha256:{}/sockets", row.sha256);
    super::sockets::page(&shared, owner, Some(row.sha256), &params, &base, &extra).await
}

/// `DELETE /v1/assemblies/sha256:{hex}` (RA6): `204`; `409
/// assembly_in_use` while a live socket line of the caller's names it.
pub(super) async fn delete(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let sha256 = reference(&name)?;
    let (writing, owner) = (Arc::clone(&shared), caller.user.key);
    let deleted = blocking(move || {
        writing
            .db
            .delete_assembly(owner, &sha256)
            .map_err(|e| db_error(&e))
    })
    .await?;
    match deleted {
        AssemblyDeleted::Gone => Ok(StatusCode::NO_CONTENT),
        AssemblyDeleted::InUse => Err(ApiError::new(
            ErrorCode::AssemblyInUse,
            "a socket line of yours names the assembly: delete the line first",
        )),
        AssemblyDeleted::NotFound => Err(not_found()),
    }
}
