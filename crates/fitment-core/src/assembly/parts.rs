//! The `fitment.parts` document: what `fitment parts` prints
//! (`docs/research/m3-socket.md`, section 10).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{AssemblyNote, ComponentRead, DefinitionKind, PathForm, read_assembly};
use crate::EXTRACTOR_VERSION;
use crate::features::{DefinitionRef, FileHeader, Status};
use crate::gate::Refusal;
use crate::geom::Rigid;
use crate::sha256::sha256_hex;

/// The `format` of every parts document.
pub const PARTS_FORMAT: &str = "fitment.parts";

/// The document layout this crate writes and reads; frozen at the first
/// publish.
pub const PARTS_FORMAT_VERSION: u32 = 1;

/// The file a document was read from, named by its content, never by its
/// path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Source {
    /// The SHA-256 of the file's bytes, as 64 lower-case hex digits.
    pub sha256: String,
    /// Its length in bytes.
    pub bytes: u64,
    /// Its header facts; `None` if it could not be parsed.
    #[serde(deserialize_with = "Option::deserialize")]
    pub header: Option<FileHeader>,
}

impl Source {
    /// The source of `step`, with `header` as read from it.
    pub(crate) fn of(step: &[u8], header: Option<FileHeader>) -> Self {
        Self {
            sha256: sha256_hex(step),
            bytes: u64::try_from(step.len()).unwrap_or(u64::MAX),
            header,
        }
    }
}

/// Whether the assembly gate passes, and why not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GateResult {
    /// Accepted, or refused with reasons.
    pub status: Status,
    /// Sorted; empty when accepted.
    pub refusals: Vec<Refusal>,
}

/// One occurrence of a listed definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PartOccurrence {
    /// The usages' `#id`s from the root down (O5).
    pub path: Vec<u64>,
    /// The usages' `id`s, for people; `None` where the file leaves one
    /// blank.
    pub label: Vec<Option<String>>,
    /// The occurrence's coordinates (mm) to the root's (mm).
    pub transform: Rigid,
    /// Its part's largest stated uncertainty, in mm (Z1).
    pub part_uncertainty_mm: f64,
    /// Its part's solid's largest vertex offset, in mm (Z1).
    pub vertex_offset_mm: f64,
    /// The largest uncertainty the contexts it is placed through state,
    /// in mm (Z1).
    pub placement_uncertainty_mm: f64,
}

/// What reading a part gave, in brief (check 36).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ComponentSummary {
    /// Accepted, or refused with reasons.
    pub gate: GateResult,
    /// Its holes, if accepted.
    #[serde(deserialize_with = "Option::deserialize")]
    pub holes: Option<u64>,
    /// Its shafts, if accepted.
    #[serde(deserialize_with = "Option::deserialize")]
    pub shafts: Option<u64>,
    /// Its `complete`, if accepted.
    #[serde(deserialize_with = "Option::deserialize")]
    pub complete: Option<bool>,
}

/// A definition reached from the root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PartEntry {
    /// The definition and its product.
    pub definition: DefinitionRef,
    /// Part or assembly.
    pub kind: DefinitionKind,
    /// How many times it occurs under the root.
    pub quantity: u64,
    /// Every occurrence, by path.
    pub occurrences: Vec<PartOccurrence>,
    /// For a part, what reading it gave; `None` for an assembly.
    #[serde(deserialize_with = "Option::deserialize")]
    pub component: Option<ComponentSummary>,
}

/// A parts document: every definition of an assembly with its
/// occurrences, or the reasons the file gives no parts list.
///
/// A document comes only from [`read`](Self::read) or
/// [`from_json`](Self::from_json).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
// The fields are private; the methods below read them.
#[allow(clippy::exhaustive_structs)]
pub struct PartsDocument {
    format: String,
    format_version: u32,
    extractor_version: u32,
    source: Source,
    status: Status,
    refusals: Vec<Refusal>,
    #[serde(deserialize_with = "Option::deserialize")]
    assembly_gate: Option<GateResult>,
    #[serde(deserialize_with = "Option::deserialize")]
    root: Option<DefinitionRef>,
    #[serde(deserialize_with = "Option::deserialize")]
    path_form: Option<PathForm>,
    parts: Vec<PartEntry>,
    notes: Vec<AssemblyNote>,
}

