//! `fitment catalog`: STEP parts stored by content and indexed for matching
//! (`docs/research/m5-catalog.md`, section 7). The catalog is named by
//! `-C/--catalog DIR` only, anywhere after `catalog` (CL1).

mod change;
mod get;
mod ingest;
mod list;
mod maintain;
mod reindex;

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Subcommand, ValueEnum};
use fitment_catalog::{Catalog, Error, Init, Options};

use super::text::{note, printable};

#[derive(clap::Args)]
pub struct Args {
    /// The catalog's directory.
    #[arg(short = 'C', long = "catalog", value_name = "DIR", global = true)]
    catalog: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Make a catalog in DIR, creating DIR if needed.
    ///
    /// Exit codes: 0 created (or already a catalog), 1 error, 2 DIR not
    /// empty and not a catalog, 3 a catalog of a newer fitment.
    Init,
    /// Store STEP part files: each passes the gate and is indexed, or is
    /// quarantined with the reason. A directory gives its .stp, .step and
    /// .p21 files, not recursively.
    ///
    /// Exit codes: 0 every file added, unchanged or a duplicate, 1 error,
    /// 2 usage (a conflict, a duplicate under --id, an input inside DIR),
    /// 3 a file quarantined or refused.
    Ingest(ingest::Args),
    /// List the catalog's entries with their status.
    ///
    /// Exit codes: 0 listed, 1 error, 2 not a catalog, 3 a newer catalog.
    List(list::Args),
    /// Show one entry, its stored features document, or its file's bytes.
    ///
    /// Exit codes: 0 shown, 1 error (a damaged file), 2 usage (an unknown
    /// or malformed reference, bytes no longer kept), 3 a quarantined or
    /// stale entry's features.
    Get(get::Args),
    /// Give an entry the bytes of another file as its next revision; the
    /// old revision's bytes are kept for `get ID@rN` until `prune`.
    ///
    /// Exit codes: 0 replaced or unchanged, 1 error, 2 usage (an unknown or
    /// stale reference, another entry's bytes, a file inside DIR), 3 the
    /// file refused (the entry unchanged).
    Replace(change::ReplaceArgs),
    /// Delete entries, all or none: their bytes and rows go, their history
    /// stays.
    ///
    /// Exit codes: 0 deleted, 1 error, 2 an unknown id, a stale ID@rN or a
    /// content reference (nothing deleted).
    Delete(change::DeleteArgs),
    /// Release kept revisions' bytes (`--revisions`).
    ///
    /// Exit codes: 0 done (also when nothing was kept), 1 error, 2 usage.
    Prune(change::PruneArgs),
    /// Remove orphan files and temporary files over an hour old.
    ///
    /// Exit codes: 0 done, 1 error (a file it could not remove), 2 not a
    /// catalog.
    Gc(maintain::GcArgs),
    /// Write the rows this build reads where an entry lacks them: after a
    /// fitment upgrade, a stale catalog refuses matches until this runs.
    /// Without REF, every stale entry.
    ///
    /// Exit codes: 0 every entry current, 1 error (a missing or damaged
    /// file: that entry stays stale), 2 usage (an unknown or stale
    /// reference), 3 a part no longer searched (newly quarantined).
    Reindex(reindex::Args),
    /// Check the catalog: its database, every stored file, row hash, part
    /// digest and posting; with --deep, every part extracted again.
    ///
    /// Exit codes: 0 consistent, 1 verify cannot run, 2 not a catalog,
    /// 3 problems found.
    Verify(maintain::VerifyArgs),
}

/// How every catalog command that prints does (CL3; CU1, CU2).
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    /// For people; not a stable format.
    Table,
    /// A `fitment.catalog` document.
    Json,
}

/// CL2's codes.
const ERROR: u8 = 1;
const USAGE: u8 = 2;
const REFUSED: u8 = crate::exit::REFUSED;

/// CL2: across several files, the first of 1, 2, 3 that any gave, else 0.
fn worst(codes: impl IntoIterator<Item = u8>) -> u8 {
    let codes: Vec<u8> = codes.into_iter().collect();
    [ERROR, USAGE, REFUSED]
        .into_iter()
        .find(|c| codes.contains(c))
        .unwrap_or(0)
}

/// The exit code of an error opening or using a catalog (CL2).
fn code_of(error: &Error) -> u8 {
    match error {
        Error::Newer { .. } => REFUSED,
        Error::NotACatalog(_)
        | Error::NotEmpty(_)
        | Error::Linked(_)
        | Error::NotUnicode(_)
        | Error::InsideCatalog(_)
        | Error::IdNeedsOneFile(_) => USAGE,
        _ => ERROR,
    }
}

/// The catalog as people see it on stderr: as given, escaped.
fn shown(dir: &Path) -> String {
    printable(&dir.display().to_string())
}

/// Reports `error` for `dir` and gives its code.
fn fail(dir: &Path, error: &Error) -> ExitCode {
    note(&format!(
        "fitment: {}: {}",
        shown(dir),
        printable(&error.to_string())
    ));
    ExitCode::from(code_of(error))
}

/// Writes `text` to stdout and gives `code`; a closed stdout ends output
/// quietly and keeps the code (CL5).
fn emit(text: &str, code: u8) -> ExitCode {
    match io::stdout().lock().write_all(text.as_bytes()) {
        Ok(()) => ExitCode::from(code),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::from(code),
        Err(error) => {
            note(&format!("error: cannot write the result: {error}"));
            ExitCode::from(ERROR)
        }
    }
}

pub fn run(args: &Args) -> ExitCode {
    let Some(dir) = &args.catalog else {
        note("error: name the catalog with -C DIR");
        return ExitCode::from(USAGE);
    };
    if let Command::Init = args.command {
        return match Catalog::init(dir, Options::default()) {
            Ok((_, Init::Created)) => {
                note(&format!("fitment: {}: catalog created", shown(dir)));
                ExitCode::SUCCESS
            }
            Ok((_, _)) => {
                note(&format!(
                    "fitment: {}: already a catalog: unchanged",
                    shown(dir)
                ));
                ExitCode::SUCCESS
            }
            Err(error) => fail(dir, &error),
        };
    }
    let options = match &args.command {
        Command::Ingest(ingest) => ingest.options(),
        Command::Replace(replace) => replace.options(),
        _ => Options::default(),
    };
    let mut catalog = match Catalog::open(dir, options) {
        Ok(catalog) => catalog,
        Err(error) => return fail(dir, &error),
    };
    match &args.command {
        Command::Init => ExitCode::SUCCESS,
        Command::Ingest(ingest) => ingest::run(ingest, dir, &mut catalog),
        Command::List(list) => list::run(list, dir, &mut catalog),
        Command::Get(get) => get::run(get, dir, &mut catalog),
        Command::Replace(replace) => change::replace(replace, dir, &mut catalog),
        Command::Delete(delete) => change::delete(delete, dir, &mut catalog),
        Command::Prune(prune) => change::prune(prune, dir, &mut catalog),
        Command::Gc(gc) => maintain::gc(gc, dir, &mut catalog),
        Command::Reindex(reindex) => reindex::run(reindex, dir, &mut catalog),
        Command::Verify(verify) => maintain::verify(verify, dir, &mut catalog),
    }
}
