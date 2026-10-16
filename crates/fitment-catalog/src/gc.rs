//! `fitment catalog gc` and the inline gc after a change
//! (`docs/research/m5-catalog.md`, ST5, ST7, CC3, SZ4, VF4): a file under
//! `blobs/` is unlinked only inside a `BEGIN IMMEDIATE` transaction that has
//! just checked no `blob` row names it, at most 1,000 to a transaction; a
//! temporary file is removed when over an hour old. gc writes no row, so a
//! full disk never blocks its own cure.

use std::io;
use std::time::{Duration, SystemTime};

use rusqlite::TransactionBehavior;

use crate::hex::Sha256Hex;
use crate::{Catalog, Error};

/// At most this many files unlinked per transaction, so writers wait at most
/// one batch (ST5).
const UNLINK_BATCH: usize = 1000;

/// A temporary file this old is an abandoned write (ST7).
const TEMPORARY_AGE: Duration = Duration::from_secs(3600);

/// What gc did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Collected {
    /// Orphan blobs unlinked.
    pub unlinked: Vec<Sha256Hex>,
    /// Old temporary files removed, by name.
    pub temporaries: Vec<String>,
    /// Files it could not remove, with why.
    pub failed: Vec<String>,
}

impl Catalog {
    /// ST5: unlinks each of `contents`' files that no `blob` row names,
    /// checked under the write lock. Returns, for people, the files left.
    pub(crate) fn unlink(&mut self, contents: &[(Sha256Hex, u64)]) -> Vec<String> {
        let shas: Vec<Sha256Hex> = contents.iter().map(|(s, _)| s.clone()).collect();
        let (_, failed) = self.unlink_unreferenced(&shas);
        failed
    }

    /// The files of `shas` no row names, unlinked in batches under the lock;
    /// returns those unlinked and, for people, those left.
    fn unlink_unreferenced(&mut self, shas: &[Sha256Hex]) -> (Vec<Sha256Hex>, Vec<String>) {
        let mut unlinked = Vec::new();
        let mut failed = Vec::new();
        for batch in shas.chunks(UNLINK_BATCH) {
            let tx = match self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
            {
                Ok(tx) => tx,
                Err(e) => {
                    failed.extend(batch.iter().map(|s| format!("{s}: {e}")));
                    continue;
                }
            };
            for sha in batch {
                let named = tx
                    .query_row(
                        "SELECT EXISTS (SELECT 1 FROM blob WHERE sha256 = ?1)",
                        [sha.as_str()],
                        |r| r.get::<_, bool>(0),
                    )
                    .unwrap_or(true);
                if named {
                    continue;
                }
                let path = self.layout.blob(sha);
                let regular = match self.fs.symlink_metadata(&path) {
                    Ok(meta) => meta.is_file(),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) => {
                        failed.push(format!("{sha}: {e}"));
                        continue;
                    }
                };
                if regular {
                    // Windows refuses to delete a read-only file (ST4).
                    let _ = self.fs.set_readonly(&path, false);
                }
                match self.fs.remove_file(&path) {
                    Ok(()) => unlinked.push(sha.clone()),
                    Err(e) => failed.push(format!("{sha}: {e}")),
                }
            }
            // Nothing was written: the transaction held the lock only.
            let _ = tx.rollback();
        }
        (unlinked, failed)
    }

    /// `fitment catalog gc`: unlinks orphan blobs and temporary files over
    /// an hour old, then truncates the WAL (CC3); with `vacuum`, runs
    /// `VACUUM` (SZ4), which needs free space about the database's size and
    /// fails whole without it.
    ///
    /// # Errors
    ///
    /// [`Error::Linked`] for a planted link (PH2, checked again before any
    /// unlink); the file system's or the database's.
    pub fn gc(&mut self, vacuum: bool) -> Result<Collected, Error> {
        self.layout
            .check_links(self.fs.as_ref())
            .map_err(|linked| Error::Linked(linked.0))?;
        let mut candidates = Vec::new();
        for fan in self.layout.fan_outs() {
            let files = self.fs.read_dir(&fan).map_err(|e| Error::Io {
                path: fan.clone(),
                source: e,
            })?;
            for file in files {
                // A name that is not a SHA-256 is not the store's: left.
                let name = file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if let Ok(sha) = Sha256Hex::parse(name) {
                    if file.parent() == Some(fan.as_path()) && sha.fan_out() == fan_name(&fan) {
                        candidates.push(sha);
                    }
                }
            }
        }
        let (unlinked, mut failed) = self.unlink_unreferenced(&candidates);
        let temporaries = self.old_temporaries(&mut failed)?;
        self.conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        if vacuum {
            self.conn.execute_batch("VACUUM")?;
        }
        Ok(Collected {
            unlinked,
            temporaries,
            failed,
        })
    }

    /// ST7: temporary files over an hour old, removed. The age decides only
    /// liveness: a removed file fails its writer's ingest, which commits
    /// nothing.
    fn old_temporaries(&self, failed: &mut Vec<String>) -> Result<Vec<String>, Error> {
        let tmp = self.layout.tmp();
        let entries = self.fs.read_dir(&tmp).map_err(|e| Error::Io {
            path: tmp.clone(),
            source: e,
        })?;
        // The clock only judges a file's age here; nothing stores it.
        #[allow(clippy::disallowed_methods)]
        let now = SystemTime::now();
        let mut removed = Vec::new();
        for path in entries {
            let Ok(meta) = self.fs.symlink_metadata(&path) else {
                continue;
            };
            let old = meta
                .modified()
                .ok()
                .and_then(|m| now.duration_since(m).ok())
                .is_some_and(|age| age > TEMPORARY_AGE);
            if !old || meta.is_dir() {
                continue;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match self.fs.remove_file(&path) {
                Ok(()) => removed.push(name),
                Err(e) => failed.push(format!("tmp/{name}: {e}")),
            }
        }
        removed.sort();
        Ok(removed)
    }
}

/// The two hex digits a fan-out directory is named by.
fn fan_name(fan: &std::path::Path) -> &str {
    fan.file_name().and_then(|n| n.to_str()).unwrap_or_default()
}
