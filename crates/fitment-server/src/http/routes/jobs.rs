//! Section 7.9's routes and the submission every job-creating route shares
//! (docs/research/m6-api.md, JB1, JB5, JB8, JB9, JB11, JB12, JS5, JS10,
//! JS11, VN5, UP4, AU8).
//!
//! A submission is validated before any row exists (JB1): its references,
//! its names, the caller's quota (UP4) and caps (JS5); an
//! `Idempotency-Key` names the job a request already made (JB9); the job,
//! its inputs and its key commit together before the answer (JS11). The
//! answer is `202` with the job, or with `Prefer: wait=N` the finished job,
//! `200`, the wait outside the handler's time (JB11, M6-53).

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use crate::Shared;
use crate::auth::documents::Page;
use crate::http::error::{ApiError, ErrorCode};
use crate::http::extract::{Admin, Principal, Reader};
use crate::http::json::{self, Json};
use crate::http::operations::Role as Needed;
use crate::http::{RequestId, blocking, cache, db_error, query};
use crate::jobs::documents::{self, FORMAT_VERSION, JOB_FORMAT};
use crate::jobs::events::{ChannelBody, Forward, StreamSlot};
use crate::jobs::model::{Kind, State as JobState, Stop};
use crate::jobs::request::{self, PinRequest, SocketRef};
use crate::jobs::{self, Full};
use crate::state::{Cancelled, JobRow, NewJob, Submitted};

/// What a route submits (JB1): its kind, its normalized request (RA9),
/// its inputs with their lengths, and the request as sent (JB9).
#[derive(Debug, Clone)]
pub(crate) struct Submission {
    pub kind: Kind,
    pub request: String,
    pub request_sha256: String,
    /// Every object it names (OB4): `(sha256, bytes)`.
    pub inputs: Vec<(String, u64)>,
    /// JB9's fingerprint of the request as sent.
    pub fingerprint: String,
    /// The socket revisions it cites (OB4's `job_cites`): a `carry_from`.
    pub cites: Vec<i64>,
}

/// HX5: a job's id; anything else names nothing (`404`).
pub(crate) fn job_id(segment: &str) -> Result<String, ApiError> {
    if crate::ids::well_formed("j", segment) {
        Ok(segment.to_owned())
    } else {
        Err(not_found())
    }
}

fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "no job of that id is yours to see")
}

/// AU8: the job `id`, if the caller owns it or is an admin.
pub(crate) async fn find(
    shared: &Arc<Shared>,
    caller: &Principal,
    id: String,
) -> Result<JobRow, ApiError> {
    let owner = (!caller.effective.permits(Needed::Admin)).then_some(caller.user.key);
    let reading = Arc::clone(shared);
    blocking(move || reading.db.job(&id, owner).map_err(|e| db_error(&e)))
        .await?
        .ok_or_else(not_found)
}

/// VN5: a job's ETag, `W/"j<seq>.<state>"`: its last persisted event and
/// its state, so progress changes nothing; then `.p` when pinned and `.c`
/// when a cancel was asked, which write no event (review S5 of PR 5).
pub(crate) fn etag(row: &JobRow) -> String {
    format!(
        "W/\"j{}.{}{}{}\"",
        row.last_seq,
        row.state.name(),
        if row.pinned { ".p" } else { "" },
        if row.cancel_requested { ".c" } else { "" }
    )
}

