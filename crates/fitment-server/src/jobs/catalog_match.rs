//! `match` over the catalog (docs/research/m6-api.md, JS6, JS12, SN10,
//! CX2, CX5, CX7, DC10, M6-25, section 7.13): the socket read and gated
//! again at dispatch (JB2) and the original read, as a file match's; then
//! one snapshot of a pooled reader (CX1), JS12's cap on an exhaustive run
//! checked in it before anything is judged, and the catalog's run with the
//! job's permits as its threads (QR8) and its observer (CA2), asked before
//! each group a judging thread takes (JS8). QR1's refusals end the job
//! `refused` with their codes and no document (section 7.13); a judged run
//! stores its document and index as a file match's does, its record
//! stating the snapshot's generation and CM8's counts (M6-25).
//!
//! Hook point (OP11): `catalog.judge`, at each group judged, on a judging
//! thread.

use std::sync::atomic::Ordering;

use serde_json::{Map, Value};

use super::Control;
use super::documents;
use super::matches::{self, CatalogExtra, Prepared};
use super::model::State;
use super::run::{self, Ran, hook};
use crate::Shared;
use crate::catalog::{Event, Matched, Searched, Selection, Watch};
use crate::gate;
use crate::http::error::ErrorCode;
use crate::matching::{self, CatalogJudged, CatalogJudging, CatalogSearch};
use crate::runs::CatalogFacts;
use crate::state::{DbError, End, JobRow};

/// The selection a match job recorded (RA9).
fn selection_of(prepared: &Prepared) -> Selection {
    match prepared.job.catalog.as_ref() {
        Some(c) if c.selection == "exhaustive" => Selection::Exhaustive,
        Some(c) if c.selection == "named" => Selection::Named(c.ids.clone()),
        _ => Selection::Full,
    }
}

/// CM8's line, as `fitment match --catalog` writes it, the catalog named
/// `catalog` (JB10: no path) and the list of what was not searched an
/// endpoint.
fn searched(selection: &Selection, found: &Matched) -> String {
    match selection {
        Selection::Full => {
            let see = if found.quarantined > 0 {
                " (GET /v1/catalog/entries?status=quarantined)"
            } else {
                ""
            };
            format!(
                "catalog  generation {}  entries {}: {} searched, {} quarantined, not searched{see}  level 1 kept {}",
                found.generation,
                found.entries,
                found.indexed,
                found.quarantined,
                found.level_one.unwrap_or_default()
            )
        }
        Selection::Named(_) => format!(
            "catalog  generation {}  {} named entries (level 1 not used)",
            found.generation, found.selected
        ),
        Selection::Exhaustive => format!(
            "catalog  generation {}  entries {}: exhaustive: level 1 bypassed",
            found.generation, found.entries
        ),
    }
}

/// M6-25: what the run's record states of its catalog.
fn facts(selection: &Selection, found: &Matched) -> CatalogFacts {
    CatalogFacts {
        generation: found.generation,
        entries: found.entries,
        searched: found.indexed,
        quarantined_not_searched: found.quarantined,
        level_one_kept: found.level_one,
        named: matches!(selection, Selection::Named(_)).then_some(found.selected),
        judged: found.candidates,
        distinct_judgments: found.judged,
    }
}

/// QR1, JS12, section 7.13: a run that judged nothing kept: `refused`
/// with its code and exit code, no document; a stale catalog's details say
/// whether a reindex is queued (M6-28).
fn not_run(shared: &Shared, row: &JobRow, searched: &Searched) -> Option<End> {
    match searched {
        Searched::Refused(exit, failure) => {
            let mut details = failure.details.clone();
            if failure.code == ErrorCode::CatalogStale {
                let reindex = shared.db.active_reindex().ok().flatten();
                details.insert("reindex".into(), reindex.map_or(Value::Null, Value::from));
            }
            Some(documents::failed(
                State::of_exit(*exit),
                Some(*exit),
                failure.code,
                &failure.detail,
                details,
                row.kind,
            ))
        }
        Searched::TooLarge { live } => {
            let cap = shared.config.limits.max_catalog_candidates;
            let mut details = Map::new();
            details.insert("limit".into(), "max_catalog_candidates".into());
            details.insert("max_catalog_candidates".into(), cap.into());
            details.insert("entries".into(), (*live).into());
            Some(documents::failed(
                State::Refused,
                Some(3),
                ErrorCode::TooLarge,
                &format!(
                    "the catalog holds {live} live entries, more than an exhaustive run judges ({cap}): run a full or named one"
                ),
                details,
                row.kind,
            ))
        }
        _ => None,
    }
}

/// The job (JS6, SN10): `prepared` is the socket gated at dispatch and
/// the original read; `window` the job's CPU permits.
pub(super) fn run(
    shared: &Shared,
    row: &JobRow,
    control: &Control,
    prepared: &Prepared,
    policy: fitment_core::MatchPolicy,
    window: usize,
) -> Result<Ran, DbError> {
    let selection = selection_of(prepared);
    let tell = |event: Event| {
        if let Event::Counts { done, total, .. } = event {
            let count = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
            control.progress(shared, matches::judging(count(done), count(total)));
            hook(shared, "catalog.judge");
        }
    };
    let watch = Watch::new(&control.flag, &tell);
    let verifier = shared.verifier();
    let judging = CatalogJudging {
        document: &prepared.document,
        policy,
        options: prepared.options,
        verifier: verifier.as_ref(),
        original: prepared
            .original
            .as_ref()
            .map(|o| o.as_ref().map_err(|&()| ())),
    };
    let search = CatalogSearch {
        dir: &shared.catalog,
        selection: &selection,
        cap: shared.config.limits.max_catalog_candidates,
        jobs: window,
        watch: &watch,
    };
    let judged = matching::judge_catalog(judging, &search, &|found| searched(&selection, found));
    hook(shared, "job.ran");
    let judged = match judged {
        Ok(judged) => judged,
        Err(error) => {
            let ending = documents::failed(
                State::Failed,
                Some(1),
                error.code(),
                error.detail(),
                error.details().clone(),
                row.kind,
            );
            return matches::halted(shared, row, control, &ending);
        }
    };
    match judged {
        CatalogJudged::Refused(error) => {
            let ending = gate::end(&error, prepared.build, row.kind);
            matches::halted(shared, row, control, &ending)
        }
        CatalogJudged::NotRun(searched) => {
            // A cancelled run, or one its deadline stopped: `run::end`
            // ends it as the stop says, nothing kept (SN7).
            let ending = not_run(shared, row, &searched)
                .unwrap_or_else(|| run::internal(row.kind, "stopped"));
            matches::halted(shared, row, control, &ending)
        }
        CatalogJudged::Done(done, found) => {
            shared
                .jobs
                .catalog_judged
                .fetch_add(u64::try_from(found.judged).unwrap_or(0), Ordering::SeqCst);
            let facts = facts(&selection, &found);
            let counts = serde_json::to_string(&facts).ok();
            shared
                .db
                .set_catalog_facts(row.key, found.generation, counts.as_deref())?;
            let extra = CatalogExtra {
                facts,
                corrupt: found.corrupt.clone(),
            };
            matches::store(shared, row, control, prepared, &done, Some(&extra))
        }
    }
}
