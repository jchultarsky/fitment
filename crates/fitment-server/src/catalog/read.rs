//! The catalog's synchronous routes (docs/research/m6-api.md, section 7.4,
//! RA2, VN3, VN4, CX2, M6-50): `list` a page at a time or whole, `get` in
//! its three forms, and `delete`, each one snapshot or one transaction, and
//! each document the CLI's `--format json` bytes (DC1): `list --status S`,
//! `get REF` and `delete ID…`. A reference is ID7's with ID9's strictness
//! (RA2); what it names nothing usable answers with the code its CLI
//! refusal maps to (CX7, section 7.13).

use super::CatalogDir;
use crate::http::error::{ApiError, ErrorCode};
use fitment_catalog::document::{CatalogDocument, Entry};
use fitment_catalog::{
    BadBlob, BadName, BadRow, Catalog, EntryStatus, FsRead, Id, Listed, Reference, Sha256Hex,
    Unnamed,
};

/// `list --status S` (CL3's statuses): which live entries a list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFilter {
    /// Every live entry.
    All,
    /// Searched by a match.
    Indexed,
    /// Refused by the gate.
    Quarantined,
    /// Read by another build.
    Stale,
}

impl StatusFilter {
    /// The filter `name` names, as `--status` takes it.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "all" => Some(Self::All),
            "indexed" => Some(Self::Indexed),
            "quarantined" => Some(Self::Quarantined),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }

    /// Its name, for a next page's query.
    pub fn name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Indexed => "indexed",
            Self::Quarantined => "quarantined",
            Self::Stale => "stale",
        }
    }

    fn wanted(self, status: Option<EntryStatus>) -> bool {
        match self {
            Self::All => true,
            Self::Indexed => status == Some(EntryStatus::Indexed),
            Self::Quarantined => status == Some(EntryStatus::Quarantined),
            Self::Stale => status.is_some_and(|s| s.name() == "stale"),
        }
    }
}

/// One page of a list (VN3): its generation (VN4), its `fitment.catalog`
/// bytes, and the cursor of the next page, the last entry's folded id.
#[derive(Debug, Clone)]
pub struct ListPage {
    /// The snapshot's generation.
    pub generation: i64,
    /// `fitment.catalog` v1: the page's entries, the catalog's counts.
    pub json: String,
    /// The next page's cursor, if there is one.
    pub next: Option<String>,
}

/// What `get` answers: the document, the generation it was read at, the
/// id as stored, and the CLI's exit code for it.
#[derive(Debug, Clone)]
pub struct Got {
    /// The snapshot's generation.
    pub generation: i64,
    /// The entry's id as stored (RA2's `Content-Location`).
    pub id: String,
    /// The revision named.
    pub revision: u32,
    /// The document's bytes.
    pub json: String,
    /// The CLI's exit code: 0, 1 for damaged stated properties (shown
    /// anyway), 3 for a quarantined entry's features.
    pub exit: u8,
}

/// `get --format step`'s bytes, re-hashed (ST11).
pub struct Step {
    /// The id as stored.
    pub id: String,
    /// The revision.
    pub revision: u32,
    /// The content's SHA-256.
    pub sha256: String,
    /// Their length.
    pub length: u64,
    /// The bytes, or a reader of them.
    pub bytes: StepBytes,
}

/// A step's bytes (OB3): read whole and checked within the one-buffer
/// size, else hashed in one pass and read in a second that fails if they
/// changed (the review of PR 7b).
pub enum StepBytes {
    /// Read whole, checked.
    Whole(Vec<u8>),
    /// The second pass's reader, its first pass done.
    Stream(Box<dyn FsRead>),
}

impl std::fmt::Debug for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Step")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("length", &self.length)
            .finish_non_exhaustive()
    }
}

/// What a delete answers (M6-50, CU1): `fitment.catalog` with `changes`.
#[derive(Debug, Clone)]
pub struct Deleted {
    /// The generation after it.
    pub generation: i64,
    /// The document's bytes.
    pub json: String,
}