/// A part carries its read and an assembly none; an accepted read its
/// counts, a refused one its reasons.
fn check_component(part: &PartEntry) -> Result<(), String> {
    let instance = part.definition.instance;
    match (part.kind, &part.component) {
        (DefinitionKind::Assembly, None) => Ok(()),
        (DefinitionKind::Part, Some(summary)) => {
            let refusals = &summary.gate.refusals;
            let sorted = refusals.windows(2).all(|pair| pair[0] < pair[1]);
            let counts = [
                summary.holes.is_some(),
                summary.shafts.is_some(),
                summary.complete.is_some(),
            ];
            let consistent = match summary.gate.status {
                Status::Accepted => refusals.is_empty() && counts.iter().all(|c| *c),
                Status::Refused => !refusals.is_empty() && counts.iter().all(|c| !*c),
            };
            if sorted && consistent {
                Ok(())
            } else {
                Err(format!(
                    "#{instance}: its read's status, counts and reasons disagree"
                ))
            }
        }
        _ => Err(format!(
            "#{instance}: a part carries its read, an assembly none"
        )),
    }
}

/// A component's read, in brief.
fn summary(read: &ComponentRead) -> ComponentSummary {
    let count = |n: usize| Some(u64::try_from(n).unwrap_or(u64::MAX));
    match read {
        ComponentRead::Accepted(part) => ComponentSummary {
            gate: GateResult {
                status: Status::Accepted,
                refusals: Vec::new(),
            },
            holes: count(part.holes.len()),
            shafts: count(part.shafts.len()),
            complete: Some(part.complete),
        },
        ComponentRead::Refused(refusals) => ComponentSummary {
            gate: GateResult {
                status: Status::Refused,
                refusals: refusals.clone(),
            },
            holes: None,
            shafts: None,
            complete: None,
        },
    }
}

/// Why a parts document could not be read back.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PartsFormatError {
    /// The text is not JSON, or not this document's shape.
    #[error("not a parts document: {message} (line {line}, column {column})")]
    Json {
        /// What `serde_json` reported.
        message: String,
        /// 1-based line.
        line: usize,
        /// 1-based column.
        column: usize,
    },
    /// The document has another `format`.
    #[error("format is '{found}', not '{PARTS_FORMAT}'")]
    Format {
        /// The format found.
        found: String,
    },
    /// The document has a layout version this crate does not read.
    #[error("format version {found} is not {PARTS_FORMAT_VERSION}")]
    Version {
        /// The version found.
        found: u32,
    },
    /// The document breaks one of its own rules.
    #[error("invalid parts document: {0}")]
    Invalid(String),
}

impl PartsDocument {
    /// Reads the parts of an assembly from a STEP file. Bytes in, values
    /// out: no I/O, and nothing panics.
    pub fn read(step: &[u8]) -> Self {
        let read = read_assembly(step);
        let source = Source::of(step, read.header().cloned());
        let Some(listing) = read.listing() else {
            return Self {
                format: PARTS_FORMAT.to_owned(),
                format_version: PARTS_FORMAT_VERSION,
                extractor_version: EXTRACTOR_VERSION,
                source,
                status: Status::Refused,
                refusals: read.refusals().to_vec(),
                assembly_gate: None,
                root: None,
                path_form: None,
                parts: Vec::new(),
                notes: Vec::new(),
            };
        };
        let gate = read.gate_refusals().to_vec();
        let parts = listing
            .definitions()
            .iter()
            .map(|listed| PartEntry {
                definition: listed.definition.clone(),
                kind: listed.kind,
                quantity: u64::try_from(listed.occurrences.len()).unwrap_or(u64::MAX),
                occurrences: listed
                    .occurrences
                    .iter()
                    .filter_map(|&index| listing.occurrences().get(index))
                    .map(|occurrence| PartOccurrence {
                        path: occurrence.path.instances(),
                        label: occurrence
                            .path
                            .usages()
                            .iter()
                            .map(|usage| usage.id.clone())
                            .collect(),
                        transform: occurrence.transform,
                        part_uncertainty_mm: occurrence.part_uncertainty_mm,
                        vertex_offset_mm: occurrence.vertex_offset_mm,
                        placement_uncertainty_mm: occurrence.placement_uncertainty_mm,
                    })
                    .collect(),
                component: listed.read.as_ref().map(summary),
            })
            .collect();
        Self {
            format: PARTS_FORMAT.to_owned(),
            format_version: PARTS_FORMAT_VERSION,
            extractor_version: EXTRACTOR_VERSION,
            source,
            status: Status::Accepted,
            refusals: Vec::new(),
            assembly_gate: Some(GateResult {
                status: if gate.is_empty() {
                    Status::Accepted
                } else {
                    Status::Refused
                },
                refusals: gate,
            }),
            root: listing
                .definitions()
                .get(listing.root())
                .map(|listed| listed.definition.clone()),
            path_form: Some(listing.path_form()),
            parts,
            notes: read.notes().to_vec(),
        }
    }

