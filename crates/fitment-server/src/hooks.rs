//! Named hook points (docs/research/m6-api.md, OP11), under `test-support`
//! only: a test makes a route wait, panic, log or answer a planted body at
//! a named point, so timeouts, panics and the leak scan's controls are
//! shown without a sleep in the server's own code.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

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
}

/// The hook points: `info` (the handler, before its blocking work) and
/// `info.blocking` (inside it, on the blocking pool).
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

    /// Runs a blocking point's action: sleeps, panics or logs; a planted
    /// body is returned for the caller to answer.
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
        }
        None
    }

    /// Runs an async point's action, as [`Hooks::blocking`] does.
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
        }
        None
    }
}