/// `fitment.job` answered with `status` (section 7.11).
pub(crate) async fn answer(shared: &Arc<Shared>, row: &JobRow, status: StatusCode) -> Response {
    let (reading, queued) = (Arc::clone(shared), row.clone());
    let position = blocking(move || reading.db.queue_position(&queued).map_err(|e| db_error(&e)))
        .await
        .ok()
        .flatten();
    let progress = jobs::progress(shared, row);
    let mut response = json::document(
        JOB_FORMAT,
        FORMAT_VERSION,
        &documents::job(row, position, progress),
    );
    *response.status_mut() = status;
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&etag(row)) {
        headers.insert(header::ETAG, value);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// JB11: the wait asked for, at most `limits.wait_max_seconds`; and the
/// `Preference-Applied` value when it was applied.
fn wait_of(shared: &Shared, wanted: Option<u64>) -> Option<u64> {
    wanted.map(|n| n.min(u64::from(shared.config.limits.wait_max_seconds)))
}

/// JS10: `429 too_many_streams`, past a stream cap.
fn too_many_streams() -> ApiError {
    ApiError::new(
        ErrorCode::TooManyStreams,
        "you hold the most event streams a user may: close one",
    )
    .with("limit", "jobs.streams_per_user")
    .retry_after(1)
}

/// JS10, JB11 (decided in PR 5's review): a wait of more than zero
/// seconds holds one of the caller's stream slots while it waits, as an
/// event stream does; past the cap it is `429 too_many_streams`.
fn wait_slot(
    shared: &Shared,
    caller: &Principal,
    wait: Option<u64>,
) -> Result<Option<StreamSlot>, ApiError> {
    match wait {
        Some(seconds) if seconds > 0 => shared
            .jobs
            .streams
            .take(caller.user.key)
            .map(Some)
            .ok_or_else(too_many_streams),
        _ => Ok(None),
    }
}

/// The answer to a job-creating request (JB11): `202` with the job, or,
/// after a wait, `200` with the finished job; `Location` names it.
pub(crate) async fn created(
    shared: &Arc<Shared>,
    row: JobRow,
    wait: Option<u64>,
) -> Result<Response, ApiError> {
    let row = match wait {
        Some(seconds) => jobs::wait(shared, row, Duration::from_secs(seconds)).await,
        None => row,
    };
    // JB9: a replay answers the status the request would get now.
    let status = if row.state.terminal() {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };
    let mut response = answer(shared, &row, status).await;
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&format!("/v1/jobs/{}", row.id)) {
        headers.insert(header::LOCATION, value);
    }
    if let Some(value) = wait.and_then(|n| HeaderValue::from_str(&format!("wait={n}")).ok()) {
        headers.insert("preference-applied", value);
    }
    Ok(response)
}

/// HX3, M6-53: the handler's own time covers validation and submission;
/// a wait comes after it.
pub(crate) async fn in_time<T>(
    shared: &Shared,
    work: impl std::future::Future<Output = Result<T, ApiError>>,
) -> Result<T, ApiError> {
    let limit = Duration::from_secs(u64::from(shared.config.server.handler_timeout_seconds));
    tokio::time::timeout(limit, work).await.unwrap_or_else(|_| {
        Err(ApiError::new(ErrorCode::RequestTimeout, "the request took too long").retryable(false))
    })
}

/// JS5's refusals: `503 queue_full` or `429 too_many_jobs`, retryable,
/// with `Retry-After`; nothing was written.
fn full(full: Full, lane: &'static str) -> ApiError {
    match full {
        Full::Lane => ApiError::new(
            ErrorCode::QueueFull,
            "the job queue is full: try again later",
        )
        .with("lane", lane)
        .retry_after(5),
        Full::Owner => ApiError::new(
            ErrorCode::TooManyJobs,
            "you have the most jobs queued a user may: try again when one ends",
        )
        .with("lane", lane)
        .retry_after(5),
    }
}

