//! Section 7.6's routes (docs/research/m6-api.md, RA4 to RA7, RA9, RA10,
//! OB5, UP6, HX8, VN3, VN5, DC4): extract a socket (the job
//! `socket_extract`, `fitment socket FILE --part X`), import a socket file
//! the CLI wrote (the job `socket_import`), review a revision (`fitment
//! review`, a new revision), list the caller's lines, read a revision's
//! exact bytes and a line's history, and delete a line (a tombstone).
//! Socket documents are parsed at a request behind HX8's slots. An
//! admin's `?owner=` reads another user's on `GET` only (AU8).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use serde_json::Value;

use fitment_core::{ReviewError, ReviewStatus, SocketDocument};

use super::jobs::{self, Id, Input, Submission};
use crate::Shared;
use crate::assemblies::{LINE_KIND, LineItem, REVISION_KIND, RevisionItem};
use crate::auth::documents::Page;
use crate::gate;
use crate::http::cache::{self, Cache, Served};
use crate::http::error::{ApiError, ErrorCode};
use crate::http::extract::Reader;
use crate::http::json::{self, Json, Sent};
use crate::http::{blocking, db_error, query};
use crate::jobs::model::Kind;
use crate::jobs::request::{
    self, CarryFrom, Decision, DecisionStatus, ImportJob, ImportRequest, ReviewRequest, SocketJob,
    SocketRef, SocketRequest,
};
use crate::matching::{self, Build, GateError};
use crate::state::{NewRevision, Reviewed};

