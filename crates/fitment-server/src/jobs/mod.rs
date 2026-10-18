//! The job system (docs/research/m6-api.md, sections 4.2 and 4.3): a job
//! exists once its row commits (JS11); the dispatcher takes it from its
//! lane's queue when the lane has a slot (JS1, JS4), holds its CPU and
//! memory permits from dispatch to return (JS2, JS3), and runs it in one
//! blocking task (JS6) with a stop flag that a cancel, its deadline or a
//! drain sets (JS8, JB7, SD1). A job's end commits with its result (JS13);
//! its events go to `job_event` rows and a broadcast (JS10). At a start,
//! the jobs a stopped process left are settled (SD4) and the queued ones
//! queued again in their order.
//!
//! Hook points (OP11): `job.submitted` (the creating commit done, before
//! the job is queued), `job.dispatched` (running, before its blocking
//! task), and those of the kinds' runs (`jobs/run.rs`).

pub(crate) mod catalog;
pub(crate) mod catalog_match;
pub mod documents;
pub(crate) mod events;
pub(crate) mod matches;
pub mod model;
pub(crate) mod notes;
pub mod request;
pub(crate) mod run;
mod scheduler;
pub(crate) mod socket;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{Notify, Semaphore, watch};

use crate::Shared;
use crate::config::Config;
use crate::state::{EventRow, JobRow, Recovered};
use documents::Progress;
use events::{Channels, Live, Streams};
use model::{Kind, Lane, State, Stop};
use scheduler::Sched;
pub(crate) use scheduler::{Entry, Full};

/// One MiB.
const MIB: u64 = 1024 * 1024;

/// JS3 (review S13 of PR 5): a job's memory per byte of its largest
/// input: the CLI's peak resident set measured at 13 to 20 times the file
/// on the corpus's files of 2 MB and more, so 20, and the input's own
/// bytes, which the job holds.
const ESTIMATE_PER_BYTE: u64 = 21;

/// A dispatched job's stop flag (JS8) and why it was set.
#[derive(Debug)]
pub struct Control {
    /// `job.key`.
    pub(crate) key: i64,
    /// `j_…`.
    pub(crate) id: String,
    /// Read where the job may stop.
    pub(crate) flag: AtomicBool,
    stop: Mutex<Option<Stop>>,
}

impl Control {
    fn new(key: i64, id: &str) -> Self {
        Self {
            key,
            id: id.to_owned(),
            flag: AtomicBool::new(false),
            stop: Mutex::new(None),
        }
    }

