//! CT8 (`docs/research/m5-catalog.md`, RX2 to RX9): catalogs whose rows
//! were written at another extractor or signature version, made by
//! rewriting the version columns with their preimage hashes recomputed,
//! as an older or newer fitment would have written them. A stale catalog
//! refuses a match; `reindex` writes what this build reads, resumes after
//! a kill, and then matches exactly as a fresh catalog does.

mod common;

use std::fs;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{Shared, catalog, core_tests, invariants, scratch};
use fitment_catalog::fs::{Call, CountingFs};
use fitment_catalog::preimage::{self, Status};
use fitment_catalog::{
    Catalog, EntryStatus, NotRun, Options, Point, Rebuilt, Reference, Scope, Selection, Sha256Hex,
    Unnamed,
};
use fitment_core::{
    EXTRACTOR_VERSION, MatchDocument, MatchOptions, MatchPolicy, Matcher, NoFitVerifier,
    SIGNATURE_VERSION, SocketDocument,
};
use rusqlite::{Connection, params};

/// The committed candidates and unit parts, and `variants` copies of the
/// bracket that differ only in their header's file name.
fn inputs(dir: &Path, variants: usize) -> Vec<PathBuf> {
    let mut inputs = vec![core_tests().join("candidates"), core_tests().join("data")];
    let bracket = fs::read_to_string(core_tests().join("candidates/m_bracket.stp")).unwrap();
    let extra = dir.join("variants");
    fs::create_dir_all(&extra).unwrap();
    for n in 0..variants {
        let text = bracket.replacen("FILE_NAME('", &format!("FILE_NAME('v{n}"), 1);
        assert_ne!(text, bracket);
        fs::write(extra.join(format!("v{n:02}.stp")), text).unwrap();
    }
    if variants > 0 {
        inputs.push(extra);
    }
    inputs
}

/// A catalog of [`inputs`], in `scratch(test)/cat`.
fn built(test: &str, variants: usize) -> Catalog {
    let dir = scratch(test);
    let mut catalog = catalog(&dir);
    let run = catalog.ingest(&inputs(&dir, variants), None).unwrap();
    assert!(run.iter().all(|l| l.revision.is_some()), "{run:?}");
    catalog
}

/// A features row's content, status, part digest and document.
type FeaturesRow = (String, String, Option<String>, Vec<u8>);

fn raw(root: &Path) -> Connection {
    let conn = Connection::open(root.join("catalog.db")).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         DROP TRIGGER IF EXISTS features_immutable;
         DROP TRIGGER IF EXISTS tokens_immutable;",
    )
    .unwrap();
    conn
}

/// The insert-only triggers `raw` dropped, back, so that reindex and gc
/// run under them as on any catalog.
fn restore(conn: &Connection) {
    conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS features_immutable BEFORE UPDATE ON features
         BEGIN SELECT RAISE(ABORT, 'features rows are insert-only'); END;
         CREATE TRIGGER IF NOT EXISTS tokens_immutable BEFORE UPDATE ON tokens
         BEGIN SELECT RAISE(ABORT, 'tokens rows are insert-only'); END;",
    )
    .unwrap();
}

/// Every tokens row and posting rewritten as signature version `sv` wrote
/// them.
fn at_signature(root: &Path, sv: u32) {
    let conn = raw(root);
    let rows: Vec<(String, u32, Vec<u8>)> = conn
        .prepare("SELECT sha256, extractor_version, tokens FROM tokens")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (sha, ev, tokens) in rows {
        let hash = preimage::tokens(&Sha256Hex::parse(&sha).unwrap(), ev, sv, &tokens);
        conn.execute(
            "UPDATE tokens SET signature_version = ?1, tokens_sha256 = ?2 WHERE sha256 = ?3",
            params![sv, hash.as_str(), sha],
        )
        .unwrap();
    }
    conn.execute("UPDATE posting SET signature_version = ?1", [sv])
        .unwrap();
    restore(&conn);
}

