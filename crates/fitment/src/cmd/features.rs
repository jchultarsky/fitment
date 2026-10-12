//! `fitment features FILE`: read one part and print its features.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::ValueEnum;
use fitment_core::{FeaturesDocument, RefusalCode, read_part};

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

pub fn run(args: &Args) -> ExitCode {
    let path = args.file.display();
    let bytes = match std::fs::read(&args.file) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: cannot read {path}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let (document, code) = match read_part(&bytes) {
        Ok(part) => (FeaturesDocument::accepted(part), ExitCode::SUCCESS),
        Err(refused) => {
            for refusal in &refused.refusals {
                eprintln!(
                    "fitment: {path}: refused: {}: {}",
                    refusal.code.name(),
                    refusal.message
                );
            }
            // An internal refusal is a fitment bug, not a property of the
            // input, so it is an error rather than a refusal.
            let internal = refused
                .refusals
                .iter()
                .any(|refusal| refusal.code == RefusalCode::Internal);
            let code = if internal {
                ExitCode::FAILURE
            } else {
                ExitCode::from(exit::REFUSED)
            };
            (FeaturesDocument::refused(refused), code)
        }
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
            eprintln!("error: cannot write the result: {error}");
            ExitCode::FAILURE
        }
    }
}

/// A summary for people: one line per fact. Not a stable format.
fn table(document: &FeaturesDocument) -> String {
    let mut lines = Vec::new();
    if let Some(header) = &document.header {
        lines.push(format!(
            "schema    {}   {}",
            header.schema.as_deref().unwrap_or("unrecognized"),
            header.originating_system.as_deref().unwrap_or("")
        ));
    }
    if let Some(part) = &document.part {
        let product = part.definition.product.as_ref();
        lines.push(format!(
            "part      #{} '{}'   frame #{}",
            part.definition.instance,
            product.and_then(|p| p.name.as_deref()).unwrap_or(""),
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
            lines.push(format!("#{:<11}{}", item.instance, item.entity));
        }
    }
    for refusal in &document.refusals {
        let at = refusal
            .instance
            .map(|id| format!("#{id} "))
            .unwrap_or_default();
        lines.push(format!(
            "refused   {}: {at}{}",
            refusal.code.name(),
            refusal.message
        ));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}
