//! `fitment catalog ingest` (`docs/research/m5-catalog.md`, section 7.2;
//! GT1 to GT9, ID3 to ID5, ST2, ST3, ST6, DB2, DB3, PH3, PH4, SZ1, SZ2).
//!
//! Each file is read once into one buffer, hashed and gated, and its blob
//! put in place, outside any transaction. Then, in one `BEGIN IMMEDIATE`
//! per batch, every decision is made again on what the database holds,
//! each blob the commit names is checked to be there (ST6) and its fan-out
//! directory synced, and the rows are written.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fitment_core::{
    EXTRACTOR_VERSION, FeaturesDocument, IndexTokens, PROPERTIES_VERSION, PartSignature,
    RefusalCode, SIGNATURE_VERSION, Sense, StatedProperties,
};
use rusqlite::{OptionalExtension as _, Transaction, params};

use crate::blobs::Placed;
use crate::fs::{Content, Point};
use crate::hex::Sha256Hex;
use crate::ids::{Id, default_id, is_exact};
use crate::preimage::{self, Status};
use crate::{Catalog, Error, MAX_DOCUMENT_BYTES, schema};

/// At most this many files per write transaction (DB3).
pub(crate) const BATCH_FILES: usize = 64;

/// And at most this much work.
pub(crate) const BATCH_TIME: Duration = Duration::from_secs(1);

/// The longest file name kept (PH3), in bytes.
const MAX_FILE_NAME: usize = 4096;

/// What became of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// A new entry, indexed or quarantined by its status.
    Added,
    /// These bytes already are this id's.
    Unchanged,
    /// These bytes are another entry's, and the id was the default: not
    /// added (ID5, exit 0).
    Duplicate {
        /// That entry's id.
        of: String,
    },
    /// These bytes are another entry's, and the id was given: exit 2.
    DuplicateUnderId {
        /// That entry's id.
        of: String,
    },
    /// The id is another content's, or none can be made (exit 2).
    Conflict {
        /// Why, for people.
        reason: String,
    },
    /// Refused at the door: too large, or not a regular file (GT3, exit 3).
    Refused {
        /// Why, for people.
        reason: String,
    },
    /// Unreadable, unwritable, or the batch could not commit (exit 1).
    Error {
        /// What failed.
        message: String,
    },
}

/// An accepted part's feature counts (GT5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Counts {
    /// Planes.
    pub planes: u32,
    /// Holes.
    pub holes: u32,
    /// Shafts.
    pub shafts: u32,
    /// Cones.
    pub cones: u32,
    /// Faces no rule reads.
    pub other_faces: u32,
}

/// What a stored document says, for people (GT5).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Summary {
    /// Accepted or refused.
    pub status: Status,
    /// PRODUCT.id, as the file states it.
    pub product_id: Option<String>,
    /// PRODUCT.name.
    pub product_name: Option<String>,
    /// The feature counts, if accepted.
    pub counts: Option<Counts>,
    /// Whether every face was read, if accepted.
    pub complete: Option<bool>,
    /// The first refusal, as `code: message`, if refused.
    pub first_refusal: Option<String>,
    /// True if the gate refused the file as `internal`: a fitment bug (GT8).
    pub internal: bool,
}

/// One file's line.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Ingested {
    /// The path read: an argument, or a file of a directory argument.
    pub input: PathBuf,
    /// The id it was given or would get.
    pub id: Option<Id>,
    /// The revision it is, when added or unchanged.
    pub revision: Option<u32>,
    /// Its SHA-256, once read.
    pub sha256: Option<Sha256Hex>,
    /// What the gate made of it, once read.
    pub summary: Option<Summary>,
    /// What became of it.
    pub outcome: Outcome,
    /// True if a blob of other bytes stood at its name and was replaced
    /// (ST3): reported, for it means the store was altered.
    pub replaced_blob: bool,
}

impl Ingested {
    pub(crate) fn failed(input: &Path, id: Option<Id>, outcome: Outcome) -> Self {
        Self {
            input: input.to_owned(),
            id,
            revision: None,
            sha256: None,
            summary: None,
            outcome,
            replaced_blob: false,
        }
    }
}

/// What an ingest did: a line per file read, in ID4's order, and why the
/// run stopped early, if it did (the catalog busy, or its database
/// failing): the files after it were not read.
#[derive(Debug)]
#[non_exhaustive]
pub struct Ingest {
    /// One line per file, in order.
    pub lines: Vec<Ingested>,
    /// Why the run stopped before its last file.
    pub stopped: Option<Error>,
}