    /// Why the flag was set, if it was: the first reason stands.
    pub(crate) fn stop(&self) -> Option<Stop> {
        *self.stop.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sets the flag (JS8).
    pub(crate) fn set(&self, stop: Stop) {
        let mut reason = self.stop.lock().unwrap_or_else(PoisonError::into_inner);
        if reason.is_none() {
            *reason = Some(stop);
        }
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Records and sends the job's progress (JB8).
    pub(crate) fn progress(&self, shared: &Shared, progress: Progress) {
        shared.jobs.channels.progress(self.key, &self.id, progress);
    }
}

/// The job system's state (JS1 to JS5, JS10).
#[derive(Debug)]
pub struct Jobs {
    sched: Mutex<Sched>,
    /// JS2: the fair CPU semaphore, `jobs.cpu` permits.
    cpu: Arc<Semaphore>,
    cpu_total: u32,
    /// JS3: the memory budget, in MiB.
    memory: Arc<Semaphore>,
    memory_total: u32,
    pub(crate) channels: Channels,
    pub(crate) streams: Arc<Streams>,
    /// Cleared when a drain starts (SD1): nothing more is dispatched.
    dispatching: AtomicBool,
    started: AtomicBool,
    /// When the dispatcher last reconciled with `server.db`, in
    /// milliseconds since `epoch`.
    reconciled_ms: AtomicU64,
    /// A reconciliation is pending.
    reconciling: AtomicBool,
    epoch: std::time::Instant,
    /// Set when the server drains (SD1): streams and waits end.
    shutdown: watch::Sender<bool>,
    /// A dispatched job ended.
    ended: Notify,
    retention_ms: i64,
    deadline_extract_ms: AtomicU64,
    deadline_match_ms: AtomicU64,
    deadline_verify_ms: AtomicU64,
    heartbeat_ms: AtomicU64,
    /// CX6: query dispatch held by the WAL barrier.
    barrier: AtomicBool,
    /// CX6: when, in milliseconds since `epoch`, a barrier may be tried
    /// again after one its readers outlasted.
    barrier_next_ms: AtomicU64,
    /// JS3: a catalog match's memory per entry its selection can judge,
    /// in KiB (`jobs.match_entry_kib`).
    match_entry_kib: u64,
    /// VT45: the judgments catalog matches made, for the tests' readout.
    pub(crate) catalog_judged: AtomicU64,
    /// JS1: a match's permits, `jobs.match_threads`.
    match_threads: u32,
    /// DC12, VT46: the match documents whose structure was scanned, once
    /// each when stored, never for a page.
    pub(crate) indexed: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(feature = "test-support")]
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

impl Jobs {
    /// The job system of `config`.
    pub fn new(config: &Config) -> Self {
        let jobs = &config.jobs;
        let cpu_total = jobs.cpu_budget().max(1);
        let memory_total =
            u32::try_from(jobs.memory_mib.clamp(1, u64::from(u32::MAX >> 4))).unwrap_or(4096);
        Self {
            sched: Mutex::new(Sched::new(jobs)),
            cpu: Arc::new(Semaphore::new(usize::try_from(cpu_total).unwrap_or(1))),
            cpu_total,
            memory: Arc::new(Semaphore::new(usize::try_from(memory_total).unwrap_or(1))),
            memory_total,
            channels: Channels::default(),
            streams: Arc::new(Streams::new(jobs.streams_per_user, jobs.streams)),
            dispatching: AtomicBool::new(true),
            started: AtomicBool::new(false),
            reconciled_ms: AtomicU64::new(0),
            reconciling: AtomicBool::new(false),
            epoch: std::time::Instant::now(),
            shutdown: watch::channel(false).0,
            ended: Notify::new(),
            retention_ms: i64::try_from(jobs.retention_days)
                .unwrap_or(i64::MAX / 86_400_000)
                .saturating_mul(86_400_000),
            deadline_extract_ms: AtomicU64::new(
                jobs.deadline_extract_minutes.saturating_mul(60_000),
            ),
            deadline_match_ms: AtomicU64::new(jobs.deadline_match_minutes.saturating_mul(60_000)),
            deadline_verify_ms: AtomicU64::new(jobs.deadline_verify_minutes.saturating_mul(60_000)),
            heartbeat_ms: AtomicU64::new(15_000),
            barrier: AtomicBool::new(false),
            barrier_next_ms: AtomicU64::new(0),
            match_entry_kib: jobs.match_entry_kib,
            catalog_judged: AtomicU64::new(0),
            match_threads: jobs.match_threads.max(1),
            indexed: AtomicU64::new(0),
        }
    }

    /// JB12: how long a terminal job's record is kept.
    pub fn retention_ms(&self) -> i64 {
        self.retention_ms
    }

    /// JB7: a kind's deadline; the write lane has none.
    pub fn deadline(&self, kind: Kind) -> Option<Duration> {
        match kind {
            Kind::Features | Kind::AssemblyRead | Kind::SocketExtract | Kind::SocketImport => Some(
                Duration::from_millis(self.deadline_extract_ms.load(Ordering::SeqCst).max(1)),
            ),
            Kind::Match => Some(Duration::from_millis(
                self.deadline_match_ms.load(Ordering::SeqCst).max(1),
            )),
            // JB7 (review F23): verify, plain or deep, under its own.
            Kind::CatalogVerify => Some(Duration::from_millis(
                self.deadline_verify_ms.load(Ordering::SeqCst).max(1),
            )),
            _ => None,
        }
    }

    /// OP11: the pure kinds' deadlines, the extractions', the matches' and
    /// verify's, shortened for a test (JB7).
    #[cfg(feature = "test-support")]
    pub fn set_deadline(&self, deadline: Duration) {
        self.deadline_extract_ms
            .store(millis(deadline), Ordering::SeqCst);
        self.deadline_match_ms
            .store(millis(deadline), Ordering::SeqCst);
        self.deadline_verify_ms
            .store(millis(deadline), Ordering::SeqCst);
    }

    /// OP11, VT45: the judgments catalog matches have made.
    #[cfg(feature = "test-support")]
    pub fn catalog_judgments(&self) -> u64 {
        self.catalog_judged.load(Ordering::SeqCst)
    }

    /// CX6: holds query dispatch (true), or lets it go on.
    pub(crate) fn set_barrier(&self, up: bool) {
        self.barrier.store(up, Ordering::SeqCst);
    }

    /// CX6: whether a barrier may be tried now.
    pub(crate) fn barrier_due(&self) -> bool {
        let now = u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        now >= self.barrier_next_ms.load(Ordering::SeqCst)
    }

    /// CX6: a barrier its readers outlasted is tried again no sooner than
    /// `bound` from now.
    pub(crate) fn barrier_missed(&self, bound: Duration) {
        let now = u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        let next = now.saturating_add(u64::try_from(bound.as_millis()).unwrap_or(u64::MAX));
        self.barrier_next_ms.store(next, Ordering::SeqCst);
    }

    /// JS3 (review F3): a catalog match's memory estimate, in MiB: 64 MiB
    /// and `jobs.match_entry_kib` per entry its selection can judge, read
    /// from the catalog's counts at dispatch: every live entry for an
    /// exhaustive run (at most the cap, JS12), every searchable one for a
    /// full run, the named ones for a named run.
    pub(crate) fn catalog_estimate(&self, entries: u64) -> u32 {
        let mib = entries
            .saturating_mul(self.match_entry_kib)
            .div_ceil(1024)
            .saturating_add(64);
        u32::try_from(mib)
            .unwrap_or(u32::MAX)
            .min(self.memory_total)
    }

    /// OP11: holds dispatch (true) or lets it go on (false), so a test sees
    /// jobs queued without draining the server.
    #[cfg(feature = "test-support")]
    pub fn hold(&self, held: bool) {
        self.dispatching.store(!held, Ordering::SeqCst);
    }

    /// OP11: a stream's heartbeat interval, shortened for a test (JB8).
    #[cfg(feature = "test-support")]
    pub fn set_heartbeat(&self, interval: Duration) {
        self.heartbeat_ms.store(millis(interval), Ordering::SeqCst);
    }

    /// A stream's heartbeat interval (JB8).
    pub(crate) fn heartbeat(&self) -> Duration {
        Duration::from_millis(self.heartbeat_ms.load(Ordering::SeqCst).max(1))
    }

    /// JS2, JS3: a job's permits and its memory estimate, from the largest
    /// input's bytes: [`ESTIMATE_PER_BYTE`] times the file, in MiB; one
    /// above the budget waits for the whole of it and runs alone.
    pub(crate) fn entry(&self, row: &JobRow, input_bytes: u64) -> Entry {
        let window = self.window(row);
        let estimate = input_bytes
            .saturating_mul(ESTIMATE_PER_BYTE)
            .saturating_mul(u64::from(window))
            .div_ceil(MIB)
            .max(1);
        let catalog = (row.kind == Kind::Match)
            .then(|| serde_json::from_str::<request::MatchJob>(&row.request).ok())
            .flatten()
            .and_then(|job| job.catalog)
            .map(|c| (c.selection, c.ids.len()));
        Entry {
            key: row.key,
            id: row.id.clone(),
            owner: row.owner,
            kind: row.kind,
            cpu: window,
            memory_mib: u32::try_from(estimate)
                .unwrap_or(u32::MAX)
                .min(self.memory_total),
            catalog,
        }
    }

    /// JS1, JS2, JS7: a job's CPU permits, which a file match uses as its
    /// window: `jobs.match_threads` for a match, but no more than its
    /// candidates or the budget, and 1 for one candidate; 1 for any other
    /// kind. Its memory estimate counts each file of the window (JS3).
    pub(crate) fn window(&self, row: &JobRow) -> u32 {
        if row.kind != Kind::Match {
            return 1;
        }
        let candidates = serde_json::from_str::<request::MatchJob>(&row.request)
            .map_or(1, |job| job.candidates());
        let candidates = u32::try_from(candidates).unwrap_or(u32::MAX);
        self.match_threads
            .min(self.cpu_total)
            .min(candidates)
            .max(1)
    }

    /// JS5: reserves room in `lane` for one of `owner`'s jobs.
    pub(crate) fn reserve(self: &Arc<Self>, lane: Lane, owner: i64) -> Result<Reservation, Full> {
        lock(&self.sched).reserve(lane, owner)?;
        Ok(Reservation {
            jobs: Arc::clone(self),
            lane,
            owner,
            held: true,
        })
    }

    /// Queues `entry` under `reservation` (JS11: its row committed).
    pub(crate) fn enqueue(&self, mut reservation: Reservation, entry: Entry) {
        let mut sched = lock(&self.sched);
        sched.unreserve(reservation.lane, reservation.owner);
        reservation.held = false;
        let _ = sched.push(entry);
    }

    /// JS4: starts every job a lane may run now, each on its own task.
    pub(crate) fn kick(&self, shared: &Arc<Shared>) {
        self.started.store(true, Ordering::SeqCst);
        if !self.dispatching.load(Ordering::SeqCst) {
            return;
        }
        let picks = {
            let mut sched = lock(&self.sched);
            let picks = sched.picks(self.barrier.load(Ordering::SeqCst));
            for entry in &picks {
                sched
                    .live
                    .insert(entry.key, Arc::new(Control::new(entry.key, &entry.id)));
            }
            picks
                .into_iter()
                .filter_map(|e| sched.live.get(&e.key).cloned().map(|c| (e, c)))
                .collect::<Vec<_>>()
        };
        for (entry, control) in picks {
            tokio::spawn(dispatch(Arc::clone(shared), entry, control));
        }
        self.reconcile(shared);
    }

    /// Review S2 of PR 5: within a second of any kick, and at most once a
    /// second, the dispatcher takes up queued jobs of `server.db` it does
    /// not hold (one a failed start or a lost submission left), so no
    /// committed job waits for a restart.
    fn reconcile(&self, shared: &Arc<Shared>) {
        if self.reconciling.swap(true, Ordering::SeqCst) {
            return;
        }
        let now = u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        let last = self.reconciled_ms.load(Ordering::SeqCst);
        let wait = if last == 0 {
            0
        } else {
            (last + 1000).saturating_sub(now)
        };
        // A server dropped meanwhile is not kept, nor its lease (SD3).
        let weak = Arc::downgrade(shared);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(wait)).await;
            let Some(shared) = weak.upgrade() else {
                return;
            };
            let jobs = &shared.jobs;
            let done = u64::try_from(jobs.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
            jobs.reconciled_ms.store(done.max(1), Ordering::SeqCst);
            jobs.reconciling.store(false, Ordering::SeqCst);
            if !jobs.dispatching.load(Ordering::SeqCst) {
                return;
            }
            let reading = Arc::clone(&shared);
            let Ok(queued) = crate::http::blocking(move || {
                reading
                    .db
                    .queued_jobs()
                    .map_err(|e| crate::http::db_error(&e))
            })
            .await
            else {
                return;
            };
            let added = {
                let mut sched = lock(&jobs.sched);
                queued
                    .iter()
                    .filter(|(row, bytes)| sched.push(jobs.entry(row, *bytes)))
                    .count()
            };
            if added > 0 {
                tracing::warn!(
                    added,
                    "queued jobs the dispatcher did not hold were taken up"
                );
                jobs.kick(&shared);
            }
        });
    }

