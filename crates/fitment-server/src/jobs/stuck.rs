//! JS8's enforcement for a stuck core call (docs/research/m6-api.md, PR 8):
//! a call that never reaches a check of its flag would hold its job's
//! permits and lane slot forever, and no cancel could free them. A watchdog
//! beside each run of a pure kind waits for the job's flag to be set by a
//! cancel or a deadline (JB5, JB7), then for `jobs.stuck_grace_seconds`; if
//! the call has still not returned, the job is given up ("abandoned"): its
//! end is committed as its stop says, with no document (`cancelled`, or
//! `failed`, `deadline_exceeded`), its CPU and memory permits are released
//! and its slot freed, so the lane goes on. The blocking thread cannot be
//! stopped in safe Rust and is left to return, if it ever does; what it
//! then tries to commit finds the job ended (`Finished::Already`) and
//! writes nothing.
//!
//! A write-lane job is never given up (the review of PR 8): its end would
//! hide the batches it committed (JB5), and its thread would keep the job
//! writer and, for a gc, the WAL barrier, so giving it up would free
//! nothing. It keeps its permits and slot until its call returns.
//!
//! Given-up runs are bounded: at most `jobs.abandoned_max` at once, and one
//! an owner. A job refused at the global bound counts toward it, so the
//! server reports itself unready (OP7, `shutting_down` with
//! `details.reason: "stuck"`) while any such job waits; a job refused for
//! its owner's bound does not, so one user cannot make the server unready
//! by cancelling their own slow jobs. A refused job keeps its permits and
//! slot (its owner's running cap counting it) and keeps being watched: when
//! a given-up thread returns, it tries again. A drain's stop (SD1) is never
//! given up on: the process is ending, and SD4 settles the job at the next
//! start; the runtime's shutdown does not wait for a stuck thread
//! (`main.rs`).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::{Notify, OwnedSemaphorePermit};
use tokio::task::{JoinError, JoinHandle};

use super::model::{Kind, Stop};
use super::{Control, Counted, documents, run};
use crate::Shared;
use crate::state::{DbError, Finished, JobRow, Rows};

/// A job's CPU and memory permits (JS2, JS3).
pub(crate) type Permits = (OwnedSemaphorePermit, OwnedSemaphorePermit);

/// What a run's blocking task returns.
pub(crate) type Joined = Result<Result<run::Ran, DbError>, JoinError>;

/// The runs given up (JS8): how many, whose, and how many jobs wait
/// refused at the global bound.
#[derive(Debug, Default)]
pub(crate) struct Abandoned {
    state: Mutex<Counts>,
    /// Told when a given-up run's thread returns: a refused job tries
    /// again.
    freed: Notify,
}

#[derive(Debug, Default)]
struct Counts {
    given_up: usize,
    owners: BTreeSet<i64>,
    refused: usize,
}

/// Why a stuck job was not given up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// `jobs.abandoned_max` runs are given up already.
    Bound,
    /// Its owner has a run given up already.
    Owner,
}

impl Abandoned {
    fn counts(&self) -> MutexGuard<'_, Counts> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The runs given up whose threads have not returned.
    pub(crate) fn given_up(&self) -> usize {
        self.counts().given_up
    }

    /// True when the given-up runs and the jobs refused at the bound reach
    /// `max`: the server is unready (OP7).
    pub(crate) fn stuck(&self, max: usize) -> bool {
        let counts = self.counts();
        counts.given_up + counts.refused >= max
    }

    /// Takes a place for `owner`'s run, or says why not.
    fn take(&self, owner: i64, max: usize) -> Result<(), Refusal> {
        let mut counts = self.counts();
        if counts.given_up >= max {
            return Err(Refusal::Bound);
        }
        if !counts.owners.insert(owner) {
            return Err(Refusal::Owner);
        }
        counts.given_up += 1;
        Ok(())
    }

    /// `owner`'s given-up run's thread returned.
    fn release(&self, owner: i64) {
        let mut counts = self.counts();
        counts.given_up = counts.given_up.saturating_sub(1);
        counts.owners.remove(&owner);
        drop(counts);
        self.freed.notify_waiters();
    }
}

/// A job refused at the global bound, counted toward it while it waits.
struct Held(Arc<Shared>);

impl Held {
    fn new(shared: &Arc<Shared>) -> Self {
        shared.jobs.given_up.counts().refused += 1;
        Self(Arc::clone(shared))
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut counts = self.0.jobs.given_up.counts();
        counts.refused = counts.refused.saturating_sub(1);
    }
}

/// Where a watched run stands: running with its permits, ended, or given
/// up by its watchdog.
#[derive(Debug)]
enum Fate {
    // Held, never read: dropped with the fate, which returns them (JS6).
    #[allow(dead_code)]
    Running(Permits),
    Ended,
    Abandoned,
}

/// What a job's blocking run and its watchdog share (JS8).
#[derive(Debug)]
pub(crate) struct Watched {
    fate: Mutex<Fate>,
}

impl Watched {
    /// A run about to start, holding `permits`.
    pub(crate) fn new(permits: Permits) -> Arc<Self> {
        Arc::new(Self {
            fate: Mutex::new(Fate::Running(permits)),
        })
    }

    fn fate(&self) -> MutexGuard<'_, Fate> {
        self.fate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Gives the run up if it is still running and the bounds allow: its
    /// permits are dropped and it is counted. `Err(None)` when it ended
    /// meanwhile (its result is coming).
    fn abandon(&self, shared: &Shared, owner: i64) -> Result<(), Option<Refusal>> {
        let mut fate = self.fate();
        if !matches!(*fate, Fate::Running(_)) {
            return Err(None);
        }
        let jobs = &shared.jobs;
        jobs.given_up
            .take(owner, jobs.abandoned_max)
            .map_err(Some)?;
        // The permits go back to the budget now (JS2, JS3).
        *fate = Fate::Abandoned;
        Ok(())
    }
}

