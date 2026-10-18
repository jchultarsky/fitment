//! Errors (docs/research/m6-api.md, section 4.7 and 7.12): one closed enum
//! of stable codes (ER1), each with its class, HTTP status, CLI exit code
//! and `retryable` flag, sent as `application/problem+json` whose members
//! are fitment's (`fitment.error` v1, DC9). No message comes from a path or
//! a library error's `Display` (ER3).

use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Map, Value};

/// The error document's format name.
pub const ERROR_FORMAT: &str = "fitment.error";

/// Its version (DC9: may grow within v1, codes added, never changed).
pub const ERROR_FORMAT_VERSION: u32 = 1;

/// The problem `type` URI's prefix; its last segment is the code. The host
/// is a placeholder for the project's documentation site (question 6).
pub const TYPE_PREFIX: &str = "https://fitment.dev/errors/";

/// An error's class (ER2): three of them the CLI's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// The request is wrong (the CLI's exit 2).
    Usage,
    /// The input or the catalog's state is refused (exit 3).
    Refused,
    /// The server could not do it, or fitment has a bug (exit 1).
    Error,
    /// Credentials or permissions (no CLI counterpart).
    Auth,
}

impl Class {
    /// The class's name as the document writes it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::Refused => "refused",
            Self::Error => "error",
            Self::Auth => "auth",
        }
    }
}

/// Every error code of section 7.12, in its order (ER1). A code never
/// changes meaning; codes are added within v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)] // each code is documented in SPECS and section 7.12
pub enum ErrorCode {
    BadRequest,
    InvalidParameter,
    InvalidReference,
    NotAnId,
    NameInvalid,
    LabelReserved,
    NoCandidates,
    OptionsConflict,
    NotFound,
    UploadNotFound,
    EntryNotFound,
    NoDocument,
    MethodNotAllowed,
    NotAcceptable,
    UploadTooSlow,
    ReferenceAmbiguous,
    RevisionNotCurrent,
    EntryDeleted,
    AssemblyInUse,
    UploadInUse,
    OriginalNotStored,
    JobFinished,
    JobNotFinished,
    TooManyTokens,
    UsernameTaken,
    LastAdmin,
    BytesNotKept,
    DerivedRowsGone,
    PreconditionFailed,
    BodyTooLarge,
    UnsupportedMediaType,
    UploadDigestMismatch,
    IdempotencyKeyReused,
    CarryFromForeign,
    RevisionRequired,
    RateLimited,
    TooManyJobs,
    TooManyStreams,
    NothingSelected,
    PartNotFound,
    PartAmbiguous,
    PartIsAssembly,
    OccurrenceNotFound,
    OccurrenceAmbiguous,
    OccurrenceMismatch,
    ReviewUnknownRequirement,
    ReviewUnknownSocket,
    ReviewDiffersBetweenSockets,
    ReviewNoteRequired,
    ReviewNoteTooLong,
    IdConflict,
    DuplicateContent,
    ReviewRefusedDocument,
    SocketUnreadable,
    SocketRefused,
    SocketStale,
    SocketUnreviewed,
    SocketNotReproduced,
    ComponentRefused,
    EntryStale,
    DerivedStale,
    TooLarge,
    SocketBlocked,
    CatalogStale,
    CatalogEmpty,
    FileRefused,
    CatalogUnavailable,
    CatalogBusy,
    RequestTimeout,
    QueueFull,
    ShuttingDown,
    StorageFull,
    QuotaExceeded,
    BlobDamaged,
    CatalogDamaged,
    IoError,
    Internal,
    RunIncomplete,
    OriginalFailsOwnSocket,
    ShortlistUnsound,
    DeadlineExceeded,
    JobInterrupted,
    Unauthenticated,
    InvalidCredentials,
    Forbidden,
}

