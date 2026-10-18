//! The documents of jobs (docs/research/m6-api.md, section 7.11, JB3,
//! JB8, DC9, M6-11, M6-59): `fitment.job` and `fitment.job.event`, built
//! from a job's row. Each has `format` and `format_version` first and
//! writes `null` rather than omitting a field; `fitment.job` alone holds
//! times. No document embeds a result document (DC1): they link to it.

use serde::Serialize;
use serde_json::Value;

use super::model::{Kind, State};
use crate::http::error::{Class, ErrorCode, SPECS};
use crate::ids::rfc3339;
use crate::state::{End, EventRow, JobRow, Stopped};

/// `fitment.job`.
pub const JOB_FORMAT: &str = "fitment.job";

/// `fitment.job.event`.
pub const EVENT_FORMAT: &str = "fitment.job.event";

/// Each of the two's version (DC9: may grow within v1).
pub const FORMAT_VERSION: u32 = 1;

/// A link to a resource and its format.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Link {
    /// The resource's URL.
    pub href: String,
    /// Its format.
    pub format: String,
}

/// A link to a job's document: its exact bytes (DC1, DC5).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DocumentLink {
    /// `/v1/jobs/{id}/document`.
    pub href: String,
    /// Its format.
    pub format: String,
    /// The SHA-256 of its bytes.
    pub sha256: String,
    /// Their length.
    pub bytes: u64,
}

/// A job's error: section 7.12's object without the HTTP framing (ER7).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobError {
    /// The stable code (ER1).
    pub code: String,
    /// Its class (ER2).
    pub class: Class,
    /// The code's title.
    pub title: String,
    /// A sentence built from the code and values, never a path (ER3).
    pub detail: String,
    /// The CLI's exit code in the same situation, or null.
    pub exit_code: Option<u8>,
    /// Whether the same request may succeed later (ER6).
    pub retryable: bool,
    /// Values: ids, counts, versions; never a path (ER3).
    pub details: serde_json::Map<String, Value>,
}

/// A job's progress (JB8): counts only, never a verdict (M6-51).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Progress {
    /// `reading`, `level_one`, `loading`, `judging` or `batch`.
    pub phase: &'static str,
    /// Done so far.
    pub done: u64,
    /// In all, if known.
    pub total: Option<u64>,
}

/// `fitment.job`'s times (M6-11), RFC 3339 UTC.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Times {
    /// When it was submitted.
    pub created: String,
    /// When it last started.
    pub started: Option<String>,
    /// When it ended.
    pub finished: Option<String>,
    /// When its record expires, unless pinned (JB12).
    pub expires: Option<String>,
}

/// `fitment.job` v1 (section 7.11).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobDocument {
    /// `fitment.job`.
    pub format: &'static str,
    /// 1.
    pub format_version: u32,
    /// `j_…`.
    pub id: String,
    /// Its kind (JB1).
    pub kind: Kind,
    /// Its owner, `u_…`.
    pub owner: String,
    /// Its state (JB3).
    pub state: State,
    /// Its attempt, 1 or 2 (SD5).
    pub attempt: i64,
    /// The normalized request (RA9).
    pub request: Value,
    /// The SHA-256 of the canonical request (JB9).
    pub request_sha256: String,
    /// Its place in its lane's queue, from 1, while queued.
    pub queue_position: Option<u64>,
    /// Its progress while it runs (JB8).
    pub progress: Option<Progress>,
    /// Its owner or an admin asked it to stop (JB5).
    pub cancel_requested: bool,
    /// The CLI's exit code (JB4), once it ended.
    pub exit_code: Option<u8>,
    /// Set only for a job that succeeded (JB3, M6-59).
    pub outcome: Option<String>,
    /// What it made.
    pub result: Option<Link>,
    /// The document it produced, as bytes elsewhere (DC1).
    pub document: Option<DocumentLink>,
    /// Why it did not succeed.
    pub error: Option<JobError>,
    /// The CLI's stderr lines as notes (JB10).
    pub notes: Value,
    /// Kept past retention (JB12).
    pub pinned: bool,
    /// Its times (M6-11).
    pub times: Times,
}

/// ER6: whether a job's error of `code` may succeed when submitted again:
/// the registry's flag, but `job_interrupted` is not for a replace or a
/// prune, whose client checks first (JB6).
pub fn retryable(code: ErrorCode, kind: Kind) -> bool {
    match code {
        ErrorCode::JobInterrupted => !matches!(kind, Kind::CatalogReplace | Kind::CatalogPrune),
        _ => code.spec().retryable,
    }
}