/// Every features and tokens row and posting rewritten as extractor
/// version `ev` wrote them.
fn at_extractor(root: &Path, ev: u32) {
    let conn = raw(root);
    let rows: Vec<FeaturesRow> = conn
        .prepare("SELECT sha256, status, part_digest, doc FROM features")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (sha, status, digest, doc) in rows {
        let key = Sha256Hex::parse(&sha).unwrap();
        let digest = digest.map(|d| Sha256Hex::parse(&d).unwrap());
        let status = Status::parse(&status).unwrap();
        // The document names the version that wrote it, as the column does.
        let doc = String::from_utf8(doc)
            .unwrap()
            .replacen(
                &format!("\"extractor_version\":{EXTRACTOR_VERSION}"),
                &format!("\"extractor_version\":{ev}"),
                1,
            )
            .into_bytes();
        let hash = preimage::features(&key, ev, status, digest.as_ref(), &doc);
        conn.execute(
            "UPDATE features SET extractor_version = ?1, doc = ?2, doc_sha256 = ?3
             WHERE sha256 = ?4",
            params![ev, doc, hash.as_str(), sha],
        )
        .unwrap();
    }
    let rows: Vec<(String, u32, Vec<u8>)> = conn
        .prepare("SELECT sha256, signature_version, tokens FROM tokens")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (sha, sv, tokens) in rows {
        let hash = preimage::tokens(&Sha256Hex::parse(&sha).unwrap(), ev, sv, &tokens);
        conn.execute(
            "UPDATE tokens SET extractor_version = ?1, tokens_sha256 = ?2 WHERE sha256 = ?3",
            params![ev, hash.as_str(), sha],
        )
        .unwrap();
    }
    conn.execute("UPDATE posting SET extractor_version = ?1", [ev])
        .unwrap();
    restore(&conn);
}

fn matcher(document: &SocketDocument) -> Matcher<'_> {
    Matcher::new(
        document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .unwrap()
}

fn bracket() -> SocketDocument {
    let bytes = fs::read(core_tests().join("assemblies/s01_bolted.stp")).unwrap();
    SocketDocument::extract(&bytes, Some("BRACKET"), None).unwrap()
}

/// A full run's digest, or why it was refused.
fn full_run(catalog: &mut Catalog, matcher: &Matcher<'_>) -> Result<String, NotRun> {
    let snapshot = catalog.snapshot().unwrap();
    let run = snapshot
        .match_catalog(matcher, &Selection::Full, 4)
        .unwrap()?;
    assert!(run.corrupt.is_empty(), "{:?}", run.corrupt);
    Ok(MatchDocument::new(matcher, run.results).digest().to_owned())
}

fn statuses(catalog: &mut Catalog) -> Vec<Option<EntryStatus>> {
    let snapshot = catalog.snapshot().unwrap();
    snapshot
        .list()
        .unwrap()
        .into_iter()
        .map(|l| l.status)
        .collect()
}

/// CT8: a signature rise is rebuilt from the stored documents with no file
/// read, and the catalog then matches as a fresh one does.
#[test]
fn a_signature_rise_reads_no_file() {
    let socket = bracket();
    let matcher = matcher(&socket);
    let mut fresh = built("signature-fresh", 0);
    let want = full_run(&mut fresh, &matcher).unwrap();

    let aged = built("signature", 0);
    let root = aged.root().to_owned();
    drop(aged);
    at_signature(&root, SIGNATURE_VERSION - 1);
    let seam = Arc::new(CountingFs::new());
    let mut catalog =
        Catalog::open_with(&root, Options::default(), Box::new(Shared(seam.clone()))).unwrap();
    assert!(matches!(
        full_run(&mut catalog, &matcher),
        Err(NotRun::Stale { .. })
    ));
    let before = seam.calls().len();
    let run = catalog.reindex(&Scope::Stale).unwrap().unwrap();
    assert!(run.stopped.is_none());
    let reads = seam.calls()[before..]
        .iter()
        .filter(|c| matches!(c, Call::Read(_)))
        .count();
    assert_eq!(reads, 0, "a signature rise reads no file");
    let indexed = run
        .lines
        .iter()
        .filter(|l| l.rebuilt == Rebuilt::Written { blob_read: false })
        .count();
    assert!(indexed > 10, "{run:?}");
    assert!(run.lines.iter().all(|l| !l.left_search()));
    assert_eq!(full_run(&mut catalog, &matcher).unwrap(), want);
    assert_eq!(invariants(&root), Vec::<String>::new());
    // Nothing is left to do: a second run writes nothing.
    let again = catalog.reindex(&Scope::Stale).unwrap().unwrap();
    assert_eq!(again.lines, Vec::new());
}

