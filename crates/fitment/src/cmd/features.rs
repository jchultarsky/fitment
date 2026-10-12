//! `fitment features FILE`: read one part and print its features.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::ValueEnum;
use fitment_core::{FeaturesDocument, RefusalCode, Status};

use crate::exit;

#[derive(clap::Args)]
pub struct Args {
    /// The STEP file to read.
    file: PathBuf,
    /// How to print the result. `json` is the `features.v1` document,
    /// printed for refused input too; `table` is a summary for people and
    /// is not a stable format.
    #[arg(long, value_enum, default_value_t = Format::Json)]
    format: Format,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Json,
    Table,
}

/// `text` with every control character escaped, so that text from a file
/// cannot start a new line or move the terminal: one reason, one line.
fn printable(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            let escaped: Vec<char> = if c.is_control() {
                c.escape_default().collect()
            } else {
                vec![c]
            };
            escaped
        })
        .collect()
}

/// Writes one line to stderr. A closed stderr is not worth a panic.
fn note(line: &str) {
    let _ = writeln!(io::stderr().lock(), "{line}");
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
    let document = FeaturesDocument::read(&bytes);
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

/// A summary for people: one line per fact. Not a stable format.
fn table(document: &FeaturesDocument) -> String {
    let mut lines = Vec::new();
    if let Some(header) = document.header() {
        lines.push(format!(
            "schema    {}   {}",
            header.schema.as_deref().unwrap_or("unrecognized"),
            printable(header.originating_system.as_deref().unwrap_or(""))
        ));
    }
    if let Some(part) = document.part() {
        let product = part.definition.product.as_ref();
        lines.push(format!(
            "part      #{} '{}'   frame #{}",
            part.definition.instance,
            printable(product.and_then(|p| p.name.as_deref()).unwrap_or("")),
            part.frame.shape_representation
        ));
        for solid in &part.solids {
            let placed = if solid.transform.is_some() {
                "   placed by a mapped item"
            } else {
                ""
            };
            lines.push(format!(
                "{:<12}{}{placed}",
                solid.id.to_string(),
                solid.entity
            ));
        }
        for item in &part.other_items {
            lines.push(format!("#{:<11}{}", item.instance, printable(&item.entity)));
        }
    }
    for refusal in document.refusals() {
        // The message names the instance at fault itself.
        lines.push(format!(
            "refused   {}: {}",
            refusal.code.name(),
            printable(&refusal.message)
        ));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}