impl<'a> IntoIterator for &'a Ingest {
    type Item = &'a Ingested;
    type IntoIter = std::slice::Iter<'a, Ingested>;

    fn into_iter(self) -> Self::IntoIter {
        self.lines.iter()
    }
}

impl std::ops::Deref for Ingest {
    type Target = [Ingested];

    fn deref(&self) -> &[Ingested] {
        &self.lines
    }
}

/// A file read and gated, ready to be decided on.
pub(crate) struct Prepared {
    pub(crate) input: PathBuf,
    pub(crate) file_name: String,
    pub(crate) id: Id,
    pub(crate) explicit: bool,
    pub(crate) bytes: Vec<u8>,
    pub(crate) sha256: Sha256Hex,
    pub(crate) doc: String,
    pub(crate) status: Status,
    pub(crate) part_digest: Option<Sha256Hex>,
    pub(crate) tokens: Option<IndexTokens>,
    /// PR3: an accepted file's stated properties, from the same parse.
    pub(crate) properties: Option<StatedProperties>,
    pub(crate) summary: Summary,
    pub(crate) replaced_blob: bool,
    /// Whether `prepare` put the blob in place, hashed.
    pub(crate) placed: bool,
}

/// PH3: the final component, lossily if not UTF-8, at most 4096 bytes.
fn file_name(path: &Path) -> String {
    let mut name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    while name.len() > MAX_FILE_NAME {
        name.pop();
    }
    name
}

/// Why derived rows cannot be stored (GT1, SZ2).
pub(crate) const TOO_LARGE: &str = "derived_too_large: its features document is too large to store";

/// GT1 and GT6: the rows `bytes` give, as `prepare` and `reindex` store
/// them: the features document, its summary and part digest, and for an
/// accepted part its tokens and its stated properties, from one parse.
/// The id is handed back with [`TOO_LARGE`] when the document is too
/// large to store.
pub(crate) fn derive(
    bytes: Vec<u8>,
    sha256: Sha256Hex,
    input: PathBuf,
    file_name: String,
    id: Id,
    explicit: bool,
) -> Result<Prepared, (Id, &'static str)> {
    let (read, stated) = FeaturesDocument::read_with_properties(&bytes);
    let doc = read.to_compact_json();
    if doc.len() > MAX_DOCUMENT_BYTES {
        return Err((id, TOO_LARGE));
    }
    let summary = summarize(&read);
    let tokens = read.part().map(|p| IndexTokens::of(&PartSignature::of(p)));
    let part_digest = read.part_digest().and_then(|d| Sha256Hex::parse(&d).ok());
    Ok(Prepared {
        input,
        file_name,
        id,
        explicit,
        sha256,
        doc,
        status: summary.status,
        part_digest,
        properties: read.part().map(|_| stated),
        tokens,
        summary,
        bytes,
        replaced_blob: false,
        placed: false,
    })
}

/// GT5's summary of a read document.
fn summarize(doc: &FeaturesDocument) -> Summary {
    let part = doc.part();
    let product = part.and_then(|p| p.definition.product.as_ref());
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let first = doc.refusals().first();
    Summary {
        status: if part.is_some() {
            Status::Accepted
        } else {
            Status::Refused
        },
        product_id: product.and_then(|p| p.id.clone()),
        product_name: product.and_then(|p| p.name.clone()),
        counts: part.map(|p| Counts {
            planes: count(p.planes.len()),
            holes: count(p.holes.len()),
            shafts: count(p.shafts.len()),
            cones: count(p.cones.len()),
            other_faces: count(p.other_faces.len()),
        }),
        complete: part.map(|p| p.complete),
        first_refusal: first.map(|r| format!("{}: {}", r.code.name(), r.message)),
        internal: doc
            .refusals()
            .iter()
            .any(|r| r.code == RefusalCode::Internal),
    }
}

