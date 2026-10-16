//! `fitment catalog gc` and `verify` (`docs/research/m5-catalog.md`,
//! sections 7.8 and 7.9).

use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use fitment_catalog::Catalog;

use super::{ERROR, REFUSED, emit, fail, shown};
use crate::cmd::text::{note, printable};

#[derive(clap::Args)]
pub struct GcArgs {
    /// Also run VACUUM, which needs free space about the database's size.
    #[arg(long)]
    vacuum: bool,
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
}

pub(super) fn gc(args: &GcArgs, dir: &Path, catalog: &mut Catalog) -> ExitCode {
    let collected = match catalog.gc(args.vacuum) {
        Ok(c) => c,
        Err(error) => return fail(dir, &error),
    };
    note(&format!(
        "fitment: {}: {} orphan files removed, {} temporary files removed",
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
    ExitCode::from(if collected.failed.is_empty() {
        0
    } else {
        ERROR
    })
}

pub(super) fn verify(args: &VerifyArgs, dir: &Path, catalog: &mut Catalog) -> ExitCode {
    let problems = match catalog.verify(args.deep, args.full) {
        Ok(p) => p,
        Err(error) => return fail(dir, &error),
    };
    let mut out = String::new();
    for p in &problems {
        // A SHA-256 shortened; anything else, as stored. Both escaped, ids
        // too: verify reads a catalog it does not trust.
        let what = if p.what.len() == 64 {
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
    emit(&out, if n == 0 { 0 } else { REFUSED })
}