/// One row of section 7.12.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    /// The code.
    pub code: ErrorCode,
    /// Its `snake_case` name.
    pub name: &'static str,
    /// Its class (ER2).
    pub class: Class,
    /// Its HTTP status; `None` for a code that occurs only in a job's
    /// `error` (section 7.12's "—").
    pub status: Option<u16>,
    /// The CLI's exit code in the same situation; `None` where the CLI has
    /// none (ER2). `catalog_unavailable`'s depends on its reason.
    pub exit_code: Option<u8>,
    /// Whether the same request may succeed later unchanged (ER6), where
    /// that does not depend on the route or the job's kind.
    pub retryable: bool,
    /// The problem's `title`.
    pub title: &'static str,
}

const fn spec(
    code: ErrorCode,
    name: &'static str,
    class: Class,
    status: Option<u16>,
    exit_code: Option<u8>,
    retryable: bool,
    title: &'static str,
) -> Spec {
    Spec {
        code,
        name,
        class,
        status,
        exit_code,
        retryable,
        title,
    }
}

use Class::{Auth, Error as Err_, Refused, Usage};
use ErrorCode as C;

/// Section 7.12, row by row, in `ErrorCode`'s order (ER1).
#[rustfmt::skip]
pub const SPECS: [Spec; 85] = [
    spec(C::BadRequest, "bad_request", Usage, Some(400), Some(2), false, "The request is malformed"),
    spec(C::InvalidParameter, "invalid_parameter", Usage, Some(400), Some(2), false, "A parameter is out of range"),
    spec(C::InvalidReference, "invalid_reference", Usage, Some(400), Some(2), false, "A reference is malformed"),
    spec(C::NotAnId, "not_an_id", Usage, Some(400), Some(2), false, "Only an id is accepted here"),
    spec(C::NameInvalid, "name_invalid", Usage, Some(400), None, false, "A name or label is not allowed"),
    spec(C::LabelReserved, "label_reserved", Usage, Some(400), None, false, "A label uses a reserved prefix"),
    spec(C::NoCandidates, "no_candidates", Usage, Some(400), Some(2), false, "The match names no candidate"),
    spec(C::OptionsConflict, "options_conflict", Usage, Some(400), Some(2), false, "Two options exclude each other"),
    spec(C::NotFound, "not_found", Usage, Some(404), None, false, "Nothing is here"),
    spec(C::UploadNotFound, "upload_not_found", Usage, Some(404), Some(1), false, "The upload is not held"),
    spec(C::EntryNotFound, "entry_not_found", Usage, Some(404), Some(2), false, "No catalog entry has this name"),
    spec(C::NoDocument, "no_document", Usage, Some(404), None, false, "The job produced no document"),
    spec(C::MethodNotAllowed, "method_not_allowed", Usage, Some(405), None, false, "The method is not allowed here"),
    spec(C::NotAcceptable, "not_acceptable", Usage, Some(406), None, false, "The route cannot answer in an accepted type"),
    spec(C::UploadTooSlow, "upload_too_slow", Usage, Some(408), None, true, "The upload was too slow"),
    spec(C::ReferenceAmbiguous, "reference_ambiguous", Usage, Some(409), Some(2), false, "The reference names several entries"),
    spec(C::RevisionNotCurrent, "revision_not_current", Usage, Some(409), Some(2), false, "The revision is not the current one"),
    spec(C::EntryDeleted, "entry_deleted", Usage, Some(409), Some(2), false, "The entry is deleted"),
    spec(C::AssemblyInUse, "assembly_in_use", Usage, Some(409), None, false, "A socket line names the assembly"),
    spec(C::UploadInUse, "upload_in_use", Usage, Some(409), None, false, "A running job names the upload"),
    spec(C::OriginalNotStored, "original_not_stored", Usage, Some(409), None, false, "The socket's assembly is not stored"),
    spec(C::JobFinished, "job_finished", Usage, Some(409), None, false, "The job has ended or cannot stop"),
    spec(C::JobNotFinished, "job_not_finished", Usage, Some(409), None, false, "The job has not ended"),
    spec(C::TooManyTokens, "too_many_tokens", Usage, Some(409), None, false, "The user holds the most live tokens allowed"),
    spec(C::UsernameTaken, "username_taken", Usage, Some(409), None, false, "The username is another user's"),
    spec(C::LastAdmin, "last_admin", Usage, Some(409), None, false, "The last enabled admin cannot be disabled or demoted"),
    spec(C::BytesNotKept, "bytes_not_kept", Usage, Some(410), Some(2), false, "The bytes are no longer kept"),
    spec(C::DerivedRowsGone, "derived_rows_gone", Usage, Some(410), Some(2), false, "The revision's derived rows are gone"),
    spec(C::PreconditionFailed, "precondition_failed", Usage, Some(412), None, false, "A precondition failed"),
    spec(C::BodyTooLarge, "body_too_large", Usage, Some(413), None, false, "The request body is too large"),
    spec(C::UnsupportedMediaType, "unsupported_media_type", Usage, Some(415), None, false, "The request body's type is not accepted"),
    spec(C::UploadDigestMismatch, "upload_digest_mismatch", Usage, Some(422), None, false, "The bytes do not hash to the name given"),
    spec(C::IdempotencyKeyReused, "idempotency_key_reused", Usage, Some(422), None, false, "The idempotency key names another request"),
    spec(C::CarryFromForeign, "carry_from_foreign", Usage, Some(422), Some(2), false, "The document to carry from is for another target"),
    spec(C::RevisionRequired, "revision_required", Usage, Some(428), None, false, "A revision must be named"),
    spec(C::RateLimited, "rate_limited", Usage, Some(429), None, true, "Too many requests"),
    spec(C::TooManyJobs, "too_many_jobs", Usage, Some(429), None, true, "Too many jobs of this owner"),
    spec(C::TooManyStreams, "too_many_streams", Usage, Some(429), None, true, "Too many streams"),
    spec(C::NothingSelected, "nothing_selected", Usage, None, Some(2), false, "The selection names nothing"),
    spec(C::PartNotFound, "part_not_found", Usage, None, Some(2), false, "No part has this name"),
    spec(C::PartAmbiguous, "part_ambiguous", Usage, None, Some(2), false, "Several parts have this name"),
    spec(C::PartIsAssembly, "part_is_assembly", Usage, None, Some(2), false, "The part named is an assembly"),
    spec(C::OccurrenceNotFound, "occurrence_not_found", Usage, None, Some(2), false, "No occurrence has this path"),
    spec(C::OccurrenceAmbiguous, "occurrence_ambiguous", Usage, None, Some(2), false, "Several occurrences match"),
    spec(C::OccurrenceMismatch, "occurrence_mismatch", Usage, None, Some(2), false, "The occurrence is not of the part"),
    spec(C::ReviewUnknownRequirement, "review_unknown_requirement", Usage, Some(422), Some(2), false, "The review names no requirement of the socket"),
    spec(C::ReviewUnknownSocket, "review_unknown_socket", Usage, Some(422), Some(2), false, "The review names no socket of the document"),
    spec(C::ReviewDiffersBetweenSockets, "review_differs_between_sockets", Usage, Some(422), Some(2), false, "The requirement's sockets are reviewed differently"),
    spec(C::ReviewNoteRequired, "review_note_required", Usage, Some(422), Some(2), false, "A removal needs a note"),
    spec(C::ReviewNoteTooLong, "review_note_too_long", Usage, Some(422), Some(2), false, "The note is too long"),
    spec(C::IdConflict, "id_conflict", Usage, None, Some(2), false, "The id names other bytes"),
    spec(C::DuplicateContent, "duplicate_content", Usage, None, Some(2), false, "The bytes are already an entry"),
    spec(C::ReviewRefusedDocument, "review_refused_document", Refused, Some(422), Some(3), false, "A refused socket document cannot be reviewed"),
    spec(C::SocketUnreadable, "socket_unreadable", Refused, Some(422), Some(3), false, "The socket document cannot be read"),
    spec(C::SocketRefused, "socket_refused", Refused, Some(422), Some(3), false, "The socket document is refused"),
    spec(C::SocketStale, "socket_stale", Refused, Some(422), Some(3), false, "The socket was read by older rules"),
    spec(C::SocketUnreviewed, "socket_unreviewed", Refused, Some(422), Some(3), false, "The socket has unreviewed requirements"),
    spec(C::SocketNotReproduced, "socket_not_reproduced", Refused, None, Some(3), false, "The assembly does not reproduce the socket"),
    spec(C::ComponentRefused, "component_refused", Refused, None, Some(3), false, "The component's assembly is refused"),
    spec(C::EntryStale, "entry_stale", Refused, Some(409), Some(3), false, "The entry was read by older rules"),
    spec(C::DerivedStale, "derived_stale", Refused, Some(409), None, false, "The derived document is of another build"),
    spec(C::TooLarge, "too_large", Refused, Some(413), Some(3), false, "The input is too large"),
    spec(C::SocketBlocked, "socket_blocked", Refused, None, Some(3), false, "The socket fails every candidate"),
    spec(C::CatalogStale, "catalog_stale", Refused, None, Some(3), false, "The catalog needs a reindex"),
    spec(C::CatalogEmpty, "catalog_empty", Refused, None, Some(3), false, "The catalog has nothing to search"),
    spec(C::FileRefused, "file_refused", Refused, None, Some(3), false, "The file is refused"),
    spec(C::CatalogUnavailable, "catalog_unavailable", Err_, Some(503), Some(1), false, "The catalog cannot be opened"),
    spec(C::CatalogBusy, "catalog_busy", Err_, Some(503), Some(1), true, "The catalog is busy"),
    spec(C::RequestTimeout, "request_timeout", Err_, Some(503), None, false, "The request took too long"),
    spec(C::QueueFull, "queue_full", Err_, Some(503), None, true, "The job queue is full"),
    spec(C::ShuttingDown, "shutting_down", Err_, Some(503), None, true, "The server is shutting down"),
    spec(C::StorageFull, "storage_full", Err_, Some(507), Some(1), false, "The storage is full"),
    spec(C::QuotaExceeded, "quota_exceeded", Err_, Some(507), None, false, "The quota is exceeded"),
    spec(C::BlobDamaged, "blob_damaged", Err_, Some(500), Some(1), false, "Stored bytes are damaged"),
    spec(C::CatalogDamaged, "catalog_damaged", Err_, Some(500), Some(1), false, "The catalog holds a damaged row"),
    spec(C::IoError, "io_error", Err_, Some(500), Some(1), false, "A storage error"),
    spec(C::Internal, "internal", Err_, Some(500), Some(1), false, "An internal error"),
    spec(C::RunIncomplete, "run_incomplete", Err_, None, Some(1), false, "The run is incomplete"),
    spec(C::OriginalFailsOwnSocket, "original_fails_own_socket", Err_, None, Some(1), false, "The original fails its own socket: a fitment bug"),
    spec(C::ShortlistUnsound, "shortlist_unsound", Err_, None, Some(1), false, "The shortlist dropped a passing candidate: a fitment bug"),
    spec(C::DeadlineExceeded, "deadline_exceeded", Err_, None, None, false, "The job passed its deadline"),
    spec(C::JobInterrupted, "job_interrupted", Err_, None, None, false, "The job was interrupted"),
    spec(C::Unauthenticated, "unauthenticated", Auth, Some(401), None, false, "No valid credentials"),
    spec(C::InvalidCredentials, "invalid_credentials", Auth, Some(401), None, false, "The credentials are wrong"),
    spec(C::Forbidden, "forbidden", Auth, Some(403), None, false, "The role is too low"),
];

