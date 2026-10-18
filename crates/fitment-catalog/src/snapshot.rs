//! One read of the catalog (`docs/research/m5-catalog.md`, DB4, RX2, ID7,
//! ID9, QR4): a read transaction and what `list` and `get` show from it.
//! A [`Snapshot`] holds no way to reach a blob (QR11): the bytes of a part
//! are read only through [`Catalog::read_blob`].

use fitment_core::{
    EXTRACTOR_VERSION, FeaturesDocument, PROPERTIES_VERSION, RefusalCode, SIGNATURE_VERSION,
    StatedProperties,
};
use rusqlite::{OptionalExtension as _, Transaction, params};

use crate::hex::Sha256Hex;
use crate::ids::{Id, Reference};
use crate::ingest::Counts;
use crate::preimage::{self, Status};
use crate::{BadBlob, Catalog, Error};

/// An entry's status, computed on every read and never stored (RX2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryStatus {
    /// Accepted and current: searched.
    Indexed,
    /// Refused by this build's gate: never searched (CR8).
    Quarantined,
    /// Read by another extractor version (the newest stored): a match
    /// refuses until `reindex`.
    StaleExtractor(u32),
    /// Accepted, but its tokens are of another signature version (the
    /// newest stored, 0 for none).
    StaleSignature(u32),
    /// Indexed and searched, but its stated properties are of another
    /// version (the newest stored, 0 for none): `reindex` captures them
    /// again, and a match never refuses for it (M5-26).
    StaleProperties(u32),
}

impl EntryStatus {
    /// The status as `list` and the document write it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Indexed => "indexed",
            Self::Quarantined => "quarantined",
            Self::StaleExtractor(_) | Self::StaleSignature(_) | Self::StaleProperties(_) => "stale",
        }
    }
}

/// A refusal as stored: its code, message and `#id` (VD10).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StoredRefusal {
    /// The machine name.
    pub code: String,
    /// For people.
    pub message: String,
    /// The `#id` at fault, if any.
    pub instance: Option<u64>,
}

/// One revision of an entry, as `list` and `get` show it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Listed {
    /// The id, as stored.
    pub id: String,
    /// The revision.
    pub revision: u32,
    /// True if it is the entry's current revision.
    pub current: bool,
    /// The content.
    pub sha256: Sha256Hex,
    /// Its size, while its bytes are kept.
    pub bytes: Option<u64>,
    /// The file name it was ingested under.
    pub file_name: Option<String>,
    /// The status, for a current revision of a live entry.
    pub status: Option<EntryStatus>,
    /// PRODUCT.id and name.
    pub product: (Option<String>, Option<String>),
    /// The extractor, signature and properties versions of the rows read.
    pub versions: (Option<u32>, Option<u32>, Option<u32>),
    /// The feature counts, if accepted.
    pub counts: Option<Counts>,
    /// Whether every face was read, if accepted.
    pub complete: Option<bool>,
    /// The refusals, if refused.
    pub refusals: Vec<StoredRefusal>,
}

/// One step of an id's history (`get`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HistoryStep {
    /// The revision.
    pub revision: u32,
    /// Its content.
    pub sha256: Sha256Hex,
    /// The generation of the event.
    pub generation: i64,
    /// `ingest`, `replace`, `delete` or `prune`.
    pub event: &'static str,
    /// Whether the revision's bytes are still kept.
    pub bytes_kept: bool,
}

/// What a reference names (ID7, ID9).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Named {
    /// The id, as its entry stores it.
    pub id: String,
    /// The revision named.
    pub revision: u32,
    /// Its entry's current revision, if that entry is live: the revision
    /// named is current, superseded by this, or (`None`) deleted.
    pub current: Option<u32>,
    /// The current revision of the id's live entry, if it has one: what
    /// a strict `ID@rN` must name (ID9).
    pub id_current: Option<u32>,
    /// The content of the revision named.
    pub sha256: Sha256Hex,
    /// The revision's row.
    key: i64,
    /// The id, case-folded, as its revisions are keyed.
    fold: String,
}

/// Why a reference names nothing usable (exit 2).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BadName {
    /// Nothing matches; the ids that share the longest prefix, at most five.
    #[error("no entry {asked}{}", suggest(.suggestions))]
    Unknown {
        /// What was asked.
        asked: String,
        /// Ids like it.
        suggestions: Vec<String>,
    },
    /// Several live entries match a content prefix.
    #[error("{asked} names several entries: {}", .ids.join(", "))]
    Ambiguous {
        /// What was asked.
        asked: String,
        /// Their ids.
        ids: Vec<String>,
    },
    /// The id never had that revision.
    #[error("{id} has no r{asked}")]
    NoRevision {
        /// The id.
        id: String,
        /// The revision asked.
        asked: u32,
    },
}

