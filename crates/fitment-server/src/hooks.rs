//! Named hook points (docs/research/m6-api.md, OP11), under `test-support`
//! only: a test makes a route or a job wait, pause at a gate it opens,
//! panic, abort the process, log or answer a planted body at a named point,
//! so timeouts, panics, stops, restarts and the leak scan's controls are
//! shown without a sleep in the test's own code.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

/// A gate a hook point pauses at until the test opens it (VT41, VT42,
/// VT49): it counts who reached it, so a test waits for that, never for a
/// time.
#[derive(Debug, Clone, Default)]
pub struct Gate(Arc<(Mutex<(usize, bool)>, Condvar)>);

impl Gate {
    /// A closed gate.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many have reached it.
    pub fn reached(&self) -> usize {
        self.0.0.lock().unwrap_or_else(PoisonError::into_inner).0
    }

    /// Opens it: every waiter and every later arrival passes.
    pub fn open(&self) {
        let (lock, condvar) = &*self.0;
        lock.lock().unwrap_or_else(PoisonError::into_inner).1 = true;
        condvar.notify_all();
    }

    /// Blocks until `n` have reached it, at most `limit`: false if fewer
    /// did.
    pub fn wait_reached(&self, n: usize, limit: Duration) -> bool {
        let (lock, condvar) = &*self.0;
        let guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = condvar
            .wait_timeout_while(guard, limit, |(reached, _)| *reached < n)
            .unwrap_or_else(PoisonError::into_inner);
        guard.0 >= n
    }

    /// As [`Gate::wait_reached`], on the blocking pool.
    pub async fn arrived(&self, n: usize, limit: Duration) -> bool {
        let gate = self.clone();
        tokio::task::spawn_blocking(move || gate.wait_reached(n, limit))
            .await
            .unwrap_or(false)
    }

    /// Arrives, and blocks until the gate is open.
    fn pass(&self) {
        let (lock, condvar) = &*self.0;
        let mut guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        guard.0 += 1;
        condvar.notify_all();
        while !guard.1 {
            guard = condvar.wait(guard).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// What a hook does when its point is reached.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Action {
    /// Waits this long (a `tokio` sleep at an async point, a thread sleep
    /// at a blocking one).
    Delay(Duration),
    /// Panics with this message.
    Panic(String),
    /// Answers `200` with this body instead (VT34's planted response).
    Plant(String),
    /// Writes this line to the log (VT34's planted log line).
    Log(String),
    /// Waits at the gate until the test opens it.
    Pause(Gate),
    /// Ends the process at once, as a kill would (VT10): a child's only.
    Abort,
}

impl Action {
    /// An action of the configuration's `[test] hooks` (OP11): `abort`,
    /// `panic` or `delay:<ms>`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "abort" => Some(Self::Abort),
            "panic" => Some(Self::Panic("a configured hook's panic".into())),
            _ => text
                .strip_prefix("delay:")
                .and_then(|ms| ms.parse::<u64>().ok())
                .map(|ms| Self::Delay(Duration::from_millis(ms))),
        }
    }
}

/// The hook points: `info` (the handler, before its blocking work),
/// `info.blocking` (inside it, on the blocking pool), `auth.hash` (inside
/// an argon2 slot, on the blocking pool: AU2), `upload.chunk` (before an
/// upload's next chunk is read), `socket.parse` (inside one of HX8's
/// parse slots, on the blocking pool), `stored.read` (inside one of the
/// stored-bytes read slots, on the blocking pool), and a job's: `job.submitted`,
/// `job.dispatched` (async), `job.read`, `job.ran`, `job.stored`,
/// `job.committing` and `job.committed` (on the blocking pool), and a
/// match's `match.window` (before each window) and `match.judge` (before
/// each candidate's judgment, inside its caught panic).
#[derive(Debug, Default)]
pub struct Hooks {
    points: Mutex<BTreeMap<String, Action>>,
}

impl Hooks {
    /// Sets the action at `point`, for every request until cleared.
    pub fn set(&self, point: &str, action: Action) {
        self.points
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(point.to_owned(), action);
    }

    /// Clears `point`.
    pub fn clear(&self, point: &str) {
        self.points
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(point);
    }

    /// The action at `point`, if any.
    pub fn at(&self, point: &str) -> Option<Action> {
        self.points
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(point)
            .cloned()
    }

    /// The configuration's `[test] hooks` (OP11), set at a start.
    pub fn configure(&self, hooks: &BTreeMap<String, String>) {
        for (point, action) in hooks {
            if let Some(action) = Action::parse(action) {
                self.set(point, action);
            } else {
                tracing::warn!(point, "an unknown hook action was ignored");
            }
        }
    }

    /// Runs a blocking point's action: sleeps, pauses, panics, aborts or
    /// logs; a planted body is returned for the caller to answer.
    ///
    /// # Panics
    ///
    /// When the action is a panic: that is its purpose.
    #[allow(clippy::panic)]
    pub fn blocking(&self, point: &str) -> Option<String> {
        match self.at(point)? {
            Action::Delay(duration) => std::thread::sleep(duration),
            Action::Panic(message) => panic!("{message}"),
            Action::Log(line) => tracing::info!(hook = point, "{line}"),
            Action::Plant(body) => return Some(body),
            Action::Pause(gate) => gate.pass(),
            Action::Abort => std::process::abort(),
        }
        None
    }

    /// Runs an async point's action, as [`Hooks::blocking`] does; a pause
    /// waits on the blocking pool.
    ///
    /// # Panics
    ///
    /// When the action is a panic.
    #[allow(clippy::panic)]
    pub async fn reached(&self, point: &str) -> Option<String> {
        match self.at(point)? {
            Action::Delay(duration) => tokio::time::sleep(duration).await,
            Action::Panic(message) => panic!("{message}"),
            Action::Log(line) => tracing::info!(hook = point, "{line}"),
            Action::Plant(body) => return Some(body),
            Action::Pause(gate) => {
                let _ = tokio::task::spawn_blocking(move || gate.pass()).await;
            }
            Action::Abort => std::process::abort(),
        }
        None
    }
}
