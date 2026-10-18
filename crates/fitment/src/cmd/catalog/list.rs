//! `fitment catalog list -C DIR` (`docs/research/m5-catalog.md`, section
//! 7.3).

use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use clap::ValueEnum;
use fitment_catalog::document::{CatalogDocument, Entry};
use fitment_catalog::{Catalog, EntryStatus, Listed};

use super::{Format, emit, fail, shown};
use crate::cmd::text::{note, printable};

#[derive(clap::Args)]
pub struct Args {
    /// Which entries to list.
    #[arg(long, value_enum, default_value_t = Which::All)]
    status: Which,
    /// How to print them.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Which {
    /// Searched by a match.
    Indexed,
    /// Refused by the gate; never searched. Each is shown with its
    /// refusals.
    Quarantined,
    /// Read by another fitment: `reindex` them.
    Stale,
    /// Every live entry.
    All,
}

fn wanted(which: Which, status: Option<EntryStatus>) -> bool {
    matches!(
        (which, status),
        (Which::All, _)
            | (Which::Indexed, Some(EntryStatus::Indexed))
            | (Which::Quarantined, Some(EntryStatus::Quarantined))
            | (
                Which::Stale,
                Some(
                    EntryStatus::StaleExtractor(_)
                        | EntryStatus::StaleSignature(_)
                        | EntryStatus::StaleProperties(_)
                )
            )
    )
}

/// The status column: RX2's status, and `*` for an incomplete part (VD11).
pub(super) fn status_text(listed: &Listed) -> String {
    match listed.status {
        Some(EntryStatus::Indexed) if listed.complete == Some(false) => "indexed*".into(),
        Some(status) => status.text(),
        None => "-".into(),
    }
}

fn count(n: Option<u32>) -> String {
    n.map_or_else(|| "-".to_owned(), |n| n.to_string())
}

/// One table row (section 7.3).
fn row(listed: &Listed) -> String {
    let c = listed.counts;
    format!(
        "{:<25} {:<4} {:<20} {:<8} {:>5} {:>6} {:>6}  {:<16}  {}",
        listed.id,
        format!("r{}", listed.revision),
        status_text(listed),
        listed.sha256.short(8),
        count(c.map(|c| c.holes)),
        count(c.map(|c| c.shafts)),
        count(c.map(|c| c.planes)),
        format!(
            "'{}'",
            printable(listed.product.0.as_deref().unwrap_or_default())
        ),
        printable(listed.file_name.as_deref().unwrap_or_default()),
    )
}

fn table(entries: &[&Listed], which: Which) -> String {
    let mut out = format!(
        "{:<25} {:<4} {:<20} {:<8} {:>5} {:>6} {:>6}  {:<16}  file\n",
        "id", "rev", "status", "sha256", "holes", "shafts", "planes", "product"
    );
    for listed in entries {
        let _ = writeln!(out, "{}", row(listed));
        if which == Which::Quarantined {
            let by = listed.versions.0.unwrap_or_default();
            let _ = writeln!(out, "    refused by extractor {by}");
            for r in &listed.refusals {
                let _ = writeln!(out, "    {}: {}", r.code, printable(&r.message));
            }
        }
    }
    if entries.iter().any(|l| l.complete == Some(false)) {
        out.push_str("* incomplete: some faces were not read, so a match can only reject it\n");
    }
    out
}

pub(super) fn run(args: &Args, dir: &Path, catalog: &mut Catalog) -> ExitCode {
    let read = catalog.snapshot().and_then(|snapshot| {
        let all = snapshot.list()?;
        let kept = snapshot.kept_revisions()?;
        Ok((snapshot.generation(), all, kept))
    });
    let (generation, all, (kept, kept_bytes)) = match read {
        Ok(read) => read,
        Err(error) => return fail(dir, &error),
    };
    let entries: Vec<&Listed> = all
        .iter()
        .filter(|l| wanted(args.status, l.status))
        .collect();
    let tally = |s: &str| {
        all.iter()
            .filter(|l| l.status.is_some_and(|x| x.name() == s))
            .count()
    };
    let mut summary = format!(
        "fitment: {}: generation {generation}: {} entries: {} indexed, {} quarantined, {} stale",
        shown(dir),
        all.len(),
        tally("indexed"),
        tally("quarantined"),
        tally("stale"),
    );
    if kept > 0 {
        let _ = write!(
            summary,
            "; {kept} old revisions kept ({kept_bytes} bytes: fitment catalog prune --revisions)"
        );
    }
    note(&summary);
    let text = match args.format {
        Format::Table => table(&entries, args.status),
        Format::Json => {
            let entries = entries.iter().map(|l| Entry::of(l, None)).collect();
            CatalogDocument::new(generation, &all, entries, None).to_json()
        }
    };
    emit(&text, 0)
}