/// PH4: the files an argument names. A directory gives its `.stp`,
/// `.step` and `.p21` files, in any case, not recursively; a file named
/// is taken whatever its extension.
fn expand(catalog: &Catalog, input: &Path) -> Result<Vec<PathBuf>, Box<Ingested>> {
    // A path with no metadata is read, and reported, as the file it names.
    let Ok(meta) = std::fs::metadata(input) else {
        return Ok(vec![input.to_owned()]);
    };
    if !meta.is_dir() {
        return Ok(vec![input.to_owned()]);
    }
    let entries = catalog.fs.read_dir(input).map_err(|e| {
        Box::new(Ingested::failed(
            input,
            None,
            Outcome::Error {
                message: format!("cannot list it: {e}"),
            },
        ))
    })?;
    Ok(entries
        .into_iter()
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                ["stp", "step", "p21"]
                    .iter()
                    .any(|x| e.eq_ignore_ascii_case(x))
            })
        })
        .collect())
}

/// ST10: an input inside the catalog's directory.
pub(crate) fn inside(root: &Path, input: &Path) -> bool {
    let (Ok(root), Ok(input)) = (std::fs::canonicalize(root), std::fs::canonicalize(input)) else {
        return false;
    };
    input.starts_with(root)
}

/// ID4's order: files whose stem is already an id first, then the others,
/// each group in byte order of the path.
fn order(files: &mut [PathBuf]) {
    files.sort_by(|a, b| {
        let key = |p: &PathBuf| !is_exact(&file_name(p));
        key(a).cmp(&key(b)).then_with(|| {
            a.as_os_str()
                .as_encoded_bytes()
                .cmp(b.as_os_str().as_encoded_bytes())
        })
    });
}

impl Catalog {
    /// `fitment catalog ingest`: each file of `inputs`, under `id` if given
    /// (then exactly one file), in ID4's order, in batches (DB3).
    ///
    /// # Errors
    ///
    /// [`Error::InsideCatalog`] or [`Error::IdNeedsOneFile`] before any file
    /// is read. A file's own failure is its [`Outcome`], and the run goes
    /// on.
    pub fn ingest(&mut self, inputs: &[PathBuf], id: Option<&Id>) -> Result<Ingest, Error> {
        let mut out = Vec::new();
        let mut files = Vec::new();
        for input in inputs {
            match expand(self, input) {
                Ok(found) => files.extend(found),
                Err(failed) => out.push(*failed),
            }
        }
        // ST10, on every file read: a directory's link into the catalog too.
        if let Some(file) = files.iter().find(|f| inside(self.root(), f)) {
            return Err(Error::InsideCatalog(file.clone()));
        }
        if id.is_some() && (files.len() != 1 || !out.is_empty()) {
            return Err(Error::IdNeedsOneFile(files.len() + out.len()));
        }
        order(&mut files);
        let budget = self.options.max_bytes.saturating_mul(2);
        let mut rest = files.as_slice();
        while !rest.is_empty() {
            let started = Instant::now();
            let mut batch = Vec::new();
            let mut held = 0_u64;
            while let Some((file, tail)) = rest.split_first() {
                let full = batch.len() >= BATCH_FILES || held >= budget;
                if full || started.elapsed() > BATCH_TIME {
                    break;
                }
                rest = tail;
                let prepared = self.prepare(file, id);
                if let Ok(p) = &prepared {
                    held = held.saturating_add(u64::try_from(p.bytes.len()).unwrap_or(u64::MAX));
                }
                batch.push(prepared);
            }
            let (lines, stopped) = self.commit(batch);
            out.extend(lines);
            if stopped.is_some() {
                return Ok(Ingest {
                    lines: out,
                    stopped,
                });
            }
        }
        Ok(Ingest {
            lines: out,
            stopped: None,
        })
    }

    /// Reads, hashes and gates one file, and puts its blob in place unless
    /// it is surely not added.
    fn prepare(&self, path: &Path, id: Option<&Id>) -> Result<Prepared, Box<Ingested>> {
        let mut prepared = self.read_file(path, id)?;
        if self.likely_added(&prepared) {
            match self.blobs().put(&prepared.sha256, &prepared.bytes) {
                Ok(placed) => {
                    prepared.replaced_blob = placed == Placed::Replaced;
                    prepared.placed = true;
                }
                Err(e) => {
                    let mut failed = Box::new(Ingested::failed(
                        path,
                        Some(prepared.id),
                        Outcome::Error {
                            message: format!("cannot store it: {e}"),
                        },
                    ));
                    failed.sha256 = Some(prepared.sha256);
                    failed.summary = Some(prepared.summary);
                    return Err(failed);
                }
            }
        }
        Ok(prepared)
    }

