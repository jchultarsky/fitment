//! CA1 and CA4 (`docs/research/m6-api.md`, VT38): an ingest from bytes and
//! names is the ingest from paths, ids, order and document alike; a file
//! naming its own id is ordered among the exact stems; and no document or
//! line holds a path the catalog was not given as an input, nor a library
//! error's text: a batch that could not commit, and a reindex line's
//! error, say the error's kind.

mod common;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use common::{copy, scratch};
use fitment_catalog::document::CatalogDocument;
use fitment_catalog::{
    Catalog, Content, Fs, FsRead, FsWrite, Id, Ingest, Input, NoObserver, NotReplaced, Options,
    Outcome, Point, Rebuilt, Reference, Replaced, Scope, StdFs,
};

/// C's prototype set (`C/proto/run.txt`): an exact stem, its copy under a
/// mapped name, a mapped name and the exact stem it maps to, an assembly
/// the gate quarantines, and bytes that are no STEP file.
fn prototype(dir: &Path) -> PathBuf {
    let inputs = dir.join("in");
    fs::create_dir_all(&inputs).unwrap();
    copy("candidates/m_bracket.stp", &inputs, "m_bracket.stp");
    copy("candidates/m_bracket.stp", &inputs, "copy of m_bracket.stp");
    copy("data/p03_blind_flat.stp", &inputs, "a b.stp");
    copy("data/p01_through.stp", &inputs, "a_b.stp");
    copy("data/r04_assembly.stp", &inputs, "r04_assembly.stp");
    fs::write(inputs.join("junk.stp"), "not STEP").unwrap();
    inputs
}

/// Each file of `dir`: its bytes, its path as text, its name.
fn read(dir: &Path) -> Vec<(Vec<u8>, String, String)> {
    let mut files: Vec<(Vec<u8>, String, String)> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (
                fs::read(&p).unwrap(),
                p.to_string_lossy().into_owned(),
                name,
            )
        })
        .collect();
    // The order given is not the order taken (ID4).
    files.sort_by(|a, b| b.1.cmp(&a.1));
    files
}

/// The ingest document of `run`, read as it stands.
fn document(catalog: &mut Catalog, run: &Ingest) -> String {
    let snapshot = catalog.snapshot().unwrap();
    CatalogDocument::ingested(&snapshot, run).unwrap().to_json()
}