pub(super) fn not_found() -> ApiError {
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
    let mut inputs = vec![Input::named(
        sha256.clone(),
        assembly.bytes,
        super::assemblies::not_found,
    )];
    let mut cites = Vec::new();
    let carry_from = match &body.carry_from {
        Some(carry) => {
            let (normalized, object, bytes, revision) = carry_from(&shared, owner, carry).await?;
            // HX8, MF11: read at the request, leniently, as the CLI reads
            // an older OUT: no socket document, or another file's, is
            // `422 carry_from_foreign`; another part's is the job's to see.
            carried_of(&shared, &object, bytes, &sha256).await?;
            let gone = if revision.is_some() {
                not_found
            } else {
                super::upload_not_found
            };
            inputs.push(Input::named(object, bytes, gone));
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

/// `422 carry_from_foreign`: the CLI refuses that OUT without `--force`.
fn foreign(why: &str) -> ApiError {
    ApiError::new(ErrorCode::CarryFromForeign, why.to_owned())
}

/// HX8, MF11: a `carry_from` document read leniently behind the parse
/// slots, as `fitment socket -o OUT` reads OUT: a socket document of any
/// version, of the request's assembly.
async fn carried_of(
    shared: &Arc<Shared>,
    object: &str,
    bytes: u64,
    assembly: &str,
) -> Result<(), ApiError> {
    let (reading, object, assembly) = (Arc::clone(shared), object.to_owned(), assembly.to_owned());
    gate::parsed(shared, move || {
        let read = reading
            .objects
            .read(&object, bytes)
            .map_err(|e| cache::object_error(&object, &e))?;
        let prior = std::str::from_utf8(&read)
            .ok()
            .and_then(|text| fitment_core::SocketDocument::prior(text).ok())
            .ok_or_else(|| foreign("carry_from is not a fitment.socket document"))?;
        if prior.sha256() == assembly {
            Ok(())
        } else {
            Err(foreign("carry_from is the socket of another file"))
        }
    })
    .await
}

/// `428 revision_required` (RA7).
fn revision_required() -> ApiError {
    ApiError::new(
        ErrorCode::RevisionRequired,
        "a review names the revision it changes: sk_…@rN",
    )
}

/// RA7: the review's line and its current revision, which `@rN` must be.
async fn reviewed(
    shared: &Arc<Shared>,
    owner: i64,
    reference: &SocketRef,
) -> Result<crate::state::RevisionRow, ApiError> {
    let n = reference.n.ok_or_else(revision_required)?;
    let (reading, id) = (Arc::clone(shared), reference.id.clone());
    let line = blocking(move || reading.db.line(owner, &id).map_err(|e| db_error(&e)))
        .await?
        .ok_or_else(not_found)?;
    let current = line.current.ok_or_else(not_found)?;
    if current.n != n {
        let (reading, id) = (Arc::clone(shared), reference.id.clone());
        let named = blocking(move || {
            reading
                .db
                .revision(owner, &id, Some(n))
                .map_err(|e| db_error(&e))
        })
        .await?;
        if named.is_none() {
            return Err(not_found());
        }
        return Err(ApiError::new(
            ErrorCode::RevisionNotCurrent,
            "the revision is not the line's current one: review the current revision",
        )
        .with("current", format!("{}@r{}", line.id, current.n)));
    }
    Ok(current)
}

/// A review's decisions applied to `text`, the stored document, in order,
/// all or none (OB5): `fitment review` once per decision. A refused
/// document is `review_refused_document`; each decision's refusal its
/// code, naming the decision by its index.
fn decide(text: &[u8], decisions: &[Decision], build: Build) -> Result<SocketDocument, ApiError> {
    let mut document =
        matching::read_socket(text, build).map_err(|e| gate::api_error(&e, build))?;
    for (index, decision) in decisions.iter().enumerate() {
        let status = match decision.status {
            DecisionStatus::Kept => ReviewStatus::Kept,
            DecisionStatus::Removed => ReviewStatus::Removed,
            DecisionStatus::Unreviewed => ReviewStatus::Unreviewed,
        };
        let decided = document.decide(
            &decision.requirement,
            decision.socket.as_deref(),
            status,
            decision.note.as_deref(),
        );
        if let Err(error) = decided {
            let (code, detail) = match error {
                ReviewError::UnknownId(_) => (
                    ErrorCode::ReviewUnknownRequirement,
                    "no socket of the document holds the decision's requirement",
                ),
                ReviewError::UnknownSocket(_) => (
                    ErrorCode::ReviewUnknownSocket,
                    "the document holds no socket of the decision's name",
                ),
                ReviewError::DiffersBetweenSockets { .. } => (
                    ErrorCode::ReviewDiffersBetweenSockets,
                    "the requirement differs between the sockets holding it: name one",
                ),
                ReviewError::NoteRequired(_) => (
                    ErrorCode::ReviewNoteRequired,
                    "removing a requirement needs a note",
                ),
                ReviewError::NoteTooLong => (
                    ErrorCode::ReviewNoteTooLong,
                    "the decision's note is too long",
                ),
                _ => (
                    ErrorCode::ReviewRefusedDocument,
                    "a refused socket document has nothing to review",
                ),
            };
            return Err(ApiError::new(code, detail)
                .with("decision", index)
                .with("requirement", decision.requirement.clone()));
        }
    }
    Ok(document)
}

/// `POST /v1/sockets/{line}@r{n}/review` (section 7.6, OB5, RA5, RA7,
/// SN3, HX8): the decisions applied to revision n, which must be the
/// line's current one, in order, all or none, as `fitment review` once
/// per decision; `201` with revision n+1's bytes and `Location` naming
/// it. Decisions that change no byte make no revision: `200` with the
/// current one's. A bare id is `428 revision_required`; a stale revision
/// `422 socket_stale`, a tightening over the CLI (SN3).
pub(super) async fn review(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(name): Path<String>,
    Json(body): Json<ReviewRequest>,
) -> Result<Response, ApiError> {
    let reference = SocketRef::parse(&name)?;
    if reference.n.is_none() {
        return Err(revision_required());
    }
    if body.decisions.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "a review makes at least one decision",
        )
        .with("parameter", "decisions"));
    }
    let owner = caller.user.key;
    let current = reviewed(&shared, owner, &reference).await?;
    let build = gate::build(&shared);
    let (extractor, rules) = (current.extractor_version, current.socket_rules_version);
    if (extractor, rules) != build {
        let stale = GateError::Stale { extractor, rules };
        return Err(gate::api_error(&stale, build));
    }
    // UP4: a revision's document counts against its owner's quota.
    super::jobs::under_quota(&shared, owner).await?;
    let (reading, sha, bytes) = (
        Arc::clone(&shared),
        current.document_sha256.clone(),
        current.document_bytes,
    );
    let decisions = body.decisions;
    let (document, staged) = gate::parsed(&shared, move || {
        let text = reading
            .objects
            .read(&sha, bytes)
            .map_err(|e| cache::object_error(&sha, &e))?;
        let document = decide(&text, &decisions, build)?;
        let staged = reading.objects.stage().and_then(|mut staged| {
            staged.write(document.to_json().as_bytes())?;
            staged.finish()
        });
        let staged = staged.map_err(|e| storage_error(&e))?;
        Ok((document, staged))
    })
    .await?;
    if staged.sha256 == current.document_sha256 {
        let at = Revision {
            id: &reference.id,
            n: current.n,
            sha256: &current.document_sha256,
            bytes: current.document_bytes,
            digest: &current.document_digest,
        };
        return answer_revision(&shared, &at, StatusCode::OK).await;
    }
    commit_review(&shared, owner, &reference.id, &current, &document, staged).await
}