impl ErrorCode {
    /// Every code, in section 7.12's order (ER1).
    pub const ALL: [Self; SPECS.len()] = {
        let mut all = [Self::BadRequest; SPECS.len()];
        let mut i = 0;
        while i < SPECS.len() {
            all[i] = SPECS[i].code;
            i += 1;
        }
        all
    };

    /// The code's row of section 7.12.
    pub fn spec(self) -> &'static Spec {
        &SPECS[self as usize]
    }

    /// Its `snake_case` name.
    pub fn name(self) -> &'static str {
        self.spec().name
    }

    /// Its class.
    pub fn class(self) -> Class {
        self.spec().class
    }

    /// Its HTTP status, or `None` for a job-only code.
    pub fn status(self) -> Option<StatusCode> {
        self.spec()
            .status
            .and_then(|s| StatusCode::from_u16(s).ok())
    }

    /// Whether a request that failed with it may succeed unchanged later
    /// (ER6): `request_timeout` on `GET`, `HEAD` and an upload's `PUT`
    /// only, since elsewhere the work may have been done.
    pub fn retryable_on(self, method: &Method, upload: bool) -> bool {
        match self {
            Self::RequestTimeout => {
                *method == Method::GET
                    || *method == Method::HEAD
                    || (upload && *method == Method::PUT)
            }
            _ => self.spec().retryable,
        }
    }
}

