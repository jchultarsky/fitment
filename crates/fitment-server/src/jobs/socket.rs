//! `socket_extract` (docs/research/m6-api.md, JS6, JS13, RA4, RA5, section
//! 7.6): `SocketDocument::extract` on the caller's assembly, as `fitment
//! socket FILE --part X` calls it; an accepted socket's decisions carried
//! as `fitment socket -o OUT` carries OUT's (MF11), from the line's current
//! revision or the request's `carry_from`; its bytes stored, then its
//! line's revision committed with the job's end, unless the bytes are the
//! current verified revision's (`unchanged`). A refused extraction writes
//! no revision ("a refused result never overwrites FILE"); a selection
//! error ends the job `refused`, exit 2, with no document (M6-52).

use fitment_core::{CarryOver, ReviewStatus, SelectError, SocketDocument, Status, sha256_hex};
use serde_json::{Map, Value};

use super::Control;
use super::documents;
use super::model::{Kind, State};
use super::request::{SocketJob, SocketRef, line_id};
use super::run::{self, Halt, Ran, hook};
use crate::Shared;
use crate::http::error::ErrorCode;
use crate::state::{DbError, End, Finished, JobRow, NewRevision, RevisionRow, Rows, SocketRows};

/// The most times an extraction carries again after another revision
/// committed under it (JS13).
const RETRIES: usize = 8;

/// The job (JS6).
pub(crate) fn extract(shared: &Shared, row: &JobRow, control: &Control) -> Result<Ran, DbError> {
    let read = || -> Result<(SocketJob, Result<SocketDocument, SelectError>), Halt> {
        let job: SocketJob = run::request(row)?;
        let bytes = run::read_input(shared, row.kind, job.assembly.trim_start_matches("sha256:"))?;
        hook(shared, "job.read");
        let document =
            SocketDocument::extract(&bytes, job.part.as_deref(), job.occurrence.as_deref());
        hook(shared, "job.ran");
        Ok((job, document))
    };
    let (job, document) = match read() {
        Ok(read) => read,
        Err(halt) => return halted(shared, row, control, &halt),
    };
    let document = match document {
        Ok(document) => document,
        Err(error) => return halted(shared, row, control, &selection_error(row.kind, &error)),
    };
    control.progress(shared, run::reading(1));
    if document.status() == Status::Refused {
        let exit = run::exit_of(document.status(), document.refusals());
        let staged = match run::stage(shared, row.kind, &document.to_json()) {
            Ok(staged) => staged,
            Err(halt) => return halted(shared, row, control, &halt),
        };
        hook(shared, "job.stored");
        let ending = run::answered(row.kind, exit, &staged, document.refusals());
        let finished = run::end(shared, row, control, &ending, Some(staged), &Rows::None)?;
        return run::conclude(shared, row, control, finished);
    }
    let assembly = job.assembly.trim_start_matches("sha256:");
    let Some((id, definition, occurrence)) =
        line_of(assembly, &document, job.occurrence.as_deref())
    else {
        let ending = run::internal(row.kind, "an accepted socket document names no target");
        return halted(shared, row, control, &ending);
    };
    let triple = (assembly, definition.as_str(), occurrence.as_str());
    for _ in 0..RETRIES {
        match revise(shared, row, control, &job, &document, &id, triple)? {
            Finished::Retry => {}
            finished => return run::conclude(shared, row, control, finished),
        }
    }
    let ending = run::internal(row.kind, "the line's revisions kept changing under the job");
    halted(shared, row, control, &ending)
}

fn halted(shared: &Shared, row: &JobRow, control: &Control, ending: &End) -> Result<Ran, DbError> {
    let finished = run::end(shared, row, control, ending, None, &Rows::None)?;
    run::conclude(shared, row, control, finished)
}