/// The submission's checks that need `server.db` (UP4, JB9) and its
/// insert (JS11). Returns the job and whether it is new.
async fn insert(
    shared: &Arc<Shared>,
    caller: &Principal,
    key: Option<String>,
    request_id: String,
    submission: Submission,
) -> Result<(JobRow, bool), ApiError> {
    if let Some(key) = &key {
        if let Some(found) = existing(shared, caller.user.key, key, &submission.fingerprint).await?
        {
            return Ok((found, false));
        }
    }
    let (checking, owner) = (Arc::clone(shared), caller.user.key);
    let quota = shared.config.limits.user_quota_gib.saturating_mul(1 << 30);
    let charged = blocking(move || {
        let now = checking.ids.now_ms();
        checking
            .db
            .charged_bytes(owner, now)
            .map_err(|e| db_error(&e))
    })
    .await?;
    if charged >= quota {
        return Err(ApiError::new(
            ErrorCode::QuotaExceeded,
            "your stored bytes are at your quota: delete uploads or let jobs expire",
        )
        .with("limit", "user_quota_gib"));
    }
    let lane = submission.kind.lane();
    let reservation = shared
        .jobs
        .reserve(lane, owner)
        .map_err(|f| full(f, lane.name()))?;
    let id = shared.ids.id("j");
    tracing::Span::current().record("job", id.as_str());
    // Review S2 of PR 5: the commit and what follows it (the job queued and
    // dispatched) run on their own task, so a client gone or the handler's
    // time spent cannot drop a committed job before it is queued.
    let committing = Arc::clone(shared);
    let task = tokio::spawn(async move {
        commit(
            &committing,
            owner,
            id,
            key,
            request_id,
            submission,
            reservation,
        )
        .await
    });
    task.await.unwrap_or_else(|_| Err(ApiError::internal()))
}

/// JS11: the job's row, inputs, citations and key in one transaction;
/// then, for a new job, `job.submitted`, its first event, its queueing and
/// a dispatch.
async fn commit(
    shared: &Arc<Shared>,
    owner: i64,
    id: String,
    key: Option<String>,
    request_id: String,
    submission: Submission,
    reservation: jobs::Reservation,
) -> Result<(JobRow, bool), ApiError> {
    let writing = Arc::clone(shared);
    let inputs: Vec<String> = submission.inputs.iter().map(|(s, _)| s.clone()).collect();
    let largest = submission.inputs.iter().map(|(_, b)| *b).max().unwrap_or(0);
    let submitted = blocking(move || {
        let new = NewJob {
            id: &id,
            owner,
            kind: submission.kind,
            request: &submission.request,
            request_sha256: &submission.request_sha256,
            idempotency: key.as_deref().map(|k| (k, submission.fingerprint.as_str())),
            request_id: &request_id,
            created_ms: writing.ids.now_ms(),
            inputs: &inputs,
            cites: &submission.cites,
        };
        writing.db.submit_job(&new).map_err(|e| db_error(&e))
    })
    .await?;
    match submitted {
        Submitted::New(row, event) => {
            jobs::hook(shared, "job.submitted").await;
            shared.jobs.publish(&row.id, std::slice::from_ref(&event));
            let entry = shared.jobs.entry(&row, largest);
            shared.jobs.enqueue(reservation, entry);
            shared.jobs.kick(shared);
            Ok((row, true))
        }
        Submitted::Existing(row) => Ok((row, false)),
        Submitted::KeyReused => Err(key_reused()),
    }
}

fn key_reused() -> ApiError {
    ApiError::new(
        ErrorCode::IdempotencyKeyReused,
        "this Idempotency-Key named another request: use a new key",
    )
}

/// JB9: the job `key` names, if the request is the same as sent.
async fn existing(
    shared: &Arc<Shared>,
    owner: i64,
    key: &str,
    fingerprint: &str,
) -> Result<Option<JobRow>, ApiError> {
    let (reading, key, fingerprint) = (Arc::clone(shared), key.to_owned(), fingerprint.to_owned());
    blocking(move || {
        reading
            .db
            .job_by_idempotency(owner, &key)
            .map_err(|e| db_error(&e))
    })
    .await?
    .map(|(row, sha)| {
        if sha == fingerprint {
            Ok(row)
        } else {
            Err(key_reused())
        }
    })
    .transpose()
}

