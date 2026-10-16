//! `fitment catalog replace`, `delete` and `prune`
//! (`docs/research/m5-catalog.md`, sections 7.5, 7.6 and 7.10).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use fitment_catalog::{
    Catalog, NotReplaced, Options, Outcome, Reference, Released, Replaced, Summary,
};

use super::{ERROR, REFUSED, USAGE, emit, fail, shown};
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
}

#[derive(clap::Args)]
pub struct PruneArgs {
    /// Release the bytes of every superseded revision replace kept.
    #[arg(long)]
    revisions: bool,
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

pub(super) fn replace(args: &ReplaceArgs, dir: &Path, catalog: &mut Catalog) -> ExitCode {
    let reference = match parse(&args.reference) {
        Ok(r) => r,
        Err(code) => return code,
    };
    let outcome = match catalog.replace(&reference, &args.file) {
        Ok(outcome) => outcome,
        Err(error) => return fail(dir, &error),
    };
    let (line, code) = match outcome {
        Ok(Replaced::Replaced {
            id,
            from,
            to,
            summary,
        }) => (
            format!(
                "replaced     {id}  r{}  {}  {}   (was r{} {}, bytes kept)",
                to.0,
                to.1.short(8),
                features(&summary),
                from.0,
                from.1.short(8)
            ),
            0,
        ),
        Ok(Replaced::Unchanged {
            id,
            revision,
            sha256,
        }) => (
            format!("unchanged    {id}  r{revision}  {}", sha256.short(8)),
            0,
        ),
        Ok(_) => (String::new(), ERROR),
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
        Err(NotReplaced::File(line)) => {
            let (why, code) = match &line.outcome {
                Outcome::Refused { reason } => (reason.clone(), REFUSED),
                Outcome::Error { message } => (message.clone(), ERROR),
                other => (format!("{other:?}"), ERROR),
            };
            note(&format!(
                "fitment: {}: {}: {}: the entry is unchanged",
                shown(dir),
                printable(&args.file.display().to_string()),
                printable(&why)
            ));
            return ExitCode::from(code);
        }
        Err(_) => return ExitCode::from(ERROR),
    };
    emit(&format!("{line}\n"), code)
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
    emit(&out, 0)
}

pub(super) fn prune(args: &PruneArgs, dir: &Path, catalog: &mut Catalog) -> ExitCode {
    if !args.revisions {
        note("error: prune what? --revisions releases the bytes of superseded revisions");
        return ExitCode::from(USAGE);
    }
    let (pruned, released) = match catalog.prune_revisions() {
        Ok(done) => done,
        Err(error) => return fail(dir, &error),
    };
    let mut out = String::new();
    for p in &pruned {
        let bytes = p.released.map_or_else(
            || "bytes held by another revision".to_owned(),
            |b| format!("{b} bytes"),
        );
        let _ = writeln!(
            out,
            "pruned       {}  r{}  {}  {bytes}",
            p.id,
            p.revision,
            p.sha256.short(8)
        );
    }
    let total: u64 = released.contents.iter().map(|(_, b)| *b).sum();
    let n = pruned.len();
    note(&format!(
        "fitment: {}: {n} {} pruned, {total} bytes released",
        shown(dir),
        if n == 1 { "revision" } else { "revisions" }
    ));
    orphans(dir, &released);
    emit(&out, 0)
}
