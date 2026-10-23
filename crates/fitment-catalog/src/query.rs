//! A match against the catalog (`docs/research/m5-catalog.md`, QR1 to
//! QR13, CM2 to CM7, RX1 to RX3, IX9, IX10): level 1 over the stored
//! postings, each survivor's stored document checked and judged as its
//! file would be, in one snapshot. Nothing here reaches a blob (QR11): a
//! [`Snapshot`] holds no way to. With a fit verifier, the one way is the
//! [`FitBlobs`] handed to [`Snapshot::match_catalog_fitted`], read for a
//! candidate only when its fit is asked (`docs/research/m7-fit.md`, FC6,
//! QR13).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicUsize, Ordering};

use fitment_core::{
    Admitted, Blocked, CandidateResult, CandidateSource, EXTRACTOR_VERSION, FeaturesDocument,
    IndexSource, IndexTokens, Matcher, SIGNATURE_VERSION, Sense, Source,
};
use rusqlite::{Connection, OptionalExtension as _, params};

use crate::Error;
use crate::blobs::FitBlobs;
use crate::change::{Unnamed, live_entry};
use crate::hex::Sha256Hex;
use crate::ids::Reference;
use crate::observe::{NoObserver, Observer, Progress};
use crate::preimage;
use crate::snapshot::{Snapshot, check, stored_id};

/// What a catalog run judges (CM2, CM3, QR10).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Selection {
    /// Level 1's survivors among the indexed entries.
    Full,
    /// The entries named, as explicit candidates; level 1 not used.
    Named(Vec<Reference>),
    /// Every live entry, quarantined ones included; level 1 bypassed.
    Exhaustive,
}

/// Why a catalog run judged nothing (QR1; nothing on stdout).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum NotRun {
    /// Entries read by another fitment (RX3; exit 3).
    Stale {
        /// How many.
        count: usize,
        /// The extractor and signature versions found among them.
        extractor: Option<u32>,
        /// The signature version found, for entries stale only on it.
        signature: Option<u32>,
    },
    /// A full run over a catalog with nothing searchable (exit 3).
    Empty {
        /// Live entries.
        entries: usize,
    },
    /// A full run with a blocked socket (IX8; exit 3).
    Blocked {
        /// The socket.
        socket: String,
        /// Why.
        why: Blocked,
    },
    /// A named entry that is not a live entry's current revision (exit 2).
    Unnamed(Vec<Unnamed>),
    /// Its observer stopped it (CA2, `docs/research/m6-api.md`): no
    /// result, not even a partial one.
    Cancelled,
}

/// What a catalog run judged.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CatalogRun {
    /// The snapshot's generation.
    pub generation: i64,
    /// Live entries, and how many are indexed (searched) and quarantined.
    pub entries: usize,
    /// Indexed entries.
    pub indexed: usize,
    /// Quarantined entries.
    pub quarantined: usize,
    /// Level 1's survivors, in a full run.
    pub level_one: Option<usize>,
    /// The distinct entries the selection named or found, before QR4.
    pub selected: usize,
    /// Each candidate judged, with its one label `catalog:<id>@r<n>`
    /// (CM6), for `MatchDocument::new`.
    pub results: Vec<(CandidateResult, Vec<String>)>,
    /// Entries whose stored rows failed QR4: not judged (exit 1).
    pub corrupt: Vec<String>,
    /// How many judgments were made: one per group of equal parts (QR6).
    pub judged: usize,
    /// Entries whose judgment panicked: a fitment bug (exit 1), not
    /// damage in the catalog.
    pub bugs: Vec<String>,
}

/// Core's level 1 over the stored postings and tokens (IX1 to IX3, IX9,
/// IX10), in the run's snapshot. A tokens row whose hash fails is a
/// corrupt row: its part is reported and not judged.
pub(crate) struct SqlSource<'c> {
    conn: &'c Connection,
    corrupt: RefCell<BTreeSet<u64>>,
}

/// SQLite's integer key as level 1's id: never negative (IX10).
fn id(key: i64) -> Result<u64, Error> {
    u64::try_from(key).map_err(|_| Error::Corrupt("a content key"))
}

