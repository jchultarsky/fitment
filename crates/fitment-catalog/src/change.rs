//! Replace, delete and `prune --revisions` (`docs/research/m5-catalog.md`,
//! CR1 to CR7, CR10, CR11, ST5, IN3, IN9): each one `BEGIN IMMEDIATE`
//! transaction that decides on what it reads (DB2), then an inline gc that
//! unlinks the files it released, each under ST5's rule.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension as _, Transaction, TransactionBehavior, params};

use crate::blobs::Placed;
use crate::fs::{Fs, Point};
use crate::hex::Sha256Hex;
use crate::ids::{Id, Reference};
use crate::ingest::{
    Ingested, Input, Outcome, PathBytes, Prepared, Summary, file_name, insert_features,
    insert_properties, insert_tokens,
};
use crate::layout::Layout;
use crate::observe::{NoObserver, Observer};
use crate::preimage::Status;
use crate::snapshot::{BadName, Named, Reader};
use crate::{Catalog, Error, schema};

/// Why a change names no entry it may change (ID7, ID9; exit 2).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Unnamed {
    /// The reference names nothing, or several.
    #[error("{0}")]
    Name(BadName),
    /// The id's entry is deleted.
    #[error("{0} is deleted")]
    Deleted(String),
    /// `ID@rN` naming a revision that is not the current one (ID9).
    #[error("{id} is at r{current}, not r{asked}")]
    NotCurrent {
        /// The id.
        id: String,
        /// Its current revision.
        current: u32,
        /// The one named.
        asked: u32,
    },
    /// A content reference where only ids are taken (`delete`, ID7).
    #[error("{0}: delete takes ids, not content references")]
    NotAnId(String),
}

/// What `replace` did.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Replaced {
    /// A new revision; the old one's bytes kept (CR3).
    Replaced {
        /// The entry's id.
        id: String,
        /// The revision and content replaced.
        from: (u32, Sha256Hex),
        /// The new revision and content.
        to: (u32, Sha256Hex),
        /// What the gate made of the new file.
        summary: Summary,
    },
    /// The file holds the entry's own bytes (CR4).
    Unchanged {
        /// The entry's id.
        id: String,
        /// Its revision.
        revision: u32,
        /// Its content.
        sha256: Sha256Hex,
    },
}

/// Why `replace` changed nothing (CR4).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum NotReplaced {
    /// The reference names no entry it may replace (exit 2).
    Unnamed(Unnamed),
    /// The bytes are another entry's (exit 2).
    Duplicate {
        /// That entry's id.
        of: String,
    },
    /// The file was refused: by the gate, its size or its type (exit 3);
    /// or could not be read (exit 1). As ingest reports it, without an id.
    /// A blob that cannot be stored is an [`Error::Io`].
    File(Box<Ingested>),
}

/// One entry deleted (CR1).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Deleted {
    /// The place, among the references `delete` was given, of the first
    /// that named it: its change's `input` (CU1).
    pub named: usize,
    /// Its id.
    pub id: String,
    /// The revision it was at.
    pub revision: u32,
    /// Its content.
    pub sha256: Sha256Hex,
}

/// What `delete` did.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Deletion {
    /// The entries deleted, in the order named.
    pub deleted: Vec<Deleted>,
    /// What their deletion released.
    pub released: Released,
}

/// One revision pruned (CR11).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Pruned {
    /// Its id.
    pub id: String,
    /// The revision.
    pub revision: u32,
    /// Its content.
    pub sha256: Sha256Hex,
    /// The bytes released, if its content was released with it; for a
    /// content several pruned revisions name, with the first only.
    pub released: Option<u64>,
}

/// What a change released, and what its inline gc could not unlink.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Released {
    /// Contents whose blob rows went, with their sizes.
    pub contents: Vec<(Sha256Hex, u64)>,
    /// Files left as orphans: `fitment catalog gc` removes them (CR1).
    pub orphans: Vec<String>,
}

