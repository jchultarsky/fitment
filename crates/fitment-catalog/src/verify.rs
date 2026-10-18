//! `fitment catalog verify` (`docs/research/m5-catalog.md`, VF1 to VF4,
//! IN1 to IN9, CC4): every check in one snapshot, every problem one line;
//! nothing repaired. No problem names an absolute path or a library
//! error's text (CA4, `docs/research/m6-api.md`): a path is the catalog
//! root's own (`blobs/sha256/ab`), an error its kind.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use fitment_core::{
    EXTRACTOR_VERSION, FeaturesDocument, IndexTokens, PROPERTIES_VERSION, PartSignature,
    SIGNATURE_VERSION, Sense, StatedProperties,
};
use rusqlite::{Connection, OptionalExtension as _, params};

use crate::blobs::{BadBlob, Blobs};
use crate::hex::Sha256Hex;
use crate::observe::{NoObserver, Observer, Progress};
use crate::preimage::{self, Status};
use crate::{Catalog, Error, schema};

/// What kind of problem (VF3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Kind {
    /// SQLite's own checks, or a key no path may be built from.
    Integrity,
    /// A file or row that should be there is not.
    Missing,
    /// A file or row whose hash is not its own.
    Damaged,
    /// A value that differs from what its checked row gives.
    Mismatch,
    /// A file under `blobs/` no row names.
    Orphan,
    /// A temporary file over an hour old.
    Temporary,
}

impl Kind {
    /// The word a line starts with.
    pub fn name(self) -> &'static str {
        match self {
            Self::Integrity => "integrity",
            Self::Missing => "missing",
            Self::Damaged => "damaged",
            Self::Mismatch => "mismatch",
            Self::Orphan => "orphan",
            Self::Temporary => "temporary",
        }
    }
}

/// One problem `verify` found.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub struct Problem {
    /// Its kind.
    pub kind: Kind,
    /// The live entry the content belongs to, if any.
    pub id: Option<String>,
    /// The content, or the file's name, as stored.
    pub what: String,
    /// What is wrong, for people.
    pub detail: String,
}

/// What `verify` found, and the generation it read (CA5's
/// `fitment.verify`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Verified {
    /// The generation of the snapshot it checked (DB8).
    pub generation: i64,
    /// The problems, in a stable order; none for a consistent catalog.
    pub problems: Vec<Problem>,
}

/// A temporary file this old is reported (ST7).
const TEMPORARY_AGE: Duration = Duration::from_secs(3600);

fn rows<T: rusqlite::types::FromSql>(conn: &Connection, sql: &str) -> rusqlite::Result<Vec<T>> {
    conn.prepare(sql)?.query_map([], |r| r.get(0))?.collect()
}

/// Collects problems, naming each content's live entry.
struct Found {
    ids: BTreeMap<String, String>,
    problems: Vec<Problem>,
}

impl Found {
    fn add(&mut self, kind: Kind, what: &str, detail: impl Into<String>) {
        self.problems.push(Problem {
            kind,
            id: self.ids.get(what).cloned(),
            what: what.to_owned(),
            detail: detail.into(),
        });
    }
}

impl Catalog {
    /// `fitment catalog verify`: VF1's checks, `--full` for SQLite's whole
    /// integrity check, `--deep` to re-extract every live content (VF2).
    /// Problems in a stable order.
    ///
    /// # Errors
    ///
    /// The database's, if `verify` itself cannot run.
    pub fn verify(&mut self, deep: bool, full: bool) -> Result<Vec<Problem>, Error> {
        self.verify_observed(deep, full, &NoObserver)
            .map(|verified| verified.problems)
    }

