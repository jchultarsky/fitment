//! Section 7.8's routes (docs/research/m6-api.md, JB1, JB2, RA8, RA9,
//! DC3, DC7, DC10, DC12, SN2, SN3, SN9, AU8, M6-59, M6-64): run a match
//! over uploads and components (the job `match`, `fitment match S FILE…
//! --format json`), list the caller's runs, read a run's `fitment.run`,
//! its document's exact bytes and a page of its candidates, sliced from
//! those bytes. Everything that needs no STEP parse is checked before the
//! job exists (JB1), the socket gate among them (JB2), in the CLI's order
//! (ER5); the catalog's runs come with PR 7. A match's id is its job's.

use std::fmt::Write as _;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use serde_json::Value;

use super::jobs::{self, Id, Submission};
use crate::Shared;
use crate::auth::documents::Page;
use crate::http::cache::{self, Cache, Served};
use crate::http::error::{ApiError, ErrorCode};
use crate::http::extract::{Principal, Reader};
use crate::http::json::{self, Sent};
use crate::http::{blocking, db_error, query};
use crate::jobs::model::Kind;
use crate::jobs::request::{
    self, CandidatesRequest, ComponentJob, FileJob, MatchJob, MatchRequest, OptionsJob, PolicyJob,
    SocketFrom, SocketRef,
};
use crate::matching::{self, ClearanceName};
use crate::runs::{self, Asked, RunDocument, RunOptions, RunSocket};
use crate::state::JobRow;

fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "no match of that id is yours to see")
}

/// RA8: a label, display text, never one a catalog entry's would read as
/// (`catalog:`).
fn label(text: &str) -> Result<(), ApiError> {
    request::text(text, "label")?;
    if text.starts_with("catalog:") {
        return Err(ApiError::new(
            ErrorCode::LabelReserved,
            "a label starting catalog: names a catalog entry's revision",
        ));
    }
    Ok(())
}

/// DC7, VN6: the policy's fields, D9's and D6's defaults filled; a
/// tolerance zero, negative, `-0.0` or (angular) at 90 degrees or more is
/// `400 invalid_parameter`.
fn policy_of(body: &MatchRequest) -> Result<PolicyJob, ApiError> {
    let asked = body.policy.as_ref();
    let mm = asked.and_then(|p| p.tolerance_mm);
    let deg = asked.and_then(|p| p.tolerance_deg);
    for (name, value) in [("tolerance_mm", mm), ("tolerance_deg", deg)] {
        if value.is_some_and(|v| v == 0.0 && v.is_sign_negative()) {
            return Err(
                ApiError::new(ErrorCode::InvalidParameter, format!("{name} is -0.0"))
                    .with("parameter", name),
            );
        }
    }
    let clearance = asked
        .and_then(|p| p.clearance)
        .unwrap_or(ClearanceName::Strict);
    let built = matching::policy(mm, deg, clearance).map_err(|why| {
        ApiError::new(ErrorCode::InvalidParameter, why).with("parameter", "policy")
    })?;
    Ok(PolicyJob {
        tolerance_mm: built.tolerance().linear_mm(),
        tolerance_deg: built.tolerance().angular_deg(),
        clearance,
    })
}

/// SN3, M6-12: `require_reviewed` as asked, or forced by the operator's
/// floor, never relaxed; and who asked.
fn options_of(shared: &Shared, body: &MatchRequest) -> OptionsJob {
    let asked = body.options.as_ref();
    let require_fit = asked.and_then(|o| o.require_fit).unwrap_or(false);
    let requested = asked.and_then(|o| o.require_reviewed).unwrap_or(false);
    let floor = shared.config.matching.require_reviewed_floor;
    OptionsJob {
        require_fit,
        require_reviewed: requested || floor,
        require_reviewed_by: if requested {
            Some("request".into())
        } else if floor {
            Some("server".into())
        } else {
            None
        },
    }
}

