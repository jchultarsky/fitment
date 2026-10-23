//! Section 7.4's routes (docs/research/m6-api.md, RA1, RA2, VN3 to VN5,
//! HX8, CX1 to CX4, M6-50, M6-60, M6-63): the catalog the server opened
//! (RA1), shared by every user. Reads are one snapshot each on a pooled
//! reader (CX2), behind the read slots (`limits.read_concurrency`), each
//! the CLI's `--format json` bytes (DC1): `list` a page at a time (its
//! cursor in `Link` alone: `fitment.catalog` v1 has no `next`) or whole
//! with `limit=all`, built in one snapshot and sent from memory, at most
//! `limits.list_all_concurrency` at once (VN3, HX8); `get`, its features
//! and its bytes. A catalog view's ETag is `"<instance>.g<generation>.
//! <build>"` (VN5, M6-63); a page of another generation than `If-Match`
//! names is `412 precondition_failed` (VN4). One user holds at most half
//! the read slots and one `limit=all` listing (the review of PR 7b). A
//! delete is synchronous, one transaction on a short-lived writer, at
//! most two at once (M6-50); a delete or a replace under `If-Match` is
//! made on condition of its generation (RA7); every other change is a
//! write-lane job, verify a maintenance-lane one (JS1), for writers; `gc`
//! and `prune` for admins (AU5). Every body is hashed, and gzipped, where
//! it was built, on the blocking pool; a step above
//! [`cache::SEND_BUFFER_MAX`] is checked in one pass and streamed in a
//! second (OB3).
//!
//! Hook points (OP11): `catalog.list` (a whole listing, in its slot),
//! `catalog.step` (between a step's snapshot and its read) and
//! `catalog.delete` (a delete, in its slot, before its writer opens).

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::Response;

use super::jobs::{self, Id, Input, Submission};
use crate::Shared;
use crate::catalog::StepBytes;
use crate::catalog::{self, StatusFilter};
use crate::http::cache;
use crate::http::error::{ApiError, ErrorCode};
use crate::http::extract::Path;
use crate::http::extract::{Admin, Reader, Writer};
use crate::http::json::{self, Sent};
use crate::http::{blocking, query};
use crate::jobs::model::Kind;
use crate::jobs::request::{
    self, DeleteRequest, GcJob, GcRequest, IngestFileJob, IngestJob, IngestRequest, PruneJob,
    PruneRequest, ReindexJob, ReindexRequest, ReplaceJob, ReplaceRequest, VerifyJob, VerifyRequest,
};

/// A catalog read on the blocking pool behind a read slot (OB3's,
/// `limits.read_concurrency`), so concurrent reads are bounded, and of
/// user `user`'s share of them, half the slots at most, so one user
/// cannot hold them all (the review of PR 7b); a busy catalog is counted
/// (OP6).
async fn read<T: Send + 'static>(
    shared: &Arc<Shared>,
    user: i64,
    work: impl FnOnce(&catalog::CatalogDir) -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let half = usize::try_from(shared.config.limits.read_concurrency / 2).unwrap_or(1);
    let share = crate::http::share(shared, user, "read_concurrency", half)?;
    let reading = Arc::clone(shared);
    let result = crate::http::catalog_read(shared, move || {
        let result = work(&reading.catalog);
        drop(share);
        result
    })
    .await;
    if let Err(error) = &result {
        if error.code() == ErrorCode::CatalogBusy {
            shared.metrics.catalog_busy();
        }
    }
    result
}