    /// Reads, hashes and gates one file (ST2, SZ1, PH4, GT1, GT6), under
    /// `id` or the id its name gives (ID3).
    pub(crate) fn read_file(
        &self,
        path: &Path,
        id: Option<&Id>,
    ) -> Result<Prepared, Box<Ingested>> {
        let name = file_name(path);
        let (id, explicit) = match id {
            Some(id) => (Some(id.clone()), true),
            None => (default_id(&name), false),
        };
        let fail = |id: Option<Id>, outcome| Box::new(Ingested::failed(path, id, outcome));
        let bytes = match self.fs.read_capped(path, self.options.max_bytes) {
            Ok(Content::Bytes(bytes)) => bytes,
            Ok(Content::NotRegular) => {
                let reason = "not a regular file: not added".to_owned();
                return Err(fail(id, Outcome::Refused { reason }));
            }
            Ok(Content::TooLarge) => {
                let mib = self.options.max_bytes / (1024 * 1024);
                let reason = format!("larger than --max-mib {mib}: not added");
                return Err(fail(id, Outcome::Refused { reason }));
            }
            Err(e) => {
                let message = format!("cannot read {name}: {e}");
                return Err(fail(id, Outcome::Error { message }));
            }
        };
        let Some(id) = id else {
            let reason = "no valid id: give --id".to_owned();
            return Err(fail(None, Outcome::Conflict { reason }));
        };
        let sha256 = Sha256Hex::of(&bytes);
        derive(bytes, sha256, path.to_owned(), name, id, explicit).map_err(|(id, reason)| {
            let reason = reason.to_owned();
            fail(Some(id), Outcome::Refused { reason })
        })
    }

    /// A first look, outside the transaction, so a file surely not added
    /// writes no blob; the transaction decides again.
    fn likely_added(&self, p: &Prepared) -> bool {
        decide(&self.conn, p).is_ok_and(|d| matches!(d, Decision::Add))
    }

    /// One write transaction for a batch: every decision taken again on
    /// what the database holds now (DB2), every named blob checked (ST6)
    /// and its directory synced (ST3), then the rows.
    fn commit(
        &mut self,
        batch: Vec<Result<Prepared, Box<Ingested>>>,
    ) -> (Vec<Ingested>, Option<Error>) {
        let blobs = crate::blobs::Blobs {
            layout: &self.layout,
            fs: self.fs.as_ref(),
        };
        let result = (|| -> Result<Vec<Ingested>, Error> {
            let tx = self
                .conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let mut lines = Vec::new();
            let mut generation = None;
            let mut synced = BTreeSet::new();
            let mut n = 0_i64;
            for item in &batch {
                let p = match item {
                    Ok(p) => p,
                    Err(failed) => {
                        lines.push((**failed).clone());
                        continue;
                    }
                };
                let decision = decide(&tx, p)?;
                let mut line = line(p, &decision);
                if let Decision::Add = decision {
                    let g = match generation {
                        Some(g) => g,
                        None => *generation.insert(schema::next_generation(&tx)?),
                    };
                    let size = u64::try_from(p.bytes.len()).unwrap_or(u64::MAX);
                    // ST6: written again if it went; then its directory is
                    // synced, once per commit, and again after a rewrite.
                    // A blob `prepare` did not place is put now, hashed.
                    let rewritten = !p.placed || !blobs.present(&p.sha256, size);
                    if rewritten {
                        let placed = blobs.put(&p.sha256, &p.bytes).map_err(|e| Error::Io {
                            path: blobs.layout.blob(&p.sha256),
                            source: e,
                        })?;
                        line.replaced_blob |= placed == Placed::Replaced;
                    }
                    let first = synced.insert(p.sha256.fan_out().to_owned());
                    if first || rewritten {
                        blobs.sync_fan_out(&p.sha256).map_err(|e| Error::Io {
                            path: blobs.layout.fan_out(&p.sha256),
                            source: e,
                        })?;
                    }
                    line.revision = Some(insert(&tx, p, g, n)?);
                    n += 1;
                }
                lines.push(line);
            }
            let point = |at| {
                blobs.fs.point(at).map_err(|e| Error::Io {
                    path: blobs.layout.database(),
                    source: e,
                })
            };
            point(Point::BeforeCommit)?;
            tx.commit()?;
            // Committed: what the seam says now changes nothing.
            let _ = blobs.fs.point(Point::AfterCommit);
            Ok(lines)
        })();
        match result {
            Ok(lines) => (lines, None),
            Err(error) => {
                let message = error.to_string();
                let lines = batch
                    .into_iter()
                    .map(|item| match item {
                        Err(failed) => *failed,
                        Ok(p) => Ingested {
                            input: p.input,
                            id: Some(p.id),
                            revision: None,
                            sha256: Some(p.sha256),
                            summary: Some(p.summary),
                            outcome: Outcome::Error {
                                message: message.clone(),
                            },
                            replaced_blob: p.replaced_blob,
                        },
                    })
                    .collect();
                // A file's own I/O fails its batch; the lock or the database
                // failing stops the run (CC6).
                let fatal = !matches!(error, Error::Io { .. });
                (lines, fatal.then_some(error))
            }
        }
    }
}

