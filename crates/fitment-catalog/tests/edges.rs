//! The edges of the store the PR 2a reviews found
//! (`docs/research/m5-catalog.md`, CC6, ST3, ST4, ST7, ST10, ID4, PH2): a
//! busy catalog stops the run; init and open change nothing that is not
//! theirs; a torn marker is an unfinished init; a planted link is unlinked,
//! never followed; a taken temporary name is passed over, never removed;
//! lines keep ID4's order.

mod common;

use std::fs;
use std::io;
use std::sync::Arc;

use common::{Shared, copy, invariants, scratch};
use fitment_catalog::fs::{Call, CountingFs};
use fitment_catalog::{Catalog, Error, Init, Options, Outcome};
use rusqlite::Connection;

/// CC6: a writer that cannot have the lock stops at its first batch, and
/// says why; nothing is committed.
#[test]
fn a_busy_catalog_stops_the_run() {
    let dir = scratch("busy");
    let root = dir.join("cat");
    Catalog::init(&root, Options::default()).unwrap();
    let holder = Connection::open(root.join("catalog.db")).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let options = Options::default().with_busy_timeout_ms(100);
    let mut catalog = Catalog::open(&root, options).unwrap();
    let inputs = [
        copy("candidates/m_bracket.stp", &dir, "a.stp"),
        copy("data/p01_through.stp", &dir, "b.stp"),
    ];
    let run = catalog.ingest(&inputs, None).unwrap();
    assert!(
        matches!(run.stopped, Some(Error::Busy)),
        "{:?}",
        run.stopped
    );
    assert!(
        run.iter()
            .all(|l| matches!(l.outcome, Outcome::Error { .. }))
    );
    holder.execute_batch("ROLLBACK").unwrap();
    assert_eq!(catalog.generation().unwrap(), 1);
}

/// An unreadable marker is an error with its path, not "not a catalog".
#[cfg(unix)]
#[test]
fn an_unreadable_marker_is_an_error() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = scratch("marker");
    let root = dir.join("cat");
    Catalog::init(&root, Options::default()).unwrap();
    let marker = root.join("FITMENT-CATALOG");
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o000)).unwrap();
    let opened = Catalog::open(&root, Options::default());
    let again = Catalog::init(&root, Options::default());
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o444)).unwrap();
    assert!(matches!(opened, Err(Error::Io { path, .. }) if path.ends_with("FITMENT-CATALOG")));
    assert!(matches!(again, Err(Error::Io { .. })));
}

/// ST10: init refuses a directory holding someone's own database, and
/// changes nothing in it: no directories made, its journal untouched.
#[test]
fn init_leaves_a_foreign_database_as_it_was() {
    let dir = scratch("foreign-db");
    let root = dir.join("cat");
    fs::create_dir_all(&root).unwrap();
    let theirs = Connection::open(root.join("catalog.db")).unwrap();
    theirs
        .execute_batch("CREATE TABLE mine (x INTEGER); INSERT INTO mine VALUES (1);")
        .unwrap();
    drop(theirs);
    assert!(matches!(
        Catalog::init(&root, Options::default()),
        Err(Error::NotEmpty(_))
    ));
    let theirs = Connection::open(root.join("catalog.db")).unwrap();
    let mode: String = theirs
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
    assert!(!root.join("blobs").exists() && !root.join("tmp").exists());
}

/// ST10: a `tmp/` of someone's files is not an unfinished init.
#[test]
fn init_refuses_a_directory_whose_tmp_is_not_ours() {
    let dir = scratch("their-tmp");
    let root = dir.join("cat");
    fs::create_dir_all(root.join("tmp")).unwrap();
    fs::write(root.join("tmp/notes.txt"), "mine").unwrap();
    assert!(matches!(
        Catalog::init(&root, Options::default()),
        Err(Error::NotEmpty(_))
    ));
}