/// VN4, VN5: a catalog view's answer to its conditions: `304` when
/// `If-None-Match` names its ETag; `412 precondition_failed` when
/// `If-Match` names another (a page of another generation).
fn conditions(headers: &HeaderMap, etag: &str) -> Result<Option<Response>, ApiError> {
    let names = |value: &str, tag: &str| {
        value
            .split(',')
            .map(str::trim)
            .any(|t| t == "*" || t == tag)
    };
    let matched: Vec<&str> = headers
        .get_all(header::IF_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    if !matched.is_empty() && !matched.iter().any(|v| names(v, etag)) {
        return Err(ApiError::new(
            ErrorCode::PreconditionFailed,
            "the catalog is at another generation than If-Match names: start the list again",
        )
        .with("etag", etag));
    }
    if cache::not_modified(headers, etag) {
        return Ok(Some(cache::not_modified_response(etag, "no-cache")));
    }
    Ok(None)
}

/// A catalog document and its `Repr-Digest`, made where it was built.
type Digested = (String, String);

/// `json` with its `Repr-Digest`, on the blocking pool.
fn digested(json: String) -> Digested {
    let digest = json::repr_digest(json.as_bytes());
    (json, digest)
}

/// A catalog document's response: its bytes, `Fitment-Format`,
/// `Fitment-Exit-Code` (DC3, DC4), the view's ETag and `no-cache` (VN5).
fn view((json, digest): Digested, etag: &str, exit: u8) -> Response {
    let mut response = json::bytes_digested(
        StatusCode::OK,
        json::JSON,
        Some((catalog::FORMAT, catalog::FORMAT_VERSION)),
        Bytes::from(json),
        &digest,
    );
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, value);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert("fitment-exit-code", HeaderValue::from(u16::from(exit)));
    response
}

/// `GET /v1/catalog` (DC11): the catalog's generation, counts and state,
/// `fitment.info`'s `catalog` object alone; its ETag the views' with its
/// state's flags folded in, so a reindex queued or the barrier raised at
/// one generation is a new tag, never a stale `304` (the review of PR
/// 7b).
pub(super) async fn info(
    State(shared): State<Arc<Shared>>,
    _: Reader,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let info = super::server::catalog_info(&shared).await?;
    let etag = shared.catalog.info_etag(&info);
    if let Some(answer) = conditions(&headers, &etag)? {
        return Ok(answer);
    }
    let mut response = json::bytes(
        StatusCode::OK,
        json::JSON,
        None,
        Bytes::from(json::pretty(&info)),
    );
    if let Ok(value) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok(response)
}

/// HX8: a `limit=all` listing's slot, taken without waiting: past
/// `limits.list_all_concurrency`, or a second of one user's (the review of
/// PR 7b), `429 too_many_streams`, retryable.
fn list_all_slot(
    shared: &Arc<Shared>,
    user: i64,
) -> Result<(crate::http::Share, tokio::sync::OwnedSemaphorePermit), ApiError> {
    let share = crate::http::share(shared, user, "list_all_concurrency", 1)?;
    let slot = Arc::clone(&shared.list_all_slots)
        .try_acquire_owned()
        .map_err(|_| {
            ApiError::new(
                ErrorCode::TooManyStreams,
                "the most whole listings are being built at once: try again, or page",
            )
            .with("limit", "list_all_concurrency")
            .retry_after(1)
        })?;
    Ok((share, slot))
}

/// `GET /v1/catalog/entries?status=&after=&limit=` (VN3, VN4, VN5): a page
/// of `catalog list --status S --format json`, its next page in `Link`;
/// with `limit=all` exactly the CLI's bytes, built in one snapshot and
/// sent from memory (HX8).
pub(super) async fn list(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user = caller.user.key;
    let params = query::params(uri.query(), &["status", "after", "limit"])?;
    let filter = match params.get("status") {
        Some(name) => StatusFilter::parse(name).ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                "status is indexed, quarantined, stale or all",
            )
            .with("parameter", "status")
        })?,
        None => StatusFilter::All,
    };
    if params.get("limit").map(String::as_str) == Some("all") {
        if params.contains_key("after") {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "limit=all lists every entry: it takes no cursor",
            )
            .with("parameter", "after"));
        }
        let slot = list_all_slot(&shared, user)?;
        let hooked = Arc::clone(&shared);
        let (generation, json) = read(&shared, user, move |dir| {
            crate::jobs::run::hook(&hooked, "catalog.list");
            let built = dir.list_all(filter);
            drop(slot);
            built.map(|(generation, json)| (generation, digested(json)))
        })
        .await?;
        let etag = shared.catalog.etag(generation);
        if let Some(answer) = conditions(&headers, &etag)? {
            return Ok(answer);
        }
        return Ok(view(json, &etag, 0));
    }
    let limit = query::limit(&params, shared.config.limits.page_max)?;
    let after = query::after(&params)?;
    let cursor = (!after.is_empty()).then_some(after);
    let size = usize::try_from(limit).unwrap_or(usize::MAX);
    let (page, json) = read(&shared, user, move |dir| {
        let mut page = dir.list_page(filter, cursor.as_deref(), size)?;
        let json = digested(std::mem::take(&mut page.json));
        Ok((page, json))
    })
    .await?;
    let etag = shared.catalog.etag(page.generation);
    if let Some(answer) = conditions(&headers, &etag)? {
        return Ok(answer);
    }
    let mut response = view(json, &etag, 0);
    if let Some(next) = page.next {
        let url = format!(
            "/v1/catalog/entries?after={}&limit={limit}&status={}",
            query::cursor(&next),
            filter.name()
        );
        if let Ok(value) = HeaderValue::from_str(&format!("<{url}>; rel=\"next\"")) {
            response.headers_mut().insert(header::LINK, value);
        }
    }
    Ok(response)
}