/// JB9: the answer for a request whose `Idempotency-Key` named a job of
/// the same request already, before a route answers without a job
/// (`POST /v1/assemblies` of a current assembly); `None` when there is
/// none.
pub(crate) async fn replay(
    shared: &Arc<Shared>,
    caller: &Principal,
    headers: &HeaderMap,
    fingerprint: &str,
) -> Result<Option<Response>, ApiError> {
    let Some(key) = request::idempotency_key(headers)? else {
        return Ok(None);
    };
    let Some(row) = existing(shared, caller.user.key, &key, fingerprint).await? else {
        return Ok(None);
    };
    let wait = wait_of(shared, request::prefer_wait(headers));
    let _slot = wait_slot(shared, caller, wait)?;
    created(shared, row, wait).await.map(Some)
}

/// Submits a job (JB1, JS11) and answers as JB11 says: the route has
/// validated `submission`.
pub(crate) async fn submit(
    shared: &Arc<Shared>,
    caller: &Principal,
    headers: &HeaderMap,
    request_id: &RequestId,
    submission: Submission,
) -> Result<Response, ApiError> {
    if shared.draining.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(ApiError::new(
            ErrorCode::ShuttingDown,
            "the server is draining: submit the job to another server or later",
        )
        .with("reason", "draining")
        .retry_after(5));
    }
    let key = request::idempotency_key(headers)?;
    let wait = wait_of(shared, request::prefer_wait(headers));
    let _slot = wait_slot(shared, caller, wait)?;
    let (row, _) = in_time(
        shared,
        insert(shared, caller, key, request_id.0.clone(), submission),
    )
    .await?;
    created(shared, row, wait).await
}

/// `GET /v1/jobs?kind=&state=&after=&limit=` (section 7.9, VN3): a page of
/// the caller's jobs by (creation time, id); an admin's `?owner=` another's.
pub(super) async fn list(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    uri: Uri,
) -> Result<Response, ApiError> {
    let params = query::params(uri.query(), &["kind", "state", "after", "limit", "owner"])?;
    let limit = query::limit(&params, shared.config.limits.page_max)?;
    let bad = |p: &str| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("{p} is not one of the list's"),
        )
        .with("parameter", p)
    };
    let kind = match params.get("kind") {
        Some(k) => Some(Kind::parse(k).ok_or_else(|| bad("kind"))?),
        None => None,
    };
    let state = match params.get("state") {
        Some(s) => Some(JobState::parse(s).ok_or_else(|| bad("state"))?),
        None => None,
    };
    let after = query::after(&params)?;
    let cursor = if after.is_empty() {
        None
    } else {
        let (ms, id) = after.split_once(' ').ok_or_else(|| bad("after"))?;
        Some((ms.parse::<i64>().map_err(|_| bad("after"))?, id.to_owned()))
    };
    let (owner, mut extra) = super::owner(&shared, &caller, &params).await?;
    for (name, value) in [("kind", params.get("kind")), ("state", params.get("state"))] {
        if let Some(value) = value {
            let _ = write!(extra, "&{name}={value}");
        }
    }
    let reading = Arc::clone(&shared);
    let mut rows = blocking(move || {
        reading
            .db
            .jobs_page(
                owner,
                kind,
                state,
                cursor.as_ref().map(|(ms, id)| (*ms, id.as_str())),
                limit.saturating_add(1),
            )
            .map_err(|e| db_error(&e))
    })
    .await?;
    let keys: Vec<String> = rows
        .iter()
        .map(|r| format!("{} {}", r.created_ms, r.id))
        .collect();
    let mut keyed: Vec<(String, JobRow)> = keys.into_iter().zip(rows.drain(..)).collect();
    let next = super::next_url("/v1/jobs", &mut keyed, limit, |(k, _)| k.as_str(), &extra);
    let mut items = Vec::with_capacity(keyed.len());
    for (_, row) in &keyed {
        let progress = jobs::progress(&shared, row);
        items.push(documents::job(row, None, progress));
    }
    Ok(super::page(&Page::new(JOB_FORMAT, items, next)))
}