    /// The first request's kick, for a server dispatching nothing yet
    /// (recovered jobs wait for it, or for `serve`).
    pub(crate) fn kick_once(&self, shared: &Arc<Shared>) {
        if !self.started.load(Ordering::SeqCst) {
            self.kick(shared);
        }
    }

    fn done(&self, entry: &Entry) {
        lock(&self.sched).done(entry);
        self.ended.notify_waiters();
    }

    /// JB5: a cancelled job leaves its queue.
    pub(crate) fn unqueue(&self, key: i64) -> bool {
        lock(&self.sched).remove(key)
    }

    /// JS8: sets a dispatched job's flag; false if it is not dispatched.
    pub(crate) fn flag(&self, key: i64, stop: Stop) -> bool {
        let control = lock(&self.sched).live.get(&key).cloned();
        control.is_some_and(|c| {
            c.set(stop);
            true
        })
    }

    /// Sends persisted events (JB8).
    pub(crate) fn publish(&self, job: &str, events: &[EventRow]) {
        self.channels.publish(job, events);
    }

    /// A lane's queued and running jobs (OP6).
    pub fn counts(&self, lane: Lane) -> (usize, usize) {
        lock(&self.sched).counts(lane)
    }

    /// DC11's `queues_full`.
    pub fn queues_full(&self) -> bool {
        lock(&self.sched).full()
    }