/// `507 storage_full` or `500 io_error` for a document that could not be
/// stored.
fn storage_error(error: &std::io::Error) -> ApiError {
    if error.kind() == std::io::ErrorKind::StorageFull {
        ApiError::new(ErrorCode::StorageFull, "the server's storage is full")
    } else {
        tracing::error!(kind = ?error.kind(), "a socket document could not be stored");
        ApiError::new(ErrorCode::IoError, "the document could not be stored")
    }
}

/// OB5: the review's revision committed over `current` only, its object
/// placed first; a review that committed meanwhile is `409
/// revision_not_current`.
async fn commit_review(
    shared: &Arc<Shared>,
    owner: i64,
    id: &str,
    current: &crate::state::RevisionRow,
    document: &SocketDocument,
    staged: crate::state::Synced,
) -> Result<Response, ApiError> {
    let unreviewed = document
        .review()
        .iter()
        .filter(|e| e.status == ReviewStatus::Unreviewed)
        .count();
    let (writing, id_owned, base, verified) = (
        Arc::clone(shared),
        id.to_owned(),
        current.key,
        current.verified,
    );
    let digest = document.digest().unwrap_or("").to_owned();
    let versions = (
        document.extractor_version(),
        document.socket_rules_version(),
    );
    let stale = document.stale_reviews().len();
    let (carried, sha256, bytes) = (current.carried, staged.sha256.clone(), staged.bytes);
    let shown = digest.clone();
    let reviewed = blocking(move || {
        let made = NewRevision {
            how: "review",
            verified,
            parent: Some(base),
            document_sha256: &staged.sha256,
            document_digest: &digest,
            extractor_version: versions.0,
            socket_rules_version: versions.1,
            unreviewed: u64::try_from(unreviewed).unwrap_or(u64::MAX),
            carried,
            stale: u64::try_from(stale).unwrap_or(u64::MAX),
        };
        let mut place = || writing.objects.place(&staged);
        let now = writing.ids.now_ms();
        let done = writing
            .db
            .review_revision(owner, &id_owned, base, &made, &mut place, staged.bytes, now)
            .map_err(|e| match e {
                crate::state::StoreError::Place(e) => storage_error(&e),
                crate::state::StoreError::Db(e) => db_error(&e),
            })?;
        if matches!(done, Reviewed::Made(_)) {
            staged.kept();
        }
        Ok(done)
    })
    .await?;
    match reviewed {
        Reviewed::Made(n) => {
            let at = Revision {
                id,
                n,
                sha256: &sha256,
                bytes,
                digest: &shown,
            };
            answer_revision(shared, &at, StatusCode::CREATED).await
        }
        Reviewed::NotCurrent(n) => Err(ApiError::new(
            ErrorCode::RevisionNotCurrent,
            "another review made a revision first: review the current revision",
        )
        .with("current", format!("{id}@r{n}"))),
        Reviewed::Gone => Err(not_found()),
    }
}

/// A revision to answer with: its line, number, object and digest.
struct Revision<'a> {
    id: &'a str,
    n: i64,
    sha256: &'a str,
    bytes: u64,
    digest: &'a str,
}