/// `GET /v1/jobs/{id}?wait=N` (JB11, VN5): the job; `If-None-Match`
/// answers `304`; a wait long-polls until it ends, at most
/// `limits.wait_max_seconds`.
pub(super) async fn get(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let params = query::params(uri.query(), &["wait"])?;
    let wait = match params.get("wait") {
        Some(n) => Some(n.parse::<u64>().map_err(|_| {
            ApiError::new(ErrorCode::InvalidParameter, "wait is a number of seconds")
                .with("parameter", "wait")
        })?),
        None => None,
    };
    let row = in_time(&shared, find(&shared, &caller, job_id(&id)?)).await?;
    let wait = wait_of(&shared, wait);
    let _slot = wait_slot(&shared, &caller, wait)?;
    let row = match wait {
        Some(seconds) => jobs::wait(&shared, row, Duration::from_secs(seconds)).await,
        None => row,
    };
    if cache::not_modified(&headers, &etag(&row)) {
        return Ok(cache::not_modified_response(&etag(&row), "no-cache"));
    }
    Ok(answer(&shared, &row, StatusCode::OK).await)
}

/// `GET /v1/jobs/{id}/events?after=SEQ` (JB8, JS10): NDJSON; the persisted
/// events after SEQ, then live ones, until the `end` event.
pub(super) async fn events(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(id): Path<String>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let params = query::params(uri.query(), &["after"])?;
    let after = match params.get("after") {
        Some(n) => n.parse::<i64>().ok().filter(|n| *n >= 0).ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                "after is a persisted event's seq",
            )
            .with("parameter", "after")
        })?,
        None => 0,
    };
    let row = find(&shared, &caller, job_id(&id)?).await?;
    let Some(slot) = shared.jobs.streams.take(caller.user.key) else {
        return Err(too_many_streams());
    };
    let live = shared.jobs.channels.subscribe(row.key);
    let (reading, key) = (Arc::clone(&shared), row.key);
    let replay = blocking(move || {
        reading
            .db
            .events_after(key, after)
            .map_err(|e| db_error(&e))
    })
    .await?;
    let (body, out) = ChannelBody::new(16);
    let reading = Arc::clone(&shared);
    let mut forward = Forward {
        job: row.id.clone(),
        last: after,
        live,
        refill: Arc::new(move |after| reading.db.events_after(key, after).unwrap_or_default()),
        shutdown: shared.jobs.shutdown_signal(),
        heartbeat: shared.jobs.heartbeat(),
        out,
    };
    let streaming = Arc::clone(&shared);
    tokio::spawn(async move {
        let _slot = slot;
        if forward.rows(&replay).await != Some(false) || *forward.shutdown.borrow() {
            return;
        }
        // Review S9 of PR 5: a job that ended, its `end` at or before
        // `after`, has nothing live to wait for.
        if jobs::reread(&streaming, &row).await.state.terminal() {
            let _ = forward.catch_up().await;
            return;
        }
        forward.run().await;
    });
    let mut response = (StatusCode::OK, axum::body::Body::new(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    response.headers_mut().insert(
        "fitment-format",
        HeaderValue::from_static("fitment.job.event/1"),
    );
    Ok(response)
}

/// `GET /v1/jobs/{id}/document` (DC1, DC4): the document the job
/// produced, exact bytes, with `Fitment-Exit-Code`; `404 no_document` when
/// there is none.
pub(super) async fn document(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let row = find(&shared, &caller, job_id(&id)?).await?;
    let (Some(sha256), Some(format)) = (row.result_sha256.clone(), row.result_format.clone())
    else {
        return Err(ApiError::new(
            ErrorCode::NoDocument,
            "the job produced no document: see its state and error",
        ));
    };
    // DC4 (review of PR 5): the digest of the revision `result` names,
    // which an `unchanged` extraction did not make.
    let named = row
        .result
        .as_deref()
        .and_then(|href| href.strip_prefix("/v1/sockets/"))
        .and_then(|name| SocketRef::parse(name).ok())
        .filter(|_| matches!(row.kind, Kind::SocketExtract | Kind::SocketImport));
    let digest = match named {
        Some(reference) => {
            let (reading, owner) = (Arc::clone(&shared), row.owner);
            blocking(move || {
                reading
                    .db
                    .revision(owner, &reference.id, reference.n)
                    .map_err(|e| db_error(&e))
            })
            .await?
            .map(|r| r.document_digest)
            .filter(|d| !d.is_empty())
        }
        None => None,
    };
    let served = cache::Served {
        sha256,
        bytes: row.result_bytes.unwrap_or(0),
        content_type: json::JSON,
        format: Some((format, 1)),
        exit_code: row.exit,
        digest,
        cache: cache::Cache::Immutable,
        disposition: None,
    };
    cache::object(&shared, &headers, served).await
}

/// `POST /v1/jobs/{id}/cancel` (JB5): by its owner or an admin; `202` with
/// the job; `409 job_finished` once it has ended.
pub(super) async fn cancel(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let row = find(&shared, &caller, job_id(&id)?).await?;
    let (writing, key) = (Arc::clone(&shared), row.key);
    let cancelled = blocking(move || {
        writing
            .db
            .cancel_job(
                key,
                writing.ids.now_ms(),
                writing.jobs.retention_ms(),
                &documents::end_data,
            )
            .map_err(|e| db_error(&e))
    })
    .await?;
    let row = match cancelled {
        Cancelled::Ended(row, events) => {
            shared.jobs.unqueue(row.key);
            shared.jobs.publish(&row.id, &events);
            crate::metrics::job_ended(&shared.metrics, &row);
            row
        }
        Cancelled::Cancelling(row, event) => {
            if let Some(event) = event {
                shared.jobs.publish(&row.id, std::slice::from_ref(&event));
            }
            shared.jobs.flag(row.key, Stop::Cancel);
            row
        }
        Cancelled::Finished(_) => {
            return Err(ApiError::new(
                ErrorCode::JobFinished,
                "the job has ended: there is nothing to cancel",
            ));
        }
    };
    Ok(answer(&shared, &row, StatusCode::ACCEPTED).await)
}

/// `POST /v1/jobs/{id}/pin` (JB12, admin): keeps a record past retention,
/// or lets it expire again.
pub(super) async fn pin(
    State(shared): State<Arc<Shared>>,
    Admin(caller): Admin,
    Path(id): Path<String>,
    Json(body): Json<PinRequest>,
) -> Result<Response, ApiError> {
    let row = find(&shared, &caller, job_id(&id)?).await?;
    let (writing, key) = (Arc::clone(&shared), row.key);
    blocking(move || {
        writing
            .db
            .pin_job(key, body.pinned)
            .map_err(|e| db_error(&e))
    })
    .await?;
    let row = jobs::reread(&shared, &row).await;
    Ok(answer(&shared, &row, StatusCode::OK).await)
}

/// `DELETE /v1/jobs/{id}` (JB12, admin): a finished job's record; what it
/// made durable stays. One that has not ended is `409 job_not_finished`.
pub(super) async fn delete(
    State(shared): State<Arc<Shared>>,
    Admin(caller): Admin,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let row = find(&shared, &caller, job_id(&id)?).await?;
    let (writing, key) = (Arc::clone(&shared), row.key);
    let deleted = blocking(move || writing.db.delete_job(key).map_err(|e| db_error(&e))).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(
            ErrorCode::JobNotFinished,
            "the job has not ended: cancel it, or wait for it",
        ))
    }
}

/// The request id of a request (HX7).
pub(crate) type Id = Extension<RequestId>;