/// CM1, HX2, RA8: the candidates' shape: at least one, at most
/// `limits.max_candidates`, each reference well formed and each name
/// valid.
fn candidates_of(
    shared: &Shared,
    candidates: &CandidatesRequest,
) -> Result<(Vec<FileJob>, Vec<ComponentJob>), ApiError> {
    let files = candidates.files.clone().unwrap_or_default();
    let components = candidates.components.clone().unwrap_or_default();
    if files.is_empty() && components.is_empty() {
        return Err(ApiError::new(
            ErrorCode::NoCandidates,
            "a match names files or components to judge",
        ));
    }
    let count = u64::try_from(files.len() + components.len()).unwrap_or(u64::MAX);
    if count > shared.config.limits.max_candidates {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "a match names at most {} candidates",
                shared.config.limits.max_candidates
            ),
        )
        .with("parameter", "candidates")
        .with("limit", "max_candidates"));
    }
    let mut out_files = Vec::with_capacity(files.len());
    for file in files {
        let sha256 = request::sha256_ref(&file.upload, "candidates.files.upload")?;
        label(&file.label)?;
        out_files.push(FileJob {
            upload: format!("sha256:{sha256}"),
            label: file.label,
        });
    }
    let mut out_components = Vec::with_capacity(components.len());
    for component in components {
        let sha256 = request::sha256_ref(&component.assembly, "candidates.components.assembly")?;
        if component.part.is_none() && component.occurrence.is_none() {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "a component names its part, or one occurrence",
            )
            .with("parameter", "candidates.components.part"));
        }
        for (field, value) in [
            ("part", &component.part),
            ("occurrence", &component.occurrence),
        ] {
            if let Some(value) = value {
                request::text(value, field)?;
            }
        }
        if let Some(text) = &component.label {
            label(text)?;
        }
        out_components.push(ComponentJob {
            assembly: format!("sha256:{sha256}"),
            part: component.part,
            occurrence: component.occurrence,
            label: component.label,
        });
    }
    Ok((out_files, out_components))
}

/// RA9: the socket a match names: an exact revision of the caller's (a
/// bare id its current one), cited by the job; or an upload of theirs.
/// Its normalized form, its document's object and length, and the
/// revision cited.
async fn socket_of(
    shared: &Arc<Shared>,
    owner: i64,
    socket: &SocketFrom,
) -> Result<(Value, String, u64, Option<i64>), ApiError> {
    match socket {
        SocketFrom::Revision(text) => {
            let reference = SocketRef::parse(text)?;
            let (reading, id, n) = (Arc::clone(shared), reference.id.clone(), reference.n);
            let revision =
                blocking(move || reading.db.revision(owner, &id, n).map_err(|e| db_error(&e)))
                    .await?
                    .ok_or_else(|| {
                        ApiError::new(ErrorCode::NotFound, "no socket of that id is yours")
                            .with("resource", "socket")
                    })?;
            Ok((
                Value::String(format!("{}@r{}", revision.line_id, revision.n)),
                revision.document_sha256,
                revision.document_bytes,
                Some(revision.key),
            ))
        }
        SocketFrom::Upload { upload } => {
            let sha256 = request::sha256_ref(upload, "socket.upload")?;
            let bytes = super::nameable(shared, owner, &sha256).await?;
            Ok((
                serde_json::json!({ "upload": format!("sha256:{sha256}") }),
                sha256,
                bytes,
                None,
            ))
        }
    }
}

/// UP3, UP5, RA3: every candidate's bytes the caller may name, and every
/// component's assembly theirs: the inputs the job holds (OB4).
async fn inputs_of(
    shared: &Arc<Shared>,
    owner: i64,
    files: &[FileJob],
    components: &[ComponentJob],
) -> Result<Vec<(String, u64)>, ApiError> {
    let mut inputs: Vec<(String, u64)> = Vec::new();
    for file in files {
        let sha256 = file.upload.trim_start_matches("sha256:").to_owned();
        if !inputs.iter().any(|(s, _)| *s == sha256) {
            let bytes = super::nameable(shared, owner, &sha256).await?;
            inputs.push((sha256, bytes));
        }
    }
    for component in components {
        let sha256 = component.assembly.trim_start_matches("sha256:").to_owned();
        if !inputs.iter().any(|(s, _)| *s == sha256) {
            let assembly = super::assemblies::of(shared, owner, sha256.clone()).await?;
            inputs.push((sha256, assembly.bytes));
        }
    }
    Ok(inputs)
}