    /// What the reading noticed that refuses nothing, sorted.
    pub fn notes(&self) -> &[AssemblyNote] {
        &self.notes
    }

    /// The file it was read from.
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// The [`EXTRACTOR_VERSION`] that wrote the document.
    pub fn extractor_version(&self) -> u32 {
        self.extractor_version
    }

    /// Accepted when the file gives a parts list.
    pub fn status(&self) -> Status {
        self.status
    }

    /// Why the file gives no parts list; empty when accepted.
    pub fn refusals(&self) -> &[Refusal] {
        &self.refusals
    }

    /// Whether `socket` would pass the assembly gate; `None` when refused.
    pub fn assembly_gate(&self) -> Option<&GateResult> {
        self.assembly_gate.as_ref()
    }

    /// The root definition; `None` when refused.
    pub fn root(&self) -> Option<&DefinitionRef> {
        self.root.as_ref()
    }

    /// How the file's paths are written; `None` when refused.
    pub fn path_form(&self) -> Option<PathForm> {
        self.path_form
    }

    /// Every definition reached from the root, the root included, by
    /// `#id`.
    pub fn parts(&self) -> &[PartEntry] {
        &self.parts
    }

    /// The document as pretty JSON, ending with a newline.
    pub fn to_json(&self) -> String {
        // Serializing these types cannot fail: every map key is a string
        // and every float is finite by construction.
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Reads a document back, checking its format, version and rules.
    ///
    /// # Errors
    ///
    /// A [`PartsFormatError`] if the text is not a valid parts document.
    pub fn from_json(text: &str) -> Result<Self, PartsFormatError> {
        let document: Self =
            serde_json::from_str(text).map_err(|error| PartsFormatError::Json {
                message: error.to_string(),
                line: error.line(),
                column: error.column(),
            })?;
        if document.format != PARTS_FORMAT {
            return Err(PartsFormatError::Format {
                found: document.format,
            });
        }
        if document.format_version != PARTS_FORMAT_VERSION {
            return Err(PartsFormatError::Version {
                found: document.format_version,
            });
        }
        document.check().map_err(PartsFormatError::Invalid)?;
        Ok(document)
    }

    /// The rules a document read back must keep.
    fn check(&self) -> Result<(), String> {
        let digest = &self.source.sha256;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("source.sha256 is not 64 lower-case hex digits".to_owned());
        }
        let sorted = |refusals: &[Refusal]| refusals.windows(2).all(|pair| pair[0] < pair[1]);
        if !sorted(&self.refusals) {
            return Err("refusals are not sorted, or repeat".to_owned());
        }
        match self.status {
            Status::Refused => {
                if self.refusals.is_empty()
                    || self.assembly_gate.is_some()
                    || self.root.is_some()
                    || self.path_form.is_some()
                    || !self.parts.is_empty()
                {
                    return Err(
                        "a refused document holds refusals and no gate result, root or parts"
                            .to_owned(),
                    );
                }
                return Ok(());
            }
            Status::Accepted => {}
        }
        let (Some(gate), Some(root), Some(_)) = (&self.assembly_gate, &self.root, self.path_form)
        else {
            return Err(
                "an accepted document holds a gate result, a root and a path form".to_owned(),
            );
        };
        if !self.refusals.is_empty() {
            return Err("an accepted document holds no refusals".to_owned());
        }
        if !sorted(&gate.refusals) || gate.refusals.is_empty() != (gate.status == Status::Accepted)
        {
            return Err(
                "the gate result's status and refusals disagree, or are not sorted".to_owned(),
            );
        }
        if !self
            .parts
            .windows(2)
            .all(|pair| pair[0].definition.instance < pair[1].definition.instance)
        {
            return Err("parts are not sorted by #id, or repeat".to_owned());
        }
        let mut roots = 0;
        for part in &self.parts {
            let instance = part.definition.instance;
            if u64::try_from(part.occurrences.len()).ok() != Some(part.quantity) {
                return Err(format!(
                    "#{instance}: quantity is not its number of occurrences"
                ));
            }
            if part.occurrences.is_empty() {
                return Err(format!("#{instance} is listed but never occurs"));
            }
            if !part
                .occurrences
                .windows(2)
                .all(|pair| pair[0].path < pair[1].path)
            {
                return Err(format!(
                    "#{instance}: occurrences are not sorted by path, or repeat"
                ));
            }
            for occurrence in &part.occurrences {
                if occurrence.label.len() != occurrence.path.len() {
                    return Err(format!("#{instance}: a label's length is not its path's"));
                }
                if occurrence.path.is_empty() {
                    roots += 1;
                    if part.definition != *root || !occurrence.transform.is_identity() {
                        return Err(format!(
                            "#{instance} has the empty path but is not the root at the identity"
                        ));
                    }
                }
            }
        }
        if roots != 1 {
            return Err("exactly one occurrence, the root's, has the empty path".to_owned());
        }
        if self.source.header.is_none() {
            return Err("an accepted document has no header".to_owned());
        }
        self.check_tree()
    }