    /// [`Catalog::verify`], with the generation it read, `observer` asked
    /// between phases and before each blob read and told of each (CA2).
    ///
    /// # Errors
    ///
    /// [`Error::Cancelled`] when the observer stops it; the database's, if
    /// `verify` itself cannot run.
    pub fn verify_observed(
        &mut self,
        deep: bool,
        full: bool,
        observer: &dyn Observer,
    ) -> Result<Verified, Error> {
        let blobs = Blobs {
            layout: &self.layout,
            fs: self.fs.as_ref(),
        };
        let tx = self.conn.transaction()?;
        let generation = schema::generation(&tx)?;
        let ids: BTreeMap<String, String> = tx
            .prepare("SELECT sha256, id FROM live")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let mut found = Found {
            ids,
            problems: Vec::new(),
        };
        phase(observer, "database")?;
        sqlite_checks(&tx, full, &mut found)?;
        let suspect = blob_rows(&tx, &blobs, &mut found, observer)?;
        phase(observer, "rows")?;
        held(&tx, &mut found)?;
        features_rows(&tx, &mut found)?;
        tokens_rows(&tx, &mut found)?;
        properties_rows(&tx, &mut found)?;
        clock(&tx, &mut found)?;
        phase(observer, "files")?;
        files(&tx, &blobs, &mut found, observer)?;
        if deep {
            deep_check(&tx, &blobs, &mut found, observer)?;
        }
        drop(tx);
        stop(observer)?;
        recheck(&mut self.conn, &blobs, &suspect, &mut found)?;
        found.problems.sort();
        found.problems.dedup();
        Ok(Verified {
            generation,
            problems: found.problems,
        })
    }
}