/// `POST /v1/matches` (section 7.8): the job `match`. Usage first (the
/// policy, the candidates' shape, the names), then the socket and the
/// gate (JB2: MF11, the strict reader, MF2 with the floor), then the
/// original's assembly (`409 original_not_stored`) and the candidates'
/// bytes; nothing is parsed but the socket document.
pub(super) async fn create(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    axum::Extension(request_id): Id,
    headers: HeaderMap,
    Sent(body, sent): Sent<MatchRequest>,
) -> Result<Response, ApiError> {
    // JB9: a key's replay before any reference.
    let fingerprint = request::fingerprint("POST", "/v1/matches", &sent);
    if let Some(replayed) = jobs::replay(&shared, &caller, &headers, &fingerprint).await? {
        return Ok(replayed);
    }
    let policy = policy_of(&body)?;
    let (files, components) = candidates_of(&shared, &body.candidates)?;
    let options = options_of(&shared, &body);
    let owner = caller.user.key;
    let (socket, document, document_bytes, revision) =
        socket_of(&shared, owner, &body.socket).await?;
    let (_, source) =
        super::sockets::gated(&shared, &document, document_bytes, options.require_reviewed).await?;
    let mut inputs = vec![(document.clone(), document_bytes)];
    let original = if body.original.unwrap_or(false) {
        let held = super::assemblies::of(&shared, owner, source.clone())
            .await
            .map_err(|_| {
                ApiError::new(
                    ErrorCode::OriginalNotStored,
                    "the original is read from the socket's own assembly: register it (POST /v1/assemblies)",
                )
            })?;
        inputs.push((source.clone(), held.bytes));
        Some(format!("sha256:{source}"))
    } else {
        None
    };
    for input in inputs_of(&shared, owner, &files, &components).await? {
        if !inputs.iter().any(|(s, _)| *s == input.0) {
            inputs.push(input);
        }
    }
    let (request, request_sha256) = request::normalized(&MatchJob {
        socket,
        socket_document: format!("sha256:{document}"),
        files,
        components,
        policy,
        options,
        original,
    });
    let submission = Submission {
        kind: Kind::Match,
        request,
        request_sha256,
        inputs,
        fingerprint,
        cites: revision.into_iter().collect(),
    };
    jobs::submit(&shared, &caller, &headers, &request_id, submission).await
}

/// AU8: the match `id`, a match job the caller owns, or any for an admin.
async fn find(shared: &Arc<Shared>, caller: &Principal, id: &str) -> Result<JobRow, ApiError> {
    let id = jobs::job_id(id).map_err(|_| not_found())?;
    let row = jobs::find(shared, caller, id)
        .await
        .map_err(|_| not_found())?;
    if row.kind == Kind::Match {
        Ok(row)
    } else {
        Err(not_found())
    }
}

