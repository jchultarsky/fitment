//! `fitment catalog replace`, `delete` and `prune`
//! (`docs/research/m5-catalog.md`, sections 7.5, 7.6 and 7.10). `delete`
//! and `prune` print `fitment.catalog` with `--format json` (CU1,
//! `docs/research/m6-api.md`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use fitment_catalog::document::{CatalogDocument, released};
use fitment_catalog::{
    Catalog, Deletion, Error, Ingested, NotReplaced, Options, Outcome, Pruned, Reference, Released,
    Replaced, Summary,
};

use super::{ERROR, Format, REFUSED, USAGE, emit, fail, shown};
use crate::cmd::text::{note, printable};

#[derive(clap::Args)]
pub struct ReplaceArgs {
    /// The entry: ID, its current ID@rN, or sha256: and 12 to 64 hex
    /// digits.
    #[arg(value_name = "REF")]
    reference: String,
    /// The STEP file whose bytes become the entry's next revision.
    file: PathBuf,
    /// The largest file read, in MiB.
    #[arg(long, value_name = "N", default_value_t = 128)]
    max_mib: u64,
    /// How to print the result.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,
}

impl ReplaceArgs {
    /// The catalog's options for this run.
    pub(super) fn options(&self) -> Options {
        Options::default().with_max_bytes(self.max_mib.saturating_mul(1024 * 1024))
    }
}

#[derive(clap::Args)]
pub struct DeleteArgs {
    /// The entries to delete: ids, or their current ID@rN; all or none.
    #[arg(required = true, value_name = "ID")]
    ids: Vec<String>,
    /// How to print the result.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,
}

#[derive(clap::Args)]
pub struct PruneArgs {
    /// Release the bytes of every superseded revision replace kept.
    #[arg(long)]
    revisions: bool,
    /// How to print the result.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,
}

/// A reference, or exit 2 with why.
fn parse(text: &str) -> Result<Reference, ExitCode> {
    Reference::parse(text).map_err(|error| {
        note(&format!("error: {}", printable(&error.to_string())));
        ExitCode::from(USAGE)
    })
}

fn features(summary: &Summary) -> String {
    let product = printable(summary.product_id.as_deref().unwrap_or_default());
    summary.counts.map_or_else(
        || format!("'{product}'"),
        |c| {
            format!(
                "'{product}'  {} holes, {} shafts, {} planes",
                c.holes, c.shafts, c.planes
            )
        },
    )
}

/// Each orphan the inline gc left, as a warning (CR1).
fn orphans(dir: &Path, released: &Released) {
    for orphan in &released.orphans {
        note(&format!(
            "warning: {}: could not remove {}: run fitment catalog gc -C {}",
            shown(dir),
            printable(orphan),
            shown(dir)
        ));
    }
}

/// The `fitment.catalog` document of a replace: the entry as it is now,
/// and the change.
fn replace_document(catalog: &mut Catalog, input: &str, done: &Replaced) -> Result<String, Error> {
    let snapshot = catalog.snapshot()?;
    Ok(CatalogDocument::replaced(&snapshot, input, done)?.to_json())
}

/// The `fitment.catalog` document of a delete (CU1).
fn delete_document(
    catalog: &mut Catalog,
    ids: &[String],
    done: &Deletion,
) -> Result<String, Error> {
    let inputs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let snapshot = catalog.snapshot()?;
    Ok(CatalogDocument::deleted(&snapshot, &inputs, done)?.to_json())
}

/// The `fitment.catalog` document of a prune (CU1).
fn prune_document(catalog: &mut Catalog, pruned: &[Pruned]) -> Result<String, Error> {
    let snapshot = catalog.snapshot()?;
    Ok(CatalogDocument::pruned(&snapshot, pruned)?.to_json())
}

/// A refused replace on stderr, and its code: a gate's refusal is 3, an
/// internal one a fitment bug (GT8, 1), a file that cannot be read 1.
fn refused(dir: &Path, file: &Path, line: &Ingested) -> u8 {
    let internal = line.summary.as_ref().is_some_and(|s| s.internal);
    let (why, code) = match &line.outcome {
        Outcome::Refused { reason } if internal => {
            (format!("{reason}: this is a fitment bug"), ERROR)
        }
        Outcome::Refused { reason } => (reason.trim_end_matches(": not added").to_owned(), REFUSED),
        Outcome::Error { message } => (message.clone(), ERROR),
        other => (format!("{other:?}"), ERROR),
    };
    note(&format!(
        "fitment: {}: {}: {}: the entry is unchanged",
        shown(dir),
        printable(&file.display().to_string()),
        printable(&why)
    ));
    code
}

/// An outcome a newer library has and this build cannot report: exit 1.
fn cannot_report(dir: &Path, outcome: &dyn std::fmt::Debug) -> ExitCode {
    note(&format!(
        "fitment: {}: an outcome this build cannot report: {outcome:?}",
        shown(dir)
    ));
    ExitCode::from(ERROR)
}