/// Why the catalog cannot be opened (CX7, ER2): each with the CLI's exit
/// code for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogReason {
    /// A newer schema (DB5): exit 3.
    NewerSchema,
    /// Not a catalog (the CLI's `NotACatalog`): exit 2.
    NotACatalog,
    /// A planted or missing directory (`Linked`): exit 2.
    Linked,
    /// A path that is not Unicode (`NotUnicode`): exit 2.
    NotUnicode,
    /// It cannot be read: exit 1.
    Unreadable,
}

impl CatalogReason {
    /// The reason's name, `details.reason`.
    pub fn name(self) -> &'static str {
        match self {
            Self::NewerSchema => "newer_schema",
            Self::NotACatalog => "not_a_catalog",
            Self::Linked => "linked",
            Self::NotUnicode => "not_unicode",
            Self::Unreadable => "unreadable",
        }
    }

    /// The CLI's exit code for it (ER2, review F25).
    pub fn exit_code(self) -> u8 {
        match self {
            Self::NewerSchema => 3,
            Self::NotACatalog | Self::Linked | Self::NotUnicode => 2,
            Self::Unreadable => 1,
        }
    }

    /// A sentence for `detail`, naming no path (ER3).
    pub fn detail(self) -> &'static str {
        match self {
            Self::NewerSchema => {
                "the catalog was made by a newer fitment; this server cannot read it"
            }
            Self::NotACatalog => "the configured catalog directory is not a catalog",
            Self::Linked => "a directory of the catalog's store is a symbolic link or missing",
            Self::NotUnicode => "the catalog's path is not Unicode",
            Self::Unreadable => "the catalog cannot be read",
        }
    }
}

