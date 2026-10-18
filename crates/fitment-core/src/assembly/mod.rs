//! Reading an assembly: its product structure, its occurrences and their
//! placements, and choosing the part to replace
//! (`docs/research/m3-socket.md`, sections 3 to 6).
//!
//! An assembly is read once, fully, before any part is chosen. Every
//! occurrence under the root is a path of usages plus one rigid transform
//! from the occurrence's frame to the root's.

mod component;
mod parts;
mod step;

use std::cmp::Ordering;
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::features::{DefinitionRef, FileHeader, Part};
use crate::gate::Refusal;
use crate::geom::Rigid;

pub use component::{ComponentError, SelectedComponent};
pub use parts::{
    BadHex, ComponentSummary, GateResult, PARTS_FORMAT, PARTS_FORMAT_VERSION, PartEntry,
    PartOccurrence, PartsDocument, PartsFormatError, Source,
};

/// At most this many occurrences are read under one root (check 34).
pub const MAX_OCCURRENCES: usize = crate::gate::assembly::MAX_OCCURRENCES;

/// At most this many suggestions follow a `--part` that matches nothing.
const SUGGESTIONS: usize = 20;

/// Reads an assembly: gate stage 1, then checks 31 to 35, and every
/// occurrence under the root with its placement. Bytes in, values out: no
/// I/O, and nothing panics.
pub fn read_assembly(step: &[u8]) -> AssemblyRead {
    step::read(step)
}

/// What [`read_assembly`] found: the parts list, if the file gives one,
/// and the reasons a socket could not be read from it.
#[derive(Debug, Clone, PartialEq)]
// The fields are private; the methods below read them.
#[allow(clippy::exhaustive_structs)]
pub struct AssemblyRead {
    header: Option<FileHeader>,
    refusals: Vec<Refusal>,
    listing: Option<Listing>,
    gate: Vec<Refusal>,
    notes: Vec<AssemblyNote>,
}

impl AssemblyRead {
    /// The file's header facts; `None` if the file could not be parsed.
    pub fn header(&self) -> Option<&FileHeader> {
        self.header.as_ref()
    }

    /// Why the file gives no parts list (gate stage 1, checks 32 to 34),
    /// sorted; empty when [`listing`](Self::listing) is there.
    pub fn refusals(&self) -> &[Refusal] {
        &self.refusals
    }

    /// The parts list: every definition reached from the root and every
    /// occurrence, placed.
    pub fn listing(&self) -> Option<&Listing> {
        self.listing.as_ref()
    }

    /// What the reading noticed that refuses nothing, sorted.
    pub fn notes(&self) -> &[AssemblyNote] {
        &self.notes
    }

    /// Why a socket cannot be read from the file: the refusals above, or
    /// the assembly gate's own (checks 31 and 35 to 38), sorted. Empty
    /// when the assembly passes.
    pub fn gate_refusals(&self) -> &[Refusal] {
        if self.refusals.is_empty() {
            &self.gate
        } else {
            &self.refusals
        }
    }
}

/// Whether a definition is a part (it uses nothing) or an assembly.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
// A definition has usages or it has none.
#[allow(clippy::exhaustive_enums)]
pub enum DefinitionKind {
    /// A leaf: a definition with no usage of its own. Only a part can be
    /// chosen (R3).
    Part,
    /// A definition that uses others.
    Assembly,
}

/// A definition reached from the root.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ListedDefinition {
    /// The definition and its product.
    pub definition: DefinitionRef,
    /// Part or assembly.
    pub kind: DefinitionKind,
    /// Its occurrences, as indices into [`Listing::occurrences`], in path
    /// order. Their number is the definition's quantity.
    pub occurrences: Vec<usize>,
    /// For a part, what reading it as a part gave (check 36): its
    /// features, or why it is not a canonical part. `None` for an
    /// assembly.
    pub read: Option<ComponentRead>,
    /// For an accepted part, each face's sound box (B8).
    pub(crate) faces: Vec<crate::features::extent::FaceBox>,
}

/// A component read as a part, once however often it occurs (P1).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ComponentRead {
    /// A canonical part, in its own frame.
    Accepted(Box<Part>),
    /// Why it is not one, sorted.
    Refused(Vec<Refusal>),
}