/// RA2: a reference in ID7's forms with ID9's strictness; a malformed one
/// is `400 invalid_reference`, its value never echoed (HX5).
///
/// # Errors
///
/// `invalid_reference`.
pub(crate) fn reference(text: &str) -> Result<Reference, ApiError> {
    Reference::parse(text).map_err(|_| {
        ApiError::new(
            ErrorCode::InvalidReference,
            "a catalog entry is named ID, ID@rN or sha256: and 12 to 64 hex digits",
        )
    })
}

/// RA2, CX7: a reference that names nothing (`404 entry_not_found`, with
/// up to five suggestions) or several (`409 reference_ambiguous`).
pub(crate) fn bad_name(bad: &BadName) -> ApiError {
    match bad {
        BadName::Unknown { suggestions, .. } => ApiError::new(
            ErrorCode::EntryNotFound,
            "no catalog entry has that id or content",
        )
        .with("suggestions", suggestions.clone()),
        BadName::Ambiguous { ids, .. } => ApiError::new(
            ErrorCode::ReferenceAmbiguous,
            "the content reference names several entries: give more hex digits, or an id",
        )
        .with("matches", ids.clone()),
        BadName::NoRevision { id, asked } => {
            ApiError::new(ErrorCode::EntryNotFound, format!("{id} never had r{asked}"))
                .with("id", id.clone())
                .with("revision", *asked)
                .with("suggestions", Vec::<String>::new())
        }
        _ => ApiError::new(ErrorCode::EntryNotFound, "no catalog entry of that name"),
    }
}

/// RA2, CX7: a change's reference that names no entry it may change.
pub(crate) fn unnamed(unnamed: &Unnamed) -> ApiError {
    match unnamed {
        Unnamed::Name(bad) => bad_name(bad),
        Unnamed::Deleted(id) => ApiError::new(ErrorCode::EntryDeleted, format!("{id} is deleted"))
            .with("id", id.clone()),
        Unnamed::NotCurrent { id, current, asked } => ApiError::new(
            ErrorCode::RevisionNotCurrent,
            format!("{id} is at r{current}, not r{asked}: nothing was changed"),
        )
        .with("id", id.clone())
        .with("current", *current)
        .with("asked", *asked),
        Unnamed::NotAnId(_) => ApiError::new(
            ErrorCode::NotAnId,
            "a delete names ids, or their current ID@rN, never a content reference",
        ),
        // RA7: an `If-Match` of another generation; nothing changed.
        Unnamed::Changed { generation, .. } => ApiError::new(
            ErrorCode::PreconditionFailed,
            "the catalog changed since the entity tag If-Match names: nothing was changed",
        )
        .with("generation", *generation),
        _ => ApiError::new(ErrorCode::EntryNotFound, "no catalog entry of that name"),
    }
}

/// M6-50, CR2: why a delete of several deleted nothing: the first of its
/// references' refusals by status, a content reference (`400`) before an
/// unknown id (`404`, every unknown one listed) before a conflict (`409`).
fn unnamed_all(list: &[Unnamed]) -> ApiError {
    let rank = |u: &Unnamed| match u {
        Unnamed::NotAnId(_) => 0,
        Unnamed::Name(BadName::Unknown { .. } | BadName::NoRevision { .. }) => 1,
        _ => 2,
    };
    let Some(first) = list.iter().min_by_key(|u| rank(u)) else {
        return ApiError::internal();
    };
    if rank(first) != 1 {
        return unnamed(first);
    }
    let unknown: Vec<String> = list
        .iter()
        .filter_map(|u| match u {
            Unnamed::Name(BadName::Unknown { asked, .. }) => Some(asked.clone()),
            Unnamed::Name(BadName::NoRevision { id, asked }) => Some(format!("{id}@r{asked}")),
            _ => None,
        })
        .collect();
    unnamed(first).with("unknown", unknown)
}

/// An entry as `list` writes it.
fn entry(listed: &Listed) -> Entry {
    Entry::of(listed, None)
}