/// An error answered by a route: a code, a sentence and values (ER3).
///
/// It becomes a response whose body the request-id layer writes, so every
/// error carries its `request_id` (HX7); see [`Problem`].
#[derive(Debug, Clone)]
pub struct ApiError {
    code: ErrorCode,
    detail: String,
    details: Map<String, Value>,
    exit_code: Option<u8>,
    retryable: Option<bool>,
    retry_after: Option<u32>,
    allow: Option<String>,
}

impl ApiError {
    /// An error of `code` with a sentence built from the code and values
    /// the client sent or the API defines, never a path (ER3). A job-only
    /// code answered as a response is a bug, answered as `internal`.
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
            details: Map::new(),
            exit_code: code.spec().exit_code,
            retryable: None,
            retry_after: None,
            allow: None,
        }
    }

    /// Adds `details.<key>`.
    #[must_use]
    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.details.insert(key.to_owned(), value.into());
        self
    }

    /// `Retry-After` in seconds (ER6).
    #[must_use]
    pub fn retry_after(mut self, seconds: u32) -> Self {
        self.retry_after = Some(seconds);
        self
    }

    /// Sets `retryable` where it depends on the route (ER6).
    #[must_use]
    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = Some(retryable);
        self
    }

    /// The `Allow` header of a `405`.
    #[must_use]
    pub fn allow(mut self, methods: impl Into<String>) -> Self {
        self.allow = Some(methods.into());
        self
    }

    /// `catalog_unavailable` for `reason`, with the CLI's exit code for it
    /// (ER2, CX7).
    pub fn catalog_unavailable(reason: CatalogReason) -> Self {
        let mut error =
            Self::new(ErrorCode::CatalogUnavailable, reason.detail()).with("reason", reason.name());
        error.exit_code = Some(reason.exit_code());
        error
    }

    /// `internal`: nothing beyond the code and the request id (ER3).
    pub fn internal() -> Self {
        Self::new(
            ErrorCode::Internal,
            "an internal error; the server's log holds it under this request_id",
        )
    }

    /// `details.refusals` as `features.v1` writes them (ER4):
    /// `[{code, message, instance}]`.
    #[must_use]
    pub fn with_refusals(self, refusals: &[fitment_core::Refusal]) -> Self {
        let list: Vec<Value> = refusals
            .iter()
            .map(|r| {
                let mut entry = Map::new();
                entry.insert("code".into(), r.code.name().into());
                entry.insert("message".into(), r.message.clone().into());
                entry.insert(
                    "instance".into(),
                    r.instance.map_or(Value::Null, Value::from),
                );
                Value::Object(entry)
            })
            .collect();
        self.with("refusals", list)
    }

    /// The code.
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// The document, with `request_id` (DC9's key order: `format` and
    /// `format_version` first).
    pub fn document(&self, method: &Method, request_id: &str) -> Problem {
        let spec = self.code.spec();
        let status = self.code.status().map_or(500, |s| s.as_u16());
        Problem {
            format: ERROR_FORMAT,
            format_version: ERROR_FORMAT_VERSION,
            type_: format!("{TYPE_PREFIX}{}", spec.name),
            title: spec.title,
            status,
            detail: self.detail.clone(),
            code: self.code,
            class: spec.class,
            exit_code: self.exit_code,
            retryable: self
                .retryable
                .unwrap_or_else(|| self.code.retryable_on(method, false)),
            details: self.details.clone(),
            request_id: request_id.to_owned(),
        }
    }
}