/// IN3 and IN9 for `contents` after a change: derived rows go where no
/// current revision of a live entry names the content; the blob row goes
/// where no current or kept revision of a live entry does. Returns the
/// contents released, with their sizes.
pub(crate) fn settle(
    tx: &Connection,
    contents: &BTreeSet<String>,
) -> rusqlite::Result<Vec<(Sha256Hex, u64)>> {
    let mut released = Vec::new();
    for sha in contents {
        let current: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM live WHERE sha256 = ?1)",
            [sha],
            |r| r.get(0),
        )?;
        if !current {
            tx.execute(
                "DELETE FROM posting WHERE content = (SELECT key FROM blob WHERE sha256 = ?1)",
                [sha],
            )?;
            tx.execute("DELETE FROM features WHERE sha256 = ?1", [sha])?;
            tx.execute("DELETE FROM properties WHERE sha256 = ?1", [sha])?;
        }
        let held: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM revision r JOIN entry e ON e.key = r.entry
                            WHERE r.sha256 = ?1 AND e.deleted_generation IS NULL
                              AND r.pruned_generation IS NULL)",
            [sha],
            |r| r.get(0),
        )?;
        if held {
            continue;
        }
        let bytes: Option<i64> = tx
            .query_row(
                "DELETE FROM blob WHERE sha256 = ?1 RETURNING bytes",
                [sha],
                |r| r.get(0),
            )
            .optional()?;
        if let (Some(bytes), Ok(sha)) = (bytes, Sha256Hex::parse(sha)) {
            released.push((sha, u64::try_from(bytes).unwrap_or_default()));
        }
    }
    Ok(released)
}

fn op(tx: &Connection, g: i64, n: i64, kind: &str, entry: i64, sha: &str) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO op (generation, n, kind, entry, sha256, fitment_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![g, n, kind, entry, sha, env!("CARGO_PKG_VERSION")],
    )?;
    Ok(())
}

/// The live entry `reference` names, its current revision (ID9).
pub(crate) fn live_entry(
    tx: &Connection,
    reference: &Reference,
) -> Result<Result<Named, Unnamed>, Error> {
    let named = match (Reader { conn: tx }).resolve(reference)? {
        Ok(named) => named,
        Err(bad) => return Ok(Err(Unnamed::Name(bad))),
    };
    let Some(current) = named.current.or(named.id_current) else {
        return Ok(Err(Unnamed::Deleted(named.id)));
    };
    if named.current != Some(named.revision) {
        return Ok(Err(Unnamed::NotCurrent {
            id: named.id,
            current,
            asked: named.revision,
        }));
    }
    Ok(Ok(named))
}

impl Catalog {
    /// `fitment catalog replace`: gives the entry `reference` names the
    /// bytes of `file` as a new revision, in one transaction (CR3). The old
    /// content's derived rows go; its bytes are kept for `get ID@rN` until
    /// `prune --revisions` (CR11). The path is an [`Input`] named by the
    /// path as given and its final component, read through the catalog's
    /// seam (CU3): one path with [`Catalog::replace_input`].
    ///
    /// # Errors
    ///
    /// [`Error::InsideCatalog`] for a file of the catalog's own (ST10); the
    /// database's, the lock's or a blob's; a refusal is the inner
    /// [`NotReplaced`], and nothing changed.
    pub fn replace(
        &mut self,
        reference: &Reference,
        file: &Path,
    ) -> Result<Result<Replaced, NotReplaced>, Error> {
        // ST10: never a file of the catalog's own.
        if crate::ingest::inside(self.root(), file) {
            return Err(Error::InsideCatalog(file.to_owned()));
        }
        let bytes = PathBytes {
            fs: Arc::clone(&self.fs),
            path: file.to_owned(),
        };
        let (input, name) = (file.to_string_lossy(), file_name(file));
        let input = Input::at(&bytes, &input, &name);
        self.replace_input(reference, &input, None, &NoObserver)
    }