    /// The occurrences form the tree `read` gives: every path unique and
    /// within the limits of check 34, every path's parent an assembly's
    /// occurrence, a part never a parent and an assembly always one, and
    /// each usage labelled one way, as the path form says.
    fn check_tree(&self) -> Result<(), String> {
        use crate::gate::assembly::{MAX_LABEL_BYTES, MAX_OCCURRENCES, MAX_PATH_SEGMENTS};
        let mut kinds: BTreeMap<&[u64], DefinitionKind> = BTreeMap::new();
        let mut labels: BTreeMap<u64, &Option<String>> = BTreeMap::new();
        let (mut segments, mut bytes) = (0usize, 0usize);
        if !self.notes.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err("notes are not sorted, or repeat".to_owned());
        }
        for part in &self.parts {
            check_component(part)?;
            for occurrence in &part.occurrences {
                let floors = [
                    occurrence.part_uncertainty_mm,
                    occurrence.vertex_offset_mm,
                    occurrence.placement_uncertainty_mm,
                ];
                if !floors.iter().all(|f| f.is_finite() && *f >= 0.0) {
                    return Err(format!("{:?}: an uncertainty is negative", occurrence.path));
                }
                if kinds.insert(&occurrence.path, part.kind).is_some() {
                    return Err(format!("the path {:?} occurs twice", occurrence.path));
                }
                segments = segments.saturating_add(occurrence.path.len());
                for (&usage, label) in occurrence.path.iter().zip(&occurrence.label) {
                    bytes = bytes.saturating_add(label.as_ref().map_or(0, String::len));
                    if *labels.entry(usage).or_insert(label) != label {
                        return Err(format!("usage #{usage} is labelled two ways"));
                    }
                }
            }
        }
        if kinds.len() > MAX_OCCURRENCES || segments > MAX_PATH_SEGMENTS || bytes > MAX_LABEL_BYTES
        {
            return Err("more occurrences than check 34 lets a file have".to_owned());
        }
        let parents: BTreeSet<&[u64]> = kinds
            .keys()
            .filter_map(|path| path.split_last().map(|(_, parent)| parent))
            .collect();
        for (path, kind) in &kinds {
            if let Some((_, parent)) = path.split_last() {
                if kinds.get(parent) != Some(&DefinitionKind::Assembly) {
                    return Err(format!(
                        "the parent of {path:?} is no assembly's occurrence"
                    ));
                }
            }
            if (*kind == DefinitionKind::Assembly) != parents.contains(path) {
                return Err(format!("{path:?}: an assembly has components, a part none"));
            }
        }
        for (usage, label) in labels {
            match (self.path_form, label) {
                (_, Some(text)) if text.trim().is_empty() => {
                    return Err(format!("usage #{usage} has a blank label"));
                }
                (Some(PathForm::UsageId), None) => {
                    return Err(format!(
                        "usage #{usage} has no label, yet paths are written by id"
                    ));
                }
                (Some(PathForm::UsageId), Some(text))
                    if text.contains('/') || text.starts_with('#') =>
                {
                    return Err(format!("usage #{usage}'s label cannot be a path segment"));
                }
                _ => {}
            }
        }
        Ok(())
    }
}
