//! `fitment catalog reindex` (`docs/research/m5-catalog.md`, RX4 to RX9,
//! section 7.7) and `gc --versions` (RX8): rows of this build's versions
//! written where they are missing, batch by batch, never a row of another
//! version written, rewritten or deleted but an older one beside its
//! current one.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use fitment_core::{
    EXTRACTOR_VERSION, IndexTokens, PROPERTIES_VERSION, PartSignature, SIGNATURE_VERSION,
    StatedProperties,
};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};

use crate::blobs::BadBlob;
use crate::change::{Unnamed, live_entry};
use crate::fs::Point;
use crate::hex::Sha256Hex;
use crate::ids::{Id, Reference};
use crate::ingest::{
    BATCH_FILES, BATCH_TIME, Prepared, derive, insert_features, insert_properties, insert_tokens,
};
use crate::preimage::Status;
use crate::snapshot::{BadRow, EntryStatus, Listed, check};
use crate::{Catalog, Error, schema};

/// Which entries `reindex` processes (RX9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every stale entry: the default.
    Stale,
    /// These entries, current or not; each must name a live entry's
    /// current revision (ID9).
    Named(Vec<Reference>),
    /// Every live entry.
    All,
}

/// What `reindex` did to one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Rebuilt {
    /// Nothing of this build's versions was missing.
    Current,
    /// The missing rows were written; `blob_read` if its file was read
    /// (an extractor or properties rise), not for a signature rise.
    Written {
        /// Whether the entry's file was read.
        blob_read: bool,
    },
    /// Its file is missing or damaged, or its stored document fails its
    /// check: nothing written, and it stays stale (RX5).
    Damaged(String),
    /// Its new features document is too large to store; it stays stale.
    TooLarge,
    /// It was replaced or deleted while `reindex` ran: that change stands.
    Changed,
    /// Its batch failed: nothing of it was written.
    Failed(String),
}

/// One entry's line (section 7.7).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Reindexed {
    /// The id, as stored.
    pub id: String,
    /// Its current revision.
    pub revision: u32,
    /// Its content.
    pub sha256: Sha256Hex,
    /// Its status before.
    pub before: Option<EntryStatus>,
    /// Whether it was searched before: accepted by the newest gate that
    /// read it.
    pub searched_before: bool,
    /// Its status after.
    pub after: Option<EntryStatus>,
    /// What was done.
    pub rebuilt: Rebuilt,
    /// Its first refusal as `code: message`, when quarantined after.
    pub first_refusal: Option<String>,
}

impl Reindexed {
    /// RX6: searched before, quarantined now, by rows this run wrote.
    pub fn left_search(&self) -> bool {
        self.written() && self.searched_before && self.after == Some(EntryStatus::Quarantined)
    }

    /// Not searched before, searched now, by rows this run wrote.
    pub fn joined_search(&self) -> bool {
        self.written()
            && !self.searched_before
            && matches!(
                self.after,
                Some(EntryStatus::Indexed | EntryStatus::StaleProperties(_))
            )
    }

    /// A change another command made is not this run's to report.
    fn written(&self) -> bool {
        matches!(self.rebuilt, Rebuilt::Written { .. })
    }
}

/// What `reindex` did.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Reindex {
    /// One line per entry processed, in order.
    pub lines: Vec<Reindexed>,
    /// The error that stopped the run, if one did (CC6): the batches
    /// before it are committed (RX7).
    pub stopped: Option<Error>,
}

/// The rows of this build's versions an entry lacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Work {
    /// No current features row: read, gate and extract again.
    extract: bool,
    /// Accepted, without current tokens: rebuilt from the stored document.
    tokens: bool,
    /// Accepted, without current properties: captured again.
    properties: bool,
}

impl Work {
    fn none(self) -> bool {
        !(self.extract || self.tokens || self.properties)
    }