impl CatalogDir {
    /// VN3, VN4, CX2: a page of `list --status S --format json`: at most
    /// `limit` live entries after the one whose id is `after`, in `list`'s
    /// order, in one snapshot whose generation the page states; its
    /// `counts` the whole catalog's. A filtered page reads on until it
    /// holds `limit` entries or the list ends.
    ///
    /// # Errors
    ///
    /// The catalog's, mapped (CX7).
    pub fn list_page(
        &self,
        filter: StatusFilter,
        after: Option<&str>,
        limit: usize,
    ) -> Result<ListPage, ApiError> {
        self.read(|catalog| {
            let snapshot = catalog.snapshot()?;
            let counts = snapshot.status_counts()?;
            let chunk = limit.saturating_add(1).max(128);
            let mut cursor = after.map(str::to_owned);
            let mut kept: Vec<Listed> = Vec::new();
            loop {
                let read = snapshot.list_page(cursor.as_deref(), chunk)?;
                let ended = read.len() < chunk;
                cursor = read.last().map(|l| l.id.clone());
                kept.extend(read.into_iter().filter(|l| filter.wanted(l.status)));
                if ended || kept.len() > limit {
                    break;
                }
            }
            let next = (kept.len() > limit).then(|| {
                kept.truncate(limit);
                kept.last().map(|l| l.id.to_ascii_lowercase())
            });
            let entries = kept.iter().map(entry).collect();
            let generation = snapshot.generation();
            Ok(ListPage {
                generation,
                json: CatalogDocument::page(generation, &counts, entries).to_json(),
                next: next.flatten(),
            })
        })
    }

    /// VN3, HX8: `list --status S --format json` whole, the CLI's bytes,
    /// built in one snapshot that then closes, and its generation.
    ///
    /// # Errors
    ///
    /// The catalog's, mapped (CX7).
    pub fn list_all(&self, filter: StatusFilter) -> Result<(i64, String), ApiError> {
        self.read(|catalog| {
            let snapshot = catalog.snapshot()?;
            let all = snapshot.list()?;
            let generation = snapshot.generation();
            let entries = all
                .iter()
                .filter(|l| filter.wanted(l.status))
                .map(entry)
                .collect();
            Ok((
                generation,
                CatalogDocument::new(generation, &all, entries, None).to_json(),
            ))
        })
    }

    /// `get REF --format json` (ID7, RA2): the entry, its properties and
    /// history, a tombstone too, as the CLI reads it, in one snapshot.
    ///
    /// # Errors
    ///
    /// `entry_not_found`, `reference_ambiguous`; the catalog's, mapped.
    pub fn get(&self, named: &str) -> Result<Got, ApiError> {
        let reference = reference(named)?;
        let read = self.read(|catalog| get_json(catalog, &reference))?;
        read.map_err(|bad| bad_name(&bad))
    }

    /// `get REF --format features` (GT2, QR4): the current revision's
    /// stored document, checked; a quarantined entry's refused document
    /// with exit 3. A superseded revision is `410 derived_rows_gone`; a
    /// stale entry `409 entry_stale`; a damaged row `500 catalog_damaged`.
    ///
    /// # Errors
    ///
    /// As the paragraph says; the catalog's, mapped.
    pub fn features(&self, named: &str) -> Result<Got, ApiError> {
        let reference = reference(named)?;
        self.read(|catalog| {
            let snapshot = catalog.snapshot()?;
            let named = match snapshot.resolve(&reference)? {
                Ok(named) => named,
                Err(bad) => return Ok(Err(bad_name(&bad))),
            };
            if named.current != Some(named.revision) {
                return Ok(Err(ApiError::new(
                    ErrorCode::DerivedRowsGone,
                    format!(
                        "{} r{} is not current: its stored document is gone (its bytes: …/step)",
                        named.id, named.revision
                    ),
                )));
            }
            let document = match snapshot.features(&named.sha256)? {
                Ok(document) => document,
                Err(BadRow::Stale { found }) => {
                    return Ok(Err(ApiError::new(
                        ErrorCode::EntryStale,
                        format!(
                            "{} was read by another fitment (extractor {found}): reindex it (POST /v1/catalog/reindex)",
                            named.id
                        ),
                    )
                    .with("extractor_version", found)));
                }
                Err(_) => return Err(fitment_catalog::Error::Corrupt("a features row")),
            };
            let exit = if document.part().is_some() { 0 } else { 3 };
            Ok(Ok(Got {
                generation: snapshot.generation(),
                id: named.id,
                revision: named.revision,
                json: document.to_json(),
                exit,
            }))
        })?
    }

