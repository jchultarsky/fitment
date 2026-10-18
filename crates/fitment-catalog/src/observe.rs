//! CA2 (`docs/research/m6-api.md`): what a long piece of the catalog's
//! work tells, and asks, whoever runs it. The server's jobs stop on a
//! cancel or a deadline and show progress through it; the CLI passes
//! [`NoObserver`], so every document is what it was.
//!
//! [`Observer::cancelled`] is asked before each file and each batch commit
//! (ingest, reindex), before each group a judging thread takes (a catalog
//! match), between phases and before each blob read (verify), and between
//! unlink batches (gc). [`Progress`] carries counts, each committed file's
//! line and each batch's generation: never anything a result depends on,
//! and never a verdict.

use crate::ingest::Ingested;

/// What a piece of the catalog's work tells and asks its runner.
pub trait Observer: Sync {
    /// Whether the work is to stop: a cancelled match judges nothing more
    /// and returns [`crate::NotRun::Cancelled`]; a cancelled ingest,
    /// reindex or gc keeps what it committed or unlinked and says so; a
    /// cancelled verify returns [`crate::Error::Cancelled`].
    fn cancelled(&self) -> bool;

    /// Told of the work's progress. It may be told from several threads
    /// at once (a match judging on several).
    fn progress(&self, event: Progress<'_>);
}

/// An observer that never cancels and hears nothing: what the existing
/// entry points pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoObserver;

impl Observer for NoObserver {
    fn cancelled(&self) -> bool {
        false
    }

    fn progress(&self, _: Progress<'_>) {}
}

/// One step of the work's progress.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum Progress<'a> {
    /// Ingest: one file's line, once its batch committed.
    Ingested(&'a Ingested),
    /// Ingest or reindex: a batch committed, at `generation` if it wrote a
    /// row; `done` of `total` files or entries are now settled.
    Committed {
        /// The generation the batch raised, if it wrote anything (DB8).
        generation: Option<i64>,
        /// Files or entries settled so far.
        done: usize,
        /// Files or entries in all.
        total: usize,
    },
    /// A catalog match: groups judged so far, of `total` (QR6).
    Judged {
        /// Groups judged.
        done: usize,
        /// Groups in all.
        total: usize,
    },
    /// Verify: a phase begun, or `done` of its `total` blobs read.
    Verifying {
        /// `database`, `blobs`, `rows`, `files` or `deep`.
        phase: &'static str,
        /// Blobs read so far in the phase.
        done: usize,
        /// Blobs the phase reads, 0 for a phase that reads none.
        total: usize,
    },
    /// gc: files looked at so far, of `total` candidates.
    Unlinked {
        /// Candidates looked at.
        done: usize,
        /// Candidates in all.
        total: usize,
    },
}