/// What the database says of a file (ID5).
enum Decision {
    Add,
    Unchanged(u32),
    Duplicate(String),
    Taken,
}

/// ID5, then ID2: the content live under this id is unchanged, under
/// another a duplicate; an id live with other content is a conflict.
fn decide(conn: &rusqlite::Connection, p: &Prepared) -> rusqlite::Result<Decision> {
    let live: Option<(String, i64)> = conn
        .query_row(
            "SELECT id, revision FROM live WHERE sha256 = ?1",
            [p.sha256.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((id, revision)) = live {
        return Ok(if id.eq_ignore_ascii_case(p.id.as_str()) {
            Decision::Unchanged(u32::try_from(revision).unwrap_or(0))
        } else {
            Decision::Duplicate(id)
        });
    }
    let taken: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM entry WHERE id_fold = ?1 AND deleted_generation IS NULL)",
        [p.id.fold()],
        |r| r.get(0),
    )?;
    Ok(if taken {
        Decision::Taken
    } else {
        Decision::Add
    })
}

/// The line a decision gives; the revision is filled in once added.
fn line(p: &Prepared, decision: &Decision) -> Ingested {
    let outcome = match decision {
        Decision::Add => Outcome::Added,
        Decision::Unchanged(_) => Outcome::Unchanged,
        Decision::Duplicate(of) if p.explicit => Outcome::DuplicateUnderId { of: of.clone() },
        Decision::Duplicate(of) => Outcome::Duplicate { of: of.clone() },
        Decision::Taken => Outcome::Conflict {
            reason: format!("the id {} is taken by another file (give --id)", p.id),
        },
    };
    Ingested {
        input: p.input.clone(),
        id: Some(p.id.clone()),
        revision: match decision {
            Decision::Unchanged(r) => Some(*r),
            _ => None,
        },
        sha256: Some(p.sha256.clone()),
        summary: Some(p.summary.clone()),
        outcome,
        replaced_blob: p.replaced_blob,
    }
}

/// GT1's rows for one added file at generation `g`, the `op` row `n` of
/// the transaction: blob, name, entry, revision, features, tokens and
/// postings. Returns the revision.
fn insert(tx: &Transaction<'_>, p: &Prepared, g: i64, n: i64) -> rusqlite::Result<u32> {
    let sha = p.sha256.as_str();
    let size = i64::try_from(p.bytes.len()).unwrap_or(i64::MAX);
    tx.execute(
        "INSERT INTO blob (sha256, bytes, added_generation) VALUES (?1, ?2, ?3)
         ON CONFLICT (sha256) DO NOTHING",
        params![sha, size, g],
    )?;
    let key: i64 = tx.query_row("SELECT key FROM blob WHERE sha256 = ?1", [sha], |r| {
        r.get(0)
    })?;
    let fold = p.id.fold();
    let revision: i64 = tx.query_row(
        "INSERT INTO name (id_fold, last_revision) VALUES (?1, 1)
         ON CONFLICT (id_fold) DO UPDATE SET last_revision = last_revision + 1
         RETURNING last_revision",
        [&fold],
        |r| r.get(0),
    )?;
    let entry: i64 = tx.query_row(
        "INSERT INTO entry (id, id_fold, created_generation) VALUES (?1, ?2, ?3) RETURNING key",
        params![p.id.as_str(), &fold, g],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT INTO revision (entry, id_fold, revision, sha256, file_name, added_generation)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![entry, &fold, revision, sha, &p.file_name, g],
    )?;
    insert_features(tx, p)?;
    if let Some(tokens) = &p.tokens {
        insert_tokens(tx, &p.sha256, tokens, key)?;
    }
    if let Some(stated) = &p.properties {
        insert_properties(tx, &p.sha256, stated)?;
    }
    tx.execute(
        "INSERT INTO op (generation, n, kind, entry, sha256, fitment_version)
         VALUES (?1, ?2, 'ingest', ?3, ?4, ?5)",
        params![g, n, entry, sha, env!("CARGO_PKG_VERSION")],
    )?;
    Ok(u32::try_from(revision).unwrap_or(u32::MAX))
}

