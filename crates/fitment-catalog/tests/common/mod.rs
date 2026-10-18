//! What the store tests share: scratch directories, the committed inputs,
//! and the invariants (`docs/research/m5-catalog.md`, section 4.16).

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use fitment_catalog::preimage::{self, Status};
use fitment_catalog::{Catalog, Options, Sha256Hex};
use fitment_core::{
    EXTRACTOR_VERSION, FeaturesDocument, IndexTokens, PROPERTIES_VERSION, SIGNATURE_VERSION, Sense,
};
use rusqlite::Connection;
use std::io;
use std::sync::Arc;

use fitment_catalog::Point;
use fitment_catalog::fs::CountingFs;

/// A fresh, empty directory for one test.
pub fn scratch(test: &str) -> PathBuf {
    // The process id only keeps two test runs' directories apart.
    #[allow(clippy::disallowed_methods)]
    let dir = std::env::temp_dir().join(format!("fitment-catalog-{test}-{}", std::process::id()));
    if dir.exists() {
        make_writable(&dir);
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Makes `path` look written long ago: an orphan or temporary file a
/// writer abandoned, not one in flight.
pub fn age(path: &Path) {
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86_400))
        .unwrap();
}

/// Clears read-only flags under `dir`, so it can be removed (ST4).
pub fn make_writable(dir: &Path) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            make_writable(&path);
        } else {
            let mut p = fs::metadata(&path).unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            p.set_readonly(false);
            let _ = fs::set_permissions(&path, p);
        }
    }
}

/// fitment-core's committed test files.
pub fn core_tests() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests")
}

/// A committed file, copied into `dir` as `name`.
pub fn copy(from: &str, dir: &Path, name: &str) -> PathBuf {
    let to = dir.join(name);
    fs::copy(core_tests().join(from), &to).unwrap();
    to
}

/// A new catalog in `<scratch>/cat`.
pub fn catalog(scratch: &Path) -> Catalog {
    Catalog::init(&scratch.join("cat"), Options::default())
        .unwrap()
        .0
}