/// CA2: [`Error::Cancelled`] once the observer asks verify to stop.
fn stop(observer: &dyn Observer) -> Result<(), Error> {
    if observer.cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// CA2: a phase that reads no blob begins, unless verify is to stop.
fn phase(observer: &dyn Observer, phase: &'static str) -> Result<(), Error> {
    stop(observer)?;
    observer.progress(Progress::Verifying {
        phase,
        done: 0,
        total: 0,
    });
    Ok(())
}

/// CC4: what a concurrent change may have caused is checked again in a
/// fresh snapshot. A blob found missing or damaged is reported only if a
/// row still names it and a second read still fails; an orphan only if
/// still no row names it.
fn recheck(
    conn: &mut Connection,
    blobs: &Blobs<'_>,
    suspect: &BTreeSet<String>,
    found: &mut Found,
) -> Result<(), Error> {
    let tx = conn.transaction()?;
    let rowed = |sha: &str| {
        tx.query_row("SELECT bytes FROM blob WHERE sha256 = ?1", [sha], |r| {
            r.get::<_, i64>(0)
        })
        .optional()
        .unwrap_or(Some(-1))
    };
    found.problems.retain(|p| match p.kind {
        Kind::Missing | Kind::Damaged if suspect.contains(&p.what) => {
            let Some(bytes) = rowed(&p.what) else {
                return false;
            };
            let read = Sha256Hex::parse(&p.what).ok().map(|sha| blobs.read(&sha));
            !matches!(read, Some(Ok(b)) if i64::try_from(b.len()).ok() == Some(bytes))
        }
        Kind::Orphan => Sha256Hex::parse(&p.what).is_err() || rowed(&p.what).is_none(),
        _ => true,
    });
    Ok(())
}

/// `quick_check` (`integrity_check` with `full`) and `foreign_key_check`.
fn sqlite_checks(conn: &Connection, full: bool, found: &mut Found) -> Result<(), Error> {
    let check = if full {
        "PRAGMA integrity_check"
    } else {
        "PRAGMA quick_check"
    };
    for line in rows::<String>(conn, check)? {
        if line != "ok" {
            found.add(Kind::Integrity, "catalog.db", line);
        }
    }
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let broken: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    for table in broken {
        found.add(
            Kind::Integrity,
            "catalog.db",
            format!("{table}: a row whose key names nothing"),
        );
    }
    Ok(())
}

/// IN1: each `blob` row's file, read once and re-hashed, of its length.
/// Returns the contents found missing or damaged, for CC4's recheck.
fn blob_rows(
    conn: &Connection,
    blobs: &Blobs<'_>,
    found: &mut Found,
    observer: &dyn Observer,
) -> Result<BTreeSet<String>, Error> {
    let mut stmt = conn.prepare("SELECT sha256, bytes FROM blob ORDER BY sha256")?;
    let all: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut suspect = BTreeSet::new();
    let total = all.len();
    for (done, (text, bytes)) in all.into_iter().enumerate() {
        stop(observer)?;
        observer.progress(Progress::Verifying {
            phase: "blobs",
            done,
            total,
        });
        // PH1: a path is built from a checked SHA-256 only.
        let Ok(sha) = Sha256Hex::parse(&text) else {
            found.add(
                Kind::Integrity,
                &text,
                "a blob key that is not a SHA-256: no file read",
            );
            continue;
        };
        match blobs.read(&sha) {
            Ok(b) if i64::try_from(b.len()).ok() == Some(bytes) => {}
            Ok(_) => {
                found.add(Kind::Damaged, &text, "the file's length is not its row's");
                suspect.insert(text);
            }
            Err(BadBlob::Missing) => {
                found.add(Kind::Missing, &text, "no file for the content");
                suspect.insert(text);
            }
            Err(BadBlob::Damaged) => {
                found.add(Kind::Damaged, &text, "the file's SHA-256 is not its name");
                suspect.insert(text);
            }
            Err(BadBlob::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                found.add(Kind::Missing, &text, "no file for the content");
                suspect.insert(text);
            }
            Err(e) => {
                found.add(Kind::Damaged, &text, e.kind_text());
                suspect.insert(text);
            }
        }
    }
    Ok(suspect)
}

/// IN3, IN4, IN9: blob rows exactly for contents a current or kept
/// revision of a live entry names; derived rows only for current ones; one
/// current revision per live entry and none for a deleted one.
fn held(conn: &Connection, found: &mut Found) -> Result<(), Error> {
    for text in rows::<String>(conn, "SELECT DISTINCT sha256 FROM revision ORDER BY sha256")? {
        if Sha256Hex::parse(&text).is_err() {
            found.add(
                Kind::Integrity,
                &text,
                "a revision's content that is not a SHA-256",
            );
        }
    }
    let unheld = "SELECT sha256 FROM blob WHERE sha256 NOT IN (
        SELECT r.sha256 FROM revision r JOIN entry e ON e.key = r.entry
        WHERE e.deleted_generation IS NULL AND r.pruned_generation IS NULL) ORDER BY sha256";
    for text in rows::<String>(conn, unheld)? {
        found.add(
            Kind::Mismatch,
            &text,
            "a blob row no kept or current revision names",
        );
    }
    let unrowed = "SELECT DISTINCT r.sha256 FROM revision r JOIN entry e ON e.key = r.entry
        WHERE e.deleted_generation IS NULL AND r.pruned_generation IS NULL
          AND r.sha256 NOT IN (SELECT sha256 FROM blob) ORDER BY r.sha256";
    for text in rows::<String>(conn, unrowed)? {
        found.add(
            Kind::Missing,
            &text,
            "a revision whose content has no blob row",
        );
    }
    let stray = "SELECT sha256 FROM features WHERE sha256 NOT IN (SELECT sha256 FROM live)
        UNION SELECT sha256 FROM tokens WHERE sha256 NOT IN (SELECT sha256 FROM live)
        UNION SELECT sha256 FROM properties WHERE sha256 NOT IN (SELECT sha256 FROM live)
        UNION SELECT b.sha256 FROM posting p JOIN blob b ON b.key = p.content
          WHERE b.sha256 NOT IN (SELECT sha256 FROM live)
        ORDER BY 1";
    for text in rows::<String>(conn, stray)? {
        found.add(
            Kind::Mismatch,
            &text,
            "derived rows for content no current revision names",
        );
    }
    let untokened = "SELECT DISTINCT b.sha256 FROM posting p JOIN blob b ON b.key = p.content
        WHERE NOT EXISTS (SELECT 1 FROM tokens t WHERE t.sha256 = b.sha256
            AND t.extractor_version = p.extractor_version
            AND t.signature_version = p.signature_version)
        ORDER BY 1";
    for text in rows::<String>(conn, untokened)? {
        found.add(Kind::Mismatch, &text, "postings without their tokens row");
    }
    // ID1 and the revision's range: what every other command refuses as a
    // damaged row, verify reports.
    for id in rows::<String>(conn, "SELECT id FROM entry ORDER BY id")? {
        if crate::ids::Id::parse(&id).is_err() {
            found.add(
                Kind::Integrity,
                "entry",
                format!("an id that is not one: {id:?}"),
            );
        }
    }
    let wide = "SELECT id_fold FROM revision WHERE revision > 4294967295 ORDER BY id_fold";
    for fold in rows::<String>(conn, wide)? {
        found.add(Kind::Integrity, &fold, "a revision out of range");
    }
    let shapes = "SELECT e.id FROM entry e WHERE
        (e.deleted_generation IS NULL) != EXISTS (SELECT 1 FROM revision r
            WHERE r.entry = e.key AND r.superseded_generation IS NULL) ORDER BY e.id";
    for id in rows::<String>(conn, shapes)? {
        found.add(
            Kind::Mismatch,
            &id,
            "a live entry without a current revision, or the reverse",
        );
    }
    Ok(())
}