impl IntoResponse for ApiError {
    /// The status and headers; the body is written by the request-id layer
    /// from the [`ApiError`] it finds in the response's extensions, so it
    /// carries the request's id (HX7). A job-only code answered here is a
    /// bug, answered as `internal`.
    fn into_response(self) -> Response {
        let error = if self.code.status().is_some() {
            self
        } else {
            tracing::error!(
                code = self.code.name(),
                "a job-only error code was answered"
            );
            Self::internal()
        };
        let status = error
            .code
            .status()
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut response = status.into_response();
        let headers = response.headers_mut();
        if let Some(seconds) = error.retry_after {
            headers.insert(header::RETRY_AFTER, HeaderValue::from(seconds));
        }
        if let Some(allow) = error
            .allow
            .as_deref()
            .and_then(|a| HeaderValue::from_str(a).ok())
        {
            headers.insert(header::ALLOW, allow);
        }
        if error.code == ErrorCode::Unauthenticated {
            headers.insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response.extensions_mut().insert(error);
        response
    }
}

/// The `fitment.error` document inside `application/problem+json` (RFC
/// 9457 with fitment's members; section 7.11).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Problem {
    /// `fitment.error`.
    pub format: &'static str,
    /// 1.
    pub format_version: u32,
    /// `https://fitment.dev/errors/<code>`.
    #[serde(rename = "type")]
    pub type_: String,
    /// The code's title.
    pub title: &'static str,
    /// The HTTP status.
    pub status: u16,
    /// A sentence built from the code and values, never a path (ER3).
    pub detail: String,
    /// The stable code (ER1): an open string, since codes are added
    /// within v1; the known ones are the OpenAPI document's
    /// `info.x-fitment-error-codes`, and a client that meets another acts
    /// on `class`.
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub code: ErrorCode,
    /// Its class (ER2).
    pub class: Class,
    /// The CLI's exit code in the same situation, or null (ER2).
    pub exit_code: Option<u8>,
    /// Whether the same request may succeed later (ER6).
    pub retryable: bool,
    /// Values: ids, counts, versions, reasons; never a path (ER3).
    pub details: Map<String, Value>,
    /// The request's id (HX7).
    pub request_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_spec_sits_at_its_codes_index() {
        for (i, spec) in SPECS.iter().enumerate() {
            assert_eq!(spec.code as usize, i, "{}", spec.name);
            assert_eq!(ErrorCode::ALL[i], spec.code);
            let serialized = serde_json::to_string(&spec.code).unwrap();
            assert_eq!(serialized, format!("\"{}\"", spec.name));
        }
    }
}