/// RA2: the id part of a reference as given, for `Content-Location`.
fn asked_id(reference: &str) -> Option<&str> {
    (!reference.starts_with("sha256:")).then(|| reference.split('@').next().unwrap_or(reference))
}

/// `GET /v1/catalog/entries/{entry}` (ID7, RA2): `catalog get REF --format
/// json`: the entry, its properties and history, a tombstone too; a
/// case-folded match carries `Content-Location` naming the id as stored.
pub(super) async fn get(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(entry): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    catalog::check_reference(&entry)?;
    let named = entry.clone();
    let (got, json) = read(&shared, caller.user.key, move |dir| {
        let mut got = dir.get(&named)?;
        let json = digested(std::mem::take(&mut got.json));
        Ok((got, json))
    })
    .await?;
    let etag = shared.catalog.etag(got.generation);
    if let Some(answer) = conditions(&headers, &etag)? {
        return Ok(answer);
    }
    let mut response = view(json, &etag, got.exit);
    if asked_id(&entry).is_some_and(|asked| asked != got.id) {
        let revision = if entry.contains('@') {
            format!("@r{}", got.revision)
        } else {
            String::new()
        };
        let location = format!("/v1/catalog/entries/{}{revision}", got.id);
        if let Ok(value) = HeaderValue::from_str(&location) {
            response
                .headers_mut()
                .insert(header::CONTENT_LOCATION, value);
        }
    }
    Ok(response)
}

/// `GET /v1/catalog/entries/{entry}/features` (GT2, QR4, VN5): the stored
/// `features.v1`, a derived document: its body's ETag, `no-cache`; a
/// quarantined entry's refused document with `Fitment-Exit-Code: 3`.
pub(super) async fn features(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(entry): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    catalog::check_reference(&entry)?;
    let gz = cache::wants_gzip(&headers);
    let (got, sha256, bytes, ready) = read(&shared, caller.user.key, move |dir| {
        let mut got = dir.features(&entry)?;
        let body = std::mem::take(&mut got.json).into_bytes();
        let sha256 = fitment_core::sha256_hex(&body);
        let bytes = u64::try_from(body.len()).unwrap_or(u64::MAX);
        let ready = cache::ready(body, &sha256, gz);
        Ok((got, sha256, bytes, ready))
    })
    .await?;
    let served = cache::Served {
        sha256,
        bytes,
        content_type: json::JSON,
        format: Some((
            fitment_core::FEATURES_FORMAT.to_owned(),
            fitment_core::FEATURES_FORMAT_VERSION,
        )),
        exit_code: Some(got.exit),
        digest: None,
        cache: cache::Cache::NoCache,
        disposition: None,
    };
    Ok(cache::memory(&headers, &served, ready))
}

