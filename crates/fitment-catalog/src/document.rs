//! The `fitment.catalog` document, version 1 (`docs/research/m5-catalog.md`,
//! section 7.11, CL3, CL9): what `list`, `get` and `ingest --format json`
//! print. It holds no time, host, absolute path or internal key, so the
//! same catalog state gives the same bytes on every OS.
//!
//! CA5 (`docs/research/m6-api.md`) adds `fitment.verify` and `fitment.gc`,
//! frozen from 0.3.0 as `fitment.catalog` is, and each change's
//! `fitment.catalog` is built here ([`CatalogDocument::ingested`] and its
//! siblings), so the CLI and the server serve one document (CU1, DC1).

use fitment_core::{
    EXTRACTOR_VERSION, PROPERTIES_VERSION, PropertyMap, SIGNATURE_VERSION, StatedProperties,
};
use serde::Serialize;

use crate::Error;
use crate::change::{Deletion, Pruned, Replaced};
use crate::gc::Collected;
use crate::ids::{Id, Reference};
use crate::ingest::{Counts, Ingested, Outcome};
use crate::preimage::Status;
use crate::reindex::Reindex;
use crate::snapshot::{EntryStatus, HistoryStep, Listed, Snapshot, StatusCounts, StoredRefusal};
use crate::verify::Verified;

/// The format's name.
pub const CATALOG_FORMAT: &str = "fitment.catalog";

/// Its version.
pub const CATALOG_FORMAT_VERSION: u32 = 1;

/// `fitment.verify`'s name (CA5).
pub const VERIFY_FORMAT: &str = "fitment.verify";

/// Its version.
pub const VERIFY_FORMAT_VERSION: u32 = 1;

/// `fitment.gc`'s name (CA5).
pub const GC_FORMAT: &str = "fitment.gc";

/// Its version.
pub const GC_FORMAT_VERSION: u32 = 1;

/// A `fitment.catalog` document.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize)]
#[allow(clippy::struct_field_names)]
struct Versions {
    extractor_version: Option<u32>,
    signature_version: Option<u32>,
    properties_version: Option<u32>,
}

/// Live entries by status.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, Serialize)]
struct Tally {
    entries: usize,
    indexed: usize,
    quarantined: usize,
    stale: usize,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize)]
struct Product {
    id: Option<String>,
    name: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize)]
struct Refusal {
    code: String,
    message: String,
    instance: Option<u64>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize)]
struct Features {
    planes: u32,
    holes: u32,
    shafts: u32,
    cones: u32,
    other_faces: u32,
}

/// The stated properties `get` shows (PR7), each with its mapped meaning.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize)]
struct Properties {
    truncated: bool,
    stated: Vec<Stated>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize)]
struct Stated {
    instance: u64,
    name: String,
    value: String,
    measure: Option<String>,
    unit: Option<String>,
    mapped: Option<MappedOut>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize)]
struct MappedOut {
    field: &'static str,
    meaning: &'static str,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize)]
struct Step {
    revision: u32,
    sha256: String,
    generation: i64,
    event: &'static str,
    bytes_kept: bool,
}

/// One entry's revision.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    properties: Option<Properties>,
    history: Option<Vec<Step>>,
}