fn rows<T: rusqlite::types::FromSql>(conn: &Connection, sql: &str) -> Vec<T> {
    conn.prepare(sql)
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// IN1 to IN9, as far as ingest reaches them: the problems found, none if
/// the catalog in `root` is consistent.
pub fn invariants(root: &Path) -> Vec<String> {
    let conn = Connection::open(root.join("catalog.db")).unwrap();
    let mut problems = Vec::new();
    blobs_hold(root, &conn, &mut problems);
    features_hold(&conn, &mut problems);
    tokens_hold(&conn, &mut problems);
    properties_hold(&conn, &mut problems);
    // IN8.
    let generation: i64 = conn
        .query_row("SELECT generation FROM clock", [], |r| r.get(0))
        .unwrap();
    let last: i64 = conn
        .query_row("SELECT max(generation) FROM op", [], |r| r.get(0))
        .unwrap();
    if generation != last {
        problems.push(format!("IN8: generation {generation}, last op {last}"));
    }
    let names: Vec<String> = rows(
        &conn,
        "SELECT n.id_fold FROM name n
         WHERE n.last_revision != (SELECT max(revision) FROM revision r WHERE r.id_fold = n.id_fold)",
    );
    if !names.is_empty() {
        problems.push(format!("IN8: last revisions wrong for {names:?}"));
    }
    // IN9: derived rows only for current, live content.
    let stray: Vec<String> = rows(
        &conn,
        "SELECT sha256 FROM features WHERE sha256 NOT IN (SELECT sha256 FROM live)
         UNION SELECT sha256 FROM tokens WHERE sha256 NOT IN (SELECT sha256 FROM live)
         UNION SELECT sha256 FROM properties WHERE sha256 NOT IN (SELECT sha256 FROM live)
         UNION SELECT b.sha256 FROM posting p JOIN blob b ON b.key = p.content
           WHERE b.sha256 NOT IN (SELECT sha256 FROM live)",
    );
    if !stray.is_empty() {
        problems.push(format!("IN9: derived rows for {stray:?}"));
    }
    problems
}

/// IN1, IN2, IN3 and IN6.
fn blobs_hold(root: &Path, conn: &Connection, problems: &mut Vec<String>) {
    let mut stmt = conn.prepare("SELECT sha256, bytes FROM blob").unwrap();
    let blobs: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let store = root.join("blobs").join("sha256");
    for (sha, bytes) in &blobs {
        let path = store.join(&sha[..2]).join(sha);
        match fs::read(&path) {
            Ok(b)
                if Sha256Hex::of(&b).as_str() == sha
                    && i64::try_from(b.len()).ok() == Some(*bytes) => {}
            _ => problems.push(format!("IN1: {sha} has no file of its bytes")),
        }
    }
    for fan in fs::read_dir(&store).unwrap().flatten() {
        for file in fs::read_dir(fan.path()).unwrap().flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            if Sha256Hex::of(&fs::read(file.path()).unwrap()).as_str() != name {
                problems.push(format!("IN2: {name} does not hash to its name"));
            }
        }
    }
    let held: BTreeSet<String> = rows(
        conn,
        "SELECT r.sha256 FROM revision r JOIN entry e ON e.key = r.entry
         WHERE e.deleted_generation IS NULL AND r.pruned_generation IS NULL",
    )
    .into_iter()
    .collect();
    let rowed: BTreeSet<String> = blobs.iter().map(|(s, _)| s.clone()).collect();
    if held != rowed {
        problems.push(format!("IN3: blob rows {rowed:?}, held {held:?}"));
    }
    // ST4: every final blob read-only.
    #[cfg(unix)]
    for (sha, _) in &blobs {
        use std::os::unix::fs::PermissionsExt as _;
        let path = store.join(&sha[..2]).join(sha);
        if let Ok(meta) = fs::metadata(&path) {
            if meta.permissions().mode() & 0o222 != 0 {
                problems.push(format!("ST4: {sha} is writable"));
            }
        }
    }
    let partial = fs::read_dir(root.join("tmp")).unwrap().count();
    if partial > 0 {
        problems.push(format!("IN6: {partial} temporary files"));
    }
}

/// IN5.
fn features_hold(conn: &Connection, problems: &mut Vec<String>) {
    let mut stmt = conn
        .prepare(
            "SELECT sha256, extractor_version, status, part_digest, doc, doc_sha256 FROM features",
        )
        .unwrap();
    let all = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, String>(5)?,
            ))
        })
        .unwrap();
    for row in all {
        let (sha, ev, status, digest, doc, hash) = row.unwrap();
        let status = Status::parse(&status).unwrap();
        let digest = digest.map(|d| Sha256Hex::parse(&d).unwrap());
        let sha = Sha256Hex::parse(&sha).unwrap();
        if preimage::features(&sha, ev, status, digest.as_ref(), &doc).as_str() != hash {
            problems.push(format!("IN5: {sha}'s features hash"));
        }
        let read = FeaturesDocument::from_json(std::str::from_utf8(&doc).unwrap()).unwrap();
        let recomputed = read.part_digest();
        if recomputed.as_deref() != digest.as_ref().map(Sha256Hex::as_str)
            || (read.part().is_some()) != (status == Status::Accepted)
            || read.extractor_version() != ev
        {
            problems.push(format!("IN5: {sha}'s document and columns disagree"));
        }
    }
}