    /// What `sha256` lacks, read in `conn`.
    fn of(conn: &Connection, sha256: &Sha256Hex) -> Result<Self, Error> {
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM features WHERE sha256 = ?1 AND extractor_version = ?2",
                params![sha256.as_str(), EXTRACTOR_VERSION],
                |r| r.get(0),
            )
            .optional()?;
        let Some(status) = status else {
            return Ok(Self {
                extract: true,
                tokens: false,
                properties: false,
            });
        };
        if status != "accepted" {
            return Ok(Self {
                extract: false,
                tokens: false,
                properties: false,
            });
        }
        let tokens: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM tokens WHERE sha256 = ?1 AND extractor_version = ?2
                            AND signature_version = ?3)",
            params![sha256.as_str(), EXTRACTOR_VERSION, SIGNATURE_VERSION],
            |r| r.get(0),
        )?;
        let properties: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM properties WHERE sha256 = ?1 AND properties_version = ?2)",
            params![sha256.as_str(), PROPERTIES_VERSION],
            |r| r.get(0),
        )?;
        Ok(Self {
            extract: false,
            tokens: !tokens,
            properties: !properties,
        })
    }
}

/// An entry with the rows it is to get, read outside the write lock.
struct Planned {
    line: Reindexed,
    work: Work,
    /// The rows of a new read (extract).
    derived: Option<Prepared>,
    /// Tokens rebuilt from the stored document.
    tokens: Option<IndexTokens>,
    /// Properties captured again.
    properties: Option<StatedProperties>,
}

fn line(listed: &Listed) -> Reindexed {
    Reindexed {
        id: listed.id.clone(),
        revision: listed.revision,
        sha256: listed.sha256.clone(),
        before: listed.status,
        searched_before: listed.counts.is_some(),
        after: None,
        rebuilt: Rebuilt::Current,
        first_refusal: None,
    }
}

impl Catalog {
    /// `fitment catalog reindex`: for each entry of `scope`, the rows of
    /// this build's versions it lacks, in batches of at most 64 entries,
    /// each one transaction (RX4, RX7). An extractor rise reads the file
    /// through its hash check (ST11); a signature rise reads none.
    ///
    /// # Errors
    ///
    /// [`Unnamed`] in an `Ok` for a reference that names no live entry's
    /// current revision, before anything is done; the database's if the
    /// entries cannot be read. An error during the run stops it, and is
    /// [`Reindex::stopped`].
    pub fn reindex(&mut self, scope: &Scope) -> Result<Result<Reindex, Unnamed>, Error> {
        let targets = match self.targets(scope)? {
            Ok(targets) => targets,
            Err(unnamed) => return Ok(Err(unnamed)),
        };
        let mut out = Reindex::default();
        let budget = self.options.max_bytes.saturating_mul(2);
        let mut rest = targets.as_slice();
        while !rest.is_empty() {
            let started = Instant::now();
            let mut batch = Vec::new();
            let mut held = 0_u64;
            let mut planning = None;
            while let Some((listed, tail)) = rest.split_first() {
                // A batch takes one entry at least, whatever the budget.
                let full = !batch.is_empty() && (batch.len() >= BATCH_FILES || held >= budget);
                if full || started.elapsed() > BATCH_TIME {
                    break;
                }
                rest = tail;
                match self.plan(listed) {
                    Ok(planned) => batch.push(planned),
                    Err(error) => {
                        planning = Some(error);
                        break;
                    }
                }
                held = held.saturating_add(listed.bytes.unwrap_or_default());
            }
            let (lines, stopped) = self.commit_reindex(batch);
            out.lines.extend(lines);
            // Committed batches stay reported whatever stops the run.
            if let Some(error) = stopped.or(planning) {
                out.stopped = Some(error);
                break;
            }
        }
        if let Err(error) = self.settle_lines(&mut out.lines) {
            out.stopped.get_or_insert(error);
        }
        Ok(Ok(out))
    }