/// IN5 and GT2: each features row's preimage hash; a current one's parse,
/// version, status and recomputed part digest.
fn features_rows(conn: &Connection, found: &mut Found) -> Result<(), Error> {
    let mut stmt = conn.prepare(
        "SELECT sha256, extractor_version, status, part_digest, doc, doc_sha256 FROM features
         ORDER BY sha256, extractor_version",
    )?;
    let all: Vec<FeaturesRow> = stmt
        .query_map([], |r| {
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
    for (text, ev, status, digest, doc, hash) in all {
        let what = format!("features (extractor {ev})");
        let Ok(ev) = u32::try_from(ev) else {
            found.add(Kind::Integrity, &text, format!("{what}: a version column"));
            continue;
        };
        let (Ok(sha), Some(status)) = (Sha256Hex::parse(&text), Status::parse(&status)) else {
            found.add(
                Kind::Integrity,
                &text,
                format!("{what}: a key or status column"),
            );
            continue;
        };
        let digest_hex = match digest.as_deref().map(Sha256Hex::parse) {
            None => None,
            Some(Ok(d)) => Some(d),
            Some(Err(_)) => {
                found.add(
                    Kind::Integrity,
                    &text,
                    format!("{what}: the part digest column"),
                );
                continue;
            }
        };
        if preimage::features(&sha, ev, status, digest_hex.as_ref(), &doc).as_str() != hash {
            found.add(
                Kind::Damaged,
                &text,
                format!("{what}: the row's hash does not cover its key, status or digest"),
            );
            continue;
        }
        if ev != EXTRACTOR_VERSION {
            continue;
        }
        let read = std::str::from_utf8(&doc)
            .ok()
            .and_then(|t| FeaturesDocument::from_json(t).ok());
        let Some(read) = read else {
            found.add(
                Kind::Damaged,
                &text,
                format!("{what}: the document does not read back"),
            );
            continue;
        };
        if !read.is_current() || (read.part().is_some()) != (status == Status::Accepted) {
            found.add(
                Kind::Mismatch,
                &text,
                format!("{what}: the document's version or status is not its row's"),
            );
        }
        if read.part_digest() != digest {
            found.add(
                Kind::Mismatch,
                &text,
                format!("{what}: part_digest is not the document's"),
            );
        }
    }
    Ok(())
}

/// A features row's columns for its checks.
type FeaturesRow = (String, i64, String, Option<String>, Vec<u8>, String);

/// A tokens row's columns, with its content's key.
type TokensRow = (String, i64, i64, Vec<u8>, String, Option<i64>);

/// A properties row as `properties_rows` reads it.
type PropertiesRow = (String, i64, bool, Vec<u8>, String);

/// The postings `tokens` gives, as (sense, bin, stack, diameter's bits).
fn postings_of(tokens: &IndexTokens) -> BTreeSet<(i64, i64, i64, u64)> {
    tokens
        .postings()
        .map(|p| {
            (
                i64::from(p.sense == Sense::Shaft),
                p.bin,
                i64::from(p.stack),
                p.diameter_mm.to_bits(),
            )
        })
        .collect()
}

/// PR3 and GT2: each properties row's preimage hash; a current one's
/// read-back, its bounds and its truncation column; and no row beside a
/// refused document or none.
fn properties_rows(conn: &Connection, found: &mut Found) -> Result<(), Error> {
    let mut stmt = conn.prepare(
        "SELECT sha256, properties_version, truncated, stated, stated_sha256 FROM properties
         ORDER BY sha256, properties_version",
    )?;
    let all: Vec<PropertiesRow> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<Result<_, _>>()?;
    // PR3: properties belong to an accepted document; a row beside a
    // refused one, or beside no document at all, is no capture of ours.
    let orphaned = "SELECT DISTINCT p.sha256 FROM properties p
        WHERE NOT EXISTS (SELECT 1 FROM features f WHERE f.sha256 = p.sha256)
           OR EXISTS (SELECT 1 FROM features f WHERE f.sha256 = p.sha256
                      AND f.extractor_version = ?1 AND f.status != 'accepted')
        ORDER BY 1";
    let mut stmt = conn.prepare(orphaned)?;
    let orphans: Vec<String> = stmt
        .query_map([EXTRACTOR_VERSION], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for text in orphans {
        found.add(
            Kind::Mismatch,
            &text,
            "properties for a refused document, or for none",
        );
    }
    for (text, version, truncated, bytes, hash) in all {
        let what = format!("properties (version {version})");
        let Ok(version) = u32::try_from(version) else {
            found.add(Kind::Integrity, &text, format!("{what}: a version column"));
            continue;
        };
        let Ok(sha) = Sha256Hex::parse(&text) else {
            found.add(Kind::Integrity, &text, format!("{what}: a key column"));
            continue;
        };
        if preimage::properties(&sha, version, truncated, &bytes).as_str() != hash {
            found.add(
                Kind::Damaged,
                &text,
                format!("{what}: the row's hash does not cover its key"),
            );
            continue;
        }
        if version != PROPERTIES_VERSION {
            continue;
        }
        let read = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|t| StatedProperties::from_json(t).ok());
        if read.is_none_or(|r| r.truncated != truncated) {
            found.add(
                Kind::Damaged,
                &text,
                format!("{what}: the stated properties do not read back"),
            );
        }
    }
    Ok(())
}

/// IN7, IX9, review F7: each tokens row's preimage hash and read; the
/// postings of a current one recomputed and compared, bit for bit.
fn tokens_rows(conn: &Connection, found: &mut Found) -> Result<(), Error> {
    let mut stmt = conn.prepare(
        "SELECT t.sha256, t.extractor_version, t.signature_version, t.tokens, t.tokens_sha256,
                b.key
         FROM tokens t LEFT JOIN blob b ON b.sha256 = t.sha256
         ORDER BY t.sha256, t.extractor_version, t.signature_version",
    )?;
    let all: Vec<TokensRow> = stmt
        .query_map([], |r| {
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
    for (text, ev, sv, bytes, hash, key) in all {
        let what = format!("tokens (signature {sv})");
        let (Ok(ev), Ok(sv)) = (u32::try_from(ev), u32::try_from(sv)) else {
            found.add(Kind::Integrity, &text, format!("{what}: a version column"));
            continue;
        };
        let Ok(sha) = Sha256Hex::parse(&text) else {
            found.add(Kind::Integrity, &text, format!("{what}: a key column"));
            continue;
        };
        if preimage::tokens(&sha, ev, sv, &bytes).as_str() != hash {
            found.add(
                Kind::Damaged,
                &text,
                format!("{what}: the row's hash does not cover its key"),
            );
            continue;
        }
        let Some(tokens) = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|t| IndexTokens::from_json(t).ok())
        else {
            found.add(
                Kind::Damaged,
                &text,
                format!("{what}: the tokens do not read back"),
            );
            continue;
        };
        let (Some(key), true) = (key, ev == EXTRACTOR_VERSION && sv == SIGNATURE_VERSION) else {
            continue;
        };
        let mut posted = conn.prepare(
            "SELECT sense, bin, stack, diameter_mm FROM posting
             WHERE content = ?1 AND extractor_version = ?2 AND signature_version = ?3",
        )?;
        let have: BTreeSet<(i64, i64, i64, u64)> = posted
            .query_map(params![key, ev, sv], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get::<_, f64>(3)?.to_bits(),
                ))
            })?
            .collect::<Result<_, _>>()?;
        let want = postings_of(&tokens);
        let (missing, extra) = (
            want.difference(&have).count(),
            have.difference(&want).count(),
        );
        if missing + extra > 0 {
            found.add(
                Kind::Mismatch,
                &text,
                format!(
                    "postings (signature {sv}): {missing} missing, {extra} extra against the tokens"
                ),
            );
        }
    }
    Ok(())
}

