//! `fitment catalog get -C DIR REF` (`docs/research/m5-catalog.md`, section
//! 7.4, ID7, ID9, ST11, PH7).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::ValueEnum;
use fitment_catalog::document::{CatalogDocument, Entry};
use fitment_catalog::{BadBlob, BadRow, Catalog, HistoryStep, Listed, Named, Reference};

use super::list::status_text;
use super::{ERROR, REFUSED, USAGE, emit, fail, shown};
use crate::cmd::text::{note, printable};
use crate::cmd::write_atomically;

#[derive(clap::Args)]
pub struct Args {
    /// The entry: ID, ID@rN (any revision the id had), or sha256: and 12
    /// to 64 hex digits of a live entry's content.
    #[arg(value_name = "REF")]
    reference: String,
    /// What to print: the entry for people, a `fitment.catalog` document,
    /// the stored features document, or the file's bytes (to -o only).
    #[arg(long, value_enum, default_value_t = GetFormat::Table)]
    format: GetFormat,
    /// Where `--format step` writes the file's bytes.
    #[arg(short = 'o', long = "output", value_name = "OUT")]
    output: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum GetFormat {
    Table,
    Json,
    Features,
    Step,
}

/// What `get` read in its one snapshot.
struct Read {
    named: Named,
    listed: Listed,
    history: Vec<HistoryStep>,
    features: Option<Result<fitment_core::FeaturesDocument, BadRow>>,
    /// Every live entry, for a document's counts.
    all: Vec<Listed>,
    generation: i64,
}

/// Reads everything `get` shows, in one snapshot (DB4).
fn read(
    catalog: &mut Catalog,
    reference: &Reference,
    format: GetFormat,
) -> Result<Result<Read, String>, fitment_catalog::Error> {
    let snapshot = catalog.snapshot()?;
    let named = match snapshot.resolve(reference)? {
        Ok(named) => named,
        Err(bad) => return Ok(Err(bad.to_string())),
    };
    let listed = snapshot.entry(&named)?;
    let history = snapshot.history(&named)?;
    let current = named.current == Some(named.revision);
    let features = if format == GetFormat::Features && current {
        Some(snapshot.features(&named.sha256)?)
    } else {
        None
    };
    let all = if format == GetFormat::Json {
        snapshot.list()?
    } else {
        Vec::new()
    };
    Ok(Ok(Read {
        all,
        generation: snapshot.generation(),
        named,
        listed,
        history,
        features,
    }))
}

fn revision_text(read: &Read) -> String {
    let n = read.named.revision;
    match read.named.current {
        Some(c) if c == n => format!("r{n} (current)"),
        Some(c) => format!("r{n} (superseded by r{c})"),
        None => format!("r{n} (deleted)"),
    }
}

fn table(read: &Read) -> String {
    let l = &read.listed;
    let quote = |s: &Option<String>| format!("'{}'", printable(s.as_deref().unwrap_or_default()));
    let mut out = String::new();
    let _ = writeln!(out, "id        {}", l.id);
    let _ = writeln!(out, "revision  {}", revision_text(read));
    if l.status.is_some() {
        let _ = writeln!(out, "status    {}", status_text(l));
    }
    let _ = writeln!(out, "sha256    {}", l.sha256);
    let bytes = l
        .bytes
        .map_or_else(|| "- (not kept)".to_owned(), |b| b.to_string());
    let _ = writeln!(out, "bytes     {bytes}");
    let _ = writeln!(out, "file      {}", quote(&l.file_name));
    if l.status.is_some() {
        let _ = writeln!(
            out,
            "product   id {}, name {}",
            quote(&l.product.0),
            quote(&l.product.1)
        );
        let version = |v: Option<u32>| v.map_or_else(|| "-".into(), |v| v.to_string());
        let _ = writeln!(
            out,
            "versions  extractor {}, signature {}",
            version(l.versions.0),
            version(l.versions.1)
        );
    }
    if let Some(c) = l.counts {
        let complete = if l.complete == Some(false) {
            "incomplete"
        } else {
            "complete"
        };
        let _ = writeln!(
            out,
            "features  {} planes, {} holes, {} shafts, {} cones, {} other faces ({complete})",
            c.planes, c.holes, c.shafts, c.cones, c.other_faces
        );
    }
    if !l.refusals.is_empty() {
        out.push_str("refusals\n");
        for r in &l.refusals {
            let at = r.instance.map(|i| format!(" (#{i})")).unwrap_or_default();
            let _ = writeln!(out, "    {}: {}{at}", r.code, printable(&r.message));
        }
    }
    out.push_str("history\n");
    for s in &read.history {
        let kept = if s.bytes_kept { " (bytes kept)" } else { "" };
        let _ = writeln!(
            out,
            "    r{}  {}  generation {}  {}{kept}",
            s.revision,
            s.sha256.short(12),
            s.generation,
            s.event
        );
    }
    out
}

/// `--format features`: the checked stored document (GT2, QR4).
fn features(read: &Read) -> (Result<String, String>, u8) {
    let n = &read.named;
    match &read.features {
        None => (
            Err(format!(
                "{} r{} is not current: its stored document is gone (get {}@r{} --format step for its bytes)",
                n.id, n.revision, n.id, n.revision
            )),
            USAGE,
        ),
        Some(Ok(doc)) => {
            let code = if doc.part().is_some() { 0 } else { REFUSED };
            (Ok(doc.to_json()), code)
        }
        Some(Err(bad @ BadRow::Stale { .. })) => (Err(format!("{}: {bad}", n.id)), REFUSED),
        Some(Err(bad)) => (Err(format!("{}: {bad}", n.id)), ERROR),
    }
}

/// `--format step`: the kept bytes, re-hashed, to `out` (ST11, PH7, CL7).
fn step(catalog: &Catalog, dir: &Path, read: &Read, out: &Path) -> u8 {
    let n = &read.named;
    if read.listed.bytes.is_none()
        || !read
            .history
            .iter()
            .any(|s| s.revision == n.revision && s.bytes_kept)
    {
        note(&format!(
            "fitment: {}: {} r{}'s bytes are no longer kept",
            shown(dir),
            n.id,
            n.revision
        ));
        return USAGE;
    }
    let bytes = match catalog.read_blob(&n.sha256) {
        Ok(bytes) => bytes,
        Err(bad) => {
            let why = match bad {
                BadBlob::Missing => "missing: the file is not in the store".to_owned(),
                BadBlob::Damaged => "damaged: the file's SHA-256 is not its name".to_owned(),
                other => other.to_string(),
            };
            note(&format!(
                "fitment: {}: {} r{}: {why}: run fitment catalog verify -C {}",
                shown(dir),
                n.id,
                n.revision,
                shown(dir)
            ));
            return ERROR;
        }
    };
    if let Err(error) = write_atomically(out, &bytes) {
        note(&format!(
            "error: cannot write {}: {error}",
            printable(&out.display().to_string())
        ));
        return ERROR;
    }
    0
}

/// ST10: an output that is the catalog's directory or inside it, the
/// target itself and not only its parent (`-o DIR/` names DIR).
fn inside(dir: &Path, out: &Path) -> bool {
    let parent = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let (Ok(dir), Ok(parent)) = (std::fs::canonicalize(dir), std::fs::canonicalize(parent)) else {
        return false;
    };
    let target = out
        .file_name()
        .map_or(parent.clone(), |name| parent.join(name));
    target.starts_with(dir)
}

pub(super) fn run(args: &Args, dir: &Path, catalog: &mut Catalog) -> ExitCode {
    let reference = match Reference::parse(&args.reference) {
        Ok(reference) => reference,
        Err(error) => {
            note(&format!("error: {}", printable(&error.to_string())));
            return ExitCode::from(USAGE);
        }
    };
    let out = match (args.format, &args.output) {
        (GetFormat::Step, None) => {
            note("error: --format step writes the file's bytes to a file only: give -o OUT");
            return ExitCode::from(USAGE);
        }
        (GetFormat::Step, Some(out)) if inside(dir, out) => {
            note("error: -o names a file inside the catalog: write it elsewhere");
            return ExitCode::from(USAGE);
        }
        (GetFormat::Step, Some(out)) if out.is_dir() => {
            note("error: -o names a directory: name the file to write");
            return ExitCode::from(USAGE);
        }
        (GetFormat::Step, Some(out)) => Some(out.clone()),
        (_, Some(_)) => {
            note("error: -o is for --format step; the other formats print to stdout");
            return ExitCode::from(USAGE);
        }
        _ => None,
    };
    let read = match read(catalog, &reference, args.format) {
        Ok(Ok(read)) => read,
        Ok(Err(bad)) => {
            note(&format!("fitment: {}: {bad}", shown(dir)));
            return ExitCode::from(USAGE);
        }
        Err(error) => return fail(dir, &error),
    };
    let target = out.as_ref().map_or_else(
        || "stdout".to_owned(),
        |o| printable(&o.display().to_string()),
    );
    let said = format!(
        "fitment: {}: generation {}: {} r{} sha256:{} -> {target}",
        shown(dir),
        read.generation,
        read.named.id,
        read.named.revision,
        read.named.sha256
    );
    let (text, code) = match args.format {
        GetFormat::Table => (Ok(table(&read)), 0),
        GetFormat::Json => {
            let entry = Entry::of(&read.listed, Some(&read.history));
            let document = CatalogDocument::new(read.generation, &read.all, vec![entry], None);
            (Ok(document.to_json()), 0)
        }
        GetFormat::Features => features(&read),
        GetFormat::Step => {
            let code = out
                .as_deref()
                .map_or(USAGE, |o| step(catalog, dir, &read, o));
            if code == 0 {
                note(&said);
            }
            return ExitCode::from(code);
        }
    };
    match text {
        Ok(text) => {
            note(&said);
            emit(&text, code)
        }
        Err(why) => {
            note(&format!("fitment: {}: {why}", shown(dir)));
            ExitCode::from(code)
        }
    }
}