fn suggest(ids: &[String]) -> String {
    if ids.is_empty() {
        String::new()
    } else {
        format!(": did you mean {}?", ids.join(", "))
    }
}

/// QR4: why a stored document cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BadRow {
    /// No row of this build's extractor: stale (RX2).
    #[error("read by another fitment (extractor {found}, this build {EXTRACTOR_VERSION})")]
    Stale {
        /// The newest version stored, 0 for none.
        found: u32,
    },
    /// The row fails a check: corrupt.
    #[error("its stored document is damaged ({0}): run fitment catalog verify")]
    Corrupt(&'static str),
}

/// One read transaction, at one generation.
pub struct Snapshot<'a> {
    tx: Transaction<'a>,
    generation: i64,
}

impl Catalog {
    /// A read snapshot (DB4): every read of a command in one transaction.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn snapshot(&mut self) -> Result<Snapshot<'_>, Error> {
        let tx = self.conn.transaction()?;
        let generation = crate::schema::generation(&tx)?;
        Ok(Snapshot { tx, generation })
    }

    /// ST11: the bytes of `sha256`, read once and returned only if they
    /// hash to it.
    ///
    /// # Errors
    ///
    /// [`BadBlob`]: missing, damaged or unreadable.
    pub fn read_blob(&self, sha256: &Sha256Hex) -> Result<Vec<u8>, BadBlob> {
        self.blobs().read(sha256)
    }
}

