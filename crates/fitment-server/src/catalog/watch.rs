//! A catalog job's observer (docs/research/m6-api.md, CA2, JS8, JB8): the
//! catalog asks it whether to stop where CA2 says it asks, which reads the
//! job's one flag (JS8), and tells it of its progress, which becomes the
//! job's events: each committed file's `fitment.catalog` change (`change`),
//! each batch's generation and count of lines (`batch`), and counts as
//! transient `progress` (JB8); never a verdict (M6-51).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use fitment_catalog::document::Change;
use fitment_catalog::{Observer, Progress};

/// What a catalog job tells its runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// One file's change, as `fitment.catalog` writes it, compact (JB8's
    /// `change`).
    Change(String),
    /// A batch committed: its generation, if it wrote, and its lines
    /// (JB8's `batch`); `done` of `total` settled.
    Batch {
        /// The generation it raised (DB8).
        generation: Option<i64>,
        /// The lines it gave.
        lines: usize,
        /// Files or entries settled so far.
        done: usize,
        /// In all.
        total: usize,
    },
    /// Counts of a phase (JB8's transient `progress`): `judging` for a
    /// match's groups, `verifying` for verify's blobs, `unlinking` for gc.
    Counts {
        /// The phase.
        phase: &'static str,
        /// Done so far.
        done: usize,
        /// In all.
        total: usize,
    },
}

/// The observer a catalog job passes (CA2): its stop flag and where its
/// events go.
pub struct Watch<'a> {
    stop: &'a AtomicBool,
    tell: &'a (dyn Fn(Event) + Sync),
    /// What the last batch had settled.
    settled: Mutex<usize>,
}

impl<'a> Watch<'a> {
    /// A watch on `stop`, telling `tell`.
    pub fn new(stop: &'a AtomicBool, tell: &'a (dyn Fn(Event) + Sync)) -> Self {
        Self {
            stop,
            tell,
            settled: Mutex::new(0),
        }
    }
}

impl std::fmt::Debug for Watch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watch").finish_non_exhaustive()
    }
}

impl Observer for Watch<'_> {
    fn cancelled(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn progress(&self, event: Progress<'_>) {
        let told = match event {
            Progress::Ingested(line) => {
                Event::Change(serde_json::to_string(&Change::of(line)).unwrap_or_default())
            }
            Progress::Committed {
                generation,
                done,
                total,
            } => {
                let mut settled = self
                    .settled
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let lines = done.saturating_sub(*settled);
                *settled = done;
                Event::Batch {
                    generation,
                    lines,
                    done,
                    total,
                }
            }
            Progress::Judged { done, total } => Event::Counts {
                phase: "judging",
                done,
                total,
            },
            Progress::Verifying { done, total, .. } => Event::Counts {
                phase: "verifying",
                done,
                total,
            },
            Progress::Unlinked { done, total } => Event::Counts {
                phase: "unlinking",
                done,
                total,
            },
            _ => return,
        };
        (self.tell)(told);
    }
}