/// IN8: the generation and each id's last revision.
fn clock(conn: &Connection, found: &mut Found) -> Result<(), Error> {
    let (generation, last): (i64, Option<i64>) = conn.query_row(
        "SELECT (SELECT generation FROM clock WHERE one = 1), (SELECT max(generation) FROM op)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if Some(generation) != last {
        found.add(
            Kind::Mismatch,
            "clock",
            format!("generation {generation}, but the last op is {last:?}"),
        );
    }
    let names = "SELECT n.id_fold FROM name n WHERE n.last_revision !=
        (SELECT max(revision) FROM revision r WHERE r.id_fold = n.id_fold) ORDER BY 1";
    for fold in rows::<String>(conn, names)? {
        found.add(
            Kind::Mismatch,
            &fold,
            "the name's last revision is not its revisions' last",
        );
    }
    Ok(())
}

/// Whether the file at `path` was last changed over an hour before `now`.
fn older_than_an_hour(blobs: &Blobs<'_>, path: &std::path::Path, now: SystemTime) -> bool {
    blobs
        .fs
        .symlink_metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|m| now.duration_since(m).ok())
        .is_some_and(|age| age > TEMPORARY_AGE)
}

/// IN2 and ST7: every file under `blobs/` over an hour old hashes to a name
/// a row holds (a younger one may be an ingest in flight); no temporary
/// file is over an hour old.
fn files(
    conn: &Connection,
    blobs: &Blobs<'_>,
    found: &mut Found,
    observer: &dyn Observer,
) -> Result<(), Error> {
    let rowed: BTreeSet<String> = rows::<String>(conn, "SELECT sha256 FROM blob")?
        .into_iter()
        .collect();
    // The clock only judges a file's age here; nothing stores it.
    #[allow(clippy::disallowed_methods)]
    let now = SystemTime::now();
    for fan in blobs.layout.fan_outs() {
        stop(observer)?;
        let Ok(entries) = blobs.fs.read_dir(&fan) else {
            // CA4: the directory as the catalog's root names it.
            let name = fan
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            found.add(
                Kind::Missing,
                &format!("blobs/sha256/{name}"),
                "a fan-out directory",
            );
            continue;
        };
        for path in entries {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let sha = Sha256Hex::parse(&name);
            if sha.as_ref().is_ok_and(|s| rowed.contains(s.as_str()))
                || !older_than_an_hour(blobs, &path, now)
            {
                continue;
            }
            match sha.map(|s| blobs.read(&s)) {
                Ok(Ok(_)) => found.add(
                    Kind::Orphan,
                    &name,
                    "a file no row names: fitment catalog gc removes it",
                ),
                // Gone meanwhile: a gc, no damage.
                Ok(Err(BadBlob::Missing)) => {}
                Ok(Err(_)) => found.add(
                    Kind::Damaged,
                    &name,
                    "a file no row names, whose SHA-256 is not its name",
                ),
                Err(_) => found.add(Kind::Orphan, &name, "a file that is not the store's"),
            }
        }
    }
    let tmp = blobs.layout.tmp();
    for path in blobs.fs.read_dir(&tmp).unwrap_or_default() {
        if older_than_an_hour(blobs, &path, now) {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            found.add(
                Kind::Temporary,
                &format!("tmp/{name}"),
                "over an hour old: fitment catalog gc removes it",
            );
        }
    }
    Ok(())
}