/// A revision's columns: id, revision, content, file name, and the
/// entry's deletion, the revision's supersession and prune generations.
type Row = (
    String,
    i64,
    String,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

/// A revision's history columns.
type Step = (
    u32,
    String,
    i64,
    String,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    bool,
);

/// QR4's columns of a features row.
type FeaturesRow = (String, u32, String, Option<String>, Vec<u8>, String);

impl Snapshot<'_> {
    /// Its transaction's connection, for the catalog match.
    pub(crate) fn connection(&self) -> &rusqlite::Connection {
        &self.tx
    }

    /// The generation it reads.
    pub fn generation(&self) -> i64 {
        self.generation
    }

    /// Every live entry's current revision, ordered by id case-folded,
    /// then by its bytes (CL3).
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn list(&self) -> Result<Vec<Listed>, Error> {
        let mut stmt = self.tx.prepare(
            "SELECT e.id, r.revision, r.sha256, r.file_name, e.deleted_generation,
                    r.superseded_generation, r.pruned_generation
             FROM entry e JOIN revision r ON r.entry = e.key AND r.superseded_generation IS NULL
             WHERE e.deleted_generation IS NULL
             ORDER BY e.id_fold, e.id",
        )?;
        let rows: Vec<Row> = stmt
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        rows.into_iter().map(|row| self.listed(row, true)).collect()
    }

    /// The revisions released by no prune of live entries, superseded:
    /// their count and bytes (`list`'s stderr, M5-41).
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn kept_revisions(&self) -> Result<(u64, u64), Error> {
        // Revisions kept; and the bytes a prune could release: contents
        // held by them and by no live revision, each once.
        let (n, bytes): (i64, i64) = self.tx.query_row(
            "WITH kept AS (
               SELECT r.sha256 FROM revision r JOIN entry e ON e.key = r.entry
               WHERE e.deleted_generation IS NULL AND r.superseded_generation IS NOT NULL
                 AND r.pruned_generation IS NULL)
             SELECT (SELECT count(*) FROM kept),
                    (SELECT coalesce(sum(b.bytes), 0) FROM blob b
                     WHERE b.sha256 IN (SELECT sha256 FROM kept)
                       AND b.sha256 NOT IN (SELECT sha256 FROM live))",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((
            u64::try_from(n).unwrap_or_default(),
            u64::try_from(bytes).unwrap_or_default(),
        ))
    }

    fn listed(&self, row: Row, live: bool) -> Result<Listed, Error> {
        let (id, revision, sha, file_name, deleted, superseded, pruned) = row;
        let id = stored_id(id)?;
        let sha256 = Sha256Hex::parse(&sha).map_err(|_| corrupt("a content key"))?;
        // Its size while its bytes are kept: not pruned, its entry live.
        let bytes: Option<i64> = if pruned.is_none() && deleted.is_none() {
            self.tx
                .query_row("SELECT bytes FROM blob WHERE sha256 = ?1", [&sha], |r| {
                    r.get(0)
                })
                .optional()?
        } else {
            None
        };
        let current = superseded.is_none() && deleted.is_none();
        let mut out = Listed {
            id,
            revision: u32::try_from(revision).map_err(|_| corrupt("a revision"))?,
            current,
            sha256,
            bytes: bytes.and_then(|b| u64::try_from(b).ok()),
            file_name,
            status: None,
            product: (None, None),
            versions: (None, None, None),
            counts: None,
            complete: None,
            refusals: Vec::new(),
        };
        if live && current {
            self.describe(&mut out)?;
        }
        Ok(out)
    }

    /// RX2's status and GT5's columns, from the current features row, or
    /// the newest one when stale.
    fn describe(&self, out: &mut Listed) -> Result<(), Error> {
        let sha = out.sha256.as_str();
        let row = self
            .tx
            .query_row(
                "SELECT extractor_version, status, product_id, product_name, holes, shafts, planes,
                        cones, other_faces, complete,
                        CASE WHEN status = 'refused' THEN doc ELSE x'' END
                 FROM features WHERE sha256 = ?1
                 ORDER BY extractor_version = ?2 DESC, extractor_version DESC LIMIT 1",
                params![sha, EXTRACTOR_VERSION],
                |r| {
                    let counts = match (r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?) {
                        (
                            Some(holes),
                            Some(shafts),
                            Some(planes),
                            Some(cones),
                            Some(other_faces),
                        ) => Some(Counts {
                            planes,
                            holes,
                            shafts,
                            cones,
                            other_faces,
                        }),
                        _ => None,
                    };
                    Ok((
                        r.get::<_, u32>(0)?,
                        r.get::<_, String>(1)?,
                        (r.get(2)?, r.get(3)?),
                        counts,
                        r.get::<_, Option<bool>>(9)?,
                        r.get::<_, Vec<u8>>(10)?,
                    ))
                },
            )
            .optional()?;
        let Some((ev, status, product, counts, complete, doc)) = row else {
            out.status = Some(EntryStatus::StaleExtractor(0));
            return Ok(());
        };
        out.product = product;
        out.counts = counts;
        out.complete = complete;
        out.versions.0 = Some(ev);
        let refused = status != Status::Accepted.as_str();
        if refused {
            out.refusals = refusals(&doc);
        }
        out.status = Some(if ev != EXTRACTOR_VERSION {
            EntryStatus::StaleExtractor(ev)
        } else if refused {
            EntryStatus::Quarantined
        } else {
            let sv: Option<u32> = self
                .tx
                .query_row(
                    "SELECT signature_version FROM tokens
                     WHERE sha256 = ?1 AND extractor_version = ?2
                     ORDER BY signature_version = ?3 DESC, signature_version DESC LIMIT 1",
                    params![sha, EXTRACTOR_VERSION, SIGNATURE_VERSION],
                    |r| r.get(0),
                )
                .optional()?;
            out.versions.1 = sv;
            let pv: Option<u32> = self
                .tx
                .query_row(
                    "SELECT properties_version FROM properties WHERE sha256 = ?1
                     ORDER BY properties_version = ?2 DESC, properties_version DESC LIMIT 1",
                    params![sha, PROPERTIES_VERSION],
                    |r| r.get(0),
                )
                .optional()?;
            out.versions.2 = pv;
            match (sv, pv) {
                (Some(s), Some(p)) if s == SIGNATURE_VERSION && p == PROPERTIES_VERSION => {
                    EntryStatus::Indexed
                }
                (Some(s), p) if s == SIGNATURE_VERSION => {
                    EntryStatus::StaleProperties(p.unwrap_or_default())
                }
                (other, _) => EntryStatus::StaleSignature(other.unwrap_or_default()),
            }
        });
        Ok(())
    }

    /// ID7 and ID9: what `reference` names. An id names its live entry, or
    /// else its latest tombstone; `ID@rN` any revision the id ever had;
    /// `sha256:` a live entry.
    ///
    /// # Errors
    ///
    /// [`BadName`] in an `Ok`, for a reference that names nothing usable;
    /// the database's otherwise.
    pub fn resolve(&self, reference: &Reference) -> Result<Result<Named, BadName>, Error> {
        Reader { conn: &self.tx }.resolve(reference)
    }

    /// `get`'s view of a named revision: as `list` shows it when current,
    /// with its bytes and file name otherwise.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn entry(&self, named: &Named) -> Result<Listed, Error> {
        let row: Row = self.tx.query_row(
            "SELECT e.id, r.revision, r.sha256, r.file_name, e.deleted_generation,
                    r.superseded_generation, r.pruned_generation
             FROM revision r JOIN entry e ON e.key = r.entry
             WHERE r.key = ?1",
            [named.key],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )?;
        self.listed(row, true)
    }

    /// The history of `id`, across delete and re-ingest (ID6), by revision.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn history(&self, named: &Named) -> Result<Vec<HistoryStep>, Error> {
        let mut stmt = self.tx.prepare(
            "SELECT r.revision, r.sha256, r.added_generation,
                    coalesce((SELECT kind FROM op WHERE op.generation = r.added_generation
                                AND op.entry = r.entry AND op.sha256 = r.sha256), 'ingest'),
                    r.superseded_generation, r.pruned_generation, e.deleted_generation,
                    EXISTS (SELECT 1 FROM blob b WHERE b.sha256 = r.sha256)
             FROM revision r JOIN entry e ON e.key = r.entry
             WHERE r.id_fold = ?1 ORDER BY r.revision",
        )?;
        let rows: Vec<Step> = stmt
            .query_map([&named.fold], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        let mut out = Vec::new();
        for (revision, sha, added, kind, superseded, pruned, deleted, blob) in rows {
            let sha256 = Sha256Hex::parse(&sha).map_err(|_| corrupt("a content key"))?;
            let kept = blob && pruned.is_none() && deleted.is_none();
            out.push(HistoryStep {
                revision,
                sha256: sha256.clone(),
                generation: added,
                event: if kind == "replace" {
                    "replace"
                } else {
                    "ingest"
                },
                bytes_kept: kept,
            });
            if let Some(g) = pruned {
                out.push(HistoryStep {
                    revision,
                    sha256: sha256.clone(),
                    generation: g,
                    event: "prune",
                    bytes_kept: false,
                });
            }
            if let (Some(g), None) = (deleted, superseded.filter(|s| Some(*s) != deleted)) {
                out.push(HistoryStep {
                    revision,
                    sha256,
                    generation: g,
                    event: "delete",
                    bytes_kept: false,
                });
            }
        }
        Ok(out)
    }

    /// PR3 and PR7: the stated properties of `sha256` at this build's
    /// version, checked against their preimage hash; `None` if none were
    /// captured at it.
    ///
    /// # Errors
    ///
    /// [`Error::Corrupt`] for a row whose hash or read-back fails; the
    /// database's otherwise.
    pub fn properties(&self, sha256: &Sha256Hex) -> Result<Option<StatedProperties>, Error> {
        let row: Option<(bool, Vec<u8>, String)> = self
            .tx
            .query_row(
                "SELECT truncated, stated, stated_sha256 FROM properties
                 WHERE sha256 = ?1 AND properties_version = ?2",
                params![sha256.as_str(), PROPERTIES_VERSION],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((truncated, stated, hash)) = row else {
            return Ok(None);
        };
        let damaged = || Error::Corrupt("a properties row");
        if preimage::properties(sha256, PROPERTIES_VERSION, truncated, &stated).as_str() != hash {
            return Err(damaged());
        }
        let read = std::str::from_utf8(&stated)
            .ok()
            .and_then(|t| StatedProperties::from_json(t).ok())
            .filter(|r| r.truncated == truncated)
            .ok_or_else(damaged)?;
        Ok(Some(read))
    }

    /// QR4: the current features row of `sha256`, checked: its preimage
    /// hash, its parse, its version and status, and its part digest
    /// recomputed. The document returned is safe to show or judge.
    ///
    /// # Errors
    ///
    /// [`BadRow`] in an `Ok` for a stale or corrupt row; the database's
    /// otherwise.
    pub fn features(&self, sha256: &Sha256Hex) -> Result<Result<FeaturesDocument, BadRow>, Error> {
        let row: Option<FeaturesRow> = self
            .tx
            .query_row(
                "SELECT sha256, extractor_version, status, part_digest, doc, doc_sha256
                 FROM features WHERE sha256 = ?1 AND extractor_version = ?2",
                params![sha256.as_str(), EXTRACTOR_VERSION],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((sha, ev, status, digest, doc, hash)) = row else {
            let found: Option<u32> = self
                .tx
                .query_row(
                    "SELECT max(extractor_version) FROM features WHERE sha256 = ?1",
                    [sha256.as_str()],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            return Ok(Err(BadRow::Stale {
                found: found.unwrap_or_default(),
            }));
        };
        Ok(check(
            sha256,
            &sha,
            ev,
            &status,
            digest.as_deref(),
            &doc,
            &hash,
        ))
    }
}

/// The reads a reference needs, on any connection: a snapshot's, or a
/// write transaction's, where a change decides on what it reads (DB2).
pub(crate) struct Reader<'c> {
    pub(crate) conn: &'c rusqlite::Connection,
}

impl Reader<'_> {
    /// ID7 and ID9: what `reference` names (see [`Snapshot::resolve`]).
    pub(crate) fn resolve(&self, reference: &Reference) -> Result<Result<Named, BadName>, Error> {
        match reference {
            Reference::Id(id) => self.by_id(id, None),
            Reference::Revision(id, n) => self.by_id(id, Some(*n)),
            Reference::Sha256(prefix) => self.by_content(prefix),
        }
    }

    fn by_id(&self, id: &Id, revision: Option<u32>) -> Result<Result<Named, BadName>, Error> {
        let fold = id.fold();
        let unknown = |this: &Reader<'_>| -> Result<Result<Named, BadName>, Error> {
            Ok(Err(BadName::Unknown {
                asked: id.to_string(),
                suggestions: this.suggestions(&fold)?,
            }))
        };
        let key: Option<i64> = match revision {
            // ID9: any revision the id ever had, across delete (ID6).
            Some(n) => self
                .conn
                .query_row(
                    "SELECT key FROM revision WHERE id_fold = ?1 AND revision = ?2",
                    params![&fold, n],
                    |r| r.get(0),
                )
                .optional()?,
            // The live entry's current revision, or else the latest
            // tombstone's last.
            None => self
                .conn
                .query_row(
                    "SELECT r.key FROM entry e JOIN revision r ON r.entry = e.key
                     WHERE e.id_fold = ?1
                     ORDER BY e.deleted_generation IS NULL DESC, e.key DESC, r.revision DESC
                     LIMIT 1",
                    [&fold],
                    |r| r.get(0),
                )
                .optional()?,
        };
        match (key, revision) {
            (Some(key), _) => self.named(key).map(Ok),
            (None, Some(n)) => {
                let known: bool = self.conn.query_row(
                    "SELECT EXISTS (SELECT 1 FROM entry WHERE id_fold = ?1)",
                    [&fold],
                    |r| r.get(0),
                )?;
                if known {
                    Ok(Err(BadName::NoRevision {
                        id: id.to_string(),
                        asked: n,
                    }))
                } else {
                    unknown(self)
                }
            }
            (None, None) => unknown(self),
        }
    }

    /// The revision of row `key`, with its entry's state and the id's.
    fn named(&self, key: i64) -> Result<Named, Error> {
        let (id, revision, sha, fold, current): (String, i64, String, String, Option<i64>) =
            self.conn.query_row(
                "SELECT e.id, r.revision, r.sha256, r.id_fold,
                        (SELECT c.revision FROM revision c
                         WHERE c.entry = e.key AND c.superseded_generation IS NULL
                           AND e.deleted_generation IS NULL)
                 FROM revision r JOIN entry e ON e.key = r.entry WHERE r.key = ?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )?;
        let id_current: Option<i64> = self
            .conn
            .query_row(
                "SELECT r.revision FROM entry e JOIN revision r
                   ON r.entry = e.key AND r.superseded_generation IS NULL
                 WHERE e.id_fold = ?1 AND e.deleted_generation IS NULL",
                [&fold],
                |r| r.get(0),
            )
            .optional()?;
        let revision_of = |v: i64| u32::try_from(v).map_err(|_| corrupt("a revision"));
        Ok(Named {
            id: stored_id(id)?,
            revision: revision_of(revision)?,
            current: current.map(revision_of).transpose()?,
            id_current: id_current.map(revision_of).transpose()?,
            sha256: Sha256Hex::parse(&sha).map_err(|_| corrupt("a content key"))?,
            key,
            fold,
        })
    }

    fn by_content(&self, prefix: &str) -> Result<Result<Named, BadName>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT r.key, e.id FROM entry e JOIN revision r
               ON r.entry = e.key AND r.superseded_generation IS NULL
             WHERE e.deleted_generation IS NULL AND substr(r.sha256, 1, length(?1)) = ?1
             ORDER BY r.sha256",
        )?;
        let found: Vec<(i64, String)> = stmt
            .query_map([prefix], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let asked = format!("sha256:{prefix}");
        match found.as_slice() {
            [] => Ok(Err(BadName::Unknown {
                asked,
                suggestions: Vec::new(),
            })),
            [(key, _)] => self.named(*key).map(Ok),
            several => Ok(Err(BadName::Ambiguous {
                asked,
                ids: several
                    .iter()
                    .map(|(_, id)| stored_id(id.clone()))
                    .collect::<Result<_, _>>()?,
            })),
        }
    }

    /// ID7: up to five ids, live or not, sharing the longest prefix with
    /// `fold`, case-folded; a stored id that is not one is not suggested.
    fn suggestions(&self, fold: &str) -> Result<Vec<String>, Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT id FROM entry WHERE id_fold != ?1 ORDER BY id_fold, id")?;
        let ids: Vec<String> = stmt
            .query_map([fold], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?
            .into_iter()
            .filter(|id| Id::parse(id).is_ok())
            .collect();
        let common = |id: &str| {
            id.to_ascii_lowercase()
                .bytes()
                .zip(fold.bytes())
                .take_while(|(a, b)| a == b)
                .count()
        };
        let best = ids.iter().map(|id| common(id)).max().unwrap_or_default();
        if best == 0 {
            return Ok(Vec::new());
        }
        Ok(ids
            .into_iter()
            .filter(|id| common(id) == best)
            .take(5)
            .collect())
    }
}

