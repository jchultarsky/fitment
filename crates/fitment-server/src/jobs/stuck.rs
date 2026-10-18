//! JS8's enforcement for a stuck core call (docs/research/m6-api.md, PR 8):
//! a call that never reaches a check of its flag would hold its job's
//! permits and lane slot forever, and no cancel could free them. A watchdog
//! beside each run waits for the job's flag to be set by a cancel or a
//! deadline (JB5, JB7), then for `jobs.stuck_grace_seconds`; if the call has
//! still not returned, the job is given up ("abandoned"): its end is
//! committed as its stop says, with no document (`cancelled`, or `failed`,
//! `deadline_exceeded`), its CPU and memory permits are released and its
//! slot freed, so the lane goes on. The blocking thread cannot be stopped in
//! safe Rust and is left to return, if it ever does; what it then tries to
//! commit finds the job ended (`Finished::Already`) and writes nothing.
//!
//! The abandoned threads are bounded: at most `jobs.abandoned_max` at once.
//! At that bound the server reports itself unready (OP7, `shutting_down`
//! with `details.reason: "stuck"`) so an orchestrator restarts it, and a
//! further stuck job is not given up: it keeps its permits and slot until
//! its call returns, as before, so the accounting is ahead of the truth by
//! at most `abandoned_max` jobs. A drain's stop (SD1) is never given up on:
//! the process is ending, and SD4 settles the job at the next start; the
//! runtime's shutdown does not wait for a stuck thread (`main.rs`).

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::OwnedSemaphorePermit;

use super::model::Stop;
use super::{Control, documents, run};
use crate::Shared;
use crate::state::{Finished, JobRow, Rows};

/// A job's CPU and memory permits (JS2, JS3).
pub(crate) type Permits = (OwnedSemaphorePermit, OwnedSemaphorePermit);

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

    fn fate(&self) -> std::sync::MutexGuard<'_, Fate> {
        self.fate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Gives the run up, if it is still running and fewer than
    /// `jobs.abandoned_max` runs are given up already: its permits are
    /// dropped and it is counted. False when it ended meanwhile (its
    /// result is coming) or at the bound.
    fn abandon(&self, shared: &Shared) -> bool {
        let mut fate = self.fate();
        if !matches!(*fate, Fate::Running(_)) {
            return false;
        }
        let jobs = &shared.jobs;
        let mut count = jobs.abandoned.load(Ordering::SeqCst);
        loop {
            if count >= jobs.abandoned_max {
                return false;
            }
            match jobs.abandoned.compare_exchange(
                count,
                count + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(now) => count = now,
            }
        }
        // The permits go back to the budget now (JS2, JS3).
        *fate = Fate::Abandoned;
        true
    }
}

/// Settles a watched run when its closure ends, unwinding included: its
/// permits are dropped (JS6), or, if its watchdog gave it up, it is no
/// longer counted.
pub(crate) struct Ending {
    pub(crate) shared: Arc<Shared>,
    pub(crate) watched: Arc<Watched>,
    pub(crate) job: String,
}

impl Drop for Ending {
    fn drop(&mut self) {
        let mut fate = self.watched.fate();
        if matches!(*fate, Fate::Abandoned) {
            self.shared.jobs.abandoned.fetch_sub(1, Ordering::SeqCst);
            tracing::warn!(
                job = self.job,
                "the call of a job given up returned at last; its result was discarded"
            );
        }
        // Dropping a `Running`'s permits returns them (JS6).
        *fate = Fate::Ended;
    }
}

/// Resolves once `control`'s flag has been set by a cancel or a deadline
/// and `jobs.stuck_grace_seconds` have passed; never for a drain's stop.
pub(crate) async fn overdue(shared: &Shared, control: &Control) -> Stop {
    let stop = control.stopped().await;
    if stop == Stop::Shutdown {
        std::future::pending::<()>().await;
    }
    tokio::time::sleep(shared.jobs.stuck_grace()).await;
    stop
}

/// The watchdog's turn (JS8): gives the run up if it may and commits the
/// job's end as `stop` says, with no document; `None` when the run is not
/// given up (it ended meanwhile, or the bound is reached), so the
/// dispatcher waits for its call.
pub(crate) async fn give_up(
    shared: &Arc<Shared>,
    row: &JobRow,
    watched: &Watched,
    stop: Stop,
) -> Option<run::Ran> {
    if !watched.abandon(shared) {
        if shared.jobs.stuck() {
            tracing::error!(
                job = row.id,
                abandoned = shared.jobs.abandoned(),
                "a job's call has not returned, and jobs.abandoned_max are given up already: \
                 it keeps its permits; restart the server"
            );
        }
        return None;
    }
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
    Some(match ended {
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
        _ => run::Ran::Ended(row.clone(), Vec::new()),
    })
}