pub(super) fn replace(args: &ReplaceArgs, dir: &Path, catalog: &mut Catalog) -> ExitCode {
    let reference = match parse(&args.reference) {
        Ok(r) => r,
        Err(code) => return code,
    };
    let outcome = match catalog.replace(&reference, &args.file) {
        Ok(outcome) => outcome,
        Err(error) => return fail(dir, &error),
    };
    let done = match outcome {
        Ok(done) => done,
        Err(NotReplaced::Unnamed(why)) => {
            note(&format!(
                "fitment: {}: {}",
                shown(dir),
                printable(&why.to_string())
            ));
            return ExitCode::from(USAGE);
        }
        Err(NotReplaced::Duplicate { of }) => {
            note(&format!(
                "fitment: {}: the file's bytes are {of}'s: not replaced",
                shown(dir)
            ));
            return ExitCode::from(USAGE);
        }
        Err(NotReplaced::File(line)) => return ExitCode::from(refused(dir, &args.file, &line)),
        Err(other) => return cannot_report(dir, &other),
    };
    let line = match &done {
        Replaced::Replaced {
            id,
            from,
            to,
            summary,
        } => {
            note(&format!(
                "fitment: {}: {id} replaced: r{} is now r{}",
                shown(dir),
                from.0,
                to.0
            ));
            format!(
                "replaced     {id}  r{}  {}  {}   (was r{} {}, bytes kept)",
                to.0,
                to.1.short(8),
                features(summary),
                from.0,
                from.1.short(8),
            )
        }
        Replaced::Unchanged {
            id,
            revision,
            sha256,
        } => {
            note(&format!(
                "fitment: {}: {id} unchanged: the file is r{revision}'s bytes",
                shown(dir)
            ));
            format!("unchanged    {id}  r{revision}  {}", sha256.short(8))
        }
        other => return cannot_report(dir, other),
    };
    let input = args.file.display().to_string();
    match args.format {
        Format::Table => emit(&format!("{line}\n"), 0),
        Format::Json => match replace_document(catalog, &input, &done) {
            Ok(text) => emit(&text, 0),
            Err(error) => fail(dir, &error),
        },
    }
}

pub(super) fn delete(args: &DeleteArgs, dir: &Path, catalog: &mut Catalog) -> ExitCode {
    let mut references = Vec::new();
    for id in &args.ids {
        match parse(id) {
            Ok(r) => references.push(r),
            Err(code) => return code,
        }
    }
    let deletion = match catalog.delete(&references) {
        Ok(Ok(deletion)) => deletion,
        Ok(Err(unnamed)) => {
            for why in unnamed {
                note(&format!(
                    "fitment: {}: {}",
                    shown(dir),
                    printable(&why.to_string())
                ));
            }
            note(&format!("fitment: {}: nothing deleted", shown(dir)));
            return ExitCode::from(USAGE);
        }
        Err(error) => return fail(dir, &error),
    };
    let mut out = String::new();
    for d in &deletion.deleted {
        let _ = writeln!(
            out,
            "deleted      {}  r{}  {}",
            d.id,
            d.revision,
            d.sha256.short(8)
        );
    }
    let n = deletion.deleted.len();
    note(&format!(
        "fitment: {}: {n} {} deleted",
        shown(dir),
        if n == 1 { "entry" } else { "entries" }
    ));
    orphans(dir, &deletion.released);
    match args.format {
        Format::Table => emit(&out, 0),
        Format::Json => match delete_document(catalog, &args.ids, &deletion) {
            Ok(text) => emit(&text, 0),
            Err(error) => fail(dir, &error),
        },
    }
}

pub(super) fn prune(args: &PruneArgs, dir: &Path, catalog: &mut Catalog) -> ExitCode {
    if !args.revisions {
        note("error: prune what? --revisions releases the bytes of superseded revisions");
        return ExitCode::from(USAGE);
    }
    let (pruned, freed) = match catalog.prune_revisions() {
        Ok(done) => done,
        Err(error) => return fail(dir, &error),
    };
    let mut out = String::new();
    for p in &pruned {
        let bytes = released(p.released);
        let _ = writeln!(
            out,
            "pruned       {}  r{}  {}  {bytes}",
            p.id,
            p.revision,
            p.sha256.short(8)
        );
    }
    let total: u64 = freed.contents.iter().map(|(_, b)| *b).sum();
    let n = pruned.len();
    note(&format!(
        "fitment: {}: {n} {} pruned, {total} bytes released",
        shown(dir),
        if n == 1 { "revision" } else { "revisions" }
    ));
    orphans(dir, &freed);
    match args.format {
        Format::Table => emit(&out, 0),
        Format::Json => match prune_document(catalog, &pruned) {
            Ok(text) => emit(&text, 0),
            Err(error) => fail(dir, &error),
        },
    }
}