/// `GET /v1/catalog/entries/{entry}/step` (ST11, OB3, HX6, VN5): the
/// stored bytes, re-hashed first: within [`cache::SEND_BUFFER_MAX`] read
/// whole, else hashed in one pass and streamed in a second that fails on
/// a change, a chunk per blocking read, nothing large held (the review of
/// PR 7b); `immutable` when the URL names an exact revision or content,
/// `no-cache` for a bare id.
pub(super) async fn step(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(entry): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    catalog::check_reference(&entry)?;
    let exact = entry.contains('@') || entry.starts_with("sha256:");
    let gz = cache::wants_gzip(&headers);
    let hooked = Arc::clone(&shared);
    let (step, body) = read(&shared, caller.user.key, move |dir| {
        let between = || crate::jobs::run::hook(&hooked, "catalog.step");
        let catalog::Step {
            id,
            revision,
            sha256,
            length,
            bytes,
        } = dir.step(&entry, cache::SEND_BUFFER_MAX, &between)?;
        let body = match bytes {
            StepBytes::Whole(bytes) => Ok(cache::ready(bytes, &sha256, gz)),
            StepBytes::Stream(reader) => Err(reader),
        };
        Ok(((id, revision, sha256, length), body))
    })
    .await?;
    let (id, revision, sha256, length) = step;
    let served = cache::Served {
        sha256,
        bytes: length,
        content_type: "application/octet-stream",
        format: None,
        exit_code: Some(0),
        digest: None,
        cache: if exact {
            cache::Cache::Immutable
        } else {
            cache::Cache::NoCache
        },
        disposition: Some(format!("{id}_r{revision}.stp")),
    };
    Ok(match body {
        Ok(ready) => cache::memory(&headers, &served, ready),
        Err(reader) => cache::stream(&headers, &served, reader),
    })
}

/// RA7: the generation a write's `If-Match` makes it conditional on:
/// none without the header or with `*` (any current entry, which the
/// write needs anyway); a tag of this catalog's views its generation (of
/// several, the latest); a tag of none, `-1`, a generation the catalog
/// is never at, so the write answers `412` once its references name what
/// it changes (RFC 9110, 13.1.1 and 13.2.2).
fn if_match(shared: &Shared, headers: &HeaderMap) -> Option<i64> {
    let values: Vec<&str> = headers
        .get_all(header::IF_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .collect();
    if values.is_empty() || values.contains(&"*") {
        return None;
    }
    let generations = values
        .iter()
        .filter_map(|tag| shared.catalog.tag_generation(tag));
    Some(generations.max().unwrap_or(-1))
}

/// RA8, SZ1: an ingested file's names and its id, checked before any job;
/// `input` defaults to `file_name`.
fn file_of(
    upload: &str,
    file_name: &str,
    input: Option<&str>,
) -> Result<(String, String, String), ApiError> {
    let sha256 = request::sha256_ref(upload, "upload")?;
    request::file_name(file_name)?;
    let input = input.unwrap_or(file_name);
    request::text(input, "input")?;
    Ok((sha256, file_name.to_owned(), input.to_owned()))
}

/// SZ1: `max_mib` at most the upload cap.
fn max_mib(shared: &Shared, asked: Option<u64>) -> Result<Option<u64>, ApiError> {
    match asked {
        Some(n) if n > shared.config.limits.max_upload_mib => Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "max_mib is at most the upload cap, {} MiB",
                shared.config.limits.max_upload_mib
            ),
        )
        .with("parameter", "max_mib")),
        other => Ok(other),
    }
}