/// What `fitment.run` takes from `row`'s request and the revision it
/// cites (DC10, RA10).
async fn asked(shared: &Arc<Shared>, row: &JobRow) -> Result<Asked, ApiError> {
    let job: MatchJob = serde_json::from_str(&row.request).map_err(|_| ApiError::internal())?;
    let (reading, key) = (Arc::clone(shared), row.key);
    let cited = blocking(move || {
        let Some((owner, id, n)) = reading.db.cited_revision(key).map_err(|e| db_error(&e))? else {
            return Ok(None);
        };
        reading
            .db
            .revision(owner, &id, Some(n))
            .map_err(|e| db_error(&e))
    })
    .await?;
    let upload = job.socket["upload"].as_str().map(str::to_owned);
    let socket = RunSocket {
        reference: job.socket.as_str().map(str::to_owned),
        upload,
        sha256: Some(job.socket_document.trim_start_matches("sha256:").to_owned()),
        digest: cited.as_ref().map(|r| r.document_digest.clone()),
        assembly: None,
        how: cited.as_ref().map(|r| r.how.clone()),
        verified: cited.as_ref().is_some_and(|r| r.verified),
        unreviewed: cited
            .as_ref()
            .map(|r| usize::try_from(r.unreviewed).unwrap_or(usize::MAX)),
    };
    let policy = matching::policy(
        Some(job.policy.tolerance_mm),
        Some(job.policy.tolerance_deg),
        job.policy.clearance,
    )
    .unwrap_or_default();
    Ok(Asked {
        socket,
        files: job.files.len(),
        components: job.components.len(),
        policy: matching::policy_facts(&policy),
        options: RunOptions {
            require_fit: job.options.require_fit,
            require_reviewed: job.options.require_reviewed,
            require_reviewed_by: job.options.require_reviewed_by,
        },
        original: job.original,
    })
}

/// `fitment.run` of `row`.
async fn run_of(shared: &Arc<Shared>, row: &JobRow) -> Result<RunDocument, ApiError> {
    Ok(runs::run(row, asked(shared, row).await?))
}

/// `GET /v1/matches?socket=&after=&limit=` (VN3): a page of the caller's
/// `fitment.run`, by (creation time, id), of one line (or one revision)
/// if given; each states `complete`, `exit_code` and `outcome`, null unless
/// the run succeeded (M6-59).
pub(super) async fn list(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    uri: Uri,
) -> Result<Response, ApiError> {
    let params = query::params(uri.query(), &["socket", "after", "limit", "owner"])?;
    let limit = query::limit(&params, shared.config.limits.page_max)?;
    let bad = |p: &str| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("{p} is not one of the list's"),
        )
        .with("parameter", p)
    };
    let socket = match params.get("socket") {
        Some(text) => Some(SocketRef::parse(text)?),
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
    if let Some(socket) = &socket {
        let _ = write!(extra, "&socket={}", socket.text());
    }
    let reading = Arc::clone(&shared);
    let filter = socket.clone();
    let rows = blocking(move || {
        reading
            .db
            .matches_page(
                owner,
                filter.as_ref().map(|s| (s.id.as_str(), s.n)),
                cursor.as_ref().map(|(ms, id)| (*ms, id.as_str())),
                limit.saturating_add(1),
            )
            .map_err(|e| db_error(&e))
    })
    .await?;
    let mut keyed: Vec<(String, JobRow)> = rows
        .into_iter()
        .map(|r| (format!("{} {}", r.created_ms, r.id), r))
        .collect();
    let next = super::next_url(
        "/v1/matches",
        &mut keyed,
        limit,
        |(k, _)| k.as_str(),
        &extra,
    );
    let mut items = Vec::with_capacity(keyed.len());
    for (_, row) in &keyed {
        items.push(run_of(&shared, row).await?);
    }
    Ok(super::page(&Page::new(runs::RUN_FORMAT, items, next)))
}

/// `GET /v1/matches/{id}` (DC10): `fitment.run`.
pub(super) async fn get(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let row = find(&shared, &caller, &id).await?;
    let run = run_of(&shared, &row).await?;
    let mut response = json::document(runs::RUN_FORMAT, runs::RUN_FORMAT_VERSION, &run);
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    Ok(response)
}

/// The run's document digest, from its recorded facts.
fn digest_of(row: &JobRow) -> Option<String> {
    row.run
        .as_deref()
        .and_then(|r| serde_json::from_str::<runs::RunFacts>(r).ok())
        .map(|f| f.digest)
}

