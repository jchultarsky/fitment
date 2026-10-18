//! The store's part of JS9's sweep (docs/research/m6-api.md, JS9, OB2,
//! OB4, UP5): expired uploads deleted, then the candidates (objects whose
//! naming row went, recorded by `server.db`'s triggers) looked at, one no
//! row names unlinked inside the `BEGIN IMMEDIATE` that has just found
//! none (ST5), in short batches, so the sweep's work follows what changed,
//! never the store's size, and an upload waits at most one batch (PR 4's
//! review); then the files of the store that no row names (a crash between
//! an upload's rename and its commit), and temporary files older than any
//! upload lives. Blocking (SV3). PR 5's sweep adds the jobs, which hold
//! objects too.

use std::time::Duration;

use super::{Db, DbError, Objects};

/// At most this many candidates looked at, and files checked, per
/// transaction, so an upload waits at most one short batch (ST5's 1,000 at
/// most).
pub const UNLINK_BATCH: u32 = 256;

/// At most this many expired upload rows deleted per transaction.
pub const EXPIRE_BATCH: u32 = 500;

/// What a sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Swept {
    /// Upload rows expired (UP5).
    pub expired: usize,
    /// Objects no row named, unlinked with their rows (OB4).
    pub unlinked: Vec<String>,
    /// Objects whose unlink failed, left for the next sweep.
    pub failed: Vec<String>,
    /// Files without a row, unlinked.
    pub orphans: usize,
    /// Temporary files older than an upload lives, removed.
    pub temporaries: usize,
}

/// Sweeps the store at `now_ms`: temporary files older than `upload_life`
/// are abandoned, since no upload lives longer (HX3's `upload_minutes`).
///
/// # Errors
///
/// SQLite's; the file system's errors are counted, never fatal.
pub fn sweep(
    db: &Db,
    objects: &Objects,
    now_ms: i64,
    upload_life: Duration,
) -> Result<Swept, DbError> {
    let mut swept = Swept::default();
    loop {
        let expired = db.expire_uploads(now_ms, EXPIRE_BATCH)?;
        swept.expired += expired;
        if expired < usize::try_from(EXPIRE_BATCH).unwrap_or(usize::MAX) {
            break;
        }
    }
    loop {
        let batch = db.unlink_unnamed(UNLINK_BATCH, |sha| objects.unlink(sha))?;
        let done = batch.looked < usize::try_from(UNLINK_BATCH).unwrap_or(usize::MAX);
        swept.unlinked.extend(batch.gone);
        // A failed unlink stays a candidate: the next sweep tries it again.
        let stuck = !batch.failed.is_empty();
        swept.failed.extend(batch.failed);
        if done || stuck {
            break;
        }
    }
    match objects.files() {
        Ok(files) => {
            for batch in files.chunks(usize::try_from(UNLINK_BATCH).unwrap_or(1000)) {
                swept.orphans += db.unlink_orphans(batch, |sha| objects.unlink(sha))?;
            }
        }
        Err(error) => tracing::warn!(kind = ?error.kind(), "the store's files could not be listed"),
    }
    match objects.clear_tmp(Some(upload_life)) {
        Ok(n) => swept.temporaries = n,
        Err(error) => tracing::warn!(kind = ?error.kind(), "state:tmp could not be listed"),
    }
    Ok(swept)
}