    /// `get REF --format step` (ST11, OB3): the revision's kept bytes,
    /// read after the snapshot closes and checked: within `whole_max`
    /// bytes read whole, above it hashed in one pass and handed back as a
    /// reader for the second; `between` runs between the snapshot and the
    /// read (OP11's `catalog.step`). Pruned or deleted bytes are `410
    /// bytes_not_kept`. A file missing or not its name's is looked for
    /// again in a fresh snapshot, as CC4 does (the review of PR 7b): a
    /// delete or a prune that released it since is `410` too; `500
    /// blob_damaged`, nothing sent, only when the fresh snapshot still
    /// keeps the revision's bytes.
    ///
    /// # Errors
    ///
    /// As the paragraph says; the catalog's, mapped.
    pub fn step(&self, named: &str, whole_max: u64, between: &dyn Fn()) -> Result<Step, ApiError> {
        let reference = reference(named)?;
        self.read(|catalog| {
            let (id, revision, sha256, length) = match kept(catalog, &reference)? {
                Ok(found) => found,
                Err(refused) => return Ok(Err(refused)),
            };
            between();
            let read = if length <= whole_max {
                catalog.read_blob(&sha256).map(StepBytes::Whole)
            } else {
                catalog
                    .open_blob(&sha256)
                    .map(|(_, reader)| StepBytes::Stream(reader))
            };
            let bad = match read {
                Ok(bytes) => {
                    return Ok(Ok(Step {
                        id,
                        revision,
                        sha256: sha256.as_str().to_owned(),
                        length,
                        bytes,
                    }));
                }
                Err(bad) => bad,
            };
            if matches!(bad, BadBlob::Missing | BadBlob::Damaged) {
                let again = Id::parse(&id).map(|i| Reference::Revision(i, revision));
                let still = match again {
                    Ok(again) => kept(catalog, &again)?.is_ok_and(|k| k.2 == sha256),
                    Err(_) => false,
                };
                if !still {
                    return Ok(Err(not_kept(&id, revision)));
                }
            }
            Ok(Err(blob_error(sha256.as_str(), &bad)))
        })?
    }

    /// M6-50, CX3, CU1: `delete ID… --format json`, all or none (CR2), on a
    /// short-lived writer that waits at most `catalog.delete_busy_ms`, then
    /// `503 catalog_busy`; `inputs` are the references as given; `at`,
    /// `If-Match`'s generation (RA7), checked in its transaction.
    ///
    /// # Errors
    ///
    /// The references' refusals (RA2), nothing deleted; the catalog's,
    /// mapped.
    pub fn delete(&self, inputs: &[String], at: Option<i64>) -> Result<Deleted, ApiError> {
        let mut references = Vec::with_capacity(inputs.len());
        for text in inputs {
            references.push(reference(text)?);
        }
        self.inspect().map_err(ApiError::from)?;
        let mut catalog = Catalog::open(self.root(), self.options_with(self.delete_busy_ms))
            .map_err(|e| self.mapped(&e))?;
        let deletion = match catalog.delete_at(&references, at) {
            Ok(Ok(deletion)) => deletion,
            Ok(Err(list)) => return Err(unnamed_all(&list)),
            Err(error) => return Err(self.mapped(&error)),
        };
        for orphan in &deletion.released.orphans {
            tracing::warn!(
                orphan = orphan.as_str(),
                "a deleted entry's file could not be removed: a gc job removes it"
            );
        }
        let named: Vec<&str> = inputs.iter().map(String::as_str).collect();
        let document = (|| {
            let snapshot = catalog.snapshot()?;
            let document = CatalogDocument::deleted(&snapshot, &named, &deletion)?;
            Ok((snapshot.generation(), document.to_json()))
        })()
        .map_err(|e: fitment_catalog::Error| self.mapped(&e))?;
        Ok(Deleted {
            generation: document.0,
            json: document.1,
        })
    }
}