/// `GET /v1/matches/{id}/document` (DC1, DC4): the `fitment.match` bytes,
/// `Fitment-Exit-Code` 0, 4 or 5, or 1 for a run kept with `complete:
/// false`; `404 no_document` when there is none.
pub(super) async fn document(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let row = find(&shared, &caller, &id).await?;
    let Some(sha256) = row.result_sha256.clone() else {
        return Err(ApiError::new(
            ErrorCode::NoDocument,
            "the match produced no document: see its job's state and error",
        ));
    };
    let served = Served {
        sha256,
        bytes: row.result_bytes.unwrap_or(0),
        content_type: json::JSON,
        format: Some((
            fitment_core::MATCH_FORMAT.to_owned(),
            fitment_core::MATCH_FORMAT_VERSION,
        )),
        exit_code: row.exit,
        digest: digest_of(&row),
        cache: Cache::Immutable,
        disposition: None,
    };
    cache::object(&shared, &headers, served).await
}

/// `GET /v1/matches/{id}/candidates?offset=&limit=` (DC12, SN5, M6-8): a
/// page of `{index, entry}`, each entry the document's own bytes of that
/// candidate, sliced through the index built at the job's end and each
/// checked against the SHA-256 it records (a range that fails is `500
/// blob_damaged`); every page states the document's digest, that it is a
/// view, and the run's `complete` and `exit_code`. Nothing is parsed.
pub(super) async fn candidates(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    Path(id): Path<String>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let params = query::params(uri.query(), &["offset", "limit"])?;
    let limit = query::limit(&params, shared.config.limits.page_max)?;
    let offset = match params.get("offset") {
        Some(n) => n.parse::<usize>().map_err(|_| {
            ApiError::new(ErrorCode::InvalidParameter, "offset is a candidate's index")
                .with("parameter", "offset")
        })?,
        None => 0,
    };
    let row = find(&shared, &caller, &id).await?;
    let (Some(document), Some(index)) = (row.result_sha256.clone(), row.index_sha256.clone())
    else {
        return Err(ApiError::new(
            ErrorCode::NoDocument,
            "the match produced no document: see its job's state and error",
        ));
    };
    let (reading, bytes) = (Arc::clone(&shared), row.result_bytes.unwrap_or(0));
    let (first, entries, total) = blocking(move || {
        let index_bytes = reading
            .db
            .object_bytes(&index)
            .map_err(|e| db_error(&e))?
            .ok_or_else(ApiError::internal)?;
        let read = reading
            .objects
            .read(&index, index_bytes)
            .map_err(|e| cache::object_error(&index, &e))?;
        let index: runs::CandidatesIndex =
            serde_json::from_slice(&read).map_err(|_| ApiError::internal())?;
        let total = index.entries.len();
        let wanted: Vec<&runs::IndexEntry> = index
            .entries
            .iter()
            .skip(offset)
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .collect();
        let ranges: Vec<(u64, u64)> = wanted.iter().map(|e| (e.start, e.end)).collect();
        let slices = reading
            .objects
            .read_ranges(&document, bytes, &ranges)
            .map_err(|e| cache::object_error(&document, &e))?;
        for (slice, entry) in slices.iter().zip(&wanted) {
            if fitment_core::sha256_hex(slice) != entry.sha256 {
                return Err(cache::object_error(
                    &document,
                    &crate::state::ObjectError::Damaged,
                ));
            }
        }
        Ok((offset, slices, total))
    })
    .await?;
    let end = first + entries.len();
    let next = (end < total).then(|| {
        format!(
            "/v1/matches/{}/candidates?offset={end}&limit={limit}",
            row.id
        )
    });
    let body = runs::page(
        first,
        &entries,
        next.as_deref(),
        &digest_of(&row).unwrap_or_default(),
        row.state == crate::jobs::model::State::Succeeded,
        row.exit,
    );
    let mut response = json::bytes(
        StatusCode::OK,
        json::JSON,
        Some((
            crate::auth::documents::PAGE_FORMAT,
            crate::auth::documents::FORMAT_VERSION,
        )),
        body.into(),
    );
    if let Some(value) = next
        .as_deref()
        .and_then(|n| axum::http::HeaderValue::from_str(&format!("<{n}>; rel=\"next\"")).ok())
    {
        response
            .headers_mut()
            .insert(axum::http::header::LINK, value);
    }
    Ok(response)
}