    /// CA1: gives the entry `reference` names the bytes of `input`, at
    /// most `max_bytes` of them (SZ1; `None` for the catalog's
    /// [`Options`](crate::Options)), for this call alone, as a new
    /// revision, as [`Catalog::replace`] does; the input's own id, if any,
    /// is not read (the entry keeps its id). `observer` is asked before the
    /// file is read and before its blob is put and the transaction begins
    /// (CA2), so a cancel leaves nothing.
    ///
    /// # Errors
    ///
    /// [`Error::Cancelled`] when the observer stops it, nothing changed;
    /// the database's, the lock's or a blob's; a refusal is the inner
    /// [`NotReplaced`], and nothing changed.
    pub fn replace_input(
        &mut self,
        reference: &Reference,
        input: &Input<'_>,
        max_bytes: Option<u64>,
        observer: &dyn Observer,
    ) -> Result<Result<Replaced, NotReplaced>, Error> {
        // ID9 first: a reference that names no entry it may replace is
        // refused before the file is read; the transaction checks again.
        let named = match live_entry(&self.conn, reference)? {
            Ok(named) => named,
            Err(unnamed) => return Ok(Err(NotReplaced::Unnamed(unnamed))),
        };
        if observer.cancelled() {
            return Err(Error::Cancelled);
        }
        // The file is read and gated outside the lock (DB3), and a refused
        // one changes nothing (CR4).
        let placeholder = Id::parse("x").map_err(|_| Error::Corrupt("an id"))?;
        let cap = max_bytes.unwrap_or(self.options.max_bytes);
        let mut prepared = match crate::ingest::read_input(input, Some(&placeholder), cap) {
            Ok(p) => p,
            Err(mut failed) => {
                failed.id = None;
                return Ok(Err(NotReplaced::File(failed)));
            }
        };
        if prepared.sha256 == named.sha256 {
            // CR4: the entry's own bytes, whatever the gate made of them.
            return Ok(Ok(Replaced::Unchanged {
                id: named.id,
                revision: named.revision,
                sha256: named.sha256,
            }));
        }
        if prepared.status == Status::Refused {
            let mut failed = Ingested::failed(
                input.given(),
                None,
                Outcome::Refused {
                    reason: prepared
                        .summary
                        .first_refusal
                        .clone()
                        .map_or_else(|| "refused".into(), |r| format!("refused: {r}")),
                },
            );
            failed.sha256 = Some(prepared.sha256);
            failed.summary = Some(prepared.summary);
            return Ok(Err(NotReplaced::File(Box::new(failed))));
        }
        // A first look, outside the lock, so a replace surely refused puts
        // no blob; the transaction decides again.
        let likely = !self
            .conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM live WHERE sha256 = ?1)",
                [prepared.sha256.as_str()],
                |r| r.get::<_, bool>(0),
            )
            .unwrap_or(true);
        if observer.cancelled() {
            return Err(Error::Cancelled);
        }
        if likely {
            let placed = self
                .blobs()
                .put(&prepared.sha256, &prepared.bytes)
                .map_err(|e| Error::Io {
                    path: self.layout.blob(&prepared.sha256),
                    source: e,
                })?;
            prepared.replaced_blob = placed == Placed::Replaced;
        }
        // The old content stays held by its kept revision: nothing is
        // released. A refusal after the put leaves the new file unnamed,
        // and the inline gc unlinks it (CR4: nothing changed).
        let outcome = self.replace_in(reference, &prepared);
        if likely && !matches!(outcome, Ok(Ok(Replaced::Replaced { .. }))) {
            let size = u64::try_from(prepared.bytes.len()).unwrap_or(u64::MAX);
            let _ = self.unlink(&[(prepared.sha256.clone(), size)]);
        }
        outcome
    }

    fn replace_in(
        &mut self,
        reference: &Reference,
        p: &Prepared,
    ) -> Result<Result<Replaced, NotReplaced>, Error> {
        let blobs = crate::blobs::Blobs {
            layout: &self.layout,
            fs: self.fs.as_ref(),
        };
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let named = match live_entry(&tx, reference)? {
            Ok(named) => named,
            Err(unnamed) => return Ok(Err(NotReplaced::Unnamed(unnamed))),
        };
        if named.sha256 == p.sha256 {
            let same = Replaced::Unchanged {
                id: named.id,
                revision: named.revision,
                sha256: named.sha256,
            };
            return Ok(Ok(same));
        }
        let elsewhere: Option<String> = tx
            .query_row(
                "SELECT id FROM live WHERE sha256 = ?1",
                [p.sha256.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(of) = elsewhere {
            let of = crate::snapshot::stored_id(of)?;
            return Ok(Err(NotReplaced::Duplicate { of }));
        }
        // ST6, then ST3's directory sync before the commit.
        let size = u64::try_from(p.bytes.len()).unwrap_or(u64::MAX);
        if !blobs.present(&p.sha256, size) {
            blobs.put(&p.sha256, &p.bytes).map_err(|e| Error::Io {
                path: blobs.layout.blob(&p.sha256),
                source: e,
            })?;
        }
        blobs.sync_fan_out(&p.sha256).map_err(|e| Error::Io {
            path: blobs.layout.fan_out(&p.sha256),
            source: e,
        })?;
        let g = schema::next_generation(&tx)?;
        let (entry, revision) = supersede(&tx, &named, p, g)?;
        insert_features(&tx, p)?;
        if let Some(tokens) = &p.tokens {
            let key: i64 = tx.query_row(
                "SELECT key FROM blob WHERE sha256 = ?1",
                [p.sha256.as_str()],
                |r| r.get(0),
            )?;
            insert_tokens(&tx, &p.sha256, tokens, key)?;
        }
        if let Some(stated) = &p.properties {
            insert_properties(&tx, &p.sha256, stated)?;
        }
        op(&tx, g, 0, "replace", entry, p.sha256.as_str())?;
        // CR3 and IN9: the old content's derived rows go; its blob row is
        // held by the revision just superseded, which is kept.
        settle(&tx, &BTreeSet::from([named.sha256.as_str().to_owned()]))?;
        commit_at_points(tx, blobs.fs, blobs.layout)?;
        let done = Replaced::Replaced {
            id: named.id,
            from: (named.revision, named.sha256),
            to: (revision, p.sha256.clone()),
            summary: p.summary.clone(),
        };
        Ok(Ok(done))
    }

    /// `fitment catalog delete`: tombstones the entries `references` name,
    /// all or none (CR1, CR2): their contents' rows go, kept revisions'
    /// included, and the inline gc unlinks the files.
    ///
    /// # Errors
    ///
    /// The database's or the lock's; a reference that names no live entry
    /// is the inner [`Unnamed`] list, and nothing changed.
    pub fn delete(
        &mut self,
        references: &[Reference],
    ) -> Result<Result<Deletion, Vec<Unnamed>>, Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut named: Vec<(usize, Named)> = Vec::new();
        let mut unnamed = Vec::new();
        for (at, reference) in references.iter().enumerate() {
            if let Reference::Sha256(prefix) = reference {
                unnamed.push(Unnamed::NotAnId(format!("sha256:{prefix}")));
                continue;
            }
            match live_entry(&tx, reference)? {
                Ok(n) if !named.iter().any(|(_, m)| m.sha256 == n.sha256) => named.push((at, n)),
                Ok(_) => {}
                Err(u) => unnamed.push(u),
            }
        }
        if !unnamed.is_empty() {
            return Ok(Err(unnamed));
        }
        if named.is_empty() {
            return Ok(Ok(Deletion {
                deleted: Vec::new(),
                released: Released::default(),
            }));
        }
        let g = schema::next_generation(&tx)?;
        let mut contents = BTreeSet::new();
        let mut deleted = Vec::new();
        for (n, (at, entry)) in (0_i64..).zip(&named) {
            let key = tombstone(&tx, entry, g)?;
            let mut stmt = tx.prepare("SELECT DISTINCT sha256 FROM revision WHERE entry = ?1")?;
            let held: Vec<String> = stmt
                .query_map([key], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            contents.extend(held);
            op(&tx, g, n, "delete", key, entry.sha256.as_str())?;
            deleted.push(Deleted {
                named: *at,
                id: entry.id.clone(),
                revision: entry.revision,
                sha256: entry.sha256.clone(),
            });
        }
        let released = settle(&tx, &contents)?;
        commit_at_points(tx, self.fs.as_ref(), &self.layout)?;
        let orphans = self.unlink(&released);
        Ok(Ok(Deletion {
            deleted,
            released: Released {
                contents: released,
                orphans,
            },
        }))
    }

    /// `fitment catalog prune --revisions` (CR11): releases the bytes of
    /// every kept revision of a live entry, in one transaction, then the
    /// inline gc unlinks them. Pruning twice prunes nothing the second time.
    ///
    /// # Errors
    ///
    /// The database's or the lock's.
    pub fn prune_revisions(&mut self) -> Result<(Vec<Pruned>, Released), Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut stmt = tx.prepare(
            "SELECT r.key, e.key, e.id, r.revision, r.sha256 FROM revision r
             JOIN entry e ON e.key = r.entry
             WHERE e.deleted_generation IS NULL AND r.superseded_generation IS NOT NULL
               AND r.pruned_generation IS NULL
             ORDER BY e.id_fold, e.id, r.revision",
        )?;
        let kept: Vec<(i64, i64, String, u32, String)> = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<Result<_, _>>()?;
        drop(stmt);
        if kept.is_empty() {
            return Ok((Vec::new(), Released::default()));
        }
        // Every key checked before anything is written (PH1).
        if kept.iter().any(|k| Sha256Hex::parse(&k.4).is_err()) {
            return Err(Error::Corrupt("a revision's content key"));
        }
        if kept.iter().any(|k| Id::parse(&k.2).is_err()) {
            return Err(Error::Corrupt("an id"));
        }
        let g = schema::next_generation(&tx)?;
        let mut contents = BTreeSet::new();
        for (n, (key, entry, _, _, sha)) in (0_i64..).zip(&kept) {
            tx.execute(
                "UPDATE revision SET pruned_generation = ?1 WHERE key = ?2",
                params![g, key],
            )?;
            op(&tx, g, n, "prune", *entry, sha)?;
            contents.insert(sha.clone());
        }
        let released = settle(&tx, &contents)?;
        commit_at_points(tx, self.fs.as_ref(), &self.layout)?;
        let mut pruned = Vec::new();
        let mut counted = BTreeSet::new();
        for (_, _, id, revision, sha) in kept {
            let sha256 = Sha256Hex::parse(&sha).map_err(|_| Error::Corrupt("a content key"))?;
            let id = crate::snapshot::stored_id(id)?;
            // A content two pruned revisions name is released, and counted,
            // once: with the first.
            let freed = released
                .iter()
                .find(|(s, _)| *s == sha256 && counted.insert(s.clone()))
                .map(|(_, b)| *b);
            pruned.push(Pruned {
                id,
                revision,
                sha256,
                released: freed,
            });
        }
        let orphans = self.unlink(&released);
        Ok((
            pruned,
            Released {
                contents: released,
                orphans,
            },
        ))
    }
}

