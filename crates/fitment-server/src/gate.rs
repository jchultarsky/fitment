//! The socket gate's refusals and HX8's parse slots (docs/research/
//! m6-api.md, UP6, JB2, HX8, ER3, ER4, section 7.12). A socket document is
//! read and gated by `matching.rs`; here its refusal becomes an error at
//! the request (`422` with its code, before any job) or a job's end at
//! dispatch (`refused`, exit 3, before any candidate is read), each with a
//! sentence built from values alone and the values themselves. Parsing a
//! socket document at a request runs on the blocking pool behind a fair
//! semaphore of `limits.parse_concurrency` slots (HX8), waiting within the
//! handler's time.
//!
//! Hook point (OP11): `socket.parse`, inside a slot, on the blocking pool.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::Shared;
use crate::http::blocking;
use crate::http::error::{ApiError, ErrorCode};
use crate::jobs::documents;
use crate::jobs::model::{Kind, State};
use crate::matching::{Build, GateError};
use crate::state::End;

/// ER4: refusals as `features.v1` writes them, `[{code, message, instance}]`.
pub fn refusal_list(refusals: &[fitment_core::Refusal]) -> Value {
    refusals
        .iter()
        .map(|r| serde_json::json!({"code": r.code.name(), "message": r.message, "instance": r.instance}))
        .collect()
}

/// Its sentence, built from values alone (ER3).
pub fn detail(error: &GateError, build: Build) -> String {
    match error {
        GateError::Utf8 => "the socket document is not UTF-8".into(),
        GateError::Stale { extractor, rules } => format!(
            "the socket was extracted with extractor {extractor} and socket rules {rules}; this build is {} and {}: extract it again (POST /v1/sockets), which carries its decisions over",
            build.0, build.1
        ),
        GateError::Unreadable(_) => "the socket document cannot be read: see details.reason".into(),
        GateError::Refused(_) => "the socket document holds no socket: see details.refusals".into(),
        GateError::Unreviewed { count, .. } => format!(
            "{count} requirements are unreviewed, under require_reviewed: review them (POST /v1/sockets/{{socket}}/review)"
        ),
    }
}

/// Its values (section 7.12): the reason and position a reader refused
/// at, the refusals, both pairs of versions, the counts.
pub fn details(error: &GateError, build: Build) -> Map<String, Value> {
    let mut details = Map::new();
    let mut put = |key: &str, value: Value| {
        details.insert(key.to_owned(), value);
    };
    match error {
        GateError::Utf8 => put("reason", "utf8".into()),
        GateError::Stale { extractor, rules } => {
            put("extractor_version", (*extractor).into());
            put("socket_rules_version", (*rules).into());
            put("build_extractor_version", build.0.into());
            put("build_socket_rules_version", build.1.into());
        }
        GateError::Unreadable(error) => {
            let reason = match error {
                fitment_core::SocketFormatError::Json { line, column, .. } => {
                    put("line", (*line).into());
                    put("column", (*column).into());
                    "json"
                }
                fitment_core::SocketFormatError::Format { .. } => "format",
                fitment_core::SocketFormatError::Version { .. } => "version",
                fitment_core::SocketFormatError::Digest => "digest",
                _ => "invalid",
            };
            put("reason", reason.into());
        }
        GateError::Refused(refusals) => put("refusals", refusal_list(refusals)),
        GateError::Unreviewed { count, first } => {
            put("count", (*count).into());
            put("first", first.clone().into());
        }
    }
    details
}

/// UP6, JB2: the refusal at a request: `422` with its code; no job.
pub fn api_error(error: &GateError, build: Build) -> ApiError {
    let mut api = ApiError::new(error.code(), detail(error, build));
    for (key, value) in details(error, build) {
        api = api.with(&key, value);
    }
    api
}

/// JB2: the refusal at dispatch: the job `refused`, exit 3, with the
/// gate's code, before any candidate is read.
pub fn end(error: &GateError, build: Build, kind: Kind) -> End {
    documents::failed(
        State::Refused,
        Some(3),
        error.code(),
        &detail(error, build),
        details(error, build),
        kind,
    )
}

/// The build's versions a socket is held to (MF11); under `test-support`
/// a seam raises the socket rules', so a test meets an upgraded build
/// (VT32).
pub(crate) fn build(shared: &Shared) -> Build {
    #[cfg(feature = "test-support")]
    let shift = shared.rules_shift.load(std::sync::atomic::Ordering::SeqCst);
    #[cfg(not(feature = "test-support"))]
    let shift = {
        let _ = shared;
        0
    };
    (
        fitment_core::EXTRACTOR_VERSION,
        fitment_core::SOCKET_RULES_VERSION.saturating_add(shift),
    )
}

/// HX8: `work`, a socket document's parse, on the blocking pool behind one
/// of `limits.parse_concurrency` fair slots. The slot is owned and moves
/// into the blocking work, so a request that times out or is dropped
/// while its parse runs still holds the slot until the parse ends: no
/// slot is freed while its parse goes on. The wait for one is half the
/// handler's time, so a sync route (a review) answers `503
/// request_timeout`, `details.limit = "parse_concurrency"`, before its
/// layer's timeout does; not retryable on a write (ER6).
///
/// # Errors
///
/// `request_timeout`, or `work`'s.
pub(crate) async fn parsed<T: Send + 'static>(
    shared: &Arc<Shared>,
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let slots = Arc::clone(&shared.parse_slots);
    let slot = tokio::time::timeout(slot_wait(shared), slots.acquire_owned())
        .await
        .map_err(|_| {
            ApiError::new(
                ErrorCode::RequestTimeout,
                "the server is busy reading socket documents: try again later",
            )
            .with("limit", "parse_concurrency")
            .retryable(false)
        })?
        .map_err(|_| ApiError::internal())?;
    let hooked = Arc::clone(shared);
    blocking(move || {
        crate::jobs::run::hook(&hooked, "socket.parse");
        let result = work();
        drop(slot);
        result
    })
    .await
}

/// HX8: the wait for a parse slot, half the handler's time, so it ends
/// before the route's layer timeout (HX3).
pub(crate) fn slot_wait(shared: &Shared) -> Duration {
    Duration::from_millis(u64::from(shared.config.server.handler_timeout_seconds) * 500)
}