/// One input's or entry's change.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
                properties_version: listed.versions.2,
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

    /// The entry with its stated properties and their meanings under
    /// `map` (PR7), as `get` shows them.
    #[must_use]
    pub fn with_properties(mut self, stated: &StatedProperties, map: &PropertyMap) -> Self {
        self.properties = Some(Properties {
            truncated: stated.truncated,
            stated: stated
                .stated
                .iter()
                .map(|p| Stated {
                    instance: p.instance,
                    name: p.name.clone(),
                    value: p.value.clone(),
                    measure: p.measure.clone(),
                    unit: p.unit.clone(),
                    mapped: map.meaning(p).map(|m| MappedOut {
                        field: m.field.as_str(),
                        meaning: m.meaning.as_str(),
                    }),
                })
                .collect(),
        });
        self
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

/// The document of a change at `snapshot`'s generation: the live entries
/// whose id, as stored, `shown` accepts, as they are now, and `changes`.
fn of_change(
    snapshot: &Snapshot<'_>,
    shown: impl Fn(&str) -> bool,
    changes: Vec<Change>,
) -> Result<CatalogDocument, Error> {
    let all = snapshot.list()?;
    let entries = all
        .iter()
        .filter(|l| shown(&l.id))
        .map(|l| Entry::of(l, None))
        .collect();
    Ok(CatalogDocument::new(
        snapshot.generation(),
        &all,
        entries,
        Some(changes),
    ))
}

impl CatalogDocument {
    /// `ingest --format json`: the entries added or unchanged, as they
    /// are at `snapshot`, and a change for every line, in order.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn ingested(snapshot: &Snapshot<'_>, lines: &[Ingested]) -> Result<Self, Error> {
        let kept: Vec<String> = lines
            .iter()
            .filter(|l| matches!(l.outcome, Outcome::Added | Outcome::Unchanged))
            .filter_map(|l| l.id.as_ref().map(Id::fold))
            .collect();
        let changes = lines.iter().map(Change::of).collect();
        of_change(
            snapshot,
            |id| kept.iter().any(|k| *k == id.to_ascii_lowercase()),
            changes,
        )
    }

    /// `replace --format json`: the entry as it is at `snapshot`, and the
    /// change, `input` the file as given.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn replaced(
        snapshot: &Snapshot<'_>,
        input: &str,
        replaced: &Replaced,
    ) -> Result<Self, Error> {
        let (change, id) = match replaced {
            Replaced::Replaced { id, from, to, .. } => {
                let detail = format!("was r{} {}, bytes kept", from.0, from.1.short(8));
                let change =
                    Change::entry(input, "replaced", id, to.0, to.1.as_str(), Some(detail));
                (change, id)
            }
            Replaced::Unchanged {
                id,
                revision,
                sha256,
            } => {
                let change =
                    Change::entry(input, "unchanged", id, *revision, sha256.as_str(), None);
                (change, id)
            }
        };
        of_change(snapshot, |l| l.eq_ignore_ascii_case(id), vec![change])
    }

    /// `delete --format json` (CU1): each entry deleted as it is now, a
    /// tombstone (CR6), in CL3's order, and a change per entry in the order
    /// named, `input` the first of `inputs` (the references as given) that
    /// named it.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn deleted(
        snapshot: &Snapshot<'_>,
        inputs: &[&str],
        deletion: &Deletion,
    ) -> Result<Self, Error> {
        let all = snapshot.list()?;
        let mut tombstones = Vec::new();
        let mut changes = Vec::new();
        for d in &deletion.deleted {
            let id = Id::parse(&d.id).map_err(|_| Error::Corrupt("an id"))?;
            if let Ok(named) = snapshot.resolve(&Reference::Revision(id, d.revision))? {
                tombstones.push(snapshot.entry(&named)?);
            }
            let input = inputs.get(d.named).copied().unwrap_or(d.id.as_str());
            let sha256 = d.sha256.as_str();
            changes.push(Change::entry(
                input, "deleted", &d.id, d.revision, sha256, None,
            ));
        }
        // CL3's order, as every document's entries.
        tombstones.sort_by(|a, b| {
            let fold = |l: &Listed| l.id.to_ascii_lowercase();
            fold(a).cmp(&fold(b)).then_with(|| a.id.cmp(&b.id))
        });
        let entries = tombstones.iter().map(|l| Entry::of(l, None)).collect();
        Ok(Self::new(
            snapshot.generation(),
            &all,
            entries,
            Some(changes),
        ))
    }

    /// `prune --revisions --format json` (CU1): the entries whose kept
    /// revisions were pruned, as they are now, and a change per revision,
    /// `input` its `ID@rN`, `detail` the bytes it released.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn pruned(snapshot: &Snapshot<'_>, pruned: &[Pruned]) -> Result<Self, Error> {
        let ids: Vec<&str> = pruned.iter().map(|p| p.id.as_str()).collect();
        let changes = pruned
            .iter()
            .map(|p| {
                let input = format!("{}@r{}", p.id, p.revision);
                let sha256 = p.sha256.as_str();
                let detail = Some(released(p.released));
                Change::entry(&input, "pruned", &p.id, p.revision, sha256, detail)
            })
            .collect();
        of_change(snapshot, |id| ids.contains(&id), changes)
    }

    /// `reindex --format json`: every entry processed as it is now, and a
    /// change per line, `input` its id.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn reindexed(snapshot: &Snapshot<'_>, run: &Reindex) -> Result<Self, Error> {
        let ids: Vec<&str> = run.lines.iter().map(|l| l.id.as_str()).collect();
        let changes = run
            .lines
            .iter()
            .map(|line| {
                Change::entry(
                    &line.id,
                    line.result(),
                    &line.id,
                    line.revision,
                    line.sha256.as_str(),
                    Some(line.detail()),
                )
            })
            .collect();
        of_change(snapshot, |id| ids.contains(&id), changes)
    }

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
                properties_version: Some(PROPERTIES_VERSION),
            },
            counts,
            entries,
            changes,
        }
    }

    /// A page of a list (`docs/research/m6-api.md`, VN3): `entries` as
    /// given, a slice, and `counts` the whole catalog's, from
    /// [`Snapshot::status_counts`], so a page states what
    /// [`CatalogDocument::new`] over every entry would.
    pub fn page(generation: i64, counts: &StatusCounts, entries: Vec<Entry>) -> Self {
        let n = |v: u64| usize::try_from(v).unwrap_or(usize::MAX);
        let mut document = Self::new(generation, &[], entries, None);
        document.counts = Tally {
            entries: n(counts.entries),
            indexed: n(counts.indexed),
            quarantined: n(counts.quarantined),
            stale: n(counts.stale),
        };
        document
    }

    /// Pretty JSON with a final newline (CL3).
    pub fn to_json(&self) -> String {
        // Every key is a string and every number finite.
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }
}