/// The code of `name`, if it is one.
pub fn code_named(name: &str) -> Option<ErrorCode> {
    SPECS.iter().find(|s| s.name == name).map(|s| s.code)
}

/// `{detail, details}`, as `server.db` keeps a job's error.
pub fn error_details(detail: &str, details: serde_json::Map<String, Value>) -> String {
    serde_json::json!({ "detail": detail, "details": Value::Object(details) }).to_string()
}

/// A job's error from its row.
fn job_error(row: &JobRow) -> Option<JobError> {
    let name = row.error_code.as_deref()?;
    let code = code_named(name)?;
    let spec = code.spec();
    let stored: Value = row
        .error_details
        .as_deref()
        .and_then(|d| serde_json::from_str(d).ok())
        .unwrap_or(Value::Null);
    Some(JobError {
        code: name.to_owned(),
        class: spec.class,
        title: spec.title.to_owned(),
        detail: stored["detail"].as_str().unwrap_or(spec.title).to_owned(),
        exit_code: row.exit,
        retryable: retryable(code, row.kind),
        details: stored["details"].as_object().cloned().unwrap_or_default(),
    })
}

/// The job's result link, if it made something.
fn result(row: &JobRow) -> Option<Link> {
    Some(Link {
        href: row.result.clone()?,
        format: row.kind.result_format()?.to_owned(),
    })
}

/// The job's document link, if it produced one.
fn document(row: &JobRow) -> Option<DocumentLink> {
    Some(DocumentLink {
        href: format!("/v1/jobs/{}/document", row.id),
        format: row.result_format.clone()?,
        sha256: row.result_sha256.clone()?,
        bytes: row.result_bytes.unwrap_or(0),
    })
}

/// `fitment.job` of `row`. `outcome` is written only for a job that
/// succeeded (JB3, M6-59), whatever the row holds.
pub fn job(row: &JobRow, queue_position: Option<u64>, progress: Option<Progress>) -> JobDocument {
    JobDocument {
        format: JOB_FORMAT,
        format_version: FORMAT_VERSION,
        id: row.id.clone(),
        kind: row.kind,
        owner: row.owner_id.clone(),
        state: row.state,
        attempt: row.attempt,
        request: serde_json::from_str(&row.request).unwrap_or(Value::Null),
        request_sha256: row.request_sha256.clone(),
        queue_position,
        progress: progress.filter(|_| matches!(row.state, State::Running | State::Cancelling)),
        cancel_requested: row.cancel_requested,
        exit_code: row.exit,
        outcome: row
            .outcome
            .clone()
            .filter(|_| row.state == State::Succeeded),
        result: result(row),
        document: document(row),
        error: job_error(row),
        notes: serde_json::from_str(&row.notes).unwrap_or_else(|_| Value::Array(Vec::new())),
        pinned: row.pinned,
        times: Times {
            created: rfc3339(row.created_ms),
            started: row.started_ms.map(rfc3339),
            finished: row.finished_ms.map(rfc3339),
            expires: row.expires_ms.filter(|_| !row.pinned).map(rfc3339),
        },
    }
}

/// The `end` event's data (JB8): `{state, exit_code, outcome, result,
/// document, error}`, `outcome` null unless the job succeeded.
pub fn end_data(row: &JobRow) -> String {
    /// The `end` event's fields, in JB8's order.
    #[derive(Serialize)]
    struct EndData {
        state: State,
        exit_code: Option<u8>,
        outcome: Option<String>,
        result: Option<Link>,
        document: Option<DocumentLink>,
        error: Option<JobError>,
    }
    let document = job(row, None, None);
    serde_json::to_string(&EndData {
        state: document.state,
        exit_code: document.exit_code,
        outcome: document.outcome,
        result: document.result,
        document: document.document,
        error: document.error,
    })
    .unwrap_or_default()
}

/// A persisted event's line (JB8): `format`, `format_version`, `job`,
/// `seq`, `type`, then its data's fields; one line, no whitespace.
pub fn event_line(job: &str, event: &EventRow) -> String {
    line(job, Some(event.seq), &event.kind, &event.data)
}

/// A transient event's line (`progress`, `heartbeat`, `lagged`,
/// `shutdown`): no `seq`, since no replay can return it (review F6).
pub fn transient_line(job: &str, kind: &str, data: &str) -> String {
    line(job, None, kind, data)
}

