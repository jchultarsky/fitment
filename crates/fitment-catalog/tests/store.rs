//! The store and ingest (`docs/research/m5-catalog.md`, M5 PR 2's tests):
//! init and open (ST10, DB5, PH2), ingest of every committed part (GT1,
//! GT2, CT5), ids and their order (ID3 to ID5, CT16's prototype set),
//! refusals at the door (GT3, SZ1, PH4), hostile names (PH3, CT15), and the
//! one read of each source (ST2). Every catalog is held to the invariants
//! (section 4.16).

mod common;

use std::fs;
use std::path::PathBuf;

use common::{catalog, copy, core_tests, invariants, scratch};
use fitment_catalog::fs::{Call, CountingFs};
use fitment_catalog::{Catalog, Error, Id, Ingested, Init, Options, Outcome, Sha256Hex, Status};
use fitment_core::FeaturesDocument;
use rusqlite::Connection;

fn outcomes(lines: &[Ingested]) -> Vec<(String, Outcome)> {
    lines
        .iter()
        .map(|l| {
            let name = l.input.file_name().unwrap().to_string_lossy().into_owned();
            (name, l.outcome.clone())
        })
        .collect()
}

/// ST10, DB5, CL1: init twice; a foreign directory refused; a plain one not
/// a catalog; a newer schema refused.
#[test]
fn init_makes_a_catalog_once_and_refuses_what_is_not_one() {
    let dir = scratch("init");
    let root = dir.join("cat");
    let (_, made) = Catalog::init(&root, Options::default()).unwrap();
    assert_eq!(made, Init::Created);
    let (catalog, again) = Catalog::init(&root, Options::default()).unwrap();
    assert_eq!(again, Init::Unchanged);
    assert_eq!(catalog.generation().unwrap(), 1);
    drop(catalog);
    assert_eq!(invariants(&root), Vec::<String>::new());

    let foreign = dir.join("foreign");
    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join("notes.txt"), "mine").unwrap();
    assert!(matches!(
        Catalog::init(&foreign, Options::default()),
        Err(Error::NotEmpty(_))
    ));
    assert!(matches!(
        Catalog::open(&foreign, Options::default()),
        Err(Error::NotACatalog(_))
    ));

    let conn = Connection::open(root.join("catalog.db")).unwrap();
    conn.pragma_update(None, "user_version", 2).unwrap();
    drop(conn);
    assert!(matches!(
        Catalog::open(&root, Options::default()),
        Err(Error::Newer { found: 2, ours: 1 })
    ));
}

/// PH2: a planted link where a fan-out directory must be is refused.
#[cfg(unix)]
#[test]
fn a_planted_link_is_refused() {
    let dir = scratch("link");
    let root = dir.join("cat");
    drop(catalog(&dir));
    let fan = root.join("blobs/sha256/ab");
    fs::remove_dir(&fan).unwrap();
    fs::create_dir_all(dir.join("elsewhere")).unwrap();
    std::os::unix::fs::symlink(dir.join("elsewhere"), &fan).unwrap();
    assert!(matches!(
        Catalog::open(&root, Options::default()),
        Err(Error::Linked(p)) if p == fan
    ));
}

/// GT1, GT2, CT5: every committed part and candidate is added with the
/// gate's status, its stored document is the file's, byte for byte, and a
/// second ingest changes nothing.
#[test]
fn every_committed_part_is_stored_as_the_gate_reads_it() {
    let dir = scratch("unit");
    let mut catalog = catalog(&dir);
    let inputs = [core_tests().join("data"), core_tests().join("candidates")];
    let lines = catalog.ingest(&inputs, None).unwrap();
    assert_eq!(lines.len(), 37);
    let conn = Connection::open(catalog.root().join("catalog.db")).unwrap();
    for line in &lines {
        assert_eq!(line.outcome, Outcome::Added, "{}", line.input.display());
        assert_eq!(line.revision, Some(1));
        let bytes = fs::read(&line.input).unwrap();
        let read = FeaturesDocument::read(&bytes);
        let summary = line.summary.as_ref().unwrap();
        assert_eq!(summary.status == Status::Accepted, read.part().is_some());
        let doc: Vec<u8> = conn
            .query_row(
                "SELECT doc FROM features WHERE sha256 = ?1",
                [Sha256Hex::of(&bytes).as_str()],
                |r| r.get(0),
            )
            .unwrap();
        // Stored as core's compact form, exactly (GT2), and reading back
        // as the file reads.
        assert_eq!(
            doc,
            read.to_compact_json().into_bytes(),
            "{}",
            line.input.display()
        );
        let stored = FeaturesDocument::from_json(std::str::from_utf8(&doc).unwrap()).unwrap();
        assert_eq!(stored.to_json(), read.to_json(), "{}", line.input.display());
    }
    let r04 = lines
        .iter()
        .find(|l| l.input.ends_with("r04_assembly.stp"))
        .unwrap();
    let refusal = r04
        .summary
        .as_ref()
        .unwrap()
        .first_refusal
        .as_deref()
        .unwrap();
    assert!(refusal.starts_with("assembly: "), "{refusal}");
    let generation = catalog.generation().unwrap();
    let again = catalog.ingest(&inputs, None).unwrap();
    assert!(again.iter().all(|l| l.outcome == Outcome::Unchanged));
    assert_eq!(catalog.generation().unwrap(), generation);
    assert_eq!(invariants(catalog.root()), Vec::<String>::new());
}