/// PR3: every properties row's hash over its preimage, and one current
/// row for each accepted current features row, none for a refused one.
fn properties_hold(conn: &Connection, problems: &mut Vec<String>) {
    let all: Vec<(String, u32, bool, Vec<u8>, String)> = conn
        .prepare(
            "SELECT sha256, properties_version, truncated, stated, stated_sha256 FROM properties",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (sha, version, truncated, stated, hash) in all {
        let sha = Sha256Hex::parse(&sha).unwrap();
        if preimage::properties(&sha, version, truncated, &stated).as_str() != hash {
            problems.push(format!("PR3: {sha}'s properties hash"));
        }
    }
    let unmatched: Vec<String> = rows(
        conn,
        &format!(
            "SELECT f.sha256 FROM features f WHERE f.extractor_version = {EXTRACTOR_VERSION}
               AND (f.status = 'accepted') != EXISTS (SELECT 1 FROM properties p
                    WHERE p.sha256 = f.sha256 AND p.properties_version = {PROPERTIES_VERSION})"
        ),
    );
    if !unmatched.is_empty() {
        problems.push(format!(
            "PR3: properties rows do not follow status for {unmatched:?}"
        ));
    }
}

/// IN7.
fn tokens_hold(conn: &Connection, problems: &mut Vec<String>) {
    let mut stmt = conn
        .prepare(
            "SELECT t.sha256, t.tokens, t.tokens_sha256, b.key FROM tokens t
             JOIN blob b ON b.sha256 = t.sha256
             WHERE t.extractor_version = ?1 AND t.signature_version = ?2",
        )
        .unwrap();
    let all: Vec<(String, Vec<u8>, String, i64)> = stmt
        .query_map([EXTRACTOR_VERSION, SIGNATURE_VERSION], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (sha, text, hash, key) in all {
        let sha = Sha256Hex::parse(&sha).unwrap();
        let ok =
            preimage::tokens(&sha, EXTRACTOR_VERSION, SIGNATURE_VERSION, &text).as_str() == hash;
        if !ok {
            problems.push(format!("IN7: {sha}'s tokens hash"));
        }
        let tokens = IndexTokens::from_json(std::str::from_utf8(&text).unwrap()).unwrap();
        let mut want: Vec<(i64, i64, i64, u64)> = tokens
            .postings()
            .map(|p| {
                (
                    i64::from(p.sense == Sense::Shaft),
                    p.bin,
                    i64::from(p.stack),
                    p.diameter_mm.to_bits(),
                )
            })
            .collect();
        want.sort_unstable();
        want.dedup();
        let mut have: Vec<(i64, i64, i64, u64)> = conn
            .prepare(
                "SELECT sense, bin, stack, diameter_mm FROM posting
                 WHERE content = ?1 AND extractor_version = ?2 AND signature_version = ?3",
            )
            .unwrap()
            .query_map(
                rusqlite::params![key, EXTRACTOR_VERSION, SIGNATURE_VERSION],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get::<_, f64>(3)?.to_bits(),
                    ))
                },
            )
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        have.sort_unstable();
        if have != want {
            problems.push(format!("IN7: {sha}'s postings are not its tokens'"));
        }
    }
    let refused: Vec<String> = rows(
        conn,
        "SELECT t.sha256 FROM tokens t JOIN features f
           ON f.sha256 = t.sha256 AND f.extractor_version = t.extractor_version
         WHERE f.status != 'accepted'",
    );
    if !refused.is_empty() {
        problems.push(format!("IN7: tokens for refused {refused:?}"));
    }
}

/// A seam the test keeps a handle on.
pub struct Shared(pub Arc<CountingFs>);

impl fitment_catalog::Fs for Shared {
    fn symlink_metadata(&self, path: &Path) -> io::Result<fs::Metadata> {
        self.0.symlink_metadata(path)
    }
    fn read_capped(&self, path: &Path, cap: u64) -> io::Result<fitment_catalog::Content> {
        self.0.read_capped(path, cap)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        self.0.read_dir(path)
    }
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.0.create_dir_all(path)
    }
    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.0.write_new(path, bytes)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.0.rename(from, to)
    }
    fn set_readonly(&self, path: &Path, readonly: bool) -> io::Result<()> {
        self.0.set_readonly(path, readonly)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.0.remove_file(path)
    }
    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        self.0.sync_dir(path)
    }
    fn point(&self, at: Point) -> io::Result<()> {
        self.0.point(at)
    }
}