/// `document` with its generation written as `_`.
fn unstamped(document: &str) -> String {
    document
        .lines()
        .map(|l| {
            if l.starts_with("  \"generation\": ") {
                "  \"generation\": _,"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every line's text: what a table or a document could show of it.
fn texts(run: &Ingest) -> String {
    format!("{:?}", run.lines)
}

/// VT38: the bytes and names of a directory's files, given in another
/// order, give the ids, order, lines and `fitment.catalog` document that
/// ingesting the directory gives; inputs named without any path give
/// documents and lines that hold no path at all.
#[test]
fn inputs_from_bytes_are_ingest_from_paths() {
    let dir = scratch("inputs-bytes");
    let inputs = prototype(&dir);
    let mut paths = Catalog::init(&dir.join("paths"), Options::default())
        .unwrap()
        .0;
    let by_path = paths.ingest(std::slice::from_ref(&inputs), None).unwrap();
    assert_eq!(by_path.len(), 6);
    let files = read(&inputs);
    let given: Vec<Input<'_>> = files
        .iter()
        .map(|(bytes, input, name)| Input::new(bytes, input, name))
        .collect();
    let mut bytes = Catalog::init(&dir.join("bytes"), Options::default())
        .unwrap()
        .0;
    let by_bytes = bytes.ingest_inputs(&given, None, &NoObserver).unwrap();
    assert_eq!(by_bytes.lines, by_path.lines);
    assert!(by_bytes.stopped.is_none());
    // DB3's batches end after a second too, so the generation an ingest
    // raises depends on its time: the rest of the document is compared.
    assert_eq!(
        unstamped(&document(&mut bytes, &by_bytes)),
        unstamped(&document(&mut paths, &by_path))
    );
    // The server's inputs: labels, no path; neither the catalog's root nor
    // any directory reaches a line or the document (DC2).
    let labels: Vec<String> = files.iter().map(|f| format!("upload {}", f.2)).collect();
    let given: Vec<Input<'_>> = files
        .iter()
        .zip(&labels)
        .map(|((bytes, _, name), label)| Input::new(bytes, label, name))
        .collect();
    let mut named = Catalog::init(&dir.join("named"), Options::default())
        .unwrap()
        .0;
    let run = named.ingest_inputs(&given, None, &NoObserver).unwrap();
    let text = document(&mut named, &run) + &texts(&run);
    let root = dir.to_string_lossy();
    assert!(!text.contains(root.as_ref()), "{text}");
    assert!(!text.contains("/in/") && !text.contains("\\in\\"), "{text}");
    assert!(text.contains("\"input\": \"upload junk.stp\""), "{text}");
}

/// CA1's ID4: an input that names its own id is ordered among the exact
/// stems, each group in byte order of its input; its id is its own, and
/// the others' are made from their file names (ID3).
#[test]
fn per_file_ids_order_with_the_exact_stems() {
    let dir = scratch("inputs-ids");
    let mut catalog = Catalog::init(&dir.join("cat"), Options::default())
        .unwrap()
        .0;
    let core = common::core_tests();
    let read = |p: &str| fs::read(core.join(p)).unwrap();
    let (bracket, blind, through) = (
        read("candidates/m_bracket.stp"),
        read("data/p03_blind_flat.stp"),
        read("data/p01_through.stp"),
    );
    let given = [
        Input::new(&blind, "a mapped", "a b.stp"),
        Input::new(&through, "z named", "z z.stp").with_id(Id::parse("ZZ").unwrap()),
        Input::new(&bracket, "q exact", "q.stp"),
    ];
    let run = catalog.ingest_inputs(&given, None, &NoObserver).unwrap();
    let order: Vec<(String, String)> = run
        .iter()
        .map(|l| {
            let id = l.id.as_ref().unwrap().as_str().to_owned();
            (l.input.to_string_lossy().into_owned(), id)
        })
        .collect();
    let want = [("q exact", "q"), ("z named", "ZZ"), ("a mapped", "a_b")];
    let want: Vec<(String, String)> = want
        .iter()
        .map(|(i, d)| ((*i).to_owned(), (*d).to_owned()))
        .collect();
    assert_eq!(order, want);
    assert!(run.iter().all(|l| l.outcome == Outcome::Added));
}

/// The real file system, every commit refused before it is made, as an
/// I/O error whose text names the database's absolute path.
struct RefusedCommits;

impl Fs for RefusedCommits {
    fn symlink_metadata(&self, path: &Path) -> io::Result<fs::Metadata> {
        StdFs.symlink_metadata(path)
    }
    fn read_capped(&self, path: &Path, cap: u64) -> io::Result<Content> {
        StdFs.read_capped(path, cap)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        StdFs.read_dir(path)
    }
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        StdFs.create_dir_all(path)
    }
    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        StdFs.write_new(path, bytes)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        StdFs.rename(from, to)
    }
    fn set_readonly(&self, path: &Path, readonly: bool) -> io::Result<()> {
        StdFs.set_readonly(path, readonly)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        StdFs.remove_file(path)
    }
    fn remove_dir(&self, path: &Path) -> io::Result<()> {
        StdFs.remove_dir(path)
    }
    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        StdFs.sync_dir(path)
    }
    fn create_new_stream(&self, path: &Path) -> io::Result<Box<dyn FsWrite>> {
        StdFs.create_new_stream(path)
    }
    fn open_stream(&self, path: &Path, cap: u64) -> io::Result<Box<dyn FsRead>> {
        StdFs.open_stream(path, cap)
    }
    fn point(&self, at: Point) -> io::Result<()> {
        match at {
            Point::BeforeCommit => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the commit is refused",
            )),
            Point::AfterCommit => Ok(()),
        }
    }
}

