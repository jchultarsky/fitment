//! The `fitment.catalog` document, version 1 (`docs/research/m5-catalog.md`,
//! section 7.11, CL3, CL9): what `list`, `get` and `ingest --format json`
//! print. It holds no time, host, absolute path or internal key, so the
//! same catalog state gives the same bytes on every OS.

use fitment_core::{EXTRACTOR_VERSION, SIGNATURE_VERSION};
use serde::Serialize;

use crate::ingest::{Counts, Ingested, Outcome};
use crate::preimage::Status;
use crate::snapshot::{EntryStatus, HistoryStep, Listed, StoredRefusal};

/// The format's name.
pub const CATALOG_FORMAT: &str = "fitment.catalog";

/// Its version.
pub const CATALOG_FORMAT_VERSION: u32 = 1;

/// A `fitment.catalog` document.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct CatalogDocument {
    format: &'static str,
    format_version: u32,
    schema_version: i64,
    generation: i64,
    build: Versions,
    counts: Tally,
    entries: Vec<Entry>,
    changes: Option<Vec<Change>>,
}

/// The versions a build or a row is of, named as the document names them.
#[derive(Debug, Clone, Copy, Serialize)]
#[allow(clippy::struct_field_names)]
struct Versions {
    extractor_version: Option<u32>,
    signature_version: Option<u32>,
    /// `PROPERTIES_VERSION` arrives with M5's properties (PR 5).
    properties_version: Option<u32>,
}

/// Live entries by status.
#[derive(Debug, Clone, Copy, Default, Serialize)]
struct Tally {
    entries: usize,
    indexed: usize,
    quarantined: usize,
    stale: usize,
}

#[derive(Debug, Clone, Serialize)]
struct Product {
    id: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct Refusal {
    code: String,
    message: String,
    instance: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct Features {
    planes: u32,
    holes: u32,
    shafts: u32,
    cones: u32,
    other_faces: u32,
}

#[derive(Debug, Clone, Serialize)]
struct Step {
    revision: u32,
    sha256: String,
    generation: i64,
    event: &'static str,
    bytes_kept: bool,
}

/// One entry's revision.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    id: String,
    revision: u32,
    status: Option<&'static str>,
    sha256: String,
    bytes: Option<u64>,
    file_name: Option<String>,
    product: Product,
    versions: Versions,
    complete: Option<bool>,
    features: Option<Features>,
    refusals: Vec<Refusal>,
    properties: Option<()>,
    history: Option<Vec<Step>>,
}

/// One input's or entry's change.
#[derive(Debug, Clone, Serialize)]
pub struct Change {
    input: String,
    result: &'static str,
    id: Option<String>,
    revision: Option<u32>,
    sha256: Option<String>,
    detail: Option<String>,
}

fn refusal(r: &StoredRefusal) -> Refusal {
    Refusal {
        code: r.code.clone(),
        message: r.message.clone(),
        instance: r.instance,
    }
}

fn features(c: Counts) -> Features {
    Features {
        planes: c.planes,
        holes: c.holes,
        shafts: c.shafts,
        cones: c.cones,
        other_faces: c.other_faces,
    }
}

impl Entry {
    /// `listed` as the document writes it, with its history for `get`.
    pub fn of(listed: &Listed, history: Option<&[HistoryStep]>) -> Self {
        Self {
            id: listed.id.clone(),
            revision: listed.revision,
            status: listed.status.map(EntryStatus::name),
            sha256: listed.sha256.as_str().to_owned(),
            bytes: listed.bytes,
            file_name: listed.file_name.clone(),
            product: Product {
                id: listed.product.0.clone(),
                name: listed.product.1.clone(),
            },
            versions: Versions {
                extractor_version: listed.versions.0,
                signature_version: listed.versions.1,
                properties_version: None,
            },
            complete: listed.complete,
            features: listed.counts.map(features),
            refusals: listed.refusals.iter().map(refusal).collect(),
            properties: None,
            history: history.map(|steps| {
                steps
                    .iter()
                    .map(|s| Step {
                        revision: s.revision,
                        sha256: s.sha256.as_str().to_owned(),
                        generation: s.generation,
                        event: s.event,
                        bytes_kept: s.bytes_kept,
                    })
                    .collect()
            }),
        }
    }
}

/// The word an ingest line starts with (section 7.2).
pub fn result(line: &Ingested) -> &'static str {
    let refused = line
        .summary
        .as_ref()
        .is_some_and(|s| s.status == Status::Refused);
    match &line.outcome {
        Outcome::Added if refused => "quarantined",
        Outcome::Added => "added",
        Outcome::Unchanged => "unchanged",
        Outcome::Duplicate { .. } | Outcome::DuplicateUnderId { .. } => "duplicate",
        Outcome::Conflict { .. } => "conflict",
        Outcome::Refused { .. } => "refused",
        _ => "error",
    }
}

/// What an ingest line says after its result, if anything.
pub fn detail(line: &Ingested) -> Option<String> {
    match &line.outcome {
        Outcome::Duplicate { of } => Some(format!("the same bytes are {of}: not added")),
        Outcome::DuplicateUnderId { of } => Some(format!(
            "the same bytes are {of}: not added under another id"
        )),
        Outcome::Conflict { reason } | Outcome::Refused { reason } => Some(reason.clone()),
        Outcome::Error { message } => Some(message.clone()),
        _ => line
            .summary
            .as_ref()
            .and_then(|s| s.first_refusal.as_ref())
            .map(|r| format!("refused: {r}")),
    }
}

impl Change {
    /// A change to one entry: `replaced` or `unchanged` by a replace.
    pub fn entry(
        input: &str,
        result: &'static str,
        id: &str,
        revision: u32,
        sha256: &str,
        detail: Option<String>,
    ) -> Self {
        Self {
            input: input.to_owned(),
            result,
            id: Some(id.to_owned()),
            revision: Some(revision),
            sha256: Some(sha256.to_owned()),
            detail,
        }
    }

    /// An ingest line as a change.
    pub fn of(line: &Ingested) -> Self {
        Self {
            input: line.input.to_string_lossy().into_owned(),
            result: result(line),
            id: line.id.as_ref().map(|i| i.as_str().to_owned()),
            revision: line.revision,
            sha256: line.sha256.as_ref().map(|s| s.as_str().to_owned()),
            detail: detail(line),
        }
    }
}

impl CatalogDocument {
    /// A document at `generation`: `entries` as given, and the counts of
    /// `all` live entries (`list`'s, whatever its filter).
    pub fn new(
        generation: i64,
        all: &[Listed],
        entries: Vec<Entry>,
        changes: Option<Vec<Change>>,
    ) -> Self {
        let mut counts = Tally {
            entries: all.len(),
            ..Tally::default()
        };
        for listed in all {
            match listed.status {
                Some(EntryStatus::Indexed) => counts.indexed += 1,
                Some(EntryStatus::Quarantined) => counts.quarantined += 1,
                _ => counts.stale += 1,
            }
        }
        Self {
            format: CATALOG_FORMAT,
            format_version: CATALOG_FORMAT_VERSION,
            schema_version: crate::schema::SCHEMA_VERSION,
            generation,
            build: Versions {
                extractor_version: Some(EXTRACTOR_VERSION),
                signature_version: Some(SIGNATURE_VERSION),
                properties_version: None,
            },
            counts,
            entries,
            changes,
        }
    }

    /// Pretty JSON with a final newline (CL3).
    pub fn to_json(&self) -> String {
        // Every key is a string and every number finite.
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }
}