/// CT8: an extractor rise reads each file through its hash check and
/// extracts it again; `verify --deep` is clean, and `gc --versions` then
/// removes the older rows, which made nothing stale.
#[test]
fn an_extractor_rise_extracts_again() {
    let socket = bracket();
    let matcher = matcher(&socket);
    let mut fresh = built("extractor-fresh", 0);
    let want = full_run(&mut fresh, &matcher).unwrap();

    let mut catalog = built("extractor", 0);
    let root = catalog.root().to_owned();
    at_extractor(&root, EXTRACTOR_VERSION - 1);
    let stale = statuses(&mut catalog);
    assert!(
        stale
            .iter()
            .all(|s| *s == Some(EntryStatus::StaleExtractor(EXTRACTOR_VERSION - 1))),
        "{stale:?}"
    );
    assert!(matches!(
        full_run(&mut catalog, &matcher),
        Err(NotRun::Stale { .. })
    ));
    let run = catalog.reindex(&Scope::Stale).unwrap().unwrap();
    assert!(run.stopped.is_none());
    assert!(
        run.lines
            .iter()
            .all(|l| l.rebuilt == Rebuilt::Written { blob_read: true }),
        "{run:?}"
    );
    assert_eq!(statuses(&mut catalog), statuses(&mut fresh));
    assert_eq!(full_run(&mut catalog, &matcher).unwrap(), want);
    assert_eq!(catalog.verify(true, false).unwrap(), Vec::new());
    assert_eq!(invariants(&root), Vec::<String>::new());

    let old: i64 = Connection::open(root.join("catalog.db"))
        .unwrap()
        .query_row(
            "SELECT count(*) FROM features WHERE extractor_version = ?1",
            [EXTRACTOR_VERSION - 1],
            |r| r.get(0),
        )
        .unwrap();
    assert!(old > 10);
    let collected = catalog.gc(true, false).unwrap();
    assert!(collected.old_rows >= usize::try_from(old).unwrap());
    let conn = Connection::open(root.join("catalog.db")).unwrap();
    let left: i64 = conn
        .query_row(
            "SELECT (SELECT count(*) FROM features WHERE extractor_version != ?1)
                  + (SELECT count(*) FROM tokens WHERE extractor_version != ?1)
                  + (SELECT count(*) FROM posting WHERE extractor_version != ?1)",
            [EXTRACTOR_VERSION],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0);
    assert_eq!(full_run(&mut catalog, &matcher).unwrap(), want);
    assert_eq!(catalog.verify(true, false).unwrap(), Vec::new());
    assert_eq!(invariants(&root), Vec::<String>::new());
}

