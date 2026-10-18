//! `match` over files and components (docs/research/m6-api.md, JS6, JS7,
//! JB2, JB3, JB4, JB10, DC10, DC12, SN1, SN7, SN9, CO5, section 7.8): the
//! socket read and gated again at dispatch, before any candidate is read
//! (JB2); the components read and chosen by core, a selection in a
//! refused assembly refused (`component_refused`); the original read from
//! the socket's own assembly; the candidates judged by `matching.rs` as
//! `fitment match` judges its files, in windows of the job's permits
//! (JS7), its stop flag asked between them (JS8); then the document's
//! bytes and its candidates index stored, and the job ended with the
//! CLI's exit code, its notes and its run's facts, in one transaction.
//!
//! Hook points (OP11): `job.read` (the socket read), `job.ran` (the
//! judging done), `job.stored`, `job.committing`, `job.committed`
//! (`jobs/run.rs`), `match.window` before each window and `match.judge`
//! before each candidate's judgment, inside its caught panic.

use std::collections::BTreeMap;

use fitment_core::{ComponentError, MatchOptions, SelectedComponent, SocketDocument};
use serde_json::{Map, Value};

use super::Control;
use super::documents::{self, Progress};
use super::model::{Kind, State};
use super::notes::Notes;
use super::request::{MatchJob, SocketRef};
use super::run::{self, Halt, Ran, hook};
use crate::Shared;
use crate::gate;
use crate::http::error::ErrorCode;
use crate::matching::{self, ComponentInput, FileInput, Judged, Judging, MatchRun};
use crate::runs::{self, RunFacts, SummaryFacts};
use crate::state::{DbError, End, JobRow, Rows};

/// What a match judges, read and checked (JB2, CO5, SN9).
struct Prepared {
    job: MatchJob,
    document: SocketDocument,
    options: MatchOptions,
    components: Vec<ComponentInput>,
    original: Option<Result<SelectedComponent, ()>>,
}

/// A judging's progress (JB8): counts only, never a verdict (M6-51).
fn judging(done: u64, total: u64) -> Progress {
    Progress {
        phase: "judging",
        done,
        total: Some(total),
    }
}

/// The job (JS6).
pub(crate) fn run(shared: &Shared, row: &JobRow, control: &Control) -> Result<Ran, DbError> {
    let prepared = match prepare(shared, row) {
        Ok(prepared) => prepared,
        Err(halt) => return halted(shared, row, control, &halt),
    };
    let Ok(policy) = matching::policy(
        Some(prepared.job.policy.tolerance_mm),
        Some(prepared.job.policy.tolerance_deg),
        prepared.job.policy.clearance,
    ) else {
        let ending = run::internal(row.kind, "the job's recorded policy cannot be built");
        return halted(shared, row, control, &ending);
    };
    let verifier = shared.verifier();
    let window = usize::try_from(shared.jobs.window(row)).unwrap_or(1);
    let judge_hook = |point: &str| hook(shared, point);
    let judging_of = Judging {
        document: &prepared.document,
        policy,
        options: prepared.options,
        verifier: verifier.as_ref(),
        files: files(&prepared.job),
        components: prepared.components.clone(),
        original: prepared.original.clone(),
        window,
        hook: &judge_hook,
    };
    let mut read = |sha256: &str| run::read_input(shared, row.kind, sha256);
    let mut between = |done: u64, total: u64| {
        control.progress(shared, judging(done, total));
        hook(shared, "match.window");
        !control.flag.load(std::sync::atomic::Ordering::SeqCst)
    };
    let judged = matching::judge(judging_of, &mut read, &mut between);
    hook(shared, "job.ran");
    match judged {
        // The flag is set: `run::end` ends the job as its stop says, a
        // match leaving no document (SN7).
        Judged::Stopped => {
            let ending = run::internal(row.kind, "stopped");
            halted(shared, row, control, &ending)
        }
        Judged::Halted(halt) => halted(shared, row, control, &halt),
        Judged::Refused(error) => {
            let ending = gate::end(&error, gate::build(shared), row.kind);
            halted(shared, row, control, &ending)
        }
        Judged::Done(done) => store(shared, row, control, &prepared, &done),
    }
}

fn halted(shared: &Shared, row: &JobRow, control: &Control, ending: &End) -> Result<Ran, DbError> {
    let finished = run::end(shared, row, control, ending, None, &Rows::None)?;
    run::conclude(shared, row, control, finished)
}