    /// The CPU permits free, of how many (JS2, OP6).
    pub fn cpu_free(&self) -> (usize, u32) {
        (self.cpu.available_permits(), self.cpu_total)
    }

    /// The memory MiB free, of how many (JS3, OP6).
    pub fn memory_free(&self) -> (usize, u32) {
        (self.memory.available_permits(), self.memory_total)
    }

    /// OP11, VT46: how many match documents were scanned for their index.
    #[cfg(feature = "test-support")]
    pub fn documents_indexed(&self) -> u64 {
        self.indexed.load(Ordering::SeqCst)
    }

    /// OP11: the live-event channels open, for the tests' leak checks.
    #[cfg(feature = "test-support")]
    pub fn channels_open(&self) -> usize {
        self.channels.open()
    }

    /// The server's drain (SD1).
    pub(crate) fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    /// SD1, step 2 and 4: dispatch stops, and every stream and wait is
    /// told.
    pub(crate) fn stop_dispatch(&self) {
        self.dispatching.store(false, Ordering::SeqCst);
        self.shutdown.send_replace(true);
    }

    /// SD1, step 3: running jobs get `drain` to finish; then their flags
    /// are set and each is recorded as JB6 says as its call returns. Waits
    /// for that up to `drain` again.
    pub(crate) async fn drain(&self, drain: Duration) {
        if !self.wait_idle(drain).await {
            let controls: Vec<Arc<Control>> = lock(&self.sched).live.values().cloned().collect();
            tracing::info!(
                running = controls.len(),
                "the drain's time is up: running jobs are stopped"
            );
            for control in controls {
                control.set(Stop::Shutdown);
            }
            if !self.wait_idle(drain.max(Duration::from_secs(10))).await {
                tracing::warn!("jobs still ran when the drain ended");
            }
        }
    }