/// `POST /v1/catalog/entries` (CA1): the job `catalog_ingest`, `catalog
/// ingest [--id ID] [--max-mib N] FILE… --format json`; each file an
/// upload of the caller's, named as the client names it (RA8), with its
/// own id if it gives one (ID1).
pub(super) async fn ingest(
    State(shared): State<Arc<Shared>>,
    Writer(caller): Writer,
    axum::Extension(request_id): Id,
    headers: HeaderMap,
    Sent(body, sent): Sent<IngestRequest>,
) -> Result<Response, ApiError> {
    let fingerprint = request::fingerprint("POST", "/v1/catalog/entries", &sent);
    if let Some(replayed) = jobs::replay(&shared, &caller, &headers, &fingerprint).await? {
        return Ok(replayed);
    }
    let cap = u64::try_from(body.files.len()).unwrap_or(u64::MAX);
    if body.files.is_empty() || cap > shared.config.limits.max_candidates {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "an ingest names 1 to {} files",
                shared.config.limits.max_candidates
            ),
        )
        .with("parameter", "files")
        .with("limit", "max_candidates"));
    }
    let max_mib = max_mib(&shared, body.max_mib)?;
    let mut files = Vec::with_capacity(body.files.len());
    let mut inputs: Vec<Input> = Vec::new();
    for file in body.files {
        let (sha256, file_name, input) =
            file_of(&file.upload, &file.file_name, file.input.as_deref())?;
        if let Some(id) = &file.id {
            catalog::check_id(id)?;
        }
        if !inputs.iter().any(|i| i.sha256 == sha256) {
            let bytes = super::nameable(&shared, caller.user.key, &sha256).await?;
            inputs.push(Input::upload(sha256.clone(), bytes));
        }
        files.push(IngestFileJob {
            upload: format!("sha256:{sha256}"),
            file_name,
            input,
            id: file.id,
        });
    }
    let (request, request_sha256) = request::normalized(&IngestJob { files, max_mib });
    let submission = Submission {
        kind: Kind::CatalogIngest,
        request,
        request_sha256,
        inputs,
        fingerprint,
        cites: Vec::new(),
    };
    jobs::submit(&shared, &caller, &headers, &request_id, submission).await
}

/// `PUT /v1/catalog/entries/{entry}` (CA1, CR3, RA2): the job
/// `catalog_replace`, `catalog replace REF FILE --format json`; `ID@rN`
/// only for the current revision, decided in the job's transaction, as is
/// `If-Match`'s generation (RA7): another ends the job `refused`,
/// `precondition_failed`, nothing changed.
pub(super) async fn replace(
    State(shared): State<Arc<Shared>>,
    Writer(caller): Writer,
    axum::Extension(request_id): Id,
    Path(entry): Path<String>,
    headers: HeaderMap,
    Sent(body, sent): Sent<ReplaceRequest>,
) -> Result<Response, ApiError> {
    let path = format!("/v1/catalog/entries/{entry}");
    let fingerprint = request::fingerprint("PUT", &path, &sent);
    if let Some(replayed) = jobs::replay(&shared, &caller, &headers, &fingerprint).await? {
        return Ok(replayed);
    }
    catalog::check_reference(&entry)?;
    let if_generation = if_match(&shared, &headers);
    if let Some(asked) = if_generation {
        // RA7: false now is `412` now; the job's transaction checks again.
        let now = read(
            &shared,
            caller.user.key,
            catalog::CatalogDir::generation_now,
        )
        .await?;
        if now != asked {
            return Err(ApiError::new(
                ErrorCode::PreconditionFailed,
                "the catalog changed since the entity tag If-Match names: nothing was changed",
            )
            .with("generation", now));
        }
    }
    let max_mib = max_mib(&shared, body.max_mib)?;
    let (sha256, file_name, input) = file_of(&body.upload, &body.file_name, body.input.as_deref())?;
    let bytes = super::nameable(&shared, caller.user.key, &sha256).await?;
    let (request, request_sha256) = request::normalized(&ReplaceJob {
        entry,
        upload: format!("sha256:{sha256}"),
        file_name,
        input,
        max_mib,
        if_generation,
    });
    let submission = Submission {
        kind: Kind::CatalogReplace,
        request,
        request_sha256,
        inputs: vec![Input::upload(sha256, bytes)],
        fingerprint,
        cites: Vec::new(),
    };
    jobs::submit(&shared, &caller, &headers, &request_id, submission).await
}