/// Something reading an assembly noticed that refuses nothing.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AssemblyNote {
    /// What it is.
    pub code: AssemblyNoteCode,
    /// The `#id` it concerns, if one.
    #[serde(deserialize_with = "Option::deserialize")]
    pub instance: Option<u64>,
    /// For people.
    pub message: String,
}

/// The kinds of [`AssemblyNote`].
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AssemblyNoteCode {
    /// Two occurrences of one definition at the same place, within the
    /// identity tolerance: usually a modelling slip.
    CoincidentOccurrences,
    /// A context on an occurrence's placement path states no length
    /// uncertainty, so it adds nothing to the floor (Z1).
    AssemblyUncertaintyMissing,
}

/// One usage on an occurrence's path.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UsageRef {
    /// The usage's `#id`.
    pub instance: u64,
    /// Its `id` attribute, if the file states one that is not blank.
    pub id: Option<String>,
}

/// The usages from the root down to an occurrence (O1); empty for the
/// root. Paths are ordered by their usage `#id`s, segment by segment (O4).
#[derive(Debug, Clone)]
// The field is private; paths come only from a listing.
#[allow(clippy::exhaustive_structs)]
pub struct OccurrencePath(Vec<UsageRef>);

impl OccurrencePath {
    /// The usages, from the root down.
    pub fn usages(&self) -> &[UsageRef] {
        &self.0
    }

    /// The usages' `#id`s, from the root down.
    pub fn instances(&self) -> Vec<u64> {
        self.0.iter().map(|usage| usage.instance).collect()
    }

    /// The path as people write it (O2): usage ids joined by `/`, or each
    /// segment `#<instance>`.
    pub fn display(&self, form: PathForm) -> String {
        let segments: Vec<String> = self
            .0
            .iter()
            .map(|usage| match (form, &usage.id) {
                (PathForm::UsageId, Some(id)) => id.clone(),
                _ => format!("#{}", usage.instance),
            })
            .collect();
        segments.join("/")
    }
}

impl PartialEq for OccurrencePath {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for OccurrencePath {}

impl PartialOrd for OccurrencePath {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OccurrencePath {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .iter()
            .map(|usage| usage.instance)
            .cmp(other.0.iter().map(|usage| usage.instance))
    }
}

/// How a file's paths are written (O2).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
// A segment is a usage id or an instance.
#[allow(clippy::exhaustive_enums)]
pub enum PathForm {
    /// Every usage has a non-blank id, unique among its siblings, with no
    /// `/` and not beginning with `#`: segments are usage ids.
    UsageId,
    /// Segments are `#<usage instance>`.
    Instance,
}

/// One place a definition occurs under the root.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Occurrence {
    /// The usages from the root down.
    pub path: OccurrencePath,
    /// The definition, an index into [`Listing::definitions`].
    pub definition: usize,
    /// The shape representation `transform` maps from, if the file names
    /// one.
    pub representation: Option<u64>,
    /// The occurrence's coordinates (mm) to the root's (mm).
    pub transform: Rigid,
    /// Its part's largest stated uncertainty, in mm; 0 for an assembly or
    /// a refused part (Z1).
    pub part_uncertainty_mm: f64,
    /// Its part's solid's largest vertex offset, in mm; 0 likewise (Z1).
    pub vertex_offset_mm: f64,
    /// The largest length uncertainty stated by the contexts it is placed
    /// through, the root's included; 0 if none states one (Z1).
    pub placement_uncertainty_mm: f64,
}

/// A usage between two listed definitions.
#[derive(Debug, Clone, PartialEq)]
struct ListedUsage {
    instance: u64,
    id: Option<String>,
    parent: usize,
}

/// The parts list of an assembly: every definition reached from the root,
/// by `#id`, and every occurrence, by path.
#[derive(Debug, Clone, PartialEq)]
// The fields are private; the methods below read them.
#[allow(clippy::exhaustive_structs)]
pub struct Listing {
    root: usize,
    definitions: Vec<ListedDefinition>,
    occurrences: Vec<Occurrence>,
    usages: Vec<ListedUsage>,
    path_form: PathForm,
}

impl Listing {
    /// The root, an index into [`definitions`](Self::definitions).
    pub fn root(&self) -> usize {
        self.root
    }