    /// True once no job is dispatched, within `limit`.
    async fn wait_idle(&self, limit: Duration) -> bool {
        let until = tokio::time::Instant::now() + limit;
        loop {
            let ended = self.ended.notified();
            if lock(&self.sched).live.is_empty() {
                return true;
            }
            if tokio::time::timeout_at(until, ended).await.is_err() {
                return lock(&self.sched).live.is_empty();
            }
        }
    }
}

/// A submission's room in its lane (JS5), given back unless the job is
/// queued under it.
#[derive(Debug)]
pub(crate) struct Reservation {
    jobs: Arc<Jobs>,
    lane: Lane,
    owner: i64,
    held: bool,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.held {
            lock(&self.jobs.sched).unreserve(self.lane, self.owner);
        }
    }
}

/// An async hook point (OP11).
pub(crate) async fn hook(shared: &Shared, point: &str) {
    #[cfg(feature = "test-support")]
    let _ = shared.hooks.reached(point).await;
    #[cfg(not(feature = "test-support"))]
    let _ = (shared, point);
}

/// JS2, JS4, JS6, JS8: one dispatched job: its permits (fair, held from
/// dispatch to return), its start, its deadline's timer, its run on the
/// blocking pool, its end's events; then its slot back and the next
/// dispatch.
async fn dispatch(shared: Arc<Shared>, mut entry: Entry, control: Arc<Control>) {
    let jobs = &shared.jobs;
    if let Some((selection, named)) = entry.catalog.clone() {
        entry.memory_mib = catalog_memory(&shared, &selection, named).await;
    }
    let cpu = Arc::clone(&jobs.cpu).acquire_many_owned(entry.cpu).await;
    let memory = Arc::clone(&jobs.memory)
        .acquire_many_owned(entry.memory_mib)
        .await;
    let (Ok(cpu), Ok(memory)) = (cpu, memory) else {
        jobs.done(&entry);
        return;
    };
    if !jobs.dispatching.load(Ordering::SeqCst) {
        // SD1: a drain began while it waited; it stays queued.
        jobs.done(&entry);
        return;
    }
    let deadline = jobs.deadline(entry.kind);
    let starting = Arc::clone(&shared);
    let key = entry.key;
    let started = crate::http::blocking(move || {
        let now = starting.ids.now_ms();
        let deadline_ms =
            deadline.map(|d| now.saturating_add(i64::try_from(d.as_millis()).unwrap_or(i64::MAX)));
        starting
            .db
            .start_job(key, now, deadline_ms)
            .map_err(|e| crate::http::db_error(&e))
    })
    .await;
    let row = match started {
        Ok(Some((row, event))) => {
            jobs.publish(&row.id, std::slice::from_ref(&event));
            row
        }
        other => {
            if other.is_err() {
                // It stays queued in server.db: the next start runs it.
                tracing::error!(job = entry.id, "a job could not be started");
            }
            jobs.done(&entry);
            shared.jobs.kick(&shared);
            return;
        }
    };
    hook(&shared, "job.dispatched").await;
    let timer = deadline.map(|d| tokio::spawn(expire(Arc::clone(&shared), row.clone(), d)));
    let ran = run_blocking(&shared, &row, &control, (cpu, memory)).await;
    if let Some(timer) = timer {
        timer.abort();
    }
    jobs.channels.forget(row.key);
    match ran {
        run::Ran::Ended(row, events) => {
            jobs.publish(&row.id, &events);
            crate::metrics::job_ended(&shared.metrics, &row);
        }
        run::Ran::Requeued(row, events) => {
            jobs.publish(&row.id, &events);
            tracing::info!(job = row.id, "a drain queued a running job again");
        }
    }
    jobs.done(&entry);
    shared.jobs.kick(&shared);
}

/// JS3: a catalog match's memory estimate from the catalog's counts at
/// dispatch: a named run's count needs none.
async fn catalog_memory(shared: &Arc<Shared>, selection: &str, named: usize) -> u32 {
    let entries = if selection == "named" {
        u64::try_from(named).unwrap_or(u64::MAX)
    } else {
        let reading = Arc::clone(shared);
        let info = crate::http::blocking(move || reading.catalog.info()).await;
        match (info, selection) {
            (Ok(info), "exhaustive") => info
                .entries
                .min(shared.config.limits.max_catalog_candidates),
            (Ok(info), _) => info.indexed,
            (Err(_), _) => 0,
        }
    };
    shared.jobs.catalog_estimate(entries)
}