/// GT2 and GT5: the stored document with its preimage hash, and the
/// columns for people. A row of this content and version already there
/// (a kept revision's bytes ingested again) is left as it is.
pub(crate) fn insert_features(tx: &Transaction<'_>, p: &Prepared) -> rusqlite::Result<()> {
    let hash = preimage::features(
        &p.sha256,
        EXTRACTOR_VERSION,
        p.status,
        p.part_digest.as_ref(),
        p.doc.as_bytes(),
    );
    let s = &p.summary;
    let c = s.counts;
    tx.execute(
        "INSERT INTO features (sha256, extractor_version, status, doc, doc_sha256, part_digest,
                               product_id, product_name, holes, shafts, planes, cones,
                               other_faces, complete, first_refusal)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT (sha256, extractor_version) DO NOTHING",
        params![
            p.sha256.as_str(),
            EXTRACTOR_VERSION,
            p.status.as_str(),
            p.doc.as_bytes(),
            hash.as_str(),
            p.part_digest.as_ref().map(Sha256Hex::as_str),
            s.product_id,
            s.product_name,
            c.map(|c| c.holes),
            c.map(|c| c.shafts),
            c.map(|c| c.planes),
            c.map(|c| c.cones),
            c.map(|c| c.other_faces),
            s.complete,
            s.first_refusal,
        ],
    )?;
    Ok(())
}

/// PR3: an accepted file's stated properties with their preimage hash. A
/// row of this content and version already there is left as it is.
pub(crate) fn insert_properties(
    tx: &Transaction<'_>,
    sha256: &Sha256Hex,
    stated: &StatedProperties,
) -> rusqlite::Result<()> {
    let text = stated.to_json();
    let hash = preimage::properties(
        sha256,
        PROPERTIES_VERSION,
        stated.truncated,
        text.as_bytes(),
    );
    tx.execute(
        "INSERT INTO properties (sha256, properties_version, truncated, stated, stated_sha256)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (sha256, properties_version) DO NOTHING",
        params![
            sha256.as_str(),
            PROPERTIES_VERSION,
            stated.truncated,
            text.as_bytes(),
            hash.as_str(),
        ],
    )?;
    Ok(())
}

/// GT6 and IX2: the tokens with their preimage hash, and their postings
/// under the blob's key.
pub(crate) fn insert_tokens(
    tx: &Transaction<'_>,
    sha256: &Sha256Hex,
    tokens: &IndexTokens,
    key: i64,
) -> rusqlite::Result<()> {
    let text = tokens.to_json();
    let hash = preimage::tokens(
        sha256,
        EXTRACTOR_VERSION,
        SIGNATURE_VERSION,
        text.as_bytes(),
    );
    let added = tx.execute(
        "INSERT INTO tokens (sha256, extractor_version, signature_version, tokens, tokens_sha256,
                             lever_mm, planes, pairs_incomplete)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (sha256, extractor_version, signature_version) DO NOTHING",
        params![
            sha256.as_str(),
            EXTRACTOR_VERSION,
            SIGNATURE_VERSION,
            text.as_bytes(),
            hash.as_str(),
            tokens.lever_mm,
            tokens.planes,
            tokens.pairs_incomplete,
        ],
    )?;
    if added == 0 {
        return Ok(());
    }
    let mut posting = tx.prepare(
        "INSERT INTO posting (extractor_version, signature_version, sense, bin, content, stack,
                              diameter_mm)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT DO NOTHING",
    )?;
    for post in tokens.postings() {
        posting.execute(params![
            EXTRACTOR_VERSION,
            SIGNATURE_VERSION,
            u8::from(post.sense == Sense::Shaft),
            post.bin,
            key,
            post.stack,
            post.diameter_mm,
        ])?;
    }
    Ok(())
}