    /// Every definition reached from the root, the root included, by
    /// `#id`.
    pub fn definitions(&self) -> &[ListedDefinition] {
        &self.definitions
    }

    /// Every occurrence, the root's (with the empty path) included, by
    /// path.
    pub fn occurrences(&self) -> &[Occurrence] {
        &self.occurrences
    }

    /// How this file's paths are written.
    pub fn path_form(&self) -> PathForm {
        self.path_form
    }

    /// The candidate a selection error names for definition `index`.
    fn candidate(&self, index: usize) -> Option<Candidate> {
        let listed = self.definitions.get(index)?;
        Some(Candidate {
            definition: listed.definition.clone(),
            kind: listed.kind,
            quantity: listed.occurrences.len(),
            first_path: listed
                .occurrences
                .first()
                .and_then(|&occurrence| self.occurrences.get(occurrence))
                .map(|occurrence| occurrence.path.display(self.path_form)),
        })
    }

    fn candidates(&self, indices: impl IntoIterator<Item = usize>) -> Vec<Candidate> {
        indices
            .into_iter()
            .filter_map(|index| self.candidate(index))
            .collect()
    }

    /// The definitions of `kind` whose product's id (or name) is `text`.
    fn named(&self, kind: DefinitionKind, text: &str, by_name: bool) -> Vec<usize> {
        self.definitions
            .iter()
            .enumerate()
            .filter(|(_, listed)| listed.kind == kind)
            .filter(|(_, listed)| {
                listed.definition.product.as_ref().is_some_and(|product| {
                    let stated = if by_name { &product.name } else { &product.id };
                    stated.as_deref() == Some(text)
                })
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// T1 to T5: the part `text` names.
    fn select_part(&self, text: &str) -> Result<(usize, MatchedBy), SelectError> {
        // T2: `#<digits>` names a definition, unless a product id is
        // literally that text too.
        if let Some(instance) = text
            .strip_prefix('#')
            .filter(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|digits| digits.parse::<u64>().ok())
        {
            let by_instance = self
                .definitions
                .iter()
                .position(|listed| listed.definition.instance == instance);
            let mut by_id = self.named(DefinitionKind::Part, text, false);
            by_id.retain(|&index| Some(index) != by_instance);
            match (by_instance, by_id.is_empty()) {
                (Some(index), false) => {
                    return Err(SelectError::PartAmbiguous {
                        part: text.to_owned(),
                        candidates: self.candidates(std::iter::once(index).chain(by_id)),
                    });
                }
                (Some(index), true) => {
                    return match self.definitions.get(index).map(|listed| listed.kind) {
                        Some(DefinitionKind::Part) => Ok((index, MatchedBy::DefinitionInstance)),
                        _ => Err(SelectError::PartIsAssembly {
                            part: text.to_owned(),
                            assembly: self.candidates([index]),
                        }),
                    };
                }
                (None, _) => {}
            }
        }
        // T3, then T4.
        for (by_name, matched_by) in [
            (false, MatchedBy::ProductId),
            (true, MatchedBy::ProductName),
        ] {
            match self.named(DefinitionKind::Part, text, by_name).as_slice() {
                [] => {}
                &[index] => return Ok((index, matched_by)),
                several => {
                    return Err(SelectError::PartAmbiguous {
                        part: text.to_owned(),
                        candidates: self.candidates(several.iter().copied()),
                    });
                }
            }
        }
        // T5.
        let mut assemblies = self.named(DefinitionKind::Assembly, text, false);
        assemblies.extend(self.named(DefinitionKind::Assembly, text, true));
        assemblies.sort_unstable();
        assemblies.dedup();
        if !assemblies.is_empty() {
            return Err(SelectError::PartIsAssembly {
                part: text.to_owned(),
                assembly: self.candidates(assemblies),
            });
        }
        let wanted = text.to_lowercase();
        let contains = |stated: &Option<String>| {
            stated
                .as_deref()
                .is_some_and(|stated| stated.to_lowercase().contains(&wanted))
        };
        let mut near: Vec<(Option<&str>, u64, usize)> = self
            .definitions
            .iter()
            .enumerate()
            .filter(|(_, listed)| listed.kind == DefinitionKind::Part)
            .filter_map(|(index, listed)| {
                let product = listed.definition.product.as_ref()?;
                (contains(&product.id) || contains(&product.name)).then_some((
                    product.id.as_deref(),
                    listed.definition.instance,
                    index,
                ))
            })
            .collect();
        near.sort_unstable();
        Err(SelectError::PartNotFound {
            part: text.to_owned(),
            suggestions: self.candidates(near.into_iter().take(SUGGESTIONS).map(|(_, _, i)| i)),
        })
    }

    /// The occurrence whose path's usage `#id`s are `path`.
    fn at(&self, path: &[u64]) -> Option<usize> {
        self.occurrences
            .binary_search_by(|occurrence| {
                occurrence
                    .path
                    .usages()
                    .iter()
                    .map(|usage| usage.instance)
                    .cmp(path.iter().copied())
            })
            .ok()
    }

    /// O3: the occurrence `text` names, segment by segment from the root:
    /// a segment is a usage id or `#<usage instance>`, and one that names
    /// two usages is refused rather than guessed.
    fn resolve(&self, text: &str, part: Option<usize>) -> Result<usize, SelectError> {
        let not_found = || SelectError::OccurrenceNotFound {
            occurrence: text.to_owned(),
            occurrences: part
                .and_then(|part| self.definitions.get(part))
                .map(|listed| {
                    listed
                        .occurrences
                        .iter()
                        .take(SUGGESTIONS)
                        .filter_map(|&index| self.occurrences.get(index))
                        .map(|occurrence| occurrence.path.display(self.path_form))
                        .collect()
                })
                .unwrap_or_default(),
        };
        if text.is_empty() {
            return Err(not_found());
        }
        let mut current = self.root;
        let mut path = Vec::new();
        for segment in text.split('/') {
            let instance = segment
                .strip_prefix('#')
                .filter(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|digits| digits.parse::<u64>().ok());
            let matching: BTreeSet<u64> = self
                .usages
                .iter()
                .filter(|usage| usage.parent == current)
                .filter(|usage| {
                    Some(usage.instance) == instance || usage.id.as_deref() == Some(segment)
                })
                .map(|usage| usage.instance)
                .collect();
            let found = match matching.len() {
                0 => return Err(not_found()),
                1 => matching.first().copied().ok_or_else(not_found)?,
                _ => {
                    return Err(SelectError::OccurrenceAmbiguous {
                        occurrence: text.to_owned(),
                        segment: segment.to_owned(),
                    });
                }
            };
            path.push(found);
            let index = self.at(&path).ok_or_else(not_found)?;
            current = self
                .occurrences
                .get(index)
                .map_or(usize::MAX, |occurrence| occurrence.definition);
        }
        self.at(&path).ok_or_else(not_found)
    }

    /// Chooses the part to replace and its occurrences (section 5): by
    /// `part` (T rules), by `occurrence` (O3), or both, which must agree.
    /// Without `occurrence`, every occurrence of the part (D11).
    ///
    /// # Errors
    ///
    /// A [`SelectError`] naming the candidates, for the user to choose.
    pub fn select(
        &self,
        part: Option<&str>,
        occurrence: Option<&str>,
    ) -> Result<Selection, SelectError> {
        let by_part = part.map(|text| self.select_part(text)).transpose()?;
        let Some(text) = occurrence else {
            let Some((definition, matched_by)) = by_part else {
                return Err(SelectError::NothingSelected);
            };
            let occurrences = self
                .definitions
                .get(definition)
                .map(|listed| listed.occurrences.clone())
                .unwrap_or_default();
            return Ok(Selection {
                definition,
                matched_by,
                occurrences,
            });
        };
        let index = self.resolve(text, by_part.map(|(definition, _)| definition))?;
        let definition = self
            .occurrences
            .get(index)
            .map_or(usize::MAX, |occurrence| occurrence.definition);
        match by_part {
            Some((wanted, _)) if wanted != definition => Err(SelectError::OccurrenceMismatch {
                occurrence: text.to_owned(),
                part: self.candidates([wanted]),
            }),
            Some((_, matched_by)) => Ok(Selection {
                definition,
                matched_by,
                occurrences: vec![index],
            }),
            None => match self.definitions.get(definition).map(|listed| listed.kind) {
                Some(DefinitionKind::Part) => Ok(Selection {
                    definition,
                    matched_by: MatchedBy::Occurrence,
                    occurrences: vec![index],
                }),
                _ => Err(SelectError::PartIsAssembly {
                    part: text.to_owned(),
                    assembly: self.candidates([definition]),
                }),
            },
        }
    }
}

/// How the part was chosen.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MatchedBy {
    /// `#<n>`, a definition `#id` (T2).
    DefinitionInstance,
    /// Its `PRODUCT.id` (T3).
    ProductId,
    /// Its `PRODUCT.name` (T4).
    ProductName,
    /// Only `--occurrence` was given.
    Occurrence,
}

/// The part chosen and its occurrences.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Selection {
    /// The part, an index into [`Listing::definitions`].
    pub definition: usize,
    /// How it was chosen.
    pub matched_by: MatchedBy,
    /// The occurrences chosen, indices into [`Listing::occurrences`], in
    /// path order.
    pub occurrences: Vec<usize>,
}

/// A definition a selection error offers the user.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Candidate {
    /// The definition and its product.
    pub definition: DefinitionRef,
    /// Part or assembly.
    pub kind: DefinitionKind,
    /// How many times it occurs.
    pub quantity: usize,
    /// Its first occurrence's path, written as the file's paths are.
    pub first_path: Option<String>,
}