/// MF3: each file once, every label it was named by in the request's
/// order.
fn files(job: &MatchJob) -> Vec<FileInput> {
    let mut out: Vec<FileInput> = Vec::new();
    for file in &job.files {
        let sha256 = file.upload.trim_start_matches("sha256:");
        match out.iter_mut().find(|f| f.sha256 == sha256) {
            Some(seen) => seen.labels.push(file.label.clone()),
            None => out.push(FileInput {
                sha256: sha256.to_owned(),
                labels: vec![file.label.clone()],
            }),
        }
    }
    out
}

/// JB2, CO5, SN9: the socket read and gated again (MF11 against the
/// running build, the strict reader, MF2 with the floor in force now);
/// the components read and chosen; the original read.
fn prepare(shared: &Shared, row: &JobRow) -> Result<Prepared, Halt> {
    let job: MatchJob = run::request(row)?;
    let bytes = run::read_input(
        shared,
        row.kind,
        job.socket_document.trim_start_matches("sha256:"),
    )?;
    hook(shared, "job.read");
    let build = gate::build(shared);
    let document = matching::read_socket(&bytes, build)
        .map_err(|e| Box::new(gate::end(&e, build, row.kind)))?;
    drop(bytes);
    let floor = shared.config.matching.require_reviewed_floor;
    let require_reviewed = job.options.require_reviewed || floor;
    matching::admit(&document, require_reviewed)
        .map_err(|e| Box::new(gate::end(&e, build, row.kind)))?;
    let options = MatchOptions::new(job.options.require_fit, require_reviewed);
    let components = components(shared, row.kind, &job)?;
    let original = job.original.as_deref().map(|sha256| {
        let sha256 = sha256.trim_start_matches("sha256:");
        // CO5: an original whose assembly cannot be read fails the run
        // `run_incomplete`, its document kept, as the CLI's exit 1.
        run::read_input(shared, row.kind, sha256)
            .map_err(|_| ())
            .and_then(|bytes| SelectedComponent::target(&bytes, &document).map_err(|_| ()))
    });
    Ok(Prepared {
        job,
        document,
        options,
        components,
        original,
    })
}

/// CO5, CO3: each component read and chosen by core, once per (assembly,
/// definition) with every label; its default label
/// `component:<sha256, first 12>/#<n>` (RA8). A selection error ends the
/// job `refused`, exit 2, with its code; a component of a refused assembly
/// `refused`, exit 3, `component_refused`, before anything is judged.
fn components(shared: &Shared, kind: Kind, job: &MatchJob) -> Result<Vec<ComponentInput>, Halt> {
    let mut assemblies: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut out: Vec<ComponentInput> = Vec::new();
    for asked in &job.components {
        let sha256 = asked.assembly.trim_start_matches("sha256:").to_owned();
        if !assemblies.contains_key(&sha256) {
            let bytes = run::read_input(shared, kind, &sha256)?;
            assemblies.insert(sha256.clone(), bytes);
        }
        let bytes = assemblies.get(&sha256).map_or(&[][..], Vec::as_slice);
        let component =
            SelectedComponent::read(bytes, asked.part.as_deref(), asked.occurrence.as_deref())
                .map_err(|e| Box::new(component_error(kind, &e)))?;
        let label = asked
            .label
            .clone()
            .unwrap_or_else(|| format!("component:{}/#{}", &sha256[..12], component.definition));
        match out.iter_mut().find(|c| {
            c.component.assembly_sha256 == component.assembly_sha256
                && c.component.definition == component.definition
        }) {
            Some(seen) => seen.labels.push(label),
            None => out.push(ComponentInput {
                component,
                labels: vec![label],
            }),
        }
    }
    Ok(out)
}

/// A component that cannot be read: its selection's error (exit 2) or the
/// gate's refusals (exit 3, `component_refused`).
fn component_error(kind: Kind, error: &ComponentError) -> End {
    match error {
        ComponentError::Select(select) => super::socket::selection_error(kind, select),
        ComponentError::Refused(refusals) => {
            let mut details = Map::new();
            details.insert("refusals".into(), gate::refusal_list(refusals));
            documents::failed(
                State::Refused,
                Some(3),
                ErrorCode::ComponentRefused,
                "a component was chosen in an assembly the gate refuses: nothing was judged",
                details,
                kind,
            )
        }
        _ => run::internal(kind, "a component's definition was not found"),
    }
}

