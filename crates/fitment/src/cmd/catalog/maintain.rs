//! `fitment catalog gc` and `verify` (`docs/research/m5-catalog.md`,
//! sections 7.8 and 7.9). With `--format json` they print `fitment.gc`
//! and `fitment.verify` (CU2, CA5, `docs/research/m6-api.md`).

use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use fitment_catalog::document::{GcDocument, VerifyDocument};
use fitment_catalog::{Catalog, NoObserver};

use super::{ERROR, Format, REFUSED, emit, fail, shown};
use crate::cmd::text::{note, printable};

#[derive(clap::Args)]
pub struct GcArgs {
    /// Also delete the rows of older fitment versions where this build's
    /// exist; a newer version's rows are never touched.
    #[arg(long)]
    versions: bool,
    /// Also run VACUUM, which needs free space about the database's size.
    #[arg(long)]
    vacuum: bool,
    /// How to print the result: `table` prints nothing on stdout.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,
}

#[derive(clap::Args)]
pub struct VerifyArgs {
    /// Also extract every live part again from its file and require the
    /// stored rows equal: the check that the index is the store's.
    #[arg(long)]
    deep: bool,
    /// SQLite's whole integrity check, not its quick one.
    #[arg(long)]
    full: bool,
    /// How to print the result.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,
}

pub(super) fn gc(args: &GcArgs, dir: &Path, catalog: &mut Catalog) -> ExitCode {
    if args.vacuum {
        // SZ4: VACUUM copies the database, its log folded in first, and
        // fails whole when the disk cannot hold the copy.
        let size = |name: &str| std::fs::metadata(catalog.root().join(name)).map_or(0, |m| m.len());
        let total = size("catalog.db") + size("catalog.db-wal");
        note(&format!(
            "fitment: {}: VACUUM needs free space about the database's size ({} MB); without it, VACUUM itself fails and changes nothing",
            shown(dir),
            total.div_ceil(1024 * 1024)
        ));
    }
    let collected = match catalog.gc(args.versions, args.vacuum) {
        Ok(c) => c,
        Err(error) => return fail(dir, &error),
    };
    let old = if args.versions {
        format!(", {} rows of older versions removed", collected.old_rows)
    } else {
        String::new()
    };
    note(&format!(
        "fitment: {}: {} orphan files removed, {} temporary files removed{old}",
        shown(dir),
        collected.unlinked.len(),
        collected.temporaries.len()
    ));
    for failed in &collected.failed {
        note(&format!(
            "warning: {}: could not remove {}",
            shown(dir),
            printable(failed)
        ));
    }
    let code = if collected.failed.is_empty() {
        0
    } else {
        ERROR
    };
    match args.format {
        Format::Table => ExitCode::from(code),
        Format::Json => {
            let document = GcDocument::new(&collected, args.versions, args.vacuum);
            emit(&document.to_json(), code)
        }
    }
}

pub(super) fn verify(args: &VerifyArgs, dir: &Path, catalog: &mut Catalog) -> ExitCode {
    let verified = match catalog.verify_observed(args.deep, args.full, &NoObserver) {
        Ok(v) => v,
        Err(error) => return fail(dir, &error),
    };
    let problems = &verified.problems;
    let mut out = String::new();
    for p in problems {
        // A SHA-256 shortened; anything else, as stored. Both escaped, ids
        // too: verify reads a catalog it does not trust.
        let what = if fitment_catalog::Sha256Hex::parse(&p.what).is_ok() {
            p.what.get(..12).unwrap_or(&p.what)
        } else {
            p.what.as_str()
        };
        let _ = writeln!(
            out,
            "{:<10} {:<20} {:<12}  {}",
            p.kind.name(),
            printable(p.id.as_deref().unwrap_or("-")),
            printable(what),
            printable(&p.detail)
        );
    }
    let n = problems.len();
    note(&if n == 0 {
        format!("fitment: {}: consistent", shown(dir))
    } else {
        format!(
            "fitment: {}: {n} {}",
            shown(dir),
            if n == 1 { "problem" } else { "problems" }
        )
    });
    let code = if n == 0 { 0 } else { REFUSED };
    match args.format {
        Format::Table => emit(&out, code),
        Format::Json => {
            let document = VerifyDocument::new(&verified, args.deep, args.full);
            emit(&document.to_json(), code)
        }
    }
}
