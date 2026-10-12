//! `fitment parts FILE`: list the parts of an assembly, with their
//! quantities and occurrences, so that a user can choose one to replace.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::ValueEnum;
use fitment_core::{DefinitionKind, PartsDocument, RefusalCode, Status};

use super::text::{note, printable};
use crate::exit;

#[derive(clap::Args)]
pub struct Args {
    /// The STEP file to read.
    file: PathBuf,
    /// How to print the result. `table` is for people choosing a part and
    /// is not a stable format; `json` is the `fitment.parts` document,
    /// printed for refused input too.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Json,
    Table,
}

pub fn run(args: &Args) -> ExitCode {
    let path = printable(&args.file.display().to_string());
    let bytes = match std::fs::read(&args.file) {
        Ok(bytes) => bytes,
        Err(error) => {
            note(&format!("error: cannot read {path}: {error}"));
            return ExitCode::FAILURE;
        }
    };
    let document = PartsDocument::read(&bytes);
    for refusal in document.refusals() {
        note(&format!(
            "fitment: {path}: refused: {}: {}",
            refusal.code.name(),
            printable(&refusal.message)
        ));
    }
    let code = match document.status() {
        Status::Accepted => ExitCode::SUCCESS,
        // An internal refusal is a fitment bug, not a property of the
        // input, so it is an error rather than a refusal.
        Status::Refused
            if document
                .refusals()
                .iter()
                .any(|refusal| refusal.code == RefusalCode::Internal) =>
        {
            ExitCode::FAILURE
        }
        Status::Refused => ExitCode::from(exit::REFUSED),
    };
    let text = match args.format {
        Format::Json => document.to_json(),
        Format::Table => table(&document),
    };
    match io::stdout().lock().write_all(text.as_bytes()) {
        Ok(()) => code,
        // A reader that stopped reading, such as `head`, is not an error.
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            note(&format!("error: cannot write the result: {error}"));
            ExitCode::FAILURE
        }
    }
}

/// One line per definition, parts first, then the assembly gate.
fn table(document: &PartsDocument) -> String {
    let mut lines = Vec::new();
    if document.status() == Status::Refused {
        lines.push("refused".to_owned());
        for refusal in document.refusals() {
            lines.push(format!(
                "  {}: {}",
                refusal.code.name(),
                printable(&refusal.message)
            ));
        }
        return lines.join("\n") + "\n";
    }
    lines.push(format!(
        "{:<24} {:<24} {:<9} {:>8} {:>10}  first occurrence",
        "id", "name", "kind", "quantity", "definition"
    ));
    let mut entries: Vec<_> = document.parts().iter().collect();
    entries.sort_by_key(|entry| (entry.kind, entry.definition.instance));
    for entry in entries {
        let product = entry.definition.product.as_ref();
        let id = printable(product.and_then(|p| p.id.as_deref()).unwrap_or(""));
        let name = printable(product.and_then(|p| p.name.as_deref()).unwrap_or(""));
        let kind = match entry.kind {
            DefinitionKind::Part => "part",
            DefinitionKind::Assembly => "assembly",
        };
        let first = entry
            .occurrences
            .first()
            .map_or_else(String::new, |occurrence| {
                let segments: Vec<String> = occurrence
                    .path
                    .iter()
                    .zip(&occurrence.label)
                    .map(|(instance, label)| match (document.path_form(), label) {
                        (Some(fitment_core::PathForm::UsageId), Some(label)) => printable(label),
                        _ => format!("#{instance}"),
                    })
                    .collect();
                if segments.is_empty() {
                    "(root)".to_owned()
                } else {
                    segments.join("/")
                }
            });
        lines.push(format!(
            "{id:<24} {name:<24} {kind:<9} {:>8} {:>10}  {first}",
            entry.quantity,
            format!("#{}", entry.definition.instance)
        ));
    }
    match document.assembly_gate() {
        Some(gate) if gate.status == Status::Accepted => {
            lines.push("assembly gate: accepted".to_owned());
        }
        Some(gate) => {
            lines.push("assembly gate: refused".to_owned());
            for refusal in &gate.refusals {
                lines.push(format!(
                    "  {}: {}",
                    refusal.code.name(),
                    printable(&refusal.message)
                ));
            }
        }
        None => {}
    }
    for note in document.notes() {
        lines.push(format!("note: {}", printable(&note.message)));
    }
    lines.join("\n") + "\n"
}