/// M6-50, CX3, CU1: a delete, synchronous, on a short-lived writer: `200`
/// with `fitment.catalog` and its `changes`; `503 catalog_busy` past
/// `catalog.delete_busy_ms`. At most two run at once; one that waits for
/// a slot past the parse slots' wait is `503 catalog_busy`, `Retry-After:
/// 1`, before any writer opens (the review of PR 7b). `at`: `If-Match`'s
/// generation (RA7).
async fn delete_ids(
    shared: &Arc<Shared>,
    ids: Vec<String>,
    at: Option<i64>,
) -> Result<Response, ApiError> {
    let slots = Arc::clone(&shared.delete_slots);
    let slot = tokio::time::timeout(crate::gate::slot_wait(shared), slots.acquire_owned())
        .await
        .map_err(|_| {
            shared.metrics.catalog_busy();
            ApiError::new(
                ErrorCode::CatalogBusy,
                "the most deletes are running at once: try again",
            )
            .with("limit", "delete_concurrency")
            .retry_after(1)
        })?
        .map_err(|_| ApiError::internal())?;
    let writing = Arc::clone(shared);
    let deleted = blocking(move || {
        crate::jobs::run::hook(&writing, "catalog.delete");
        let deleted = writing.catalog.delete(&ids, at);
        let deleted = deleted.map(|d| (d.generation, digested(d.json)));
        drop(slot);
        deleted
    })
    .await;
    let deleted = deleted.inspect_err(|e| {
        if e.code() == ErrorCode::CatalogBusy {
            shared.metrics.catalog_busy();
        }
    })?;
    let (_, (json, digest)) = deleted;
    let mut response = json::bytes_digested(
        StatusCode::OK,
        json::JSON,
        Some((catalog::FORMAT, catalog::FORMAT_VERSION)),
        Bytes::from(json),
        &digest,
    );
    response
        .headers_mut()
        .insert("fitment-exit-code", HeaderValue::from_static("0"));
    Ok(response)
}

/// `DELETE /v1/catalog/entries/{entry}` (M6-50, CU1): `catalog delete ID
/// --format json`; a content reference is `400 not_an_id` (RA2); under
/// `If-Match`, on condition of its generation, else `412
/// precondition_failed`, nothing deleted (RA7).
pub(super) async fn delete(
    State(shared): State<Arc<Shared>>,
    _: Writer,
    Path(entry): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let at = if_match(&shared, &headers);
    delete_ids(&shared, vec![entry], at).await
}

/// `POST /v1/catalog/delete` (CR2, CU1): `catalog delete ID… --format
/// json`, all or none: unknown ids `404 entry_not_found` listing them,
/// nothing deleted.
pub(super) async fn delete_many(
    State(shared): State<Arc<Shared>>,
    _: Writer,
    json::Json(body): json::Json<DeleteRequest>,
) -> Result<Response, ApiError> {
    let cap = u64::try_from(body.ids.len()).unwrap_or(u64::MAX);
    if body.ids.is_empty() || cap > shared.config.limits.max_candidates {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "a delete names 1 to {} ids",
                shared.config.limits.max_candidates
            ),
        )
        .with("parameter", "ids"));
    }
    delete_ids(&shared, body.ids, None).await
}

/// A job of the catalog's with nothing to hold: its kind and normalized
/// request, submitted (JB1).
async fn simple(
    shared: &Arc<Shared>,
    caller: &crate::http::extract::Principal,
    headers: &HeaderMap,
    request_id: &crate::http::RequestId,
    kind: Kind,
    normalized: (String, String),
    fingerprint: String,
) -> Result<Response, ApiError> {
    let (request, request_sha256) = normalized;
    let submission = Submission {
        kind,
        request,
        request_sha256,
        inputs: Vec::new(),
        fingerprint,
        cites: Vec::new(),
    };
    jobs::submit(shared, caller, headers, request_id, submission).await
}