    /// RX9's entries, each with its status before.
    fn targets(&mut self, scope: &Scope) -> Result<Result<Vec<Listed>, Unnamed>, Error> {
        let snapshot = self.snapshot()?;
        let all = snapshot.list()?;
        Ok(Ok(match scope {
            Scope::All => all,
            Scope::Stale => all
                .into_iter()
                .filter(|l| {
                    matches!(
                        l.status,
                        Some(
                            EntryStatus::StaleExtractor(_)
                                | EntryStatus::StaleSignature(_)
                                | EntryStatus::StaleProperties(_)
                        )
                    )
                })
                .collect(),
            Scope::Named(references) => {
                let mut named = Vec::new();
                for reference in references {
                    match live_entry(snapshot.connection(), reference)? {
                        Ok(entry) => {
                            if let Some(found) = all.iter().find(|l| l.id == entry.id) {
                                if !named.iter().any(|n: &Listed| n.id == found.id) {
                                    named.push(found.clone());
                                }
                            }
                        }
                        Err(unnamed) => return Ok(Err(unnamed)),
                    }
                }
                named
            }
        }))
    }

    /// What `listed` lacks, and the rows that fill it, read and checked
    /// outside the write lock.
    fn plan(&self, listed: &Listed) -> Result<Planned, Error> {
        let mut planned = Planned {
            line: line(listed),
            work: Work::of(&self.conn, &listed.sha256)?,
            derived: None,
            tokens: None,
            properties: None,
        };
        let work = planned.work;
        if work.none() {
            return Ok(planned);
        }
        let damaged = |why: String| Rebuilt::Damaged(why);
        let bytes = if work.extract || work.properties {
            match self.blobs().read(&listed.sha256) {
                Ok(bytes) => Some(bytes),
                Err(bad) => {
                    planned.line.rebuilt = match bad {
                        BadBlob::Missing => damaged("missing: the file is not in the store".into()),
                        BadBlob::Damaged => {
                            damaged("damaged: the file's SHA-256 is not its name".into())
                        }
                        // A read that failed says nothing of the file.
                        other => Rebuilt::Failed(format!("cannot read its file: {other}")),
                    };
                    return Ok(planned);
                }
            }
        } else {
            None
        };
        if work.extract {
            let id = Id::parse(&listed.id).map_err(|_| Error::Corrupt("an id"))?;
            let bytes = bytes.unwrap_or_default();
            match derive(
                bytes,
                listed.sha256.clone(),
                PathBuf::new(),
                String::new(),
                id,
                false,
            ) {
                Ok(mut derived) => {
                    // Only the rows are kept; the bytes are not written again.
                    derived.bytes = Vec::new();
                    planned.derived = Some(derived);
                }
                Err(_) => planned.line.rebuilt = Rebuilt::TooLarge,
            }
            return Ok(planned);
        }
        if work.tokens {
            match self.stored_document(&listed.sha256)? {
                Ok(document) => {
                    planned.tokens = document
                        .part()
                        .map(|p| IndexTokens::of(&PartSignature::of(p)));
                }
                Err(bad) => {
                    planned.line.rebuilt = damaged(format!("its stored document: {bad}"));
                    return Ok(planned);
                }
            }
        }
        if let (true, Some(bytes)) = (work.properties, &bytes) {
            planned.properties = Some(StatedProperties::read(bytes));
        }
        Ok(planned)
    }