/// ID3 to ID5 and CT16, on C's prototype set: exact stems first, so
/// `a_b.stp` keeps its id and `a b.stp` conflicts; the copy of the bracket
/// is a duplicate; junk and an assembly are quarantined.
#[test]
fn ids_go_to_exact_stems_first() {
    let dir = scratch("ids");
    let inputs = dir.join("in");
    fs::create_dir_all(&inputs).unwrap();
    copy("candidates/m_bracket.stp", &inputs, "m_bracket.stp");
    copy("candidates/m_bracket.stp", &inputs, "copy of m_bracket.stp");
    copy("data/p03_blind_flat.stp", &inputs, "a b.stp");
    copy("data/p01_through.stp", &inputs, "a_b.stp");
    copy("data/r04_assembly.stp", &inputs, "r04_assembly.stp");
    fs::write(inputs.join("junk.stp"), "not STEP at all").unwrap();
    let mut catalog = catalog(&dir);
    let lines = catalog.ingest(std::slice::from_ref(&inputs), None).unwrap();
    let conflict = Outcome::Conflict {
        reason: "the id a_b is taken by another file (give --id)".into(),
    };
    assert_eq!(
        outcomes(&lines),
        [
            ("a_b.stp".into(), Outcome::Added),
            ("junk.stp".into(), Outcome::Added),
            ("m_bracket.stp".into(), Outcome::Added),
            ("r04_assembly.stp".into(), Outcome::Added),
            ("a b.stp".into(), conflict),
            (
                "copy of m_bracket.stp".into(),
                Outcome::Duplicate {
                    of: "m_bracket".into()
                }
            ),
        ]
    );
    let quarantined: Vec<&str> = lines
        .iter()
        .filter(|l| {
            l.summary
                .as_ref()
                .is_some_and(|s| s.status == Status::Refused)
        })
        .map(|l| l.id.as_ref().unwrap().as_str())
        .collect();
    assert_eq!(quarantined, ["junk", "r04_assembly"]);
    // The same bytes under a given id are refused; --id names one file.
    let bracket = inputs.join("m_bracket.stp");
    let other = Id::parse("BRKT").unwrap();
    let named = catalog
        .ingest(std::slice::from_ref(&bracket), Some(&other))
        .unwrap();
    assert_eq!(
        named[0].outcome,
        Outcome::DuplicateUnderId {
            of: "m_bracket".into()
        }
    );
    assert!(matches!(
        catalog.ingest(&[bracket.clone(), inputs.join("a_b.stp")], Some(&other)),
        Err(Error::IdNeedsOneFile(2))
    ));
    // `a b.stp` under its own id is added; case folding finds `M_BRACKET`.
    let a_b = Id::parse("a_b_2").unwrap();
    let added = catalog
        .ingest(&[inputs.join("a b.stp")], Some(&a_b))
        .unwrap();
    assert_eq!(added[0].outcome, Outcome::Added);
    let upper = Id::parse("M_BRACKET").unwrap();
    let same = catalog.ingest(&[bracket], Some(&upper)).unwrap();
    assert_eq!(same[0].outcome, Outcome::Unchanged);
    assert_eq!(invariants(catalog.root()), Vec::<String>::new());
}

/// ST10: an input inside the catalog is refused before anything is read.
#[test]
fn an_input_inside_the_catalog_is_refused() {
    let dir = scratch("inside");
    let mut catalog = catalog(&dir);
    let inside = copy(
        "candidates/m_bracket.stp",
        &catalog.root().join("tmp"),
        "x.stp",
    );
    assert!(matches!(
        catalog.ingest(&[inside], None),
        Err(Error::InsideCatalog(_))
    ));
    assert_eq!(catalog.generation().unwrap(), 1);
}