impl IndexSource for SqlSource<'_> {
    type Error = Error;

    fn postings(
        &self,
        sense: Sense,
        bins: RangeInclusive<i64>,
        each: &mut dyn FnMut(u64, u32, f64),
    ) -> Result<(), Error> {
        let mut stmt = self.conn.prepare(
            "SELECT content, stack, diameter_mm FROM posting
             WHERE extractor_version = ?1 AND signature_version = ?2 AND sense = ?3
               AND bin BETWEEN ?4 AND ?5
             ORDER BY content, stack, diameter_mm",
        )?;
        let rows = stmt.query_map(
            params![
                EXTRACTOR_VERSION,
                SIGNATURE_VERSION,
                u8::from(sense == Sense::Shaft),
                bins.start(),
                bins.end()
            ],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, f64>(2)?,
                ))
            },
        )?;
        for row in rows {
            let (content, stack, diameter) = row?;
            let stack = u32::try_from(stack).map_err(|_| Error::Corrupt("a posting's stack"))?;
            each(id(content)?, stack, diameter);
        }
        Ok(())
    }

    fn tokens(
        &self,
        ids: Option<&[u64]>,
        each: &mut dyn FnMut(u64, &IndexTokens),
    ) -> Result<(), Error> {
        let keys: Vec<i64> = match ids {
            Some(ids) => ids.iter().filter_map(|i| i64::try_from(*i).ok()).collect(),
            None => self
                .conn
                .prepare(
                    "SELECT b.key FROM tokens t JOIN blob b ON b.sha256 = t.sha256
                     WHERE t.extractor_version = ?1 AND t.signature_version = ?2
                     ORDER BY b.key",
                )?
                .query_map(params![EXTRACTOR_VERSION, SIGNATURE_VERSION], |r| r.get(0))?
                .collect::<Result<_, _>>()?,
        };
        let mut stmt = self.conn.prepare(
            "SELECT t.sha256, t.tokens, t.tokens_sha256 FROM tokens t
             JOIN blob b ON b.sha256 = t.sha256
             WHERE b.key = ?1 AND t.extractor_version = ?2 AND t.signature_version = ?3",
        )?;
        for key in keys {
            let k = id(key)?;
            let row: Option<(String, Vec<u8>, String)> = stmt
                .query_row(params![key, EXTRACTOR_VERSION, SIGNATURE_VERSION], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .optional()?;
            match row.and_then(|(sha, text, hash)| checked_tokens(&sha, &text, &hash)) {
                Some(tokens) => each(k, &tokens),
                // Never skipped silently: a part level 1 cannot read is
                // reported, and the run exits 1.
                None => {
                    self.corrupt.borrow_mut().insert(k);
                }
            }
        }
        Ok(())
    }
}

/// IX9: a tokens row whose preimage hash holds and which reads back.
fn checked_tokens(sha: &str, text: &[u8], hash: &str) -> Option<IndexTokens> {
    let sha = Sha256Hex::parse(sha).ok()?;
    let ok = preimage::tokens(&sha, EXTRACTOR_VERSION, SIGNATURE_VERSION, text).as_str() == hash;
    ok.then(|| std::str::from_utf8(text).ok())
        .flatten()
        .and_then(|t| IndexTokens::from_json(t).ok())
}

/// QR4's checked row: the content, its size, its document and the part
/// digest recomputed from it; the only input QR6 and QR7 take.
struct CheckedRow {
    key: u64,
    sha256: Sha256Hex,
    /// QR7: from the checked SHA-256, the blob row's size and the
    /// document's header.
    source: Source,
    document: FeaturesDocument,
    digest: Option<String>,
    label: String,
}

/// The rows QR4 checks: a live entry's current content, by blob key.
type Loaded = (
    String,
    i64,
    u32,
    String,
    Option<String>,
    Vec<u8>,
    String,
    String,
    u32,
);