/// QR4's steps 2 to 5 on one row's columns.
pub(crate) fn check(
    key: &Sha256Hex,
    sha: &str,
    ev: u32,
    status: &str,
    digest: Option<&str>,
    doc: &[u8],
    hash: &str,
) -> Result<FeaturesDocument, BadRow> {
    let (Ok(sha), Some(status)) = (Sha256Hex::parse(sha), Status::parse(status)) else {
        return Err(BadRow::Corrupt("a key or status column"));
    };
    let digest = match digest.map(Sha256Hex::parse) {
        None => None,
        Some(Ok(d)) => Some(d),
        Some(Err(_)) => return Err(BadRow::Corrupt("the part digest column")),
    };
    if sha != *key || preimage::features(&sha, ev, status, digest.as_ref(), doc).as_str() != hash {
        return Err(BadRow::Corrupt(
            "the row's hash does not cover its key, status or digest",
        ));
    }
    let text = std::str::from_utf8(doc).map_err(|_| BadRow::Corrupt("not UTF-8"))?;
    let read = FeaturesDocument::from_json(text).map_err(|_| BadRow::Corrupt("does not parse"))?;
    if !read.is_current() || read.extractor_version() != ev {
        return Err(BadRow::Corrupt("its version is not its column's"));
    }
    if (read.part().is_some()) != (status == Status::Accepted) {
        return Err(BadRow::Corrupt("its status is not its column's"));
    }
    if read.part_digest().as_deref() != digest.as_ref().map(Sha256Hex::as_str) {
        return Err(BadRow::Corrupt("part_digest is not the document's"));
    }
    Ok(read)
}

/// The refusals of a stored refused document; empty if it does not parse,
/// which `get`'s checked read reports.
fn refusals(doc: &[u8]) -> Vec<StoredRefusal> {
    let Some(read) = std::str::from_utf8(doc)
        .ok()
        .and_then(|t| FeaturesDocument::from_json(t).ok())
    else {
        return Vec::new();
    };
    read.refusals()
        .iter()
        .map(|r| StoredRefusal {
            code: r.code.name().to_owned(),
            message: r.message.clone(),
            instance: r.instance,
        })
        .collect()
}

/// Whether a refusal list holds an internal one (GT8).
pub fn has_internal(refusals: &[StoredRefusal]) -> bool {
    refusals
        .iter()
        .any(|r| r.code == RefusalCode::Internal.name())
}

/// An id as stored, checked against ID1 like every other column read
/// back: a hand-edited one is a damaged row, never printed raw.
pub(crate) fn stored_id(id: String) -> Result<String, Error> {
    Id::parse(&id).map_err(|_| corrupt("an id"))?;
    Ok(id)
}

fn corrupt(what: &'static str) -> Error {
    Error::Corrupt(what)
}