/// `POST /v1/catalog/reindex` (RX4 to RX9): the job `catalog_reindex`,
/// `catalog reindex [REF…] [--all] --format json`; neither field, every
/// stale entry; `refs` with `all` is `400 options_conflict`.
pub(super) async fn reindex(
    State(shared): State<Arc<Shared>>,
    Writer(caller): Writer,
    axum::Extension(request_id): Id,
    headers: HeaderMap,
    Sent(body, sent): Sent<ReindexRequest>,
) -> Result<Response, ApiError> {
    let fingerprint = request::fingerprint("POST", "/v1/catalog/reindex", &sent);
    if let Some(replayed) = jobs::replay(&shared, &caller, &headers, &fingerprint).await? {
        return Ok(replayed);
    }
    let refs = body.refs.unwrap_or_default();
    let all = body.all.unwrap_or(false);
    if all && !refs.is_empty() {
        return Err(ApiError::new(
            ErrorCode::OptionsConflict,
            "refs and all exclude each other, as REF and --all do",
        ));
    }
    if u64::try_from(refs.len()).unwrap_or(u64::MAX) > shared.config.limits.max_candidates {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "a reindex names at most {} entries",
                shared.config.limits.max_candidates
            ),
        )
        .with("parameter", "refs"));
    }
    for text in &refs {
        catalog::check_reference(text)?;
    }
    let normalized = request::normalized(&ReindexJob { refs, all });
    simple(
        &shared,
        &caller,
        &headers,
        &request_id,
        Kind::CatalogReindex,
        normalized,
        fingerprint,
    )
    .await
}

/// `POST /v1/catalog/verify` (CU2, M6-60): the job `catalog_verify` on
/// the maintenance lane, for writers.
pub(super) async fn verify(
    State(shared): State<Arc<Shared>>,
    Writer(caller): Writer,
    axum::Extension(request_id): Id,
    headers: HeaderMap,
    Sent(body, sent): Sent<VerifyRequest>,
) -> Result<Response, ApiError> {
    let fingerprint = request::fingerprint("POST", "/v1/catalog/verify", &sent);
    if let Some(replayed) = jobs::replay(&shared, &caller, &headers, &fingerprint).await? {
        return Ok(replayed);
    }
    let normalized = request::normalized(&VerifyJob {
        deep: body.deep.unwrap_or(false),
        full: body.full.unwrap_or(false),
    });
    simple(
        &shared,
        &caller,
        &headers,
        &request_id,
        Kind::CatalogVerify,
        normalized,
        fingerprint,
    )
    .await
}

/// `POST /v1/catalog/gc` (CU2, AU5): the job `catalog_gc`, for admins.
pub(super) async fn gc(
    State(shared): State<Arc<Shared>>,
    Admin(caller): Admin,
    axum::Extension(request_id): Id,
    headers: HeaderMap,
    Sent(body, sent): Sent<GcRequest>,
) -> Result<Response, ApiError> {
    let fingerprint = request::fingerprint("POST", "/v1/catalog/gc", &sent);
    if let Some(replayed) = jobs::replay(&shared, &caller, &headers, &fingerprint).await? {
        return Ok(replayed);
    }
    let normalized = request::normalized(&GcJob {
        versions: body.versions.unwrap_or(false),
        vacuum: body.vacuum.unwrap_or(false),
    });
    simple(
        &shared,
        &caller,
        &headers,
        &request_id,
        Kind::CatalogGc,
        normalized,
        fingerprint,
    )
    .await
}

/// `POST /v1/catalog/prune` (CR11, CU1, AU5): the job `catalog_prune`,
/// for admins; `{"revisions": true}` is required, as the flag is.
pub(super) async fn prune(
    State(shared): State<Arc<Shared>>,
    Admin(caller): Admin,
    axum::Extension(request_id): Id,
    headers: HeaderMap,
    Sent(body, sent): Sent<PruneRequest>,
) -> Result<Response, ApiError> {
    let fingerprint = request::fingerprint("POST", "/v1/catalog/prune", &sent);
    if let Some(replayed) = jobs::replay(&shared, &caller, &headers, &fingerprint).await? {
        return Ok(replayed);
    }
    if body.revisions != Some(true) {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "prune what? {\"revisions\": true} releases the bytes of superseded revisions",
        )
        .with("parameter", "revisions"));
    }
    let normalized = request::normalized(&PruneJob { revisions: true });
    simple(
        &shared,
        &caller,
        &headers,
        &request_id,
        Kind::CatalogPrune,
        normalized,
        fingerprint,
    )
    .await
}