/// CA4: a batch that could not commit says the error's kind, never its
/// text: an I/O error at the commit (whose text names the database's
/// absolute path), and a database past its page quota (`storage full`,
/// which stops the run). A reindex line's error likewise.
#[test]
fn a_failed_batch_line_holds_only_the_error_kind() {
    let dir = scratch("failed-batch");
    let inputs = prototype(&dir);
    let root = dir.join("io");
    Catalog::init(&root, Options::default()).unwrap();
    let mut catalog =
        Catalog::open_with(&root, Options::default(), Box::new(RefusedCommits)).unwrap();
    let run = catalog.ingest(std::slice::from_ref(&inputs), None).unwrap();
    let failed: Vec<&str> = run
        .iter()
        .filter_map(|l| match &l.outcome {
            Outcome::Error { message } => Some(message.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(failed.len(), run.len(), "{run:?}");
    assert!(
        failed.iter().all(|m| *m == "io: permission denied"),
        "{failed:?}"
    );
    let text = document(&mut catalog, &run) + &texts(&run);
    assert!(!text.contains("catalog.db"), "{text}");
    drop(catalog);

    // A database past its quota: `storage full`, and the run stops.
    let full = dir.join("full");
    let (catalog, _) = Catalog::init(&full, Options::default()).unwrap();
    let pages: i64 = catalog
        .connection()
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    drop(catalog);
    let options = Options::default().with_max_db_pages(u64::try_from(pages).unwrap());
    let mut catalog = Catalog::open(&full, options).unwrap();
    let run = catalog.ingest(std::slice::from_ref(&inputs), None).unwrap();
    assert!(matches!(
        run.stopped,
        Some(fitment_catalog::Error::Database(_))
    ));
    let messages: Vec<String> = run
        .iter()
        .filter_map(|l| match &l.outcome {
            Outcome::Error { message } => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_ne!(messages.len(), 0);
    assert!(messages.iter().all(|m| m == "storage full"), "{messages:?}");

    // A reindex whose batch cannot commit: each line `failed`, its detail
    // the kind.
    let stale = dir.join("stale");
    let (mut catalog, _) = Catalog::init(&stale, Options::default()).unwrap();
    catalog.ingest(std::slice::from_ref(&inputs), None).unwrap();
    catalog
        .connection()
        .execute("DELETE FROM properties", [])
        .unwrap();
    drop(catalog);
    let mut catalog =
        Catalog::open_with(&stale, Options::default(), Box::new(RefusedCommits)).unwrap();
    let run = catalog.reindex(&Scope::Stale).unwrap().unwrap();
    assert_ne!(run.lines.len(), 0);
    for line in &run.lines {
        assert_eq!(
            line.rebuilt,
            Rebuilt::Failed("io: permission denied".into())
        );
        assert_eq!(line.detail(), "io: permission denied");
        assert_eq!(line.result(), "failed");
    }
    let snapshot = catalog.snapshot().unwrap();
    let text = CatalogDocument::reindexed(&snapshot, &run)
        .unwrap()
        .to_json();
    assert!(!text.contains("catalog.db"), "{text}");
}

/// The review of PR 7a: a cap is one call's (SZ1). The server's one
/// writer takes each ingest's or replace's `max_mib` with its call, and
/// the next call, naming none, reads under the catalog's own.
#[test]
fn a_cap_is_one_calls() {
    let dir = scratch("cap");
    let mut catalog = Catalog::init(&dir.join("cat"), Options::default())
        .unwrap()
        .0;
    let bytes = fs::read(common::core_tests().join("candidates/m_bracket.stp")).unwrap();
    let input = [Input::new(&bytes, "m_bracket.stp", "m_bracket.stp")];
    let too_large = Outcome::Refused {
        reason: "larger than --max-mib 0: not added".into(),
    };
    let run = catalog
        .ingest_inputs(&input, Some(1024), &NoObserver)
        .unwrap();
    assert_eq!(run[0].outcome, too_large);
    let run = catalog.ingest_inputs(&input, None, &NoObserver).unwrap();
    assert_eq!(run[0].outcome, Outcome::Added);
    // Replace likewise: refused under its cap, and the next call is not.
    let next = String::from_utf8(bytes.clone())
        .unwrap()
        .replacen("FILE_NAME('", "FILE_NAME('next", 1)
        .into_bytes();
    let reference = Reference::parse("m_bracket").unwrap();
    let input = Input::new(&next, "next.stp", "next.stp");
    let refused = catalog
        .replace_input(&reference, &input, Some(1024), &NoObserver)
        .unwrap();
    assert!(
        matches!(&refused, Err(NotReplaced::File(line)) if line.outcome == too_large),
        "{refused:?}"
    );
    let done = catalog
        .replace_input(&reference, &input, None, &NoObserver)
        .unwrap();
    assert!(matches!(done, Ok(Replaced::Replaced { .. })), "{done:?}");
}

/// The review of PR 7a: a line's `input` is the path as given, byte for
/// byte, as 0.2.0 kept it, for a path that is not Unicode too; an ingest's
/// and a refused replace's alike. The paths need not exist: a file that
/// cannot be read has its line.
#[cfg(unix)]
#[test]
fn a_path_that_is_not_unicode_is_its_lines_input() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;
    let dir = scratch("not-unicode");
    let mut catalog = Catalog::init(&dir.join("cat"), Options::default())
        .unwrap()
        .0;
    let raw = dir.join(OsStr::from_bytes(b"caf\xe9.stp"));
    let run = catalog.ingest(std::slice::from_ref(&raw), None).unwrap();
    assert_eq!(
        run[0].input.as_os_str().as_bytes(),
        raw.as_os_str().as_bytes()
    );
    let bracket = common::copy("candidates/m_bracket.stp", &dir, "m_bracket.stp");
    catalog.ingest(&[bracket], None).unwrap();
    let reference = Reference::parse("m_bracket").unwrap();
    match catalog.replace(&reference, &raw).unwrap() {
        Err(NotReplaced::File(line)) => {
            assert_eq!(
                line.input.as_os_str().as_bytes(),
                raw.as_os_str().as_bytes()
            );
        }
        other => panic!("{other:?}"),
    }
}

/// A source that panics on its `n`th read, and counts them.
struct Panicking {
    bytes: Vec<u8>,
    n: usize,
    reads: std::sync::atomic::AtomicUsize,
}

impl fitment_catalog::InputBytes for Panicking {
    fn read(&self, cap: u64) -> io::Result<Content> {
        let k = self
            .reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert!(k + 1 < self.n, "a source that fails");
        self.bytes.read(cap)
    }
}

/// An observer that panics when asked the `n`th time.
struct PanicsAt(usize, std::sync::atomic::AtomicUsize);

impl fitment_catalog::Observer for PanicsAt {
    fn cancelled(&self) -> bool {
        let k = self.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert!(k + 1 < self.0, "an observer that fails");
        false
    }
    fn progress(&self, _: fitment_catalog::Progress<'_>) {}
}

/// The files under `root/blobs`.
fn blob_files(root: &Path) -> usize {
    fs::read_dir(root.join("blobs/sha256"))
        .unwrap()
        .map(|fan| fs::read_dir(fan.unwrap().path()).unwrap().count())
        .sum()
}

/// The review of PR 7a: a panic in an input's source, or in the observer,
/// while a batch is built goes on, and leaves none of the batch's blobs
/// behind, as a cancel leaves none (ST5).
#[test]
fn a_panic_while_a_batch_is_built_leaves_no_blob() {
    let dir = scratch("panics");
    let bracket = fs::read(common::core_tests().join("candidates/m_bracket.stp")).unwrap();
    let variants: Vec<Vec<u8>> = (0..4)
        .map(|n| {
            String::from_utf8(bracket.clone())
                .unwrap()
                .replacen("FILE_NAME('", &format!("FILE_NAME('v{n}"), 1)
                .into_bytes()
        })
        .collect();
    let names: Vec<String> = (0..4).map(|n| format!("v{n}.stp")).collect();
    let quiet = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    // The source: its third read panics, after two blobs are placed.
    let root = dir.join("source");
    let (mut catalog, _) = Catalog::init(&root, Options::default()).unwrap();
    let sources: Vec<Panicking> = variants
        .iter()
        .map(|bytes| Panicking {
            bytes: bytes.clone(),
            n: 1,
            reads: std::sync::atomic::AtomicUsize::new(0),
        })
        .collect();
    let mut inputs: Vec<Input<'_>> = variants
        .iter()
        .zip(&names)
        .map(|(bytes, name)| Input::new(bytes, name, name))
        .collect();
    inputs[2] = Input::new(&sources[2], &names[2], &names[2]);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        catalog.ingest_inputs(&inputs, None, &NoObserver)
    }));
    assert!(caught.is_err());
    assert_eq!(blob_files(&root), 0);
    // The observer: asked before the fourth file, it panics.
    let root = dir.join("observer");
    let (mut catalog, _) = Catalog::init(&root, Options::default()).unwrap();
    let inputs: Vec<Input<'_>> = variants
        .iter()
        .zip(&names)
        .map(|(bytes, name)| Input::new(bytes, name, name))
        .collect();
    let observer = PanicsAt(4, std::sync::atomic::AtomicUsize::new(0));
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        catalog.ingest_inputs(&inputs, None, &observer)
    }));
    std::panic::set_hook(quiet);
    assert!(caught.is_err());
    assert_eq!(blob_files(&root), 0);
    let rows: i64 = catalog
        .connection()
        .query_row("SELECT count(*) FROM blob", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

/// Cancels from its `k`th question on.
struct StopAt(usize, std::sync::atomic::AtomicUsize);

impl fitment_catalog::Observer for StopAt {
    fn cancelled(&self) -> bool {
        self.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= self.0
    }
    fn progress(&self, _: fitment_catalog::Progress<'_>) {}
}

/// CA2 for `replace_input`: stopped before its file is read, and before
/// its blob is put and its transaction begins, it changes nothing and
/// leaves no file; asked twice, it replaces.
#[test]
fn replace_stops_at_both_its_points() {
    let dir = scratch("replace-stops");
    let bracket = fs::read(common::core_tests().join("candidates/m_bracket.stp")).unwrap();
    let next = String::from_utf8(bracket.clone())
        .unwrap()
        .replacen("FILE_NAME('", "FILE_NAME('next", 1)
        .into_bytes();
    let reference = Reference::parse("m_bracket").unwrap();
    for k in [0, 1, 2] {
        let root = dir.join(format!("k{k}"));
        let (mut catalog, _) = Catalog::init(&root, Options::default()).unwrap();
        let first = [Input::new(&bracket, "m_bracket.stp", "m_bracket.stp")];
        catalog.ingest_inputs(&first, None, &NoObserver).unwrap();
        let generation = catalog.generation().unwrap();
        let stop = StopAt(k, std::sync::atomic::AtomicUsize::new(0));
        let input = Input::new(&next, "next.stp", "next.stp");
        let outcome = catalog.replace_input(&reference, &input, None, &stop);
        if k < 2 {
            assert!(
                matches!(outcome, Err(fitment_catalog::Error::Cancelled)),
                "{k}"
            );
            assert_eq!(catalog.generation().unwrap(), generation, "{k}");
            assert_eq!(blob_files(&root), 1, "{k}: only the bracket's");
        } else {
            assert!(matches!(outcome, Ok(Ok(Replaced::Replaced { .. }))), "{k}");
        }
        assert_eq!(
            stop.1.load(std::sync::atomic::Ordering::Relaxed),
            (k + 1).min(2)
        );
    }
}