    /// The current features row of `sha256`, checked (QR4).
    fn stored_document(
        &self,
        sha256: &Sha256Hex,
    ) -> Result<Result<fitment_core::FeaturesDocument, BadRow>, Error> {
        type Row = (String, String, Option<String>, Vec<u8>, String);
        let row: Option<Row> = self
            .conn
            .query_row(
                "SELECT sha256, status, part_digest, doc, doc_sha256 FROM features
                 WHERE sha256 = ?1 AND extractor_version = ?2",
                params![sha256.as_str(), EXTRACTOR_VERSION],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((sha, status, digest, doc, hash)) = row else {
            return Ok(Err(BadRow::Stale { found: 0 }));
        };
        Ok(check(
            sha256,
            &sha,
            EXTRACTOR_VERSION,
            &status,
            digest.as_deref(),
            &doc,
            &hash,
        ))
    }

    /// One write transaction for a batch: each entry checked again to be
    /// live, current and lacking the rows (DB2, RX7), then its rows.
    fn commit_reindex(&mut self, batch: Vec<Planned>) -> (Vec<Reindexed>, Option<Error>) {
        let result = (|| -> Result<Vec<Reindexed>, Error> {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut lines = Vec::new();
            let mut generation = None;
            let mut n = 0_i64;
            for planned in &batch {
                let mut line = planned.line.clone();
                let entry: Option<(i64, i64)> = tx
                    .query_row(
                        "SELECT l.entry, b.key FROM live l JOIN blob b ON b.sha256 = l.sha256
                         WHERE l.id = ?1 AND l.revision = ?2 AND l.sha256 = ?3",
                        params![line.id, line.revision, line.sha256.as_str()],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                // An entry replaced or deleted since its plan, whatever the
                // plan found, is that change's (CC4's re-check).
                let Some((entry, key)) = entry else {
                    line.rebuilt = Rebuilt::Changed;
                    lines.push(line);
                    continue;
                };
                if line.rebuilt != Rebuilt::Current || planned.work.none() {
                    lines.push(line);
                    continue;
                }
                let now = Work::of(&tx, &line.sha256)?;
                if now.none() {
                    // Another reindex wrote them first.
                    lines.push(line);
                    continue;
                }
                if now != planned.work {
                    // Its rows changed under the plan (another build's
                    // gc or reindex): planned again on the next run.
                    line.rebuilt = Rebuilt::Changed;
                    lines.push(line);
                    continue;
                }
                let g = match generation {
                    Some(g) => g,
                    None => *generation.insert(schema::next_generation(&tx)?),
                };
                write(&tx, planned, key)?;
                tx.execute(
                    "INSERT INTO op (generation, n, kind, entry, sha256, fitment_version)
                     VALUES (?1, ?2, 'reindex', ?3, ?4, ?5)",
                    params![g, n, entry, line.sha256.as_str(), env!("CARGO_PKG_VERSION")],
                )?;
                n += 1;
                line.rebuilt = Rebuilt::Written {
                    blob_read: planned.work.extract || planned.work.properties,
                };
                lines.push(line);
            }
            self.fs.point(Point::BeforeCommit).map_err(|e| Error::Io {
                path: self.layout.database(),
                source: e,
            })?;
            tx.commit()?;
            let _ = self.fs.point(Point::AfterCommit);
            Ok(lines)
        })();
        match result {
            Ok(lines) => (lines, None),
            Err(error) => {
                let message = error.to_string();
                let lines = batch
                    .into_iter()
                    .map(|planned| {
                        let mut line = planned.line;
                        if line.rebuilt == Rebuilt::Current && !planned.work.none() {
                            line.rebuilt = Rebuilt::Failed(message.clone());
                        }
                        line
                    })
                    .collect();
                (lines, Some(error))
            }
        }
    }

    /// Each line's status after, and its first refusal when quarantined.
    fn settle_lines(&mut self, lines: &mut [Reindexed]) -> Result<(), Error> {
        let snapshot = self.snapshot()?;
        let now = snapshot.list()?;
        let by_id: BTreeMap<&str, &Listed> = now.iter().map(|l| (l.id.as_str(), l)).collect();
        for line in lines {
            if let Some(listed) = by_id.get(line.id.as_str()) {
                line.after = listed.status;
                line.first_refusal = listed
                    .refusals
                    .first()
                    .map(|r| format!("{}: {}", r.code, r.message));
            }
        }
        Ok(())
    }
}

/// `planned`'s rows, each with its preimage hash, under content `key`.
fn write(tx: &rusqlite::Transaction<'_>, planned: &Planned, key: i64) -> Result<(), Error> {
    let sha = &planned.line.sha256;
    if let Some(derived) = &planned.derived {
        insert_features(tx, derived)?;
        if derived.status == Status::Refused {
            // PR3: a refused document has no properties; those an older
            // gate's acceptance left go, this build's version or older
            // (never a newer one, RX8).
            tx.execute(
                "DELETE FROM properties WHERE sha256 = ?1 AND properties_version <= ?2",
                params![sha.as_str(), PROPERTIES_VERSION],
            )?;
        }
        if let Some(tokens) = &derived.tokens {
            insert_tokens(tx, sha, tokens, key)?;
        }
        if let Some(stated) = &derived.properties {
            insert_properties(tx, sha, stated)?;
        }
        return Ok(());
    }
    if let Some(tokens) = &planned.tokens {
        insert_tokens(tx, sha, tokens, key)?;
    }
    if let Some(stated) = &planned.properties {
        insert_properties(tx, sha, stated)?;
    }
    Ok(())
}

/// RX8: deletes the derived rows of versions older than this build's for
/// content whose current rows exist, never a newer version's; returns how
/// many rows went. One transaction, with a `gc` op row if any did.
pub(crate) fn drop_old_versions(conn: &mut Connection) -> Result<usize, Error> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (ev, sv, pv) = (EXTRACTOR_VERSION, SIGNATURE_VERSION, PROPERTIES_VERSION);
    let mut removed = 0;
    // An older features row goes where a current one exists, its tokens
    // first, so that they are counted (a cascade's deletes are not).
    removed += tx.execute(
        "DELETE FROM tokens WHERE extractor_version < ?1
           AND EXISTS (SELECT 1 FROM features c WHERE c.sha256 = tokens.sha256
                       AND c.extractor_version = ?1)",
        [ev],
    )?;
    removed += tx.execute(
        "DELETE FROM features WHERE extractor_version < ?1
           AND EXISTS (SELECT 1 FROM features c WHERE c.sha256 = features.sha256
                       AND c.extractor_version = ?1)",
        [ev],
    )?;
    removed += tx.execute(
        "DELETE FROM tokens WHERE extractor_version = ?1 AND signature_version < ?2
           AND EXISTS (SELECT 1 FROM tokens c WHERE c.sha256 = tokens.sha256
                       AND c.extractor_version = ?1 AND c.signature_version = ?2)",
        [ev, sv],
    )?;
    // Postings of an older pair whose tokens row is gone.
    removed += tx.execute(
        "DELETE FROM posting
         WHERE (extractor_version < ?1 OR (extractor_version = ?1 AND signature_version < ?2))
           AND NOT EXISTS (SELECT 1 FROM tokens t JOIN blob b ON b.sha256 = t.sha256
                           WHERE b.key = posting.content
                             AND t.extractor_version = posting.extractor_version
                             AND t.signature_version = posting.signature_version)",
        [ev, sv],
    )?;
    removed += tx.execute(
        "DELETE FROM properties WHERE properties_version < ?1
           AND EXISTS (SELECT 1 FROM properties c WHERE c.sha256 = properties.sha256
                       AND c.properties_version = ?1)",
        [pv],
    )?;
    // PR3: properties beside a document this build refuses, left by an
    // older gate's acceptance, this build's version or older.
    removed += tx.execute(
        "DELETE FROM properties WHERE properties_version <= ?1
           AND EXISTS (SELECT 1 FROM features f WHERE f.sha256 = properties.sha256
                       AND f.extractor_version = ?2 AND f.status = 'refused')",
        [pv, ev],
    )?;
    if removed > 0 {
        let g = schema::next_generation(&tx)?;
        tx.execute(
            "INSERT INTO op (generation, n, kind, fitment_version) VALUES (?1, 0, 'gc', ?2)",
            params![g, env!("CARGO_PKG_VERSION")],
        )?;
    }
    tx.commit()?;
    Ok(removed)
}