fn line(job: &str, seq: Option<i64>, kind: &str, data: &str) -> String {
    let rest = data
        .trim()
        .strip_prefix('{')
        .and_then(|d| d.strip_suffix('}'))
        .unwrap_or("");
    let seq = seq.map_or_else(String::new, |s| format!(r#","seq":{s}"#));
    let tail = if rest.trim().is_empty() {
        String::new()
    } else {
        format!(",{rest}")
    };
    // The job's id and the type are of fixed alphabets (M6-55, JB8).
    format!(
        r#"{{"format":"{EVENT_FORMAT}","format_version":{FORMAT_VERSION},"job":"{job}"{seq},"type":"{kind}"{tail}}}"#
    ) + "\n"
}

/// The documented shape of `fitment.job.event` (JB8), for the OpenAPI
/// document: every line has these first; a type's own fields follow.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EventDocument {
    /// `fitment.job.event`.
    pub format: &'static str,
    /// 1.
    pub format_version: u32,
    /// The job's id.
    pub job: String,
    /// Persisted events only: 1, 2, 3 … (review F6).
    pub seq: Option<i64>,
    /// `state`, `batch`, `change`, `note`, `end`; transient `progress`,
    /// `heartbeat`, `lagged`, `shutdown`.
    #[serde(rename = "type")]
    pub kind: String,
}

/// An end with no document or result: `state`, the error's code, its
/// sentence and values.
pub fn failed(
    state: State,
    exit: Option<u8>,
    code: ErrorCode,
    detail: &str,
    details: serde_json::Map<String, Value>,
    kind: Kind,
) -> End {
    let release = matches!(state, State::Interrupted | State::Cancelled)
        || (state == State::Failed && retryable(code, kind));
    End {
        state,
        exit,
        outcome: None,
        error: Some((code.name().to_owned(), error_details(detail, details))),
        release_key: release,
        document: None,
        result: None,
    }
}

/// The ends a stop that committed first holds a job to (JB5, JB7): a
/// cancel leaves nothing; a deadline fails it, `deadline_exceeded`.
pub fn stopped(kind: Kind) -> Stopped {
    Stopped {
        cancelled: End {
            state: State::Cancelled,
            exit: None,
            outcome: None,
            error: None,
            release_key: true,
            document: None,
            result: None,
        },
        deadline: failed(
            State::Failed,
            None,
            ErrorCode::DeadlineExceeded,
            "the job passed its deadline and was stopped; it left no document",
            serde_json::Map::new(),
            kind,
        ),
    }
}

/// JB6: a job the process stopped under that will not run again:
/// `interrupted`, `job_interrupted`, retryable but for a replace or a
/// prune (ER6).
pub fn interrupted(row: &JobRow) -> End {
    let mut details = serde_json::Map::new();
    details.insert("attempt".into(), row.attempt.into());
    failed(
        State::Interrupted,
        None,
        ErrorCode::JobInterrupted,
        "the server stopped while the job ran; submit it again",
        details,
        row.kind,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JB8, review F6: a persisted line has `seq`, a transient one none;
    /// both are one line of compact JSON with the fields first.
    #[test]
    fn event_lines_are_one_compact_object() {
        let event = EventRow {
            job: 1,
            seq: 3,
            at_ms: 0,
            kind: "state".into(),
            data: r#"{"state":"running","attempt":1}"#.into(),
        };
        let line = event_line("j_x", &event);
        assert_eq!(
            line,
            "{\"format\":\"fitment.job.event\",\"format_version\":1,\"job\":\"j_x\",\"seq\":3,\
             \"type\":\"state\",\"state\":\"running\",\"attempt\":1}\n"
        );
        let value: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(value["seq"], 3);
        let beat = transient_line("j_x", "heartbeat", "{}");
        let value: Value = serde_json::from_str(beat.trim()).unwrap();
        assert!(value.get("seq").is_none());
        assert_eq!(value["type"], "heartbeat");
    }

    /// ER6: `job_interrupted` is retryable but for a replace or a prune.
    #[test]
    fn interrupted_is_retryable_but_for_replace_and_prune() {
        assert!(retryable(ErrorCode::JobInterrupted, Kind::CatalogIngest));
        assert!(!retryable(ErrorCode::JobInterrupted, Kind::CatalogReplace));
        assert!(!retryable(ErrorCode::JobInterrupted, Kind::CatalogPrune));
        assert!(!retryable(ErrorCode::DeadlineExceeded, Kind::Features));
        assert!(retryable(ErrorCode::CatalogBusy, Kind::CatalogIngest));
    }
}