/// RX2 and RX8: rows a newer fitment wrote make an entry stale too, and
/// neither `reindex` nor `gc --versions` writes, rewrites or deletes them.
#[test]
fn a_newer_version_is_never_touched() {
    let mut catalog = built("newer", 0);
    let root = catalog.root().to_owned();
    at_extractor(&root, EXTRACTOR_VERSION + 1);
    let count = |root: &Path| -> i64 {
        Connection::open(root.join("catalog.db"))
            .unwrap()
            .query_row(
                "SELECT count(*) FROM features WHERE extractor_version = ?1",
                [EXTRACTOR_VERSION + 1],
                |r| r.get(0),
            )
            .unwrap()
    };
    let newer = count(&root);
    let derived = |root: &Path| -> i64 {
        Connection::open(root.join("catalog.db"))
            .unwrap()
            .query_row(
                "SELECT (SELECT count(*) FROM tokens WHERE extractor_version = ?1)
                      + (SELECT count(*) FROM posting WHERE extractor_version = ?1)",
                [EXTRACTOR_VERSION + 1],
                |r| r.get(0),
            )
            .unwrap()
    };
    let newer_derived = derived(&root);
    assert!(newer_derived > 0);
    assert!(
        statuses(&mut catalog)
            .iter()
            .all(|s| *s == Some(EntryStatus::StaleExtractor(EXTRACTOR_VERSION + 1)))
    );
    let run = catalog.reindex(&Scope::Stale).unwrap().unwrap();
    assert!(run.stopped.is_none());
    assert!(
        statuses(&mut catalog)
            .iter()
            .all(|s| !matches!(s, Some(EntryStatus::StaleExtractor(_))))
    );
    catalog.gc(true, false).unwrap();
    assert_eq!(count(&root), newer);
    assert_eq!(derived(&root), newer_derived);
    assert_eq!(catalog.verify(false, false).unwrap(), Vec::new());
}

/// CT8 and RX7: a reindex killed in its second batch keeps its first, and
/// the match stays refused; a rerun does exactly what is missing, and the
/// catalog then matches as a fresh one does.
#[test]
fn a_killed_reindex_resumes() {
    let socket = bracket();
    let matcher = matcher(&socket);
    let mut fresh = built("killed-fresh", 30);
    let want = full_run(&mut fresh, &matcher).unwrap();

    let aged = built("killed", 30);
    let root = aged.root().to_owned();
    drop(aged);
    at_extractor(&root, EXTRACTOR_VERSION - 1);
    let commits = Arc::new(AtomicUsize::new(0));
    let seen = commits.clone();
    let seam = Arc::new(CountingFs::with_hook(move |call| {
        let commit = *call == Call::Point(Point::BeforeCommit);
        assert!(
            !(commit && seen.fetch_add(1, Ordering::SeqCst) == 1),
            "killed before the second batch commits"
        );
    }));
    let mut catalog =
        Catalog::open_with(&root, Options::default(), Box::new(Shared(seam))).unwrap();
    let killed = std::panic::catch_unwind(AssertUnwindSafe(|| catalog.reindex(&Scope::Stale)));
    assert!(killed.is_err());
    drop(catalog);
    assert_eq!(commits.load(Ordering::SeqCst), 2);

    let mut catalog = Catalog::open(&root, Options::default()).unwrap();
    let after = statuses(&mut catalog);
    let done = after
        .iter()
        .filter(|s| !matches!(s, Some(EntryStatus::StaleExtractor(_))))
        .count();
    assert!(done > 0 && done < after.len(), "{done} of {}", after.len());
    assert!(matches!(
        full_run(&mut catalog, &matcher),
        Err(NotRun::Stale { .. })
    ));
    assert_eq!(invariants(&root), Vec::<String>::new());

    let rerun = catalog.reindex(&Scope::Stale).unwrap().unwrap();
    assert!(rerun.stopped.is_none());
    assert_eq!(rerun.lines.len(), after.len() - done);
    assert_eq!(full_run(&mut catalog, &matcher).unwrap(), want);
    assert_eq!(catalog.verify(true, false).unwrap(), Vec::new());
    assert_eq!(invariants(&root), Vec::<String>::new());
}