/// A revision's bytes answered with `status` and `Location` naming it
/// (section 7.6), with DC4's headers.
async fn answer_revision(
    shared: &Arc<Shared>,
    at: &Revision<'_>,
    status: StatusCode,
) -> Result<Response, ApiError> {
    let served = Served {
        sha256: at.sha256.to_owned(),
        bytes: at.bytes,
        content_type: json::JSON,
        format: Some((
            fitment_core::SOCKET_FORMAT.to_owned(),
            fitment_core::SOCKET_FORMAT_VERSION,
        )),
        exit_code: Some(0),
        digest: Some(at.digest.to_owned()).filter(|d| !d.is_empty()),
        cache: Cache::Immutable,
        disposition: None,
    };
    let mut response = cache::object(shared, &HeaderMap::new(), served).await?;
    *response.status_mut() = status;
    let location = format!("/v1/sockets/{}@r{}", at.id, at.n);
    if let Ok(value) = axum::http::HeaderValue::from_str(&location) {
        response
            .headers_mut()
            .insert(axum::http::header::LOCATION, value);
    }
    Ok(response)
}

/// `POST /v1/sockets/import` (section 7.6, RA10, UP6, M6-62): the uploaded
/// document read as `fitment match` reads a file, before any job (`413`
/// past `limits.max_socket_mib`, `422 socket_stale`, `socket_unreadable`,
/// `socket_refused`); then the job `socket_import`, which extracts from
/// the caller's assembly of the document's source when it holds one.
pub(super) async fn import(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    axum::Extension(request_id): Id,
    headers: HeaderMap,
    Sent(body, sent): Sent<ImportRequest>,
) -> Result<Response, ApiError> {
    let fingerprint = request::fingerprint("POST", "/v1/sockets/import", &sent);
    if let Some(replayed) = jobs::replay(&shared, &caller, &headers, &fingerprint).await? {
        return Ok(replayed);
    }
    let sha256 = request::sha256_ref(&body.upload, "upload")?;
    let owner = caller.user.key;
    let bytes = super::nameable(&shared, owner, &sha256).await?;
    let (line, source) = gated(&shared, &sha256, bytes, false).await?;
    let mut inputs = vec![Input::upload(sha256.clone(), bytes)];
    let assembly = super::assemblies::found(&shared, owner, source.clone()).await?;
    if let Some(held) = &assembly {
        inputs.push(Input::named(
            source.clone(),
            held.bytes,
            super::assemblies::not_found,
        ));
    }
    let (request, request_sha256) = request::normalized(&ImportJob {
        upload: format!("sha256:{sha256}"),
        line,
        assembly: assembly.map(|_| format!("sha256:{source}")),
    });
    let submission = Submission {
        kind: Kind::SocketImport,
        request,
        request_sha256,
        inputs,
        fingerprint,
        cites: Vec::new(),
    };
    jobs::submit(&shared, &caller, &headers, &request_id, submission).await
}

/// UP6, HX8: an uploaded socket document read behind the parse slots,
/// within `limits.max_socket_mib`, as `fitment match` reads a file: MF11's
/// versions, the strict reader, MF2 (with `require_reviewed` for a
/// match); its line (RA4) and source.
pub(super) async fn gated(
    shared: &Arc<Shared>,
    sha256: &str,
    bytes: u64,
    require_reviewed: bool,
) -> Result<(String, String), ApiError> {
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
    let build = gate::build(shared);
    let (reading, sha) = (Arc::clone(shared), sha256.to_owned());
    gate::parsed(shared, move || {
        let read = reading
            .objects
            .read(&sha, bytes)
            .map_err(|e| cache::object_error(&sha, &e))?;
        let document =
            matching::read_socket(&read, build).map_err(|e| gate::api_error(&e, build))?;
        matching::admit(&document, require_reviewed).map_err(|e| gate::api_error(&e, build))?;
        let source = document.source().sha256.clone();
        let occurrence = document.selection().and_then(|s| s.occurrence.clone());
        let (line, _, _) = crate::jobs::socket::line_of(&source, &document, occurrence.as_deref())
            .ok_or_else(ApiError::internal)?;
        Ok((line, source))
    })
    .await
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