/// A marker cut short (a crash while writing it) is an init that stopped:
/// init finishes it, and the catalog opens.
#[test]
fn a_torn_marker_is_an_unfinished_init() {
    let dir = scratch("torn");
    let root = dir.join("cat");
    Catalog::init(&root, Options::default()).unwrap();
    fs::write(root.join("FITMENT-CATALOG"), "fitment.cat").unwrap();
    assert!(matches!(
        Catalog::open(&root, Options::default()),
        Err(Error::NotACatalog(_))
    ));
    let (_, made) = Catalog::init(&root, Options::default()).unwrap();
    assert_eq!(made, Init::Created);
    Catalog::open(&root, Options::default()).unwrap();
    assert_eq!(invariants(&root), Vec::<String>::new());
}

/// PH2 and ST3: a link planted at a blob's name is unlinked and replaced,
/// and what it points to is left alone.
#[cfg(unix)]
#[test]
fn a_link_at_a_blobs_name_is_never_followed() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = scratch("planted");
    let root = dir.join("cat");
    Catalog::init(&root, Options::default()).unwrap();
    let source = copy("candidates/m_bracket.stp", &dir, "m_bracket.stp");
    let secret = dir.join("secret");
    fs::write(&secret, "private").unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let sha = fitment_catalog::Sha256Hex::of(&fs::read(&source).unwrap());
    let blob = root
        .join("blobs/sha256")
        .join(sha.fan_out())
        .join(sha.as_str());
    std::os::unix::fs::symlink(&secret, &blob).unwrap();
    let mut catalog = Catalog::open(&root, Options::default()).unwrap();
    let run = catalog.ingest(&[source], None).unwrap();
    assert_eq!(run[0].outcome, Outcome::Added);
    assert!(run[0].replaced_blob);
    let mode = fs::metadata(&secret).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(fs::read(&secret).unwrap(), b"private");
    assert!(fs::symlink_metadata(&blob).unwrap().is_file());
    drop(catalog);
    assert_eq!(invariants(&root), Vec::<String>::new());
}

/// ST7: a temporary name another file holds is passed over, and that file
/// is never removed.
#[test]
fn a_taken_temporary_name_is_passed_over() {
    let dir = scratch("taken");
    let source = copy("candidates/m_bracket.stp", &dir, "m_bracket.stp");
    let probe_root = dir.join("probe");
    Catalog::init(&probe_root, Options::default()).unwrap();
    let probe = Arc::new(CountingFs::new());
    let mut catalog = Catalog::open_with(
        &probe_root,
        Options::default(),
        Box::new(Shared(probe.clone())),
    )
    .unwrap();
    catalog.ingest(std::slice::from_ref(&source), None).unwrap();
    let k = probe
        .calls()
        .iter()
        .position(|c| matches!(c, Call::WriteNew(_)))
        .unwrap();
    let root = dir.join("cat");
    Catalog::init(&root, Options::default()).unwrap();
    let seam = Arc::new(CountingFs::failing_at(k, io::ErrorKind::AlreadyExists));
    let mut catalog =
        Catalog::open_with(&root, Options::default(), Box::new(Shared(seam.clone()))).unwrap();
    let run = catalog.ingest(&[source], None).unwrap();
    assert_eq!(run[0].outcome, Outcome::Added);
    assert!(!seam.calls().iter().any(|c| matches!(c, Call::Remove(_))));
}

/// ID4: one line per file in ID4's order, whether it failed early or not;
/// ST10 on the files a directory gives, a link into the catalog among them.
#[test]
fn lines_keep_their_order_and_links_inside_are_refused() {
    let dir = scratch("order");
    let mut catalog = common::catalog(&dir);
    let a = copy("candidates/m_bracket.stp", &dir, "a.stp");
    let run = catalog.ingest(&[dir.join("b.stp"), a], None).unwrap();
    let names: Vec<String> = run
        .iter()
        .map(|l| l.input.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["a.stp", "b.stp"]);
    #[cfg(unix)]
    {
        let inputs = dir.join("in");
        fs::create_dir_all(&inputs).unwrap();
        std::os::unix::fs::symlink(catalog.root().join("catalog.db"), inputs.join("db.stp"))
            .unwrap();
        assert!(matches!(
            catalog.ingest(&[inputs], None),
            Err(Error::InsideCatalog(_))
        ));
    }
}