impl Snapshot<'_> {
    /// `fitment match --catalog`: the socket document's `matcher` against
    /// the catalog, `selection`'s candidates judged on up to `jobs`
    /// threads, in this one snapshot (QR2).
    ///
    /// # Errors
    ///
    /// The database's; a refusal of QR1 is the inner [`NotRun`].
    pub fn match_catalog(
        &self,
        matcher: &Matcher<'_>,
        selection: &Selection,
        jobs: usize,
    ) -> Result<Result<CatalogRun, NotRun>, Error> {
        self.match_catalog_observed(matcher, selection, jobs, &NoObserver)
    }

    /// [`Snapshot::match_catalog`], `observer` asked before the run and
    /// before each group a judging thread takes, and told of each group
    /// judged (CA2): a cancelled run is [`NotRun::Cancelled`], with no
    /// results.
    ///
    /// # Errors
    ///
    /// As [`Snapshot::match_catalog`].
    pub fn match_catalog_observed(
        &self,
        matcher: &Matcher<'_>,
        selection: &Selection,
        jobs: usize,
        observer: &dyn Observer,
    ) -> Result<Result<CatalogRun, NotRun>, Error> {
        self.run(matcher, selection, jobs, observer, None)
    }

    /// [`Snapshot::match_catalog_observed`] with a fit verifier's reader
    /// of stored files (FC6): a candidate's file is read through `blobs`,
    /// hash-checked, only when its fit is asked, and a file that cannot be
    /// read makes its fit `not_verified`, `stored_bytes_unreadable`, and
    /// the run incomplete. Dedup shares nothing when the verifier has an
    /// identity: every member is judged in full with its own file (QR6).
    ///
    /// # Errors
    ///
    /// As [`Snapshot::match_catalog`].
    pub fn match_catalog_fitted(
        &self,
        matcher: &Matcher<'_>,
        selection: &Selection,
        jobs: usize,
        observer: &dyn Observer,
        blobs: &FitBlobs,
    ) -> Result<Result<CatalogRun, NotRun>, Error> {
        self.run(matcher, selection, jobs, observer, Some(blobs))
    }

    fn run(
        &self,
        matcher: &Matcher<'_>,
        selection: &Selection,
        jobs: usize,
        observer: &dyn Observer,
        blobs: Option<&FitBlobs>,
    ) -> Result<Result<CatalogRun, NotRun>, Error> {
        if observer.cancelled() {
            return Ok(Err(NotRun::Cancelled));
        }
        let conn: &Connection = self.connection();
        let (entries, indexed, quarantined) = tally(conn)?;
        let mut run = CatalogRun {
            generation: self.generation(),
            entries,
            indexed,
            quarantined,
            level_one: None,
            selected: 0,
            results: Vec::new(),
            corrupt: Vec::new(),
            judged: 0,
            bugs: Vec::new(),
        };
        let keys = match keys(conn, matcher, selection, &mut run)? {
            Ok(keys) => keys,
            Err(refused) => return Ok(Err(refused)),
        };
        run.selected = keys.len();
        let mut rows = Vec::new();
        for key in keys {
            match load(conn, key)? {
                Ok(row) => rows.push(row),
                Err(id) => run.corrupt.push(id),
            }
        }
        let Some(judged) = judge(matcher, rows, (jobs, blobs), observer) else {
            return Ok(Err(NotRun::Cancelled));
        };
        run.results = judged.results;
        run.judged = judged.made;
        run.corrupt.extend(judged.corrupt);
        run.bugs = judged.bugs;
        run.bugs.sort();
        run.corrupt.sort();
        run.corrupt.dedup();
        Ok(Ok(run))
    }
}

impl Snapshot<'_> {
    /// Level 1 over this snapshot's stored index alone, for CT3's
    /// comparison with the in-memory index.
    ///
    /// # Errors
    ///
    /// The database's.
    #[cfg(feature = "test-support")]
    pub fn admitted(&self, matcher: &Matcher<'_>) -> Result<Admitted, Error> {
        let source = SqlSource {
            conn: self.connection(),
            corrupt: RefCell::new(BTreeSet::new()),
        };
        matcher.admitted(&source)
    }
}

