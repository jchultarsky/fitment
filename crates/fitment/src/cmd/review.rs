//! `fitment review FILE`: check a socket document and show its review, or
//! record one decision in it (`docs/research/m3-socket.md`, 9.4 and 12).

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::ValueEnum;
use fitment_core::{ReviewError, ReviewStatus, SocketDocument};

use super::socket::table;
use super::text::{note, printable};
use super::write_atomically;
use crate::exit;

#[derive(clap::Args)]
pub struct Args {
    /// The socket document.
    file: PathBuf,
    /// `table` is the review view, for people, and not a stable format;
    /// `json` is the document.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,
    /// Mark a requirement reviewed and kept.
    #[arg(long, value_name = "ID", group = "decision")]
    keep: Option<String>,
    /// Take a requirement out of matching; needs --note.
    #[arg(long, value_name = "ID", group = "decision")]
    remove: Option<String>,
    /// Return a requirement to unreviewed.
    #[arg(long, value_name = "ID", group = "decision")]
    unreview: Option<String>,
    /// Why: required to remove.
    #[arg(long, requires = "decision")]
    note: Option<String>,
    /// Only in this socket.
    #[arg(long, requires = "decision")]
    socket: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Table,
    Json,
}

pub fn run(args: &Args) -> ExitCode {
    let path = printable(&args.file.display().to_string());
    let text = match super::socket_text(&args.file, &path) {
        Ok(text) => text,
        Err(code) => return code,
    };
    let mut document = match SocketDocument::from_json(&text) {
        Ok(document) => document,
        Err(error) => {
            note(&format!(
                "fitment: {path}: refused: {}",
                printable(&error.to_string())
            ));
            return ExitCode::from(exit::REFUSED);
        }
    };
    let decision = [
        (&args.keep, ReviewStatus::Kept, "kept"),
        (&args.remove, ReviewStatus::Removed, "removed"),
        (&args.unreview, ReviewStatus::Unreviewed, "unreviewed"),
    ]
    .into_iter()
    .find_map(|(id, status, word)| id.as_ref().map(|id| (id, status, word)));
    if let Some((id, status, word)) = decision {
        match document.decide(id, args.socket.as_deref(), status, args.note.as_deref()) {
            Ok(sockets) => {
                if let Err(error) = write_atomically(&args.file, document.to_json().as_bytes()) {
                    note(&format!("error: cannot write {path}: {error}"));
                    return ExitCode::FAILURE;
                }
                note(&format!(
                    "fitment: {path}: {} {word} in {sockets} sockets",
                    printable(id)
                ));
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                note(&format!("error: {path}: {}", printable(&error.to_string())));
                return match error {
                    ReviewError::Refused => ExitCode::from(exit::REFUSED),
                    _ => ExitCode::from(2),
                };
            }
        }
    }
    let view = match args.format {
        Format::Json => document.to_json(),
        Format::Table => view(&document),
    };
    match io::stdout().lock().write_all(view.as_bytes()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            note(&format!("error: cannot write the result: {error}"));
            ExitCode::FAILURE
        }
    }
}

/// The review view: the source, the target and its decisions, then what
/// could not be carried over.
fn view(document: &SocketDocument) -> String {
    let mut out = format!("source sha256 {}\n", document.source().sha256);
    if let Some(extracted) = document.extracted() {
        let product = extracted.target.definition.product.as_ref();
        let _ = writeln!(
            out,
            "target #{} '{}'{}",
            extracted.target.definition.instance,
            printable(product.and_then(|p| p.id.as_deref()).unwrap_or("")),
            if document.is_current() {
                ""
            } else {
                " (extracted by another version: extract it again)"
            }
        );
    }
    out.push_str(&table(document));
    for stale in document.stale_reviews() {
        let _ = writeln!(
            out,
            "stale {} {}: {:?} ({:?}){}",
            stale.socket,
            stale.requirement,
            stale.status,
            stale.reason,
            stale
                .note
                .as_deref()
                .map_or_else(String::new, |n| format!(": {}", printable(n)))
        );
    }
    out
}