/// The job's run on the blocking pool, its permits dropped when the
/// closure ends, a panic included (JS6). A panic or a failure of
/// `server.db` ends the job `failed`, `internal`, the panic's text in the
/// log under the job's id only (VT9, XH13).
async fn run_blocking(
    shared: &Arc<Shared>,
    row: &JobRow,
    control: &Arc<Control>,
    permits: (
        tokio::sync::OwnedSemaphorePermit,
        tokio::sync::OwnedSemaphorePermit,
    ),
) -> run::Ran {
    let (working, running, ctl) = (Arc::clone(shared), row.clone(), Arc::clone(control));
    let result = tokio::task::spawn_blocking(move || {
        let _permits = permits;
        run::execute(&working, &running, &ctl)
    })
    .await;
    let failure = match result {
        Ok(Ok(ran)) => return ran,
        Ok(Err(error)) => format!("server.db failed: {error}"),
        Err(join) => join
            .try_into_panic()
            .ok()
            .and_then(|p| {
                p.downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            })
            .unwrap_or_else(|| "a panic".to_owned()),
    };
    tracing::error!(
        job = row.id,
        failure = failure.as_str(),
        "a job failed: internal"
    );
    let (ending, key, kind) = (Arc::clone(shared), row.key, row.kind);
    let ended = crate::http::blocking(move || {
        let end = run::internal(
            kind,
            "an internal error; the server's log holds it under the job's id",
        );
        let stopped = documents::stopped(kind);
        let mut none = || Ok(());
        ending
            .db
            .finish_job(
                key,
                &end,
                &stopped,
                &mut none,
                &crate::state::Rows::None,
                ending.ids.now_ms(),
                ending.jobs.retention_ms(),
                &documents::end_data,
            )
            .map_err(|_| crate::http::error::ApiError::internal())
    })
    .await;
    match ended {
        Ok(crate::state::Finished::Ended(row, events)) => run::Ran::Ended(row, events),
        // It ended before the failure (a panic after its commit): its own
        // events are sent again, which streams and waits take by `seq`.
        Ok(crate::state::Finished::Already(row)) => {
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
    }
}

/// JB7: past its deadline, a running job becomes `cancelling` in
/// `server.db`, then its flag is set; it ends `failed`,
/// `deadline_exceeded`, when its call returns.
async fn expire(shared: Arc<Shared>, row: JobRow, deadline: Duration) {
    tokio::time::sleep(deadline).await;
    let expiring = Arc::clone(&shared);
    let key = row.key;
    let marked = crate::http::blocking(move || {
        expiring
            .db
            .deadline_job(key, expiring.ids.now_ms())
            .map_err(|e| crate::http::db_error(&e))
    })
    .await;
    if let Ok(Some(event)) = marked {
        shared.jobs.publish(&row.id, std::slice::from_ref(&event));
        shared.jobs.flag(key, Stop::Deadline);
        tracing::warn!(job = row.id, "a job passed its deadline");
    }
}

/// SD4: settles what a stopped process left (JB6, JS13, SD5), then queues
/// every queued job again in submission order. Blocking: at `build`.
///
/// # Errors
///
/// `server.db`'s.
pub(crate) fn recover(shared: &Shared) -> Result<Vec<Recovered>, crate::state::DbError> {
    let now = shared.ids.now_ms();
    let retention = shared.jobs.retention_ms();
    // JB6: the catalog's generation now, for an interrupted write job.
    let recovery = crate::catalog::generation(&shared.catalog).ok();
    let interrupted = |row: &JobRow| documents::interrupted_at(row, recovery);
    let recovered = shared.db.recover_jobs(
        &documents::stopped(Kind::Features),
        &interrupted,
        now,
        retention,
        &documents::end_data,
    )?;
    for job in &recovered {
        tracing::info!(
            job = job.row.id,
            state = job.row.state.name(),
            attempt = job.row.attempt,
            "a job the last process left was settled"
        );
    }
    let queued = shared.db.queued_jobs()?;
    let mut sched = lock(&shared.jobs.sched);
    for (row, bytes) in &queued {
        let _ = sched.push(shared.jobs.entry(row, *bytes));
    }
    Ok(recovered)
}

/// SD3 (8), M6-28, M6-56: with `jobs.reindex_on_start`, a catalog with a
/// stale entry (RX2) gets a reindex job on the write lane, owned by the
/// `system` user, unless one is queued or running already; catalog
/// matches are refused as stale until it ends (RX3, CX5), and
/// `fitment.info` says so. Blocking: at `build`, after recovery.
///
/// # Errors
///
/// `server.db`'s.
pub(crate) fn reindex_on_start(shared: &Shared) -> Result<(), crate::state::DbError> {
    let stale = match shared.catalog.info() {
        Ok(info) => info.stale,
        Err(error) => {
            tracing::warn!(
                code = error.code().name(),
                "the catalog's staleness is unknown"
            );
            return Ok(());
        }
    };
    if stale == 0 {
        return Ok(());
    }
    let request = request::normalized(&request::ReindexJob {
        refs: Vec::new(),
        all: false,
    });
    if let Some(row) = queue_system(shared, Kind::CatalogReindex, &request)? {
        tracing::info!(job = row.id, stale, "a stale catalog's reindex was queued");
    }
    Ok(())
}

/// JS9, M6-56, CX8: the daily catalog gc (`jobs.catalog_gc_at`), without
/// `versions` or `vacuum`, owned by the `system` user, unless one is
/// queued or running already. Blocking.
///
/// # Errors
///
/// `server.db`'s.
pub(crate) fn queue_daily_gc(shared: &Shared) -> Result<Option<JobRow>, crate::state::DbError> {
    let request = request::normalized(&request::GcJob {
        versions: false,
        vacuum: false,
    });
    let queued = queue_system(shared, Kind::CatalogGc, &request)?;
    if let Some(row) = &queued {
        tracing::info!(job = row.id, "the daily catalog gc was queued");
    }
    Ok(queued)
}

/// M6-56: a job of `kind` the server queues for itself, owned by the
/// `system` user, unless one of its kind is queued or running: written
/// (JS11) and queued; the dispatcher's next kick runs it.
fn queue_system(
    shared: &Shared,
    kind: Kind,
    (request, request_sha256): &(String, String),
) -> Result<Option<JobRow>, crate::state::DbError> {
    if shared.db.active_of(kind)?.is_some() {
        return Ok(None);
    }
    let (id, request_id) = (shared.ids.id("j"), shared.ids.id("r"));
    let new = crate::state::NewJob {
        id: &id,
        owner: shared.db.system_key()?,
        kind,
        request,
        request_sha256,
        idempotency: None,
        request_id: &request_id,
        created_ms: shared.ids.now_ms(),
        inputs: &[],
        cites: &[],
    };
    Ok(match shared.db.submit_job(&new)? {
        crate::state::Submitted::New(row, event) => {
            shared.jobs.publish(&row.id, std::slice::from_ref(&event));
            let _ = lock(&shared.jobs.sched).push(shared.jobs.entry(&row, 0));
            Some(row)
        }
        _ => None,
    })
}

/// JS9: the milliseconds from `now_ms` to the next `minute` of a UTC day
/// (`jobs.catalog_gc_at`), at least one.
pub(crate) fn until_minute(now_ms: i64, minute: u32) -> u64 {
    const DAY: i64 = 86_400_000;
    let at = i64::from(minute) * 60_000;
    let today = now_ms.rem_euclid(DAY);
    let wait = if at > today {
        at - today
    } else {
        DAY - today + at
    };
    u64::try_from(wait.max(1)).unwrap_or(1)
}

/// JB11: waits up to `limit` for job `row` to end, or for the drain:
/// the job as it then stands.
pub(crate) async fn wait(shared: &Arc<Shared>, row: JobRow, limit: Duration) -> JobRow {
    if row.state.terminal() || limit.is_zero() {
        return row;
    }
    let mut live = shared.jobs.channels.subscribe(row.key);
    let mut shutdown = shared.jobs.shutdown_signal();
    let until = tokio::time::Instant::now() + limit;
    let mut current = reread(shared, &row).await;
    while !current.state.terminal() && !*shutdown.borrow() {
        let event = tokio::select! {
            event = tokio::time::timeout_at(until, live.recv()) => event,
            _ = shutdown.changed() => break,
        };
        match event {
            Err(_) | Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                current = reread(shared, &current).await;
                break;
            }
            Ok(
                Ok(Live { kind: "end", .. })
                | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)),
            ) => {
                current = reread(shared, &current).await;
            }
            Ok(Ok(_)) => {}
        }
    }
    drop(live);
    current
}

