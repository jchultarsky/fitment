//! `fitment features FILE`: read one part and print its features.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::ValueEnum;
use fitment_core::{Bottom, FeaturesDocument, Part, RefusalCode, StackEnd, Status};

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
/// cannot start a new line or move the terminal: one reason, one line. The
/// Unicode line and paragraph separators and the bidirectional controls
/// are escaped too: they split or reorder lines in some terminals and log
/// viewers.
fn printable(text: &str) -> String {
    let unsafe_format = |c: char| {
        matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
    };
    text.chars()
        .flat_map(|c| {
            let escaped: Vec<char> = if c.is_control() || unsafe_format(c) {
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

/// A number for people: up to 4 decimals, trailing zeros dropped.
fn mm(x: f64) -> String {
    let text = format!("{x:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

fn vector(v: [f64; 3]) -> String {
    format!("({}, {}, {})", mm(v[0]), mm(v[1]), mm(v[2]))
}

/// An end of a hole or shaft, briefly: its kind and station.
fn end(end: &StackEnd) -> String {
    match end {
        StackEnd::Opening {
            rim_mm,
            countersink,
            ..
        } => match countersink {
            Some(c) => format!(
                "opening@{} countersink Ø{} {}°",
                mm(*rim_mm),
                mm(c.outer_diameter_mm),
                mm(c.angle_rad.to_degrees())
            ),
            None => format!("opening@{}", mm(*rim_mm)),
        },
        StackEnd::Bottom { at_mm, bottom, .. } => {
            let kind = match bottom {
                Bottom::Flat { .. } => "flat".to_owned(),
                Bottom::ConeThenFlat { angle_rad, .. } => {
                    format!("cone {}° then flat", mm(angle_rad.to_degrees()))
                }
                Bottom::Point {
                    angle_rad,
                    tip_length_mm,
                    ..
                } => format!(
                    "point {}° tip {}",
                    mm(angle_rad.to_degrees()),
                    mm(*tip_length_mm)
                ),
                Bottom::Annular { post_radius_mm, .. } => {
                    format!("annular post Ø{}", mm(2.0 * post_radius_mm))
                }
                _ => "bottom".to_owned(),
            };
            format!("bottom@{} {kind}", mm(*at_mm))
        }
        StackEnd::FreeEnd { rim_mm, .. } => format!("free@{}", mm(*rim_mm)),
        StackEnd::Root { rim_mm, .. } => format!("root@{}", mm(*rim_mm)),
        StackEnd::Irregular { .. } => "irregular".to_owned(),
        StackEnd::Unknown { reason, .. } => format!("unknown ({})", serde_name(reason)),
        _ => "?".to_owned(),
    }
}

/// A value's name as the JSON writes it.
fn serde_name<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.get("kind").and_then(|k| k.as_str()).map(str::to_owned))
        })
        .unwrap_or_default()
}

/// One line per feature.
fn features(part: &Part, lines: &mut Vec<String>) {
    lines.push(format!(
        "faces     {} ({} planar, {} cylindrical, {} conical, {} toroidal, {} other)   complete {}",
        part.face_counts.total,
        part.face_counts.plane,
        part.face_counts.cylindrical,
        part.face_counts.conical,
        part.face_counts.toroidal,
        part.face_counts.spherical + part.face_counts.other,
        part.complete
    ));
    for plane in &part.planes {
        lines.push(format!(
            "{:<16}normal {} offset {}   area {}   {} face(s)",
            plane.id.to_string(),
            vector(plane.normal.to_array()),
            mm(plane.offset_mm),
            plane.area_mm2.map_or_else(|| "unknown".to_owned(), mm),
            plane.faces.len()
        ));
    }
    for c in &part.cylinders {
        lines.push(format!(
            "{:<16}{} Ø{} {}   axis {} through {}",
            c.id.to_string(),
            serde_name(&c.sense),
            mm(2.0 * c.radius_mm),
            serde_name(&c.class),
            vector(c.axis.direction.to_array()),
            vector(c.axis.point_mm.to_array())
        ));
    }
    for (id, what) in part
        .cones
        .iter()
        .map(|c| {
            (
                c.id,
                format!("semi-angle {}°", mm(c.semi_angle_rad.to_degrees())),
            )
        })
        .chain(part.tori.iter().map(|t| {
            (
                t.id,
                format!("R {} r {}", mm(t.major_radius_mm), mm(t.minor_radius_mm)),
            )
        }))
        .chain(
            part.other_faces
                .iter()
                .map(|o| (o.id, printable(&o.entity))),
        )
    {
        lines.push(format!("{:<16}{what}", id.to_string()));
    }
    stacks(part, lines);
    for note in &part.notes {
        lines.push(format!(
            "note      #{} {}: {}",
            note.instance,
            serde_name(&note.code),
            printable(&note.message)
        ));
    }
}

/// One line per hole and shaft.
fn stacks(part: &Part, lines: &mut Vec<String>) {
    for h in &part.holes {
        let size = h
            .length_mm
            .map(|l| format!("length {}", mm(l)))
            .or_else(|| h.depth_mm.map(|d| format!("depth {}", mm(d))));
        let diameters: Vec<String> = h
            .segments
            .iter()
            .map(|s| format!("Ø{}", mm(s.diameter_mm)))
            .collect();
        lines.push(format!(
            "{:<16}{} {} {}   {} → {}",
            h.id.to_string(),
            serde_name(&h.kind),
            diameters.join("/"),
            size.unwrap_or_default(),
            h.ends.first().map(end).unwrap_or_default(),
            h.ends.get(1).map(end).unwrap_or_default()
        ));
    }
    for s in &part.shafts {
        let size = s
            .protrusion_mm
            .map(|p| format!("protrusion {}", mm(p)))
            .or_else(|| s.length_mm.map(|l| format!("length {}", mm(l))));
        let diameters: Vec<String> = s
            .segments
            .iter()
            .map(|g| format!("Ø{}", mm(g.diameter_mm)))
            .collect();
        lines.push(format!(
            "{:<16}{} {} {}   {} → {}",
            s.id.to_string(),
            serde_name(&s.kind),
            diameters.join("/"),
            size.unwrap_or_default(),
            s.ends.first().map(end).unwrap_or_default(),
            s.ends.get(1).map(end).unwrap_or_default()
        ));
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
        features(part, &mut lines);
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