/// Why `--part` and `--occurrence` choose no part (T6): a usage error, not
/// a gate refusal, since the file was read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SelectError {
    /// Neither a part nor an occurrence was named.
    #[error("name a part (--part) or an occurrence (--occurrence)")]
    NothingSelected,
    /// No part has that `#id`, id or name (T5).
    #[error("no part is '{part}'")]
    PartNotFound {
        /// The value given.
        part: String,
        /// Parts whose id or name contains it, ignoring case, by id.
        suggestions: Vec<Candidate>,
    },
    /// Several parts have that id, or that name (T2 to T4).
    #[error("several parts are '{part}'")]
    PartAmbiguous {
        /// The value given.
        part: String,
        /// The parts it names.
        candidates: Vec<Candidate>,
    },
    /// Only an assembly is that; only a part can be chosen (R3).
    #[error("'{part}' is an assembly; choose one of its parts")]
    PartIsAssembly {
        /// The value given.
        part: String,
        /// The assemblies it names.
        assembly: Vec<Candidate>,
    },
    /// No occurrence has that path (O3).
    #[error("no occurrence is '{occurrence}'")]
    OccurrenceNotFound {
        /// The path given.
        occurrence: String,
        /// The chosen part's occurrences, if a part was named.
        occurrences: Vec<String>,
    },
    /// A segment names more than one usage (O3).
    #[error("'{segment}' in '{occurrence}' names more than one usage")]
    OccurrenceAmbiguous {
        /// The path given.
        occurrence: String,
        /// The segment that is ambiguous.
        segment: String,
    },
    /// The occurrence is not one of the part named.
    #[error("'{occurrence}' is not an occurrence of the part named")]
    OccurrenceMismatch {
        /// The path given.
        occurrence: String,
        /// The part named.
        part: Vec<Candidate>,
    },
}

impl SelectError {
    /// The machine name, as tests and scripts compare it.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NothingSelected => "nothing_selected",
            Self::PartNotFound { .. } => "part_not_found",
            Self::PartAmbiguous { .. } => "part_ambiguous",
            Self::PartIsAssembly { .. } => "part_is_assembly",
            Self::OccurrenceNotFound { .. } => "occurrence_not_found",
            Self::OccurrenceAmbiguous { .. } => "occurrence_ambiguous",
            Self::OccurrenceMismatch { .. } => "occurrence_mismatch",
        }
    }

    /// The definitions the error offers, for the message.
    pub fn candidates(&self) -> &[Candidate] {
        match self {
            Self::PartNotFound { suggestions, .. } => suggestions,
            Self::PartAmbiguous { candidates, .. } => candidates,
            Self::PartIsAssembly { assembly, .. } => assembly,
            Self::OccurrenceMismatch { part, .. } => part,
            Self::NothingSelected
            | Self::OccurrenceNotFound { .. }
            | Self::OccurrenceAmbiguous { .. } => &[],
        }
    }
}