/// The job's row, read again; `row` if it cannot be.
pub(crate) async fn reread(shared: &Arc<Shared>, row: &JobRow) -> JobRow {
    let (reading, key) = (Arc::clone(shared), row.key);
    crate::http::blocking(move || {
        reading
            .db
            .job_by_key(key)
            .map_err(|e| crate::http::db_error(&e))
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| row.clone())
}

/// A job's state now: for `fitment.job`'s queue position and progress.
pub(crate) fn progress(shared: &Shared, row: &JobRow) -> Option<Progress> {
    matches!(row.state, State::Running | State::Cancelling)
        .then(|| shared.jobs.channels.last_progress(row.key))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JS9: the daily gc's wait runs to the next `HH:MM` of a UTC day.
    #[test]
    fn the_daily_gc_waits_for_its_minute() {
        let minute = 3 * 60 + 30;
        assert_eq!(until_minute(0, minute), 12_600_000);
        assert_eq!(until_minute(12_600_000, minute), 86_400_000, "a day later");
        assert_eq!(until_minute(86_400_000 + 12_599_000, minute), 1_000);
    }

    /// JS2, JS3: a job takes one CPU permit and 21 times its largest
    /// input in MiB, at least one (review S13 of PR 5); an estimate above
    /// the budget takes the whole budget, so it waits for it and runs
    /// alone, never refused.
    #[test]
    fn a_jobs_permits_are_its_estimate() {
        let mut config = Config::default();
        config.jobs.memory_mib = 64;
        let jobs = Jobs::new(&config);
        let row = |key| crate::state::JobRow {
            key,
            id: format!("j_{key}"),
            owner: 1,
            owner_id: "u_1".into(),
            kind: Kind::Features,
            state: State::Queued,
            request: "{}".into(),
            request_sha256: String::new(),
            attempt: 1,
            request_id: String::new(),
            created_ms: 0,
            started_ms: None,
            finished_ms: None,
            deadline_ms: None,
            cancel_requested: false,
            exit: None,
            outcome: None,
            error_code: None,
            error_details: None,
            result_sha256: None,
            result_bytes: None,
            result_format: None,
            result: None,
            notes: "[]".into(),
            pinned: false,
            expires_ms: None,
            last_seq: 1,
            index_sha256: None,
            run: None,
            catalog_generation: None,
        };
        assert_eq!(jobs.entry(&row(1), 0).memory_mib, 1);
        assert_eq!(jobs.entry(&row(1), 40_000).memory_mib, 1);
        assert_eq!(jobs.entry(&row(1), 3 * MIB).memory_mib, 63);
        assert_eq!(
            jobs.entry(&row(1), 100 * MIB).memory_mib,
            64,
            "the whole budget"
        );
        assert_eq!(jobs.entry(&row(1), MIB).cpu, 1);
        assert_eq!(jobs.memory_free(), (64, 64));
    }

    /// JS1, JS2, JS7: a match's window is its CPU permits: `match_threads`,
    /// no more than its candidates or the budget, 1 for one candidate; its
    /// memory estimate counts each file of the window.
    #[test]
    fn a_matchs_window_is_its_permits() {
        let mut config = Config::default();
        config.jobs.cpu = 8;
        config.jobs.match_threads = 4;
        let jobs = Jobs::new(&config);
        let file = serde_json::json!({ "upload": "sha256:a", "label": "a" });
        let row = |n: usize| {
            let request = serde_json::json!({
                "socket": "sk_x@r1", "socket_document": "sha256:b",
                "files": vec![file.clone(); n], "components": [],
                "policy": { "tolerance_mm": 0.01, "tolerance_deg": 0.01, "clearance": "strict" },
                "options": { "require_fit": false, "require_reviewed": false, "require_reviewed_by": null },
                "original": null });
            crate::state::JobRow {
                kind: Kind::Match,
                request: request.to_string(),
                ..sample_row()
            }
        };
        assert_eq!(jobs.window(&row(1)), 1, "one candidate, one permit");
        assert_eq!(jobs.window(&row(3)), 3);
        assert_eq!(jobs.window(&row(40)), 4, "match_threads");
        assert_eq!(jobs.entry(&row(40), 2 * MIB).cpu, 4);
        assert_eq!(
            jobs.entry(&row(40), 2 * MIB).memory_mib,
            168,
            "4 files of 21 times 2 MiB"
        );
        config.jobs.cpu = 2;
        assert_eq!(Jobs::new(&config).window(&row(40)), 2, "the budget");
        let features = crate::state::JobRow {
            kind: Kind::Features,
            ..sample_row()
        };
        assert_eq!(jobs.window(&features), 1);
    }

    fn sample_row() -> crate::state::JobRow {
        crate::state::JobRow {
            key: 1,
            id: "j_1".into(),
            owner: 1,
            owner_id: "u_1".into(),
            kind: Kind::Features,
            state: State::Queued,
            request: "{}".into(),
            request_sha256: String::new(),
            attempt: 1,
            request_id: String::new(),
            created_ms: 0,
            started_ms: None,
            finished_ms: None,
            deadline_ms: None,
            cancel_requested: false,
            exit: None,
            outcome: None,
            error_code: None,
            error_details: None,
            result_sha256: None,
            result_bytes: None,
            result_format: None,
            result: None,
            notes: "[]".into(),
            pinned: false,
            expires_ms: None,
            last_seq: 1,
            index_sha256: None,
            run: None,
            catalog_generation: None,
        }
    }
}