/// GT3, SZ1, PH4: a file over the cap and a missing file are refused or
/// failed with nothing stored; an empty file is quarantined; a FIFO is
/// never opened.
#[test]
fn files_refused_at_the_door_store_nothing() {
    let dir = scratch("door");
    let small = Options::default().with_max_bytes(1000);
    let root = dir.join("cat");
    Catalog::init(&root, Options::default()).unwrap();
    let mut catalog = Catalog::open(&root, small).unwrap();
    let big = copy("candidates/m_bracket.stp", &dir, "big.stp");
    let empty = dir.join("empty.stp");
    fs::write(&empty, "").unwrap();
    let mut inputs = vec![big, dir.join("nowhere.stp"), empty];
    #[cfg(unix)]
    {
        let fifo = dir.join("fifo.stp");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if made.is_ok_and(|s| s.success()) {
            inputs.push(fifo);
        }
    }
    let lines = catalog.ingest(&inputs, None).unwrap();
    let by_name = outcomes(&lines);
    let find = |name: &str| {
        by_name
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, o)| o.clone())
    };
    assert_eq!(
        find("big.stp"),
        Some(Outcome::Refused {
            reason: "larger than --max-mib 0: not added".into()
        })
    );
    assert!(matches!(find("nowhere.stp"), Some(Outcome::Error { .. })));
    assert_eq!(find("empty.stp"), Some(Outcome::Added));
    if let Some(fifo) = find("fifo.stp") {
        assert_eq!(
            fifo,
            Outcome::Refused {
                reason: "not a regular file: not added".into()
            }
        );
    }
    let conn = Connection::open(root.join("catalog.db")).unwrap();
    let blobs: i64 = conn
        .query_row("SELECT count(*) FROM blob", [], |r| r.get(0))
        .unwrap();
    assert_eq!(blobs, 1, "only the empty file is stored");
    assert_eq!(invariants(&root), Vec::<String>::new());
}

/// PH3 and CT15: hostile file names are stored as text and mapped to ids
/// that hold none of their characters.
#[test]
fn hostile_names_are_stored_and_mapped() {
    let dir = scratch("names");
    let names = [
        "evil\u{1b}[31mred\u{202e}.stp",
        "new\nline.stp",
        "with space.stp",
    ];
    let parts = [
        "data/p01_through.stp",
        "data/p03_blind_flat.stp",
        "data/p05_cbore.stp",
    ];
    // Pushed to on Linux, which allows a name that is not UTF-8.
    #[allow(unused_mut)]
    let mut inputs: Vec<PathBuf> = names
        .iter()
        .zip(parts)
        .map(|(name, part)| copy(part, &dir, name))
        .collect();
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let name = std::ffi::OsStr::from_bytes(b"bad\xff.stp");
        let path = dir.join(name);
        fs::copy(core_tests().join("data/p06_csink.stp"), &path).unwrap();
        inputs.push(path);
    }
    let mut catalog = catalog(&dir);
    let lines = catalog.ingest(&inputs, None).unwrap();
    let ids: Vec<&str> = lines
        .iter()
        .map(|l| l.id.as_ref().unwrap().as_str())
        .collect();
    assert!(ids.contains(&"evil_31mred_"));
    assert!(ids.contains(&"new_line"));
    assert!(ids.contains(&"with_space"));
    assert!(lines.iter().all(|l| l.outcome == Outcome::Added));
    let conn = Connection::open(catalog.root().join("catalog.db")).unwrap();
    let stored: String = conn
        .query_row(
            "SELECT file_name FROM revision r JOIN entry e ON e.key = r.entry WHERE e.id = 'new_line'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, "new\nline.stp");
}

/// ST2: the source is read once; a file rewritten after that read cannot
/// make the blob differ from its document.
#[test]
fn the_source_is_read_once() {
    let dir = scratch("once");
    let root = dir.join("cat");
    Catalog::init(&root, Options::default()).unwrap();
    let source = copy("candidates/m_bracket.stp", &dir, "m_bracket.stp");
    let original = fs::read(&source).unwrap();
    let swapped = source.clone();
    let seam = std::sync::Arc::new(CountingFs::with_hook(move |call| {
        if *call == Call::Read(swapped.clone()) {
            fs::write(&swapped, b"other bytes").unwrap();
        }
    }));
    let mut catalog = Catalog::open_with(
        &root,
        Options::default(),
        Box::new(common::Shared(seam.clone())),
    )
    .unwrap();
    let lines = catalog.ingest(std::slice::from_ref(&source), None).unwrap();
    assert_eq!(lines[0].outcome, Outcome::Added);
    // The bracket's own document, read once: never the swapped bytes'.
    assert_eq!(lines[0].summary.as_ref().unwrap().status, Status::Accepted);
    let reads = seam
        .calls()
        .iter()
        .filter(|c| **c == Call::Read(source.clone()))
        .count();
    assert_eq!(reads, 1);
    assert_eq!(lines[0].sha256, Some(Sha256Hex::of(&original)));
    assert_eq!(fs::read(&source).unwrap(), b"other bytes");
    drop(catalog);
    assert_eq!(invariants(&root), Vec::<String>::new());
}