/// Rewrites `to`'s current features row as `from`'s document read at
/// version `ev`, the current row of `to` removed: as an older gate that
/// judged `to` as it judges `from` would have left it.
fn judged_as(root: &Path, to: &str, from: &str, ev: u32) {
    let conn = raw(root);
    let sha = |id: &str| -> String {
        conn.query_row("SELECT sha256 FROM live WHERE id = ?1", [id], |r| r.get(0))
            .unwrap()
    };
    let (to, from) = (sha(to), sha(from));
    let (status, digest, doc): (String, Option<String>, Vec<u8>) = conn
        .query_row(
            "SELECT status, part_digest, doc FROM features WHERE sha256 = ?1",
            [&from],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    conn.execute("DELETE FROM features WHERE sha256 = ?1", [&to])
        .unwrap();
    conn.execute("DELETE FROM tokens WHERE sha256 = ?1", [&to])
        .unwrap();
    conn.execute(
        "DELETE FROM posting WHERE content = (SELECT key FROM blob WHERE sha256 = ?1)",
        [&to],
    )
    .unwrap();
    let key = Sha256Hex::parse(&to).unwrap();
    let parsed = Status::parse(&status).unwrap();
    let digest_hex = digest.as_deref().map(|d| Sha256Hex::parse(d).unwrap());
    let hash = preimage::features(&key, ev, parsed, digest_hex.as_ref(), &doc);
    conn.execute(
        "INSERT INTO features SELECT ?1, ?2, status, encoding, doc, ?3, part_digest, product_id,
                product_name, holes, shafts, planes, cones, other_faces, complete, first_refusal
         FROM features WHERE sha256 = ?4",
        params![to, ev, hash.as_str(), from],
    )
    .unwrap();
    restore(&conn);
}

/// Gives `to` the properties row of `from`, hashed as its own: what an
/// older gate that accepted `to` would have stored.
fn stated_like(root: &Path, to: &str, from: &str) {
    let conn = Connection::open(root.join("catalog.db")).unwrap();
    let sha = |id: &str| -> String {
        conn.query_row("SELECT sha256 FROM live WHERE id = ?1", [id], |r| r.get(0))
            .unwrap()
    };
    let (to, from) = (sha(to), sha(from));
    let (truncated, stated): (bool, Vec<u8>) = conn
        .query_row(
            "SELECT truncated, stated FROM properties WHERE sha256 = ?1",
            [&from],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let key = Sha256Hex::parse(&to).unwrap();
    let hash = preimage::properties(&key, fitment_core::PROPERTIES_VERSION, truncated, &stated);
    conn.execute(
        "INSERT INTO properties VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            to,
            fitment_core::PROPERTIES_VERSION,
            truncated,
            stated,
            hash.as_str()
        ],
    )
    .unwrap();
}

/// RX6 and RX9: a quarantined entry the gate now accepts is searched; an
/// indexed one it now refuses leaves the search, loudly, with its first
/// reason, and the properties its old acceptance left go with it (PR3, the
/// PR 6 review), so `verify` passes at once.
#[test]
fn transitions_are_reported() {
    let mut catalog = built("transitions", 0);
    let root = catalog.root().to_owned();
    // r04_assembly is refused, the brackets accepted; each rewrite reads
    // a row the other left alone.
    judged_as(&root, "m_bracket", "r04_assembly", EXTRACTOR_VERSION - 1);
    judged_as(
        &root,
        "r04_assembly",
        "m_bracket_lug",
        EXTRACTOR_VERSION - 1,
    );
    stated_like(&root, "r04_assembly", "m_bracket_lug");
    assert_eq!(catalog.verify(false, false).unwrap(), Vec::new());
    let run = catalog.reindex(&Scope::Stale).unwrap().unwrap();
    let line = |id: &str| run.lines.iter().find(|l| l.id == id).unwrap();
    let joined = line("m_bracket");
    assert!(
        !joined.searched_before && joined.joined_search(),
        "{joined:?}"
    );
    assert_eq!(joined.after, Some(EntryStatus::Indexed));
    let left = line("r04_assembly");
    assert!(left.left_search(), "{left:?}");
    assert!(
        left.first_refusal
            .as_deref()
            .is_some_and(|r| r.starts_with("assembly")),
        "{left:?}"
    );
    assert_eq!(run.lines.len(), 2);
    assert_eq!(catalog.verify(false, false).unwrap(), Vec::new());
    // The older rows, another part's document under each key, go.
    assert!(catalog.gc(true, false).unwrap().old_rows >= 2);
    assert_eq!(invariants(&root), Vec::<String>::new());
}

/// RX5: a missing or damaged file is reported, nothing of it is written,
/// and it stays stale, so a full run keeps refusing; the others are done.
#[test]
fn a_damaged_file_stays_stale() {
    let socket = bracket();
    let matcher = matcher(&socket);
    let mut catalog = built("damaged", 0);
    let root = catalog.root().to_owned();
    at_extractor(&root, EXTRACTOR_VERSION - 1);
    let sha: String = Connection::open(root.join("catalog.db"))
        .unwrap()
        .query_row("SELECT sha256 FROM live WHERE id = 'm_bracket'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let blob = |sha: &str| root.join("blobs/sha256").join(&sha[..2]).join(sha);
    common::make_writable(&root.join("blobs"));
    fs::write(blob(&sha), "not the bytes").unwrap();
    let gone: String = Connection::open(root.join("catalog.db"))
        .unwrap()
        .query_row(
            "SELECT sha256 FROM live WHERE id = 'p01_through'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    fs::remove_file(blob(&gone)).unwrap();
    let run = catalog.reindex(&Scope::Stale).unwrap().unwrap();
    let damaged: Vec<&str> = run
        .lines
        .iter()
        .filter(|l| matches!(l.rebuilt, Rebuilt::Damaged(_)))
        .map(|l| l.id.as_str())
        .collect();
    assert_eq!(damaged, ["m_bracket", "p01_through"]);
    let statuses = statuses(&mut catalog);
    let stale = statuses
        .iter()
        .filter(|s| matches!(s, Some(EntryStatus::StaleExtractor(_))))
        .count();
    assert_eq!(stale, 2);
    assert!(matches!(
        full_run(&mut catalog, &matcher),
        Err(NotRun::Stale { count: 2, .. })
    ));
}

/// RX9 and ID9: named entries are processed current or not; an unknown
/// one, or an `ID@rN` that is not current, refuses the whole run.
#[test]
fn named_entries_and_their_refusals() {
    let mut catalog = built("named", 0);
    let named = |text: &str| Scope::Named(vec![Reference::parse(text).unwrap()]);
    let run = catalog.reindex(&named("m_bracket")).unwrap().unwrap();
    assert_eq!(run.lines.len(), 1);
    assert_eq!(run.lines[0].rebuilt, Rebuilt::Current);
    // CR10: a quarantined entry named is gated by this build already, so
    // nothing is missing.
    let run = catalog.reindex(&named("r04_assembly")).unwrap().unwrap();
    assert_eq!(run.lines[0].rebuilt, Rebuilt::Current);
    assert_eq!(run.lines[0].after, Some(EntryStatus::Quarantined));
    assert!(matches!(
        catalog.reindex(&named("nothing")).unwrap(),
        Err(Unnamed::Name(_))
    ));
    assert!(matches!(
        catalog.reindex(&named("m_bracket@r2")).unwrap(),
        Err(Unnamed::Name(_) | Unnamed::NotCurrent { .. })
    ));
    let all = catalog.reindex(&Scope::All).unwrap().unwrap();
    assert!(all.lines.iter().all(|l| l.rebuilt == Rebuilt::Current));
    assert!(all.lines.len() > 30);
}

/// M5-26 and RX4: entries without current properties are stale
/// (properties); reindex captures them again from their files.
#[test]
fn properties_are_captured_again() {
    let mut catalog = built("properties", 0);
    let root = catalog.root().to_owned();
    Connection::open(root.join("catalog.db"))
        .unwrap()
        .execute("DELETE FROM properties", [])
        .unwrap();
    assert!(statuses(&mut catalog).contains(&Some(EntryStatus::StaleProperties(0))));
    let run = catalog.reindex(&Scope::Stale).unwrap().unwrap();
    assert!(
        run.lines
            .iter()
            .all(|l| l.rebuilt == Rebuilt::Written { blob_read: true })
    );
    assert!(
        statuses(&mut catalog)
            .iter()
            .all(|s| !matches!(s, Some(EntryStatus::StaleProperties(_))))
    );
    assert_eq!(invariants(&root), Vec::<String>::new());
}

/// Three parts, one refused, read by the previous extractor.
fn small(test: &str) -> PathBuf {
    let dir = scratch(test);
    let mut catalog = catalog(&dir);
    let inputs = [
        common::copy("candidates/m_bracket.stp", &dir, "m_bracket.stp"),
        common::copy("data/p01_through.stp", &dir, "p01.stp"),
        common::copy("data/r04_assembly.stp", &dir, "r04.stp"),
    ];
    catalog.ingest(&inputs, None).unwrap();
    let root = catalog.root().to_owned();
    drop(catalog);
    at_extractor(&root, EXTRACTOR_VERSION - 1);
    root
}

/// What a reindex writes: each row's key and version, to compare a
/// faulted and rerun reindex with an unfaulted one.
fn rows(root: &Path) -> Vec<String> {
    let conn = Connection::open(root.join("catalog.db")).unwrap();
    conn.prepare(
        "SELECT 'f ' || sha256 || ' ' || extractor_version || ' ' || doc_sha256 FROM features
         UNION ALL SELECT 't ' || sha256 || ' ' || extractor_version || ' ' || signature_version
                   || ' ' || tokens_sha256 FROM tokens
         UNION ALL SELECT 'p ' || sha256 || ' ' || properties_version || ' ' || stated_sha256
                   FROM properties
         UNION ALL SELECT 'n ' || count(*) FROM posting
         ORDER BY 1",
    )
    .unwrap()
    .query_map([], |r| r.get(0))
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

/// CT13 for a reindex batch: a fault at any file call leaves each entry
/// written whole or not at all (a fault at a file's read fails that entry
/// alone), the invariants holding, and a rerun completes it.
#[test]
fn a_faulted_reindex_batch_is_all_or_nothing() {
    let probe = small("fault-probe");
    let seam = Arc::new(CountingFs::new());
    let mut catalog =
        Catalog::open_with(&probe, Options::default(), Box::new(Shared(seam.clone()))).unwrap();
    let opened = seam.calls().len();
    let g0 = catalog.generation().unwrap();
    let run = catalog.reindex(&Scope::Stale).unwrap().unwrap();
    assert!(run.stopped.is_none());
    let total = seam.calls().len();
    assert_eq!(catalog.generation().unwrap(), g0 + 1, "one batch");
    drop(catalog);
    let after = rows(&probe);
    for k in opened..total {
        for kind in [std::io::ErrorKind::StorageFull, std::io::ErrorKind::Other] {
            let root = small(&format!("fault-{k}-{kind:?}"));
            let mut catalog = Catalog::open_with(
                &root,
                Options::default(),
                Box::new(CountingFs::failing_at(k, kind)),
            )
            .unwrap();
            let _ = catalog.reindex(&Scope::Stale).unwrap();
            let g = catalog.generation().unwrap();
            drop(catalog);
            assert!(g == g0 || g == g0 + 1, "k {k}: generation {g}");
            assert_eq!(invariants(&root), Vec::<String>::new(), "k {k} {kind:?}");
            let mut again = Catalog::open(&root, Options::default()).unwrap();
            let rerun = again.reindex(&Scope::Stale).unwrap().unwrap();
            assert!(rerun.stopped.is_none(), "k {k}: {rerun:?}");
            drop(again);
            assert_eq!(rows(&root), after, "k {k} {kind:?}: rerun");
        }
    }
}

/// RX10: no stored row depends on the match or socket rules: no verdict
/// is cached and no socket stored, so neither version can make a catalog
/// stale.
#[test]
fn nothing_stored_names_the_match_or_socket_rules() {
    let catalog = built("rules", 0);
    let conn = Connection::open(catalog.root().join("catalog.db")).unwrap();
    let schema: Vec<String> = conn
        .prepare("SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let schema = schema.join("\n").to_ascii_lowercase();
    for word in ["rules", "verdict", "socket"] {
        assert!(!schema.contains(word), "{word}");
    }
}
