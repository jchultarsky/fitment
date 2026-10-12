//! `fitment socket FILE --part X`: what the neighbours of a part require of
//! any part that replaces it, as a `fitment.socket` document.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::ValueEnum;
use fitment_core::{
    BoreSpan, Fit, RefusalCode, Requires, SelectError, ShaftSpan, SocketDocument, Status,
};

use super::text::{mm, note, printable};
use crate::exit;

#[derive(clap::Args)]
pub struct Args {
    /// The assembly to read.
    file: PathBuf,
    /// The part to replace: its PRODUCT.id, its name, or `#<n>` for a
    /// definition `#id`.
    #[arg(long)]
    part: Option<String>,
    /// One occurrence only, by its path: usage ids or `#<n>` joined by `/`.
    #[arg(long)]
    occurrence: Option<String>,
    /// `json` is the `fitment.socket` document; `table` is a review view
    /// for people and is not a stable format.
    #[arg(long, value_enum, default_value_t = Format::Json)]
    format: Format,
    /// Write the document here instead of printing it.
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Json,
    Table,
}

/// The candidates a selection error offers, one line each.
fn report(path: &str, error: &SelectError) {
    note(&format!(
        "fitment: {path}: {}: {}",
        error.code(),
        printable(&error.to_string())
    ));
    for candidate in error.candidates() {
        let product = candidate.definition.product.as_ref();
        note(&format!(
            "  #{} '{}' ({}), {} occurrences{}",
            candidate.definition.instance,
            printable(product.and_then(|p| p.id.as_deref()).unwrap_or("")),
            printable(product.and_then(|p| p.name.as_deref()).unwrap_or("")),
            candidate.quantity,
            candidate
                .first_path
                .as_ref()
                .map_or_else(String::new, |p| format!(", first at {}", printable(p)))
        ));
    }
    if let SelectError::OccurrenceNotFound { occurrences, .. } = error {
        for occurrence in occurrences {
            note(&format!("  {}", printable(occurrence)));
        }
    }
    note("  (run `fitment parts` to list the parts)");
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
    if args.part.is_none() && args.occurrence.is_none() {
        note("error: name the part to replace with --part, or one occurrence with --occurrence");
        return ExitCode::from(2);
    }
    let document =
        match SocketDocument::extract(&bytes, args.part.as_deref(), args.occurrence.as_deref()) {
            Ok(document) => document,
            Err(error) => {
                report(&path, &error);
                return ExitCode::from(2);
            }
        };
    for refusal in document.refusals() {
        note(&format!(
            "fitment: {path}: refused: {}: {}",
            refusal.code.name(),
            printable(&refusal.message)
        ));
    }
    let code = match document.status() {
        Status::Accepted => {
            summary(&path, &document);
            ExitCode::SUCCESS
        }
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
    if let Some(output) = &args.output {
        if document.status() == Status::Refused {
            note(&format!(
                "fitment: {}: left unchanged: the assembly was refused",
                printable(&output.display().to_string())
            ));
        } else if let Err(error) = write_atomically(output, document.to_json().as_bytes()) {
            note(&format!(
                "error: cannot write {}: {error}",
                printable(&output.display().to_string())
            ));
            return ExitCode::FAILURE;
        }
        if args.format == Format::Json && document.status() == Status::Accepted {
            return code;
        }
    }
    match io::stdout().lock().write_all(text.as_bytes()) {
        Ok(()) => code,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            note(&format!("error: cannot write the result: {error}"));
            ExitCode::FAILURE
        }
    }
}

impl PartialEq for Format {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Json, Self::Json) | (Self::Table, Self::Table)
        )
    }
}

/// Writes `bytes` to a temporary file beside `path`, then renames it.
fn write_atomically(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

/// One line on stderr: occurrences, sockets, requirements, left out, and a
/// warning per requirement no candidate can pass yet.
fn summary(path: &str, document: &SocketDocument) {
    let Some(extracted) = document.extracted() else {
        return;
    };
    let requirements: usize = extracted.sockets.iter().map(|s| s.requirements.len()).sum();
    let left: usize = extracted.sockets.iter().map(|s| s.left_out.len()).sum();
    let name = extracted
        .target
        .definition
        .product
        .as_ref()
        .and_then(|p| p.id.as_deref())
        .unwrap_or("");
    note(&format!(
        "fitment: {path}: {}: {} occurrences, {} sockets, {requirements} requirements, {left} left out",
        printable(name),
        extracted.occurrences.len(),
        extracted.sockets.len()
    ));
    for socket in &extracted.sockets {
        for requirement in &socket.requirements {
            if matches!(requirement.requires, Requires::Unresolved { .. }) {
                note(&format!(
                    "warning: {}: {}: no rule reads it; every candidate fails it until it is removed in review",
                    socket.id, requirement.id
                ));
            }
        }
    }
}

fn fit(fit: Fit) -> &'static str {
    match fit {
        Fit::Clearance => "clearance",
        Fit::NotRead => "not read",
        Fit::LineToLine => "line to line",
        Fit::Interference => "interference",
    }
}

fn bore_span(span: &BoreSpan) -> String {
    format!(
        "{}..{} Ø{} {}{}{}",
        mm(span.from_mm),
        mm(span.to_mm),
        mm(span.hole_diameter_mm),
        fit(span.fit),
        span.clear_diameter_mm
            .map_or_else(String::new, |c| format!(" clear Ø{}", mm(c))),
        if span.pinned { " pinned" } else { "" }
    )
}

fn shaft_span(span: &ShaftSpan) -> String {
    format!(
        "{}..{} Ø{} {}{}{}",
        mm(span.from_mm),
        mm(span.to_mm),
        mm(span.shaft_diameter_mm),
        fit(span.fit),
        span.clear_diameter_mm
            .map_or_else(String::new, |c| format!(" at most Ø{}", mm(c))),
        if span.pinned { " pinned" } else { "" }
    )
}

/// The review view: one line per requirement.
fn table(document: &SocketDocument) -> String {
    let mut lines = Vec::new();
    let Some(extracted) = document.extracted() else {
        lines.push("refused".to_owned());
        for refusal in document.refusals() {
            lines.push(format!(
                "  {}: {}",
                refusal.code.name(),
                printable(&refusal.message)
            ));
        }
        return lines.join("\n") + "\n";
    };
    for socket in &extracted.sockets {
        lines.push(format!(
            "{}  {} occurrences",
            socket.id,
            socket.occurrences.len()
        ));
        for requirement in &socket.requirements {
            let what = match &requirement.requires {
                Requires::Bore { spans, open, .. } => {
                    let spans: Vec<String> = spans.iter().map(bore_span).collect();
                    format!(
                        "{}{}{}",
                        spans.join("; "),
                        if open.start { " open at start" } else { "" },
                        if open.finish { " open at finish" } else { "" }
                    )
                }
                Requires::Shaft { spans, .. } => {
                    spans.iter().map(shaft_span).collect::<Vec<_>>().join("; ")
                }
                Requires::Unresolved { causes, .. } => causes
                    .iter()
                    .map(|c| format!("{:?} ({} faces)", c.cause, c.faces.len()))
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => String::new(),
            };
            lines.push(format!("  -  {:<36} {what}", requirement.id));
        }
        for left in &socket.left_out {
            lines.push(format!(
                "  left out {} Ø{} ({:?})",
                left.feature,
                mm(left.diameter_mm),
                left.reason
            ));
        }
    }
    lines.join("\n") + "\n"
}