/// VF2: every live content extracted again from its checked bytes, its
/// stored document, part digest and tokens required equal, byte for byte.
fn deep_check(
    conn: &Connection,
    blobs: &Blobs<'_>,
    found: &mut Found,
    observer: &dyn Observer,
) -> Result<(), Error> {
    let live = rows::<String>(conn, "SELECT DISTINCT sha256 FROM live ORDER BY sha256")?;
    let total = live.len();
    for (done, text) in live.into_iter().enumerate() {
        stop(observer)?;
        observer.progress(Progress::Verifying {
            phase: "deep",
            done,
            total,
        });
        let Ok(sha) = Sha256Hex::parse(&text) else {
            continue;
        };
        let current: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM features WHERE sha256 = ?1 AND extractor_version = ?2)",
            params![&text, EXTRACTOR_VERSION],
            |r| r.get(0),
        )?;
        // Stale (RX2): reindex's work, which `list` reports; and a blob
        // that cannot be read is reported above.
        let (true, Ok(bytes)) = (current, blobs.read(&sha)) else {
            continue;
        };
        let (read, stated) = FeaturesDocument::read_with_properties(&bytes);
        let stored: Option<Vec<u8>> = conn
            .query_row(
                "SELECT doc FROM features WHERE sha256 = ?1 AND extractor_version = ?2",
                params![&text, EXTRACTOR_VERSION],
                |r| r.get(0),
            )
            .optional()?;
        if stored.as_deref() != Some(read.to_compact_json().as_bytes()) {
            found.add(Kind::Mismatch, &text, format!("features (extractor {EXTRACTOR_VERSION}): the stored document is not the file's"));
        }
        let tokens: Option<Vec<u8>> = conn
            .query_row(
                "SELECT tokens FROM tokens
                 WHERE sha256 = ?1 AND extractor_version = ?2 AND signature_version = ?3",
                params![&text, EXTRACTOR_VERSION, SIGNATURE_VERSION],
                |r| r.get(0),
            )
            .optional()?;
        let want = read.part().map(|p| {
            IndexTokens::of(&PartSignature::of(p))
                .to_json()
                .into_bytes()
        });
        if tokens != want {
            found.add(
                Kind::Mismatch,
                &text,
                format!("tokens (signature {SIGNATURE_VERSION}): not the file's"),
            );
        }
        // A missing row is stale (properties), `list`'s to report (RX2).
        let properties: Option<Vec<u8>> = conn
            .query_row(
                "SELECT stated FROM properties WHERE sha256 = ?1 AND properties_version = ?2",
                params![&text, PROPERTIES_VERSION],
                |r| r.get(0),
            )
            .optional()?;
        let want = read.part().map(|_| stated.to_json().into_bytes());
        if properties.is_some() && properties != want {
            found.add(
                Kind::Mismatch,
                &text,
                format!("properties (version {PROPERTIES_VERSION}): not the file's"),
            );
        }
    }
    Ok(())
}