/// `get REF --format json`'s read, in one snapshot.
fn get_json(
    catalog: &mut Catalog,
    reference: &Reference,
) -> Result<Result<Got, BadName>, fitment_catalog::Error> {
    let snapshot = catalog.snapshot()?;
    let named = match snapshot.resolve(reference)? {
        Ok(named) => named,
        Err(bad) => return Ok(Err(bad)),
    };
    let listed = snapshot.entry(&named)?;
    let history = snapshot.history(&named)?;
    // Only an accepted document has properties (PR3); a stale extractor's
    // are not this build's to show.
    let accepted = matches!(
        listed.status,
        Some(EntryStatus::Indexed | EntryStatus::StaleProperties(_))
    );
    let mut entry = Entry::of(&listed, Some(&history));
    let mut exit = 0;
    if accepted {
        match snapshot.properties(&named.sha256) {
            Ok(Some(stated)) => {
                entry = entry.with_properties(&stated, &fitment_core::PropertyMap::shipped());
            }
            Ok(None) => {}
            // QR4: a damaged row is shown as absent, and the run exits 1.
            Err(fitment_catalog::Error::Corrupt(_)) => exit = 1,
            Err(other) => return Err(other),
        }
    }
    // The counts in one query (the review of PR 7b): `page` states what
    // `new` over the whole listing would, byte for byte.
    let counts = snapshot.status_counts()?;
    let generation = snapshot.generation();
    let json = CatalogDocument::page(generation, &counts, vec![entry]).to_json();
    Ok(Ok(Got {
        generation,
        id: named.id,
        revision: named.revision,
        json,
        exit,
    }))
}

/// What `step` reads (ST11): the revision `reference` names, its bytes
/// kept, as id, revision, content and length; else its refusal (`404`,
/// `409`, `410 bytes_not_kept`). One snapshot, closed on return.
type Kept = Result<(String, u32, Sha256Hex, u64), ApiError>;

fn kept(catalog: &mut Catalog, reference: &Reference) -> Result<Kept, fitment_catalog::Error> {
    let snapshot = catalog.snapshot()?;
    let named = match snapshot.resolve(reference)? {
        Ok(named) => named,
        Err(bad) => return Ok(Err(bad_name(&bad))),
    };
    let listed = snapshot.entry(&named)?;
    let kept = snapshot
        .history(&named)?
        .iter()
        .any(|s| s.revision == named.revision && s.bytes_kept);
    match listed.bytes {
        Some(length) if kept => Ok(Ok((named.id, named.revision, named.sha256, length))),
        _ => Ok(Err(not_kept(&named.id, named.revision))),
    }
}

/// `410 bytes_not_kept` (ST11): a pruned or deleted revision's bytes.
fn not_kept(id: &str, revision: u32) -> ApiError {
    ApiError::new(
        ErrorCode::BytesNotKept,
        format!("{id} r{revision}'s bytes are no longer kept"),
    )
}

/// ST11, OB3: a stored file that is missing or not its name's is `500
/// blob_damaged`, nothing sent; one that cannot be read `io_error`. Logged
/// by its content's name, never its path.
fn blob_error(sha256: &str, bad: &BadBlob) -> ApiError {
    match bad {
        BadBlob::Missing | BadBlob::Damaged => {
            tracing::error!(content = sha256, error = %bad, "a catalog file is damaged");
            ApiError::new(
                ErrorCode::BlobDamaged,
                "the stored bytes are missing or not their name's: nothing was sent; run a verify job",
            )
        }
        _ => {
            tracing::error!(content = sha256, "a catalog file could not be read");
            ApiError::new(ErrorCode::IoError, "the stored bytes could not be read")
        }
    }
}