/// RA4: the line an accepted document's triple names: the target's
/// definition `#id`, and the occurrence chosen as its usage `#id`s joined
/// by `/`, or `*` for all.
pub(crate) fn line_of(
    assembly: &str,
    document: &SocketDocument,
    occurrence: Option<&str>,
) -> Option<(String, String, String)> {
    let extracted = document.extracted()?;
    let definition = format!("#{}", extracted.target.definition.instance);
    let occurrence = match occurrence {
        None => "*".to_owned(),
        Some(_) => extracted
            .occurrences
            .iter()
            .map(|o| {
                o.path
                    .iter()
                    .map(|n| format!("#{n}"))
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .collect::<Vec<_>>()
            .join(","),
    };
    let id = line_id(assembly, &definition, &occurrence);
    Some((id, definition, occurrence))
}

/// A selection error (M6-52): `refused`, exit 2, its own code, with the
/// candidates and occurrences the CLI lists.
fn selection_error(kind: Kind, error: &SelectError) -> End {
    let code = documents::code_named(error.code()).unwrap_or(ErrorCode::NothingSelected);
    let candidates: Vec<Value> = error
        .candidates()
        .iter()
        .map(|c| {
            let product = c.definition.product.as_ref();
            serde_json::json!({
                "definition": format!("#{}", c.definition.instance),
                "id": product.and_then(|p| p.id.clone()),
                "name": product.and_then(|p| p.name.clone()),
                "quantity": c.quantity,
                "first_path": c.first_path,
            })
        })
        .collect();
    let mut details = Map::new();
    details.insert("candidates".into(), Value::Array(candidates));
    if let SelectError::OccurrenceNotFound { occurrences, .. } = error {
        details.insert("occurrences".into(), serde_json::json!(occurrences));
    }
    documents::failed(
        State::Refused,
        Some(2),
        code,
        "the selection names no part to extract; details.candidates lists what it could mean",
        details,
        kind,
    )
}

/// `carry_from_foreign` (exit 2): the CLI refuses an OUT that is not this
/// file's and target's socket, or no socket document, without `--force`.
fn foreign(kind: Kind, why: &str) -> Halt {
    Box::new(documents::failed(
        State::Refused,
        Some(2),
        ErrorCode::CarryFromForeign,
        why,
        Map::new(),
        kind,
    ))
}

/// Carries the decisions of `text`, an earlier document read leniently
/// (`SocketDocument::prior`, MF11), to `document`, if it is the same
/// file's and target's.
fn carry_text(kind: Kind, document: &mut SocketDocument, text: &[u8]) -> Result<CarryOver, Halt> {
    let old = std::str::from_utf8(text)
        .ok()
        .and_then(|t| SocketDocument::prior(t).ok())
        .ok_or_else(|| foreign(kind, "carry_from is not a fitment.socket document"))?;
    let target = document.extracted().map(|e| e.target.definition.instance);
    if old.sha256() != document.source().sha256 {
        return Err(foreign(kind, "carry_from is the socket of another file"));
    }
    if old.target().is_some() && old.target() != target {
        return Err(foreign(kind, "carry_from is the socket of another part"));
    }
    Ok(document.carry_over_prior(&old))
}

/// What the new revision carries from: its parent, and the counts.
type Carried = (Option<i64>, CarryOver);

/// The request's `carry_from`, or the line's current revision (RA4: a
/// revived line carries only from a `carry_from`).
fn carry(
    shared: &Shared,
    row: &JobRow,
    job: &SocketJob,
    current: Option<&RevisionRow>,
    document: &mut SocketDocument,
) -> Result<Option<Carried>, Halt> {
    let source: Option<(Option<i64>, String)> = match &job.carry_from {
        Some(Value::String(reference)) => {
            let reference = SocketRef::parse(reference)
                .map_err(|_| foreign(row.kind, "carry_from names no revision"))?;
            let revision = shared
                .db
                .revision(row.owner, &reference.id, reference.n)
                .map_err(|_| Box::new(run::internal(row.kind, "server.db could not be read")))?
                .ok_or_else(|| foreign(row.kind, "carry_from names no revision of yours"))?;
            Some((Some(revision.key), revision.document_sha256))
        }
        Some(value) => {
            let upload = value["upload"].as_str().unwrap_or("");
            Some((None, upload.trim_start_matches("sha256:").to_owned()))
        }
        None if job.carry => current.map(|c| (Some(c.key), c.document_sha256.clone())),
        None => None,
    };
    let Some((parent, sha256)) = source else {
        return Ok(None);
    };
    let text = run::read_input(shared, row.kind, &sha256)?;
    let carried = carry_text(row.kind, document, &text)?;
    Ok(Some((parent, carried)))
}

/// One attempt at the line's revision (JS13): its current revision read,
/// the decisions carried, the bytes stored, the revision committed with
/// the job's end over that current one only.
fn revise(
    shared: &Shared,
    row: &JobRow,
    control: &Control,
    job: &SocketJob,
    extracted: &SocketDocument,
    id: &str,
    triple: (&str, &str, &str),
) -> Result<Finished, DbError> {
    let current = shared.db.line(row.owner, id)?.and_then(|line| line.current);
    let mut document = extracted.clone();
    let carried = match carry(shared, row, job, current.as_ref(), &mut document) {
        Ok(carried) => carried,
        Err(halt) => return run::end(shared, row, control, &halt, None, &Rows::None),
    };
    let json = document.to_json();
    let sha256 = sha256_hex(json.as_bytes());
    let unchanged = current
        .as_ref()
        .is_some_and(|c| c.document_sha256 == sha256 && c.verified);
    let staged = match run::stage(shared, row.kind, &json) {
        Ok(staged) => staged,
        Err(halt) => return run::end(shared, row, control, &halt, None, &Rows::None),
    };
    hook(shared, "job.stored");
    let (parent, counts) = match carried {
        Some((parent, counts)) => (parent.or(current.as_ref().map(|c| c.key)), counts),
        None => (current.as_ref().map(|c| c.key), CarryOver::default()),
    };
    let count = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
    let unreviewed = document
        .review()
        .iter()
        .filter(|e| e.status == ReviewStatus::Unreviewed)
        .count();
    let digest = document.digest().unwrap_or("").to_owned();
    let revision = (!unchanged).then(|| NewRevision {
        how: "extract",
        verified: true,
        parent,
        document_sha256: &sha256,
        document_digest: &digest,
        extractor_version: document.extractor_version(),
        socket_rules_version: document.socket_rules_version(),
        unreviewed: count(unreviewed),
        carried: count(counts.carried),
        stale: count(counts.stale),
    });
    let ending = End {
        state: State::Succeeded,
        exit: Some(0),
        outcome: unchanged.then(|| "unchanged".to_owned()),
        error: None,
        release_key: false,
        document: Some((
            sha256.clone(),
            staged.bytes,
            Kind::SocketExtract.document_format().to_owned(),
        )),
        result: None,
    };
    let rows = Rows::Socket(SocketRows {
        owner: row.owner,
        line_id: id,
        assembly_sha256: triple.0,
        definition: triple.1,
        occurrence: triple.2,
        base: current.as_ref().map(|c| c.key),
        revision,
    });
    run::end(shared, row, control, &ending, Some(staged), &rows)
}