/// Live entries, indexed and quarantined, at this build's versions.
fn tally(conn: &Connection) -> Result<(usize, usize, usize), Error> {
    let (entries, indexed, quarantined): (i64, i64, i64) = conn.query_row(
        "SELECT count(*),
                count(f.sha256) FILTER (WHERE f.status = 'accepted'),
                count(f.sha256) FILTER (WHERE f.status = 'refused')
         FROM live l LEFT JOIN features f ON f.sha256 = l.sha256 AND f.extractor_version = ?1",
        [EXTRACTOR_VERSION],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let n = |v: i64| usize::try_from(v).unwrap_or_default();
    Ok((n(entries), n(indexed), n(quarantined)))
}

/// A live entry's content, whether it has a current features row, whether
/// that is accepted, whether it has current tokens, and the newest versions.
type StaleRow = (String, bool, Option<bool>, bool, Option<u32>, Option<u32>);

/// Labelled results, as `MatchDocument::new` takes them.
type Labelled = Vec<(CandidateResult, Vec<String>)>;

/// RX2 and RX3: the live entries among `shas` (all, for `None`) that are
/// stale, with the versions found.
fn stale(conn: &Connection, shas: Option<&[Sha256Hex]>) -> Result<Option<NotRun>, Error> {
    let mut stmt = conn.prepare(
        "SELECT l.sha256,
                EXISTS (SELECT 1 FROM features f WHERE f.sha256 = l.sha256
                          AND f.extractor_version = ?1),
                (SELECT f.status FROM features f WHERE f.sha256 = l.sha256
                   AND f.extractor_version = ?1) = 'accepted',
                EXISTS (SELECT 1 FROM tokens t WHERE t.sha256 = l.sha256
                          AND t.extractor_version = ?1 AND t.signature_version = ?2),
                (SELECT max(extractor_version) FROM features f WHERE f.sha256 = l.sha256),
                (SELECT max(signature_version) FROM tokens t WHERE t.sha256 = l.sha256)
         FROM live l ORDER BY l.sha256",
    )?;
    let rows: Vec<StaleRow> = stmt
        .query_map(params![EXTRACTOR_VERSION, SIGNATURE_VERSION], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    let (mut count, mut extractor, mut signature) = (0, None, None);
    for (sha, current, accepted, tokened, ev, sv) in rows {
        if shas.is_some_and(|s| !s.iter().any(|x| x.as_str() == sha)) {
            continue;
        }
        if !current {
            count += 1;
            extractor = extractor.max(ev);
        } else if accepted == Some(true) && !tokened {
            count += 1;
            signature = signature.max(sv);
        }
    }
    Ok((count > 0).then_some(NotRun::Stale {
        count,
        extractor,
        signature,
    }))
}

/// The blob keys `selection` judges, after QR1's gates.
fn keys(
    conn: &Connection,
    matcher: &Matcher<'_>,
    selection: &Selection,
    run: &mut CatalogRun,
) -> Result<Result<Vec<u64>, NotRun>, Error> {
    match selection {
        Selection::Full => {
            if let Some(refused) = stale(conn, None)? {
                return Ok(Err(refused));
            }
            if run.indexed == 0 {
                return Ok(Err(NotRun::Empty {
                    entries: run.entries,
                }));
            }
            let source = SqlSource {
                conn,
                corrupt: RefCell::new(BTreeSet::new()),
            };
            match matcher.admitted(&source)? {
                Admitted::Parts { candidates, .. } => {
                    run.level_one = Some(candidates.len());
                    for k in source.corrupt.borrow().iter() {
                        run.corrupt.push(label_of_key(conn, *k)?);
                    }
                    Ok(Ok(candidates))
                }
                Admitted::Blocked { socket, why, .. } => Ok(Err(NotRun::Blocked { socket, why })),
                _ => Err(Error::Corrupt(
                    "an answer of level 1 this build cannot read",
                )),
            }
        }
        Selection::Exhaustive => {
            if let Some(refused) = stale(conn, None)? {
                return Ok(Err(refused));
            }
            // A live entry without its blob row is damage, named, never
            // silently left out of the run meant to miss nothing (QR10).
            let all: Vec<(Option<i64>, String, i64)> = conn
                .prepare(
                    "SELECT b.key, l.id, l.revision FROM live l
                     LEFT JOIN blob b ON b.sha256 = l.sha256 ORDER BY b.key, l.id",
                )?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<_, _>>()?;
            let mut keys = Vec::new();
            for (key, entry, revision) in all {
                match key {
                    Some(key) => keys.push(id(key)?),
                    None => run
                        .corrupt
                        .push(format!("{}@r{revision}", stored_id(entry)?)),
                }
            }
            Ok(Ok(keys))
        }
        Selection::Named(references) => named(conn, references),
    }
}

/// CM3: the named entries' current contents, each a live entry's current
/// revision (ID9), and none stale (RX3).
fn named(conn: &Connection, references: &[Reference]) -> Result<Result<Vec<u64>, NotRun>, Error> {
    let mut shas = Vec::new();
    let mut unnamed = Vec::new();
    for reference in references {
        match live_entry(conn, reference)? {
            Err(why) => unnamed.push(why),
            Ok(n) if !shas.contains(&n.sha256) => shas.push(n.sha256),
            Ok(_) => {}
        }
    }
    if !unnamed.is_empty() {
        return Ok(Err(NotRun::Unnamed(unnamed)));
    }
    if let Some(refused) = stale(conn, Some(&shas))? {
        return Ok(Err(refused));
    }
    let mut keys = Vec::new();
    for sha in &shas {
        let key: i64 = conn.query_row(
            "SELECT key FROM blob WHERE sha256 = ?1",
            [sha.as_str()],
            |r| r.get(0),
        )?;
        keys.push(id(key)?);
    }
    keys.sort_unstable();
    Ok(Ok(keys))
}

/// The label of the live entry whose content is blob `key`.
fn label_of_key(conn: &Connection, key: u64) -> Result<String, Error> {
    let key = i64::try_from(key).map_err(|_| Error::Corrupt("a content key"))?;
    let found: Option<(String, i64)> = conn
        .query_row(
            "SELECT l.id, l.revision FROM live l JOIN blob b ON b.sha256 = l.sha256
             WHERE b.key = ?1",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match found {
        Some((id, revision)) => Ok(format!("{}@r{revision}", stored_id(id)?)),
        None => Ok(format!("content {key}")),
    }
}

/// QR4 for blob `key`: the row's preimage hash, parse, version, status and
/// recomputed digest; `Err` with the entry's label if any fails.
fn load(conn: &Connection, key: u64) -> Result<Result<CheckedRow, String>, Error> {
    let k = i64::try_from(key).map_err(|_| Error::Corrupt("a content key"))?;
    let row: Option<Loaded> = conn
        .query_row(
            "SELECT b.sha256, b.bytes, f.extractor_version, f.status, f.part_digest, f.doc,
                    f.doc_sha256, l.id, l.revision
             FROM blob b JOIN features f ON f.sha256 = b.sha256 AND f.extractor_version = ?2
             JOIN live l ON l.sha256 = b.sha256
             WHERE b.key = ?1",
            params![k, EXTRACTOR_VERSION],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            },
        )
        .optional()?;
    let Some((sha, bytes, ev, status, digest, doc, hash, id, revision)) = row else {
        return Ok(Err(label_of_key(conn, key)?));
    };
    let (Ok(key_hex), Ok(id), Ok(bytes)) = (
        Sha256Hex::parse(&sha),
        stored_id(id.clone()),
        u64::try_from(bytes),
    ) else {
        // The id itself may be the damage: named by its content key.
        return Ok(Err(format!("content {key}")));
    };
    let label = format!("{id}@r{revision}");
    match check(&key_hex, &sha, ev, &status, digest.as_deref(), &doc, &hash) {
        Ok(document) => {
            let digest = document.part_digest();
            let Ok(source) = Source::new(key_hex.as_str(), bytes, document.header().cloned())
            else {
                return Ok(Err(label));
            };
            Ok(Ok(CheckedRow {
                key,
                sha256: key_hex,
                source,
                document,
                digest,
                label,
            }))
        }
        Err(_) => Ok(Err(label)),
    }
}

/// What judging one group's representative came to.
enum Judgment {
    Done(Box<CandidateResult>),
    /// `judge_document` refused it as not current: a damaged row.
    NotCurrent,
    /// The judgment panicked: a fitment bug.
    Panicked,
    /// The observer stopped the run before it.
    Cancelled,
}

/// What judging the groups came to: the results, the judgments made, the
/// entries refused as not current, and those whose judgment panicked.
struct Judged {
    results: Labelled,
    made: usize,
    corrupt: Vec<String>,
    bugs: Vec<String>,
}

/// QR6 to QR8: the rows grouped by their recomputed part digest (only
/// while the fit verifier names no kernel), one representative of each,
/// its lowest key, judged on `jobs` threads; each member gets a copy with
/// its own source. `None` when the observer stopped it (CA2).
fn judge(
    matcher: &Matcher<'_>,
    rows: Vec<CheckedRow>,
    (jobs, blobs): (usize, Option<&FitBlobs>),
    observer: &dyn Observer,
) -> Option<Judged> {
    let share = matcher.fit_kernel().is_none();
    let mut groups: BTreeMap<String, Vec<CheckedRow>> = BTreeMap::new();
    for row in rows {
        let key = match (&row.digest, share) {
            (Some(digest), true) => format!("part {digest}"),
            _ => format!("content {}", row.sha256),
        };
        groups.entry(key).or_default().push(row);
    }
    let mut groups: Vec<Vec<CheckedRow>> = groups.into_values().collect();
    for group in &mut groups {
        group.sort_by_key(|r| r.key);
    }
    let mut judged = Judged {
        results: Vec::new(),
        made: 0,
        corrupt: Vec::new(),
        bugs: Vec::new(),
    };
    let judgments = judge_representatives(matcher, &groups, (jobs, blobs), observer);
    if judgments.iter().any(|j| matches!(j, Judgment::Cancelled)) {
        return None;
    }
    for (group, judgment) in groups.iter().zip(judgments) {
        let labels = group.iter().map(|r| r.label.clone());
        match judgment {
            Judgment::Done(result) => {
                judged.made += 1;
                for row in group {
                    let mut copy = (*result).clone();
                    copy.source = CandidateSource::File(row.source.clone());
                    judged
                        .results
                        .push((copy, vec![format!("catalog:{}", row.label)]));
                }
            }
            Judgment::NotCurrent => judged.corrupt.extend(labels),
            Judgment::Panicked => judged.bugs.extend(labels),
            Judgment::Cancelled => {}
        }
    }
    Some(judged)
}

/// No more threads than this, whatever `jobs` asks.
const MAX_JOBS: usize = 256;

/// One row judged: with `blobs`, its file fetched through them only if its
/// fit is asked (FC6); else from its stored document alone (QR11).
fn judge_row(
    matcher: &Matcher<'_>,
    row: &CheckedRow,
    blobs: Option<&FitBlobs>,
) -> Result<CandidateResult, fitment_core::NotCurrent> {
    let Some(blobs) = blobs else {
        return matcher.judge_document(&row.document, row.source.clone());
    };
    let sha256 = row.sha256.as_str();
    let fetch = || {
        blobs
            .read(sha256)
            .map_err(|e| fitment_core::fit::FetchError::new(e.sentence()))
    };
    matcher.judge_document_with_fetch(&row.document, row.source.clone(), &fetch)
}

/// Each group's representative judged, on up to `jobs` threads, in order.
/// A judgment's panic is caught per group, the same on one thread or
/// many, so the result never depends on `jobs` (QR8). Each thread asks
/// `observer` before each group it takes, and tells it of each judged
/// (CA2); a panic of the observer's goes on to the caller, on one thread
/// or many.
fn judge_representatives(
    matcher: &Matcher<'_>,
    groups: &[Vec<CheckedRow>],
    (jobs, blobs): (usize, Option<&FitBlobs>),
    observer: &dyn Observer,
) -> Vec<Judgment> {
    let total = groups.len();
    let done = AtomicUsize::new(0);
    let one = |group: &Vec<CheckedRow>| {
        if observer.cancelled() {
            return Judgment::Cancelled;
        }
        let Some(row) = group.first() else {
            return Judgment::NotCurrent;
        };
        let judged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            judge_row(matcher, row, blobs)
        }));
        let done = done.fetch_add(1, Ordering::Relaxed) + 1;
        observer.progress(Progress::Judged { done, total });
        match judged {
            Ok(Ok(result)) => Judgment::Done(Box::new(result)),
            Ok(Err(_)) => Judgment::NotCurrent,
            Err(_) => Judgment::Panicked,
        }
    };
    let jobs = jobs.clamp(1, groups.len().clamp(1, MAX_JOBS));
    if jobs == 1 {
        return groups.iter().map(one).collect();
    }
    let chunk = groups.len().div_ceil(jobs);
    std::thread::scope(|scope| {
        let handles: Vec<_> = groups
            .chunks(chunk)
            .map(|part| scope.spawn(move || part.iter().map(one).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            // A judgment's own panic is caught in `one`: what reaches the
            // join is the observer's, which goes on as it does on one
            // thread, never read as the groups' bugs.
            .flat_map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect()
    })
}