/// Commits `tx`, telling the seam of the points around it.
fn commit_at_points(tx: Transaction<'_>, fs: &dyn Fs, layout: &Layout) -> Result<(), Error> {
    fs.point(Point::BeforeCommit).map_err(|e| Error::Io {
        path: layout.database(),
        source: e,
    })?;
    tx.commit()?;
    let _ = fs.point(Point::AfterCommit);
    Ok(())
}

/// CR3: the entry's current revision superseded at `g`, and a new one for
/// `p`'s content, the id's next (ID6). Returns the entry and the revision.
fn supersede(tx: &Connection, named: &Named, p: &Prepared, g: i64) -> rusqlite::Result<(i64, u32)> {
    let (entry, fold): (i64, String) = tx.query_row(
        "SELECT r.entry, r.id_fold FROM revision r JOIN entry e ON e.key = r.entry
         WHERE r.sha256 = ?1 AND r.superseded_generation IS NULL AND e.deleted_generation IS NULL",
        [named.sha256.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    tx.execute(
        "UPDATE revision SET superseded_generation = ?1
         WHERE entry = ?2 AND superseded_generation IS NULL",
        params![g, entry],
    )?;
    let revision: u32 = tx.query_row(
        "UPDATE name SET last_revision = last_revision + 1 WHERE id_fold = ?1
         RETURNING last_revision",
        [&fold],
        |r| r.get(0),
    )?;
    let size = i64::try_from(p.bytes.len()).unwrap_or(i64::MAX);
    tx.execute(
        "INSERT INTO blob (sha256, bytes, added_generation) VALUES (?1, ?2, ?3)
         ON CONFLICT (sha256) DO NOTHING",
        params![p.sha256.as_str(), size, g],
    )?;
    tx.execute(
        "INSERT INTO revision (entry, id_fold, revision, sha256, file_name, added_generation)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![entry, &fold, revision, p.sha256.as_str(), &p.file_name, g],
    )?;
    Ok((entry, revision))
}

/// CR1's steps 1 and 2: the entry's deletion and its current revision's
/// supersession, at `g`. Returns the entry's key.
fn tombstone(tx: &Connection, named: &Named, g: i64) -> rusqlite::Result<i64> {
    let entry: i64 = tx.query_row(
        "SELECT r.entry FROM revision r JOIN entry e ON e.key = r.entry
         WHERE r.sha256 = ?1 AND r.superseded_generation IS NULL AND e.deleted_generation IS NULL",
        [named.sha256.as_str()],
        |r| r.get(0),
    )?;
    tx.execute(
        "UPDATE entry SET deleted_generation = ?1 WHERE key = ?2",
        params![g, entry],
    )?;
    tx.execute(
        "UPDATE revision SET superseded_generation = ?1
         WHERE entry = ?2 AND superseded_generation IS NULL",
        params![g, entry],
    )?;
    Ok(entry)
}