/// The error of a run that exits 1 (JB3, section 7.13): the original
/// failing its own socket, a pass after a shortlist drop, or another
/// fitment bug or an unreadable original (`run_incomplete`); its details
/// the bugs' labels and the original's problem.
fn bug_error(done: &MatchRun) -> (ErrorCode, String) {
    let code = if done
        .original
        .as_deref()
        .is_some_and(|o| o.starts_with("the original fails"))
    {
        ErrorCode::OriginalFailsOwnSocket
    } else if done.shortlist_unsound {
        ErrorCode::ShortlistUnsound
    } else {
        ErrorCode::RunIncomplete
    };
    let mut details = Map::new();
    details.insert("bugs".into(), Value::from(done.bugs.clone()));
    details.insert("corrupt".into(), Value::Array(Vec::new()));
    details.insert(
        "original".into(),
        done.original.clone().map_or(Value::Null, Value::from),
    );
    let detail = "the run's document is kept, but it is no result: see details and the job's notes";
    (code, documents::error_details(detail, details))
}

/// The revision `job` names is unverified (RA10): the run notes it.
fn unverified(shared: &Shared, row: &JobRow, job: &MatchJob) -> bool {
    let Some(reference) = job.socket.as_str().and_then(|s| SocketRef::parse(s).ok()) else {
        return false;
    };
    shared
        .db
        .revision(row.owner, &reference.id, reference.n)
        .ok()
        .flatten()
        .is_some_and(|r| !r.verified)
}

/// The document and its index stored (DC1, DC12), then the job's end: the
/// state and exit code the CLI's would be (JB3, JB4), `outcome` for a
/// success only (M6-59), the notes (JB10) and the run's facts (DC10).
fn store(
    shared: &Shared,
    row: &JobRow,
    control: &Control,
    prepared: &Prepared,
    done: &MatchRun,
) -> Result<Ran, DbError> {
    let job = &prepared.job;
    let staged = |json: &str| run::stage(shared, row.kind, json);
    shared
        .jobs
        .indexed
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let Ok(entries) = runs::index(done.json.as_bytes()) else {
        let ending = run::internal(
            row.kind,
            "the match document's candidates cannot be indexed",
        );
        return halted(shared, row, control, &ending);
    };
    let document = match staged(&done.json) {
        Ok(document) => document,
        Err(halt) => return halted(shared, row, control, &halt),
    };
    let index = runs::CandidatesIndex {
        format: runs::INDEX_FORMAT.into(),
        format_version: 1,
        document: document.sha256.clone(),
        entries,
    };
    let index = match staged(&(serde_json::to_string(&index).unwrap_or_default() + "\n")) {
        Ok(index) => index,
        Err(halt) => return halted(shared, row, control, &halt),
    };
    hook(shared, "job.stored");
    let mut notes = Notes::default();
    notes.matched(&done.notes);
    if unverified(shared, row, job) {
        notes.unverified_socket();
    }
    let state = State::of_exit(done.exit);
    let facts = RunFacts {
        socket_sha256: job.socket_document.trim_start_matches("sha256:").to_owned(),
        socket_digest: prepared.document.digest().map(str::to_owned),
        socket_assembly: prepared.document.source().sha256.clone(),
        unreviewed: done.unreviewed,
        files: job.files.len(),
        components: job.components.len(),
        distinct: done.distinct,
        summary: SummaryFacts::of(&done.summary),
        digest: done.digest.clone(),
        policy: matching::policy_facts(&match_policy(job)),
        fit_verifier: done.fit_verifier.clone(),
        bugs: done.bugs.clone(),
        original: done.original.clone(),
        original_judged: done.original_judged,
    };
    let ending = End {
        exit: Some(done.exit),
        outcome: (state == State::Succeeded).then(|| done.outcome.to_owned()),
        error: (done.exit == 1).then(|| {
            let (code, details) = bug_error(done);
            (code.name().to_owned(), details)
        }),
        document: Some((
            document.sha256.clone(),
            document.bytes,
            fitment_core::MATCH_FORMAT.to_owned(),
        )),
        result: Some(format!("/v1/matches/{}", row.id)),
        notes: notes.to_json(),
        index: Some((index.sha256.clone(), index.bytes)),
        run: serde_json::to_string(&facts).ok(),
        ..End::bare(state)
    };
    let finished = run::end_staged(
        shared,
        row,
        control,
        &ending,
        vec![document, index],
        &Rows::None,
    )?;
    run::conclude(shared, row, control, finished)
}

/// The job's policy, as built at its request (DC7).
fn match_policy(job: &MatchJob) -> fitment_core::MatchPolicy {
    matching::policy(
        Some(job.policy.tolerance_mm),
        Some(job.policy.tolerance_deg),
        job.policy.clearance,
    )
    .unwrap_or_default()
}
