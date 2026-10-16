//! `fitment catalog verify` (`docs/research/m5-catalog.md`, VF1 to VF4,
//! IN1 to IN9, CC4): every check in one snapshot, every problem one line;
//! nothing repaired.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use fitment_core::{
    EXTRACTOR_VERSION, FeaturesDocument, IndexTokens, PartSignature, SIGNATURE_VERSION, Sense,
};
use rusqlite::{Connection, OptionalExtension as _, params};

use crate::blobs::{BadBlob, Blobs};
use crate::hex::Sha256Hex;
use crate::preimage::{self, Status};
use crate::{Catalog, Error};

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
        let blobs = Blobs {
            layout: &self.layout,
            fs: self.fs.as_ref(),
        };
        let tx = self.conn.transaction()?;
        let ids: BTreeMap<String, String> = tx
            .prepare("SELECT sha256, id FROM live")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let mut found = Found {
            ids,
            problems: Vec::new(),
        };
        sqlite_checks(&tx, full, &mut found)?;
        let suspect = blob_rows(&tx, &blobs, &mut found)?;
        held(&tx, &mut found)?;
        features_rows(&tx, &mut found)?;
        tokens_rows(&tx, &mut found)?;
        clock(&tx, &mut found)?;
        files(&tx, &blobs, &mut found)?;
        if deep {
            deep_check(&tx, &blobs, &mut found)?;
        }
        drop(tx);
        // CC4: a blob missing or damaged is reported only if, in a fresh
        // snapshot, a row still names it: gc may have released it meanwhile.
        if !suspect.is_empty() {
            let tx = self.conn.transaction()?;
            found.problems.retain(|p| {
                !(matches!(p.kind, Kind::Missing | Kind::Damaged) && suspect.contains(&p.what))
                    || tx
                        .query_row(
                            "SELECT EXISTS (SELECT 1 FROM blob WHERE sha256 = ?1)",
                            [&p.what],
                            |r| r.get::<_, bool>(0),
                        )
                        .unwrap_or(true)
            });
        }
        found.problems.sort();
        found.problems.dedup();
        Ok(found.problems)
    }
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
) -> Result<BTreeSet<String>, Error> {
    let mut stmt = conn.prepare("SELECT sha256, bytes FROM blob ORDER BY sha256")?;
    let all: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut suspect = BTreeSet::new();
    for (text, bytes) in all {
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
            Err(e) => found.add(Kind::Damaged, &text, e.to_string()),
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
        ORDER BY 1";
    for text in rows::<String>(conn, stray)? {
        found.add(
            Kind::Mismatch,
            &text,
            "derived rows for content no current revision names",
        );
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
type FeaturesRow = (String, u32, String, Option<String>, Vec<u8>, String);

/// A tokens row's columns, with its content's key.
type TokensRow = (String, u32, u32, Vec<u8>, String, Option<i64>);

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

/// IN2 and ST7: every file under `blobs/` hashes to a name a row holds;
/// no temporary file is over an hour old.
fn files(conn: &Connection, blobs: &Blobs<'_>, found: &mut Found) -> Result<(), Error> {
    let rowed: BTreeSet<String> = rows::<String>(conn, "SELECT sha256 FROM blob")?
        .into_iter()
        .collect();
    for fan in blobs.layout.fan_outs() {
        let Ok(entries) = blobs.fs.read_dir(&fan) else {
            found.add(
                Kind::Missing,
                &fan.display().to_string(),
                "a fan-out directory",
            );
            continue;
        };
        for path in entries {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match Sha256Hex::parse(&name) {
                Ok(sha) if rowed.contains(sha.as_str()) => {}
                Ok(sha) => match blobs.read(&sha) {
                    Ok(_) => found.add(
                        Kind::Orphan,
                        &name,
                        "a file no row names: fitment catalog gc removes it",
                    ),
                    Err(_) => found.add(
                        Kind::Damaged,
                        &name,
                        "a file no row names, whose SHA-256 is not its name",
                    ),
                },
                Err(_) => found.add(Kind::Orphan, &name, "a file that is not the store's"),
            }
        }
    }
    // The clock only judges a file's age here; nothing stores it.
    #[allow(clippy::disallowed_methods)]
    let now = SystemTime::now();
    let tmp = blobs.layout.tmp();
    for path in blobs.fs.read_dir(&tmp).unwrap_or_default() {
        let old = blobs
            .fs
            .symlink_metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > TEMPORARY_AGE);
        if old {
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
fn deep_check(conn: &Connection, blobs: &Blobs<'_>, found: &mut Found) -> Result<(), Error> {
    for text in rows::<String>(conn, "SELECT DISTINCT sha256 FROM live ORDER BY sha256")? {
        let Ok(sha) = Sha256Hex::parse(&text) else {
            continue;
        };
        let Ok(bytes) = blobs.read(&sha) else {
            continue;
        };
        let read = FeaturesDocument::read(&bytes);
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
    }
    Ok(())
}