/// Settles a watched run when its closure ends, unwinding included: its
/// permits are dropped (JS6), or, if its watchdog gave it up, it is no
/// longer counted and a refused job may try again; and a query job's CX6
/// count drops only now, when its thread no longer pins the WAL.
pub(crate) struct Ending {
    pub(crate) shared: Arc<Shared>,
    pub(crate) watched: Arc<Watched>,
    pub(crate) job: String,
    pub(crate) owner: i64,
    pub(crate) counted: Option<Counted>,
}

impl Drop for Ending {
    fn drop(&mut self) {
        let mut fate = self.watched.fate();
        if matches!(*fate, Fate::Abandoned) {
            self.shared.jobs.given_up.release(self.owner);
            tracing::warn!(
                job = self.job,
                "the call of a job given up returned at last; its result was discarded"
            );
        }
        // Dropping a `Running`'s permits returns them (JS6).
        *fate = Fate::Ended;
        drop(fate);
        drop(self.counted.take());
    }
}

/// Resolves once `control`'s flag has been set by a cancel or a deadline
/// and `jobs.stuck_grace_seconds` have passed; never for a drain's stop,
/// and never for a write-lane job (`kind` not pure).
async fn overdue(shared: &Shared, kind: Kind, control: &Control) -> Stop {
    if !kind.pure() {
        std::future::pending::<()>().await;
    }
    let stop = control.stopped().await;
    if stop == Stop::Shutdown {
        std::future::pending::<()>().await;
    }
    tokio::time::sleep(shared.jobs.stuck_grace()).await;
    stop
}

/// How a watched run ended for the dispatcher.
pub(crate) enum Watch {
    /// Given up: the end committed.
    GivenUp(run::Ran),
    /// Its call returned.
    Returned(Joined),
}

/// JS8's watchdog over `handle`, job `row`'s run: its call's result, or,
/// once it is overdue and the bounds allow, the job given up. A job the
/// bounds refuse keeps its permits and is tried again whenever a given-up
/// thread returns; refused at the global bound, it counts toward it.
pub(crate) async fn watch(
    shared: &Arc<Shared>,
    row: &JobRow,
    control: &Control,
    watched: &Watched,
    handle: &mut JoinHandle<Result<run::Ran, DbError>>,
) -> Watch {
    let stop = tokio::select! {
        joined = &mut *handle => return Watch::Returned(joined),
        stop = overdue(shared, row.kind, control) => stop,
    };
    let mut held: Option<Held> = None;
    let mut said = false;
    loop {
        let freed = shared.jobs.given_up.freed.notified();
        tokio::pin!(freed);
        freed.as_mut().enable();
        match watched.abandon(shared, row.owner) {
            Ok(()) => {
                drop(held);
                return Watch::GivenUp(give_up(shared, row, stop).await);
            }
            Err(None) => return Watch::Returned(handle.await),
            Err(Some(refusal)) => {
                if !said {
                    said = true;
                    tracing::error!(
                        job = row.id,
                        refusal = ?refusal,
                        abandoned = shared.jobs.abandoned(),
                        "a job's call has not returned, and a bound on jobs given up is met: \
                         it keeps its permits until one returns"
                    );
                }
                held = match refusal {
                    Refusal::Bound => held.or_else(|| Some(Held::new(shared))),
                    Refusal::Owner => None,
                };
                tokio::select! {
                    joined = &mut *handle => return Watch::Returned(joined),
                    () = &mut freed => {}
                }
            }
        }
    }
}

/// Commits a given-up job's end as `stop` says, with no document. If
/// `server.db` fails here, the row stays `cancelling` with no `end` event,
/// its permits already released: SD4 settles it at the next start, as a
/// crash's, and the failure is logged (decided in the review of PR 8).
async fn give_up(shared: &Arc<Shared>, row: &JobRow, stop: Stop) -> run::Ran {
    tracing::error!(
        job = row.id,
        stop = ?stop,
        abandoned = shared.jobs.abandoned(),
        "a job's call did not return within jobs.stuck_grace_seconds of its stop: \
         the job is given up and its permits released"
    );
    let (ending, key, kind) = (Arc::clone(shared), row.key, row.kind);
    let ended = crate::http::blocking(move || {
        let stopped = documents::stopped(kind);
        let end = match stop {
            Stop::Cancel => stopped.cancelled.clone(),
            _ => stopped.deadline.clone(),
        };
        let mut none = || Ok(());
        ending
            .db
            .finish_job(
                key,
                &end,
                &stopped,
                &mut none,
                &Rows::None,
                ending.ids.now_ms(),
                ending.jobs.retention_ms(),
                &documents::end_data,
            )
            .map_err(|_| crate::http::error::ApiError::internal())
    })
    .await;
    match ended {
        Ok(Finished::Ended(row, events)) => run::Ran::Ended(row, events),
        // Its call returned and committed meanwhile: its own events are
        // sent again, which streams and waits take by `seq`.
        Ok(Finished::Already(row)) => {
            let (reading, key) = (Arc::clone(shared), row.key);
            let events = crate::http::blocking(move || {
                reading
                    .db
                    .events_after(key, 0)
                    .map_err(|e| crate::http::db_error(&e))
            })
            .await
            .unwrap_or_default();
            run::Ran::Ended(row, events)
        }
        _ => {
            tracing::error!(
                job = row.id,
                "a given-up job's end could not be written: it stays cancelling until a restart"
            );
            run::Ran::Ended(row.clone(), Vec::new())
        }
    }
}
