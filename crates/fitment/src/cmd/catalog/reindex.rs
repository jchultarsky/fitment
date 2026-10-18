//! `fitment catalog reindex -C DIR [REF...] [--all]`
//! (`docs/research/m5-catalog.md`, section 7.7, RX4 to RX9): one line per
//! entry processed with its transition, a summary on stderr, and RX6's
//! lines for each part that leaves the search.

use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use fitment_catalog::document::{CatalogDocument, Change, Entry};
use fitment_catalog::{Catalog, EntryStatus, Error, Rebuilt, Reference, Reindex, Reindexed, Scope};

use super::list::status_name;
use super::{ERROR, Format, REFUSED, USAGE, emit, fail, shown, worst};
use crate::cmd::text::{note, printable};

#[derive(clap::Args)]
pub struct Args {
    /// Entries to process, current or not: ID, ID@rN (the current
    /// revision) or sha256:HEX. Without one, every stale entry.
    #[arg(value_name = "REF", conflicts_with = "all")]
    references: Vec<String>,
    /// Every live entry.
    #[arg(long)]
    all: bool,
    /// How to print the result.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,
}

/// The first word of a line: the status after, or what went wrong.
fn result(line: &Reindexed) -> &'static str {
    match &line.rebuilt {
        Rebuilt::Damaged(_) => "damaged",
        Rebuilt::TooLarge => "too_large",
        Rebuilt::Changed => "changed",
        Rebuilt::Failed(_) => "failed",
        Rebuilt::Current | Rebuilt::Written { .. } => line.after.map_or("-", EntryStatus::name),
        _ => "-",
    }
}

/// What a line says after its transition.
fn detail(line: &Reindexed) -> String {
    let status = |s: Option<EntryStatus>| s.map_or_else(|| "-".to_owned(), status_name);
    let mut text = format!("{} -> {}", status(line.before), status(line.after));
    match &line.rebuilt {
        Rebuilt::Damaged(why) => return format!("{}: {why}", status(line.before)),
        Rebuilt::TooLarge => {
            return "derived_too_large: its features document is too large to store".into();
        }
        Rebuilt::Changed => return "replaced or deleted while reindex ran".into(),
        Rebuilt::Failed(why) => return why.clone(),
        Rebuilt::Current => return text + " (nothing missing)",
        _ => {}
    }
    if let (Some(EntryStatus::Quarantined), Some(first)) = (line.after, &line.first_refusal) {
        let _ = write!(text, ": {first}");
    }
    if line.left_search() {
        text.push_str(" (no longer searched)");
    } else if line.joined_search() {
        text.push_str(" (now searched)");
    }
    text
}

/// One table line (section 7.7).
fn row(line: &Reindexed) -> String {
    format!(
        "{:<12} {:<25} r{:<3} {}  {}",
        result(line),
        printable(&line.id),
        line.revision,
        line.sha256.short(8),
        printable(&detail(line))
    )
}

/// CL2's code: 1 for a damaged entry, a failed batch or a stopped run; 3
/// for a part that left the search (RX6) or whose document is too large
/// to store, as ingest refuses it; else 0.
fn code(run: &Reindex) -> u8 {
    let codes = run.lines.iter().map(|line| match &line.rebuilt {
        Rebuilt::Damaged(_) | Rebuilt::Failed(_) => ERROR,
        Rebuilt::TooLarge => REFUSED,
        _ if line.left_search() => REFUSED,
        _ => 0,
    });
    let stopped = run.stopped.as_ref().map(|_| ERROR);
    worst(codes.chain(stopped))
}

fn summary(dir: &Path, run: &Reindex) {
    let written: Vec<&Reindexed> = run
        .lines
        .iter()
        .filter(|l| matches!(l.rebuilt, Rebuilt::Written { .. }))
        .collect();
    let quarantined = written
        .iter()
        .filter(|l| l.after == Some(EntryStatus::Quarantined))
        .count();
    let indexed = written
        .iter()
        .filter(|l| {
            matches!(
                l.after,
                Some(EntryStatus::Indexed | EntryStatus::StaleProperties(_))
            )
        })
        .count();
    let joined = run.lines.iter().filter(|l| l.joined_search()).count();
    let left = run.lines.iter().filter(|l| l.left_search()).count();
    note(&format!(
        "fitment: {}: {} reindexed: {indexed} indexed, {quarantined} quarantined; {joined} newly searched, {left} no longer searched",
        shown(dir),
        written.len()
    ));
    for line in run.lines.iter().filter(|l| l.left_search()) {
        note(&format!(
            "fitment: {}: {}: no longer searched: {}",
            shown(dir),
            printable(&line.id),
            printable(line.first_refusal.as_deref().unwrap_or_default())
        ));
    }
    for line in &run.lines {
        if let Rebuilt::Damaged(why) = &line.rebuilt {
            note(&format!(
                "fitment: {}: {}: {}: left stale: run fitment catalog verify -C {}",
                shown(dir),
                printable(&line.id),
                printable(why),
                shown(dir)
            ));
        }
    }
    if let Some(error) = &run.stopped {
        note(&format!(
            "fitment: {}: stopped: {}: run reindex again to finish",
            shown(dir),
            printable(&error.to_string())
        ));
    }
}

/// The document: every entry processed as it is now, and every line.
fn document(catalog: &mut Catalog, run: &Reindex) -> Result<String, Error> {
    let snapshot = catalog.snapshot()?;
    let all = snapshot.list()?;
    let processed: std::collections::BTreeSet<&str> =
        run.lines.iter().map(|l| l.id.as_str()).collect();
    let entries = all
        .iter()
        .filter(|l| processed.contains(l.id.as_str()))
        .map(|l| Entry::of(l, None))
        .collect();
    let changes = run
        .lines
        .iter()
        .map(|line| {
            Change::entry(
                &line.id,
                result(line),
                &line.id,
                line.revision,
                line.sha256.as_str(),
                Some(detail(line)),
            )
        })
        .collect();
    let document = CatalogDocument::new(snapshot.generation(), &all, entries, Some(changes));
    Ok(document.to_json())
}

pub(super) fn run(args: &Args, dir: &Path, catalog: &mut Catalog) -> ExitCode {
    let scope = if args.all {
        Scope::All
    } else if args.references.is_empty() {
        Scope::Stale
    } else {
        let mut references = Vec::new();
        for text in &args.references {
            match Reference::parse(text) {
                Ok(r) => references.push(r),
                Err(error) => {
                    note(&format!("error: {}", printable(&error.to_string())));
                    return ExitCode::from(USAGE);
                }
            }
        }
        Scope::Named(references)
    };
    let run = match catalog.reindex(&scope) {
        Ok(Ok(run)) => run,
        Ok(Err(unnamed)) => {
            note(&format!(
                "fitment: {}: {}",
                shown(dir),
                printable(&unnamed.to_string())
            ));
            return ExitCode::from(USAGE);
        }
        Err(error) => return fail(dir, &error),
    };
    summary(dir, &run);
    let code = code(&run);
    let text = match args.format {
        Format::Table => run.lines.iter().map(|l| row(l) + "\n").collect(),
        Format::Json => match document(catalog, &run) {
            Ok(text) => text,
            Err(error) => return fail(dir, &error),
        },
    };
    emit(&text, code)
}
