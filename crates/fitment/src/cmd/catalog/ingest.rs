//! `fitment catalog ingest -C DIR FILE|DIR...` (`docs/research/m5-catalog.md`,
//! section 7.2).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use fitment_catalog::document::{CatalogDocument, detail, result};
use fitment_catalog::{Catalog, Id, Ingested, Options, Outcome, Status};

use super::{ERROR, Format, REFUSED, USAGE, emit, fail, shown, worst};
use crate::cmd::text::{note, printable};

#[derive(clap::Args)]
pub struct Args {
    /// STEP files, or directories of them.
    #[arg(required = true, value_name = "FILE|DIR")]
    inputs: Vec<PathBuf>,
    /// Store the one file named under this id; without it, each file's id
    /// is made from its name.
    #[arg(long, value_name = "ID")]
    id: Option<String>,
    /// The largest file read, in MiB.
    #[arg(long, value_name = "N", default_value_t = 128)]
    max_mib: u64,
    /// How to print the result.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,
}

impl Args {
    /// The catalog's options for this run.
    pub(super) fn options(&self) -> Options {
        Options::default().with_max_bytes(self.max_mib.saturating_mul(1024 * 1024))
    }
}

/// CL2's code for one file.
fn code(line: &Ingested) -> u8 {
    let summary = line.summary.as_ref();
    let quarantined = summary.is_some_and(|s| s.status == Status::Refused);
    let internal = summary.is_some_and(|s| s.internal);
    match &line.outcome {
        Outcome::Added | Outcome::Unchanged if internal => ERROR,
        Outcome::Added | Outcome::Unchanged if quarantined => REFUSED,
        Outcome::Added | Outcome::Unchanged | Outcome::Duplicate { .. } => 0,
        Outcome::DuplicateUnderId { .. } | Outcome::Conflict { .. } => USAGE,
        Outcome::Refused { .. } => REFUSED,
        _ => ERROR,
    }
}

/// The file's name, escaped, for a line without an id.
fn name(line: &Ingested) -> String {
    printable(&line.input.file_name().unwrap_or_default().to_string_lossy())
}

/// One table line (section 7.2).
fn table_line(line: &Ingested) -> String {
    let id = line
        .id
        .as_ref()
        .map_or_else(|| name(line), |id| id.as_str().to_owned());
    let sha = line.sha256.as_ref().map_or("-", |s| s.short(8));
    let product = || {
        let id = line.summary.as_ref().and_then(|s| s.product_id.as_deref());
        format!("'{}'", printable(id.unwrap_or_default()))
    };
    let rest = match (&line.outcome, line.summary.as_ref()) {
        (Outcome::Added, Some(s)) => match (s.counts, &s.first_refusal) {
            (Some(c), _) => format!(
                "{}  {} holes, {} shafts, {} planes",
                product(),
                c.holes,
                c.shafts,
                c.planes
            ),
            (None, Some(r)) => format!("{}  refused: {}", product(), printable(r)),
            (None, None) => product(),
        },
        (Outcome::Unchanged, _) => product(),
        _ => detail(line).map(|d| printable(&d)).unwrap_or_default(),
    };
    format!("{:<12} {id:<20} {sha:<8}  {rest}", result(line))
}

/// stderr's count line, then a line per quarantined file (section 7.2).
fn report(dir: &Path, lines: &[Ingested]) {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for word in [
        "added",
        "unchanged",
        "quarantined",
        "duplicate",
        "conflict",
        "refused",
        "error",
    ] {
        let n = lines.iter().filter(|l| result(l) == word).count();
        if n > 0 {
            counts.push((word, n));
        }
    }
    let counts: Vec<String> = counts.iter().map(|(w, n)| format!("{n} {w}")).collect();
    let files = if lines.len() == 1 { "file" } else { "files" };
    note(&format!(
        "fitment: {}: {} {files}: {}",
        shown(dir),
        lines.len(),
        counts.join(", ")
    ));
    for line in lines {
        let input = printable(&line.input.display().to_string());
        if line.replaced_blob {
            note(&format!(
                "warning: {input}: the stored file of its content held other bytes and was \
                 replaced: run fitment catalog verify -C {}",
                shown(dir)
            ));
        }
        let Some(summary) = line.summary.as_ref() else {
            continue;
        };
        if result(line) != "quarantined" {
            continue;
        }
        if summary.internal {
            note(&format!(
                "fitment: {input}: refused as internal: this is a fitment bug; please report it"
            ));
        }
        if let Some(reason) = &summary.first_refusal {
            note(&format!(
                "fitment: {input}: quarantined: {}",
                printable(reason)
            ));
        }
    }
}

/// The document: the entries added or unchanged, and every line.
fn document(catalog: &mut Catalog, lines: &[Ingested]) -> Result<String, fitment_catalog::Error> {
    let snapshot = catalog.snapshot()?;
    Ok(CatalogDocument::ingested(&snapshot, lines)?.to_json())
}

pub(super) fn run(args: &Args, dir: &Path, catalog: &mut Catalog) -> ExitCode {
    let id = match args.id.as_deref().map(Id::parse) {
        None => None,
        Some(Ok(id)) => Some(id),
        Some(Err(error)) => {
            note(&format!("error: {}", printable(&error.to_string())));
            return ExitCode::from(USAGE);
        }
    };
    let run = match catalog.ingest(&args.inputs, id.as_ref()) {
        Ok(run) => run,
        Err(error) => return fail(dir, &error),
    };
    let lines = &run.lines;
    report(dir, lines);
    let mut code = worst(lines.iter().map(code));
    if let Some(error) = &run.stopped {
        note(&format!(
            "fitment: {}: stopped: {}; the files after it were not read",
            shown(dir),
            printable(&error.to_string())
        ));
        code = ERROR;
    }
    match args.format {
        Format::Table => {
            let mut text = String::new();
            for line in lines {
                let _ = writeln!(text, "{}", table_line(line));
            }
            emit(&text, code)
        }
        Format::Json => match document(catalog, lines) {
            Ok(text) => emit(&text, code),
            Err(error) => fail(dir, &error),
        },
    }
}