/// What a pruned revision released, as its line and change say it.
pub fn released(bytes: Option<u64>) -> String {
    bytes.map_or_else(
        || "bytes released with another revision, or still held".to_owned(),
        |b| format!("{b} bytes"),
    )
}

/// A `fitment.verify` document (CA5): what `verify` found, in its order,
/// at the generation it read. It holds no path but the catalog root's own
/// (CA4) and no time.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct VerifyDocument {
    format: &'static str,
    format_version: u32,
    generation: i64,
    deep: bool,
    full: bool,
    consistent: bool,
    problems: Vec<ProblemOut>,
}

/// One problem, as `fitment.verify` writes it (VF3).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize)]
struct ProblemOut {
    kind: &'static str,
    id: Option<String>,
    what: String,
    detail: String,
}

impl VerifyDocument {
    /// `verified`, as a run with `deep` and `full` found it.
    pub fn new(verified: &Verified, deep: bool, full: bool) -> Self {
        Self {
            format: VERIFY_FORMAT,
            format_version: VERIFY_FORMAT_VERSION,
            generation: verified.generation,
            deep,
            full,
            consistent: verified.problems.is_empty(),
            problems: verified
                .problems
                .iter()
                .map(|p| ProblemOut {
                    kind: p.kind.name(),
                    id: p.id.clone(),
                    what: p.what.clone(),
                    detail: p.detail.clone(),
                })
                .collect(),
        }
    }

    /// Pretty JSON with a final newline (CL3).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }
}

/// A `fitment.gc` document (CA5): what `gc` removed, the SHA-256s and the
/// temporary files' names in order, and what it could not, each with its
/// error's kind (CA4).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct GcDocument {
    format: &'static str,
    format_version: u32,
    generation: i64,
    versions: bool,
    vacuum: bool,
    unlinked: Vec<String>,
    temporaries: Vec<String>,
    old_rows: usize,
    failed: Vec<String>,
}

impl GcDocument {
    /// `collected`, by a run with `versions` and `vacuum`.
    pub fn new(collected: &Collected, versions: bool, vacuum: bool) -> Self {
        let mut unlinked: Vec<String> = collected
            .unlinked
            .iter()
            .map(|s| s.as_str().to_owned())
            .collect();
        unlinked.sort();
        Self {
            format: GC_FORMAT,
            format_version: GC_FORMAT_VERSION,
            generation: collected.generation,
            versions,
            vacuum,
            unlinked,
            temporaries: collected.temporaries.clone(),
            old_rows: collected.old_rows,
            failed: collected.failed.clone(),
        }
    }

    /// Pretty JSON with a final newline (CL3).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }
}
