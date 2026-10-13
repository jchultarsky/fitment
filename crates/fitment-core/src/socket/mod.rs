//! The socket of a part in an assembly: what its neighbours require of any
//! part that replaces it (`docs/research/m3-socket.md`, sections 7 to 9).

mod axial;
mod block;
mod check;
mod combine;
mod geo;
mod merge;
mod model;
mod overlap;
mod planar;
mod review;
mod rules;
mod touch;

use serde::{Deserialize, Serialize};

pub use review::{CarryOver, ReviewError, StaleReason, StaleReview};

pub use model::{
    Basis, BoreSpan, Cause, CauseFaces, Depth, Envelope, Extracted, Fit, Flag, LeftOut,
    LeftOutReason, Notes, Open, PlacedRef, Requirement, Requires, ReviewEntry, ReviewStatus, Ring,
    SeatContact, SelectionRecord, ShaftSpan, Socket, SocketConstants, SourceRef, Target, Toucher,
};

use std::collections::{BTreeMap, BTreeSet};

use crate::EXTRACTOR_VERSION;
use crate::assembly::{
    AssemblyRead, ComponentRead, DefinitionKind, Listing, SelectError, Selection, Source,
    read_assembly,
};
use crate::features::{CylinderClass, Extraction, FeatureId, Part, Status};
use crate::gate::{self, Refusal, RefusalCode};
use crate::sha256::sha256_hex;
use geo::Motion;
use rules::Neighbour;

/// The `format` of every socket document.
pub const SOCKET_FORMAT: &str = "fitment.socket";

/// The layout this crate writes and reads; frozen at the first publish.
pub const SOCKET_FORMAT_VERSION: u32 = 1;

/// The version of the socket rules: it rises whenever a socket rule,
/// constant, order or field could change any socket (R20).
pub const SOCKET_RULES_VERSION: u32 = 5;

/// The least contact tolerance, in mm (Z3).
pub(crate) const CONTACT_MM: f64 = 0.01;

/// The near band, and the reach of the bounded rules, in mm (Z6).
pub(crate) const NEAR_MM: f64 = 1.0;

/// The largest sine between axes compared as parallel (Z5).
pub(crate) const PARALLEL_SINE_REL: f64 = 0.01;

/// The most candidate pairs examined, over all selected occurrences
/// (check 39).
pub(crate) const MAX_RELATIONS: u64 = 10_000_000;

/// Lengths within this of each other merge occurrences (9.1), in mm.
pub(crate) const MERGE_MM: f64 = 1e-6;

/// Directions and angles within this merge occurrences (9.1).
pub(crate) const MERGE_SINE: f64 = 1e-9;

/// The constants of these rules.
fn constants() -> SocketConstants {
    SocketConstants {
        contact_mm: CONTACT_MM,
        near_mm: NEAR_MM,
        parallel_sine_rel: PARALLEL_SINE_REL,
        merge_mm: MERGE_MM,
        merge_sine: MERGE_SINE,
        max_occurrences: crate::assembly::MAX_OCCURRENCES as u64,
        max_relations: MAX_RELATIONS,
    }
}

/// The constants a document was extracted with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SocketExtraction {
    /// The features' constants.
    pub features: Extraction,
    /// The socket rules'.
    pub socket: SocketConstants,
}

/// A socket document: the sockets of every chosen occurrence of a part, or
/// the reasons the assembly gives none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
// The fields are private; documents come only from `extract` and
// `from_json`.
#[allow(clippy::exhaustive_structs)]
pub struct SocketDocument {
    format: String,
    format_version: u32,
    extractor_version: u32,
    socket_rules_version: u32,
    extraction: SocketExtraction,
    source: Source,
    #[serde(deserialize_with = "Option::deserialize")]
    selection: Option<SelectionRecord>,
    status: Status,
    refusals: Vec<Refusal>,
    notes: Notes,
    #[serde(deserialize_with = "Option::deserialize")]
    extracted: Option<Extracted>,
    #[serde(deserialize_with = "Option::deserialize")]
    digest: Option<String>,
    review: Vec<ReviewEntry>,
    stale_reviews: Vec<StaleReview>,
}

/// Why a socket document could not be read back.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SocketFormatError {
    /// The text is not JSON, or not this document's shape.
    #[error("not a socket document: {message} (line {line}, column {column})")]
    Json {
        /// What `serde_json` reported.
        message: String,
        /// 1-based line.
        line: usize,
        /// 1-based column.
        column: usize,
    },
    /// The document has another `format`.
    #[error("format is '{found}', not '{SOCKET_FORMAT}'")]
    Format {
        /// The format found.
        found: String,
    },
    /// The document has a layout version this crate does not read.
    #[error("format version {found} is not {SOCKET_FORMAT_VERSION}")]
    Version {
        /// The version found.
        found: u32,
    },
    /// Its digest is not that of what it states: a value was changed by
    /// hand. Record decisions under `review`, or extract it again.
    #[error("the digest does not match the document: a value was changed after extraction")]
    Digest,
    /// The document breaks one of its own rules.
    #[error("invalid socket document: {0}")]
    Invalid(String),
}

impl SocketDocument {
    /// Extracts the socket of the part `part` names (or the occurrence
    /// `occurrence` names) from an assembly. Bytes in, values out.
    ///
    /// # Errors
    ///
    /// A [`SelectError`] if the selection names no part: a usage error,
    /// since the file was read. A refused assembly is a document with
    /// status refused.
    pub fn extract(
        step: &[u8],
        part: Option<&str>,
        occurrence: Option<&str>,
    ) -> Result<Self, SelectError> {
        let read = read_assembly(step);
        let source = Source::of(step, read.header().cloned());
        let Some(listing) = read.listing() else {
            return Ok(Self::refused(source, None, read.refusals().to_vec(), &read));
        };
        let selection = listing.select(part, occurrence)?;
        let record = SelectionRecord {
            part: part.map(str::to_owned),
            occurrence: occurrence.map(str::to_owned),
            matched_by: selection.matched_by,
        };
        if !read.gate_refusals().is_empty() {
            return Ok(Self::refused(
                source,
                Some(record),
                read.gate_refusals().to_vec(),
                &read,
            ));
        }
        Ok(match extract_listing(listing, &selection) {
            Ok(extracted) => Self::accepted(source, record, extracted, &read),
            Err(refusal) => Self::refused(source, Some(record), vec![refusal], &read),
        })
    }

    fn base(source: Source, selection: Option<SelectionRecord>, read: &AssemblyRead) -> Self {
        Self {
            format: SOCKET_FORMAT.to_owned(),
            format_version: SOCKET_FORMAT_VERSION,
            extractor_version: EXTRACTOR_VERSION,
            socket_rules_version: SOCKET_RULES_VERSION,
            extraction: SocketExtraction {
                features: Extraction::CURRENT,
                socket: constants(),
            },
            source,
            selection,
            status: Status::Refused,
            refusals: Vec::new(),
            notes: read.notes().to_vec(),
            extracted: None,
            digest: None,
            review: Vec::new(),
            stale_reviews: Vec::new(),
        }
    }

    fn refused(
        source: Source,
        selection: Option<SelectionRecord>,
        refusals: Vec<Refusal>,
        read: &AssemblyRead,
    ) -> Self {
        let mut document = Self::base(source, selection, read);
        document.refusals = gate::finish(refusals);
        document
    }

    fn accepted(
        source: Source,
        selection: SelectionRecord,
        extracted: Extracted,
        read: &AssemblyRead,
    ) -> Self {
        let mut document = Self::base(source, Some(selection), read);
        document.status = Status::Accepted;
        document.review = extracted
            .sockets
            .iter()
            .flat_map(|socket| {
                socket.requirements.iter().map(|r| ReviewEntry {
                    socket: socket.id.clone(),
                    requirement: r.id.clone(),
                    status: ReviewStatus::Unreviewed,
                    note: None,
                })
            })
            .collect();
        document.extracted = Some(extracted);
        document.digest = Some(document.compute_digest());
        document
    }

    /// `"sha256:<hex>"` of the document's compact JSON without its digest
    /// and its review (R19).
    fn compute_digest(&self) -> String {
        let mut value = serde_json::to_value(self).unwrap_or_default();
        if let Some(map) = value.as_object_mut() {
            map.remove("digest");
            map.remove("review");
            map.remove("stale_reviews");
        }
        format!("sha256:{}", sha256_hex(value.to_string().as_bytes()))
    }

    /// Accepted or refused.
    pub fn status(&self) -> Status {
        self.status
    }

    /// Why no socket could be read; empty when accepted.
    pub fn refusals(&self) -> &[Refusal] {
        &self.refusals
    }

    /// What extraction derived; `None` when refused.
    pub fn extracted(&self) -> Option<&Extracted> {
        self.extracted.as_ref()
    }

    /// The file it was read from.
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// The review decisions, one per (socket, requirement).
    pub fn review(&self) -> &[ReviewEntry] {
        &self.review
    }

    /// The decisions that could not be carried over to this extraction.
    pub fn stale_reviews(&self) -> &[StaleReview] {
        &self.stale_reviews
    }

    /// Applies one review decision to requirement `id` in every socket
    /// holding it, or only in `socket`; returns how many sockets it
    /// changed. The digest does not cover the review, so it stands.
    ///
    /// # Errors
    ///
    /// A [`ReviewError`] for an unknown id or socket, a removal without a
    /// note, a note too long, or an id whose requirement differs between
    /// the sockets holding it when no socket is named.
    pub fn decide(
        &mut self,
        id: &str,
        socket: Option<&str>,
        status: ReviewStatus,
        note: Option<&str>,
    ) -> Result<usize, ReviewError> {
        let Some(extracted) = &self.extracted else {
            return Err(ReviewError::Refused);
        };
        review::decide(extracted, &mut self.review, id, socket, status, note)
    }

    /// Carries the decisions of `old`, a document of the same file and
    /// target, over to this one (9.5); those that cannot be carried go to
    /// `stale_reviews`, with `old`'s own stale ones.
    pub fn carry_over(&mut self, old: &Self) -> CarryOver {
        let (Some(new), Some(previous)) = (&self.extracted, &old.extracted) else {
            return CarryOver::default();
        };
        let mut stale = old.stale_reviews.clone();
        let outcome = review::carry_over(new, &mut self.review, previous, &old.review, &mut stale);
        self.stale_reviews = stale;
        outcome
    }

    /// The assembly's notes.
    pub fn notes(&self) -> &Notes {
        &self.notes
    }

    /// True if written by this crate's extractor and socket rules.
    pub fn is_current(&self) -> bool {
        self.extractor_version == EXTRACTOR_VERSION
            && self.socket_rules_version == SOCKET_RULES_VERSION
    }

    /// The document as pretty JSON, ending with a newline.
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Reads a document back, checking its format, version, digest and
    /// rules.
    ///
    /// # Errors
    ///
    /// A [`SocketFormatError`] if the text is not a valid socket document.
    pub fn from_json(text: &str) -> Result<Self, SocketFormatError> {
        let document: Self =
            serde_json::from_str(text).map_err(|error| SocketFormatError::Json {
                message: error.to_string(),
                line: error.line(),
                column: error.column(),
            })?;
        if document.format != SOCKET_FORMAT {
            return Err(SocketFormatError::Format {
                found: document.format,
            });
        }
        if document.format_version != SOCKET_FORMAT_VERSION {
            return Err(SocketFormatError::Version {
                found: document.format_version,
            });
        }
        match (&document.extracted, &document.digest) {
            (Some(_), Some(digest)) => {
                if *digest != document.compute_digest() {
                    return Err(SocketFormatError::Digest);
                }
            }
            (None, None) => {}
            _ => {
                return Err(SocketFormatError::Invalid(
                    "a document holds both extracted values and their digest, or neither"
                        .to_owned(),
                ));
            }
        }
        document.check().map_err(SocketFormatError::Invalid)?;
        Ok(document)
    }

    fn check(&self) -> Result<(), String> {
        let sorted = self.refusals.windows(2).all(|pair| pair[0] < pair[1]);
        match (self.status, &self.extracted) {
            (Status::Accepted, Some(_)) if self.refusals.is_empty() => {}
            (Status::Refused, None) if !self.refusals.is_empty() && sorted => {}
            _ => return Err("its status, refusals and extracted values disagree".to_owned()),
        }
        let Some(extracted) = &self.extracted else {
            return Ok(());
        };
        for socket in &extracted.sockets {
            for requirement in &socket.requirements {
                let derived = combine::id_of(&requirement.requires);
                if derived != requirement.id {
                    return Err(format!(
                        "requirement '{}' is not named by what it requires",
                        requirement.id
                    ));
                }
            }
        }
        check_occurrences(extracted)?;
        check::extracted(extracted)?;
        check::review(extracted, &self.review)
    }
}

/// Each occurrence is in exactly one socket, the one it names; every
/// source's occurrence belongs to its socket; every neighbour a source
/// names is listed, and every listed neighbour is cited (9.4).
fn check_occurrences(extracted: &Extracted) -> Result<(), String> {
    let mut placed: Vec<(&[u64], &str)> = extracted
        .occurrences
        .iter()
        .map(|o| (o.path.as_slice(), o.socket.as_deref().unwrap_or("")))
        .collect();
    let mut grouped: Vec<(&[u64], &str)> = extracted
        .sockets
        .iter()
        .flat_map(|s| s.occurrences.iter().map(|o| (o.as_slice(), s.id.as_str())))
        .collect();
    placed.sort_unstable();
    grouped.sort_unstable();
    if placed != grouped || placed.windows(2).any(|w| w[0].0 == w[1].0) {
        return Err("the occurrences and the sockets that group them disagree".to_owned());
    }
    let listed: BTreeSet<&[u64]> = extracted
        .neighbours
        .iter()
        .map(|n| n.path.as_slice())
        .collect();
    let mut cited = BTreeSet::new();
    for socket in &extracted.sockets {
        for requirement in &socket.requirements {
            for source in &requirement.sources {
                if !socket.occurrences.contains(&source.occurrence) {
                    return Err(format!(
                        "a source of '{}' in {} is of an occurrence outside it",
                        requirement.id, socket.id
                    ));
                }
                if !source.neighbour.is_empty() {
                    if !listed.contains(source.neighbour.as_slice()) {
                        return Err(format!(
                            "a source of '{}' in {} names an unlisted neighbour",
                            requirement.id, socket.id
                        ));
                    }
                    cited.insert(source.neighbour.as_slice());
                }
            }
        }
    }
    // Every neighbour a toucher or ring names is listed too.
    let named = extracted
        .sockets
        .iter()
        .flat_map(|s| &s.requirements)
        .flat_map(|r| match &r.requires {
            Requires::Seat {
                contacts, rings, ..
            } => contacts
                .iter()
                .flat_map(|c| c.by.iter().map(|t| t.neighbour.as_slice()))
                .chain(rings.iter().map(|r| r.neighbour.as_slice()))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        });
    for path in named {
        if !listed.contains(path) {
            return Err("a seat names a neighbour the document does not list".to_owned());
        }
    }
    if cited.len() != listed.len() || listed.len() != extracted.neighbours.len() {
        return Err("a listed neighbour is cited by no source, or listed twice".to_owned());
    }
    Ok(())
}

/// The floor of an occurrence's own uncertainties (Z1).
fn floor_of(occurrence: &crate::assembly::Occurrence) -> f64 {
    occurrence
        .part_uncertainty_mm
        .max(occurrence.vertex_offset_mm)
        .max(occurrence.placement_uncertainty_mm)
}

fn accepted_part(listing: &Listing, definition: usize) -> Option<&Part> {
    match listing.definitions().get(definition)?.read.as_ref()? {
        ComponentRead::Accepted(part) => Some(part),
        ComponentRead::Refused(_) => None,
    }
}

fn placed_ref(listing: &Listing, index: usize, socket: Option<String>) -> Option<PlacedRef> {
    let occurrence = listing.occurrences().get(index)?;
    Some(PlacedRef {
        path: occurrence.path.instances(),
        label: occurrence
            .path
            .usages()
            .iter()
            .map(|u| u.id.clone())
            .collect(),
        definition: listing
            .definitions()
            .get(occurrence.definition)?
            .definition
            .clone(),
        transform: occurrence.transform,
        part_uncertainty_mm: occurrence.part_uncertainty_mm,
        vertex_offset_mm: occurrence.vertex_offset_mm,
        placement_uncertainty_mm: occurrence.placement_uncertainty_mm,
        socket,
    })
}

/// Every accepted definition's axial elements, once, by definition.
fn definition_elements(listing: &Listing) -> BTreeMap<usize, Vec<axial::Element>> {
    listing
        .definitions()
        .iter()
        .enumerate()
        .filter_map(|(i, _)| Some((i, axial::elements(accepted_part(listing, i)?))))
        .collect()
}

/// What the plane rules read one target occurrence against.
struct Planes<'a, 'l> {
    target: &'a Part,
    targets: &'a [axial::Element],
    boxes: &'a [crate::features::extent::FaceBox],
    neighbours: &'a [Neighbour<'l>],
}

impl Planes<'_, '_> {
    /// C1 to C6, H1, L1, L2 and Q1: the seats, grips, conical seats and
    /// unresolved contacts; `TooMany` past check 39's count.
    fn requirements(
        &self,
        found: &rules::Found,
        (own, paths): (&[u64], &[Vec<u64>]),
        pairs: &mut u64,
    ) -> Result<Vec<Requirement>, rules::TooMany> {
        let faces = planar::target_faces(self.target);
        let mut plane_found = planar::relate(self.target, &faces, self.neighbours, pairs)?;
        let read = touch::Read {
            targets: self.targets,
            axial: found,
            contacts: &plane_found.contacts,
            cones: &plane_found.cones,
            rings: &found.rings,
        };
        plane_found.unread_contacts =
            touch::relate(self.target, self.boxes, self.neighbours, &read, pairs)?;
        Ok(combine::planar(
            self.target,
            &faces,
            &plane_found,
            &found.rings,
            own,
            paths,
        ))
    }
}

/// The target's record (V2): its definition, frame, solid, floors,
/// extent and the features its requirements may anchor to.
fn target_of(
    listing: &Listing,
    selection: &Selection,
    target: &Part,
    targets: &[axial::Element],
) -> Result<model::Target, Refusal> {
    let internal = |m: &str| Refusal::new(RefusalCode::Internal, None, m);
    Ok(model::Target {
        definition: listing
            .definitions()
            .get(selection.definition)
            .map(|d| d.definition.clone())
            .ok_or_else(|| internal("the chosen definition is missing"))?,
        frame: target.frame.clone(),
        solid: target
            .solids
            .first()
            .map(|s| s.id)
            .ok_or_else(|| internal("no solid"))?,
        part_uncertainty_mm: target.max_uncertainty_mm,
        vertex_offset_mm: target
            .solids
            .first()
            .map_or(0.0, |s| s.max_vertex_offset_mm),
        complete: target.complete,
        extent: target.extent,
        anchors: {
            // Partial cylinders appear only through their requirements.
            let mut anchors: Vec<_> = targets
                .iter()
                .filter(|e| e.partial.is_none())
                .map(|e| e.feature)
                .collect();
            anchors.sort_by_key(|f| f.instance());
            anchors.dedup();
            anchors
        },
    })
}

/// The sockets of the chosen occurrences, or check 39's refusal.
fn extract_listing(listing: &Listing, selection: &Selection) -> Result<Extracted, Refusal> {
    let internal = |m: &str| Refusal::new(RefusalCode::Internal, None, m);
    let target = accepted_part(listing, selection.definition)
        .ok_or_else(|| internal("the chosen part was not read"))?;
    // U3: a side of a target stack with no known end runs to the part's
    // extent, as a neighbour's does.
    let targets: Vec<axial::Element> = axial::elements(target)
        .iter()
        .map(|e| axial::placed(e, &Motion::IDENTITY, target.extent.as_ref()))
        .collect();
    let elements = definition_elements(listing);
    let too_many = || {
        Refusal::new(
            RefusalCode::TooManyRelations,
            None,
            format!(
                "more than {MAX_RELATIONS} candidate pairs: fitment examines at most that many"
            ),
        )
    };
    let mut pairs = 0;
    let mut sockets = Vec::new();
    let mut cited = std::collections::BTreeSet::new();
    for (k, &index) in selection.occurrences.iter().enumerate() {
        let occurrence = listing
            .occurrences()
            .get(index)
            .ok_or_else(|| internal("a chosen occurrence is missing"))?;
        let around = Around {
            listing,
            selection,
            elements: &elements,
            index,
            target,
        };
        let (neighbours, indices, planes) = around.neighbours(&mut pairs)?.ok_or_else(too_many)?;
        let plane_refs: Vec<(Motion, &Part)> = planes.iter().map(|(m, p)| (*m, *p)).collect();
        let found = rules::relate(&targets, &neighbours, &plane_refs, &mut pairs)
            .map_err(|_| too_many())?;
        let paths: Vec<Vec<u64>> = neighbours.iter().map(|n| n.path.clone()).collect();
        let own = occurrence.path.instances();
        let (mut requirements, left_out) =
            combine::requirements(target, &targets, &found, &own, &paths);
        let boxes = listing
            .definitions()
            .get(selection.definition)
            .map_or(&[][..], |d| d.faces.as_slice());
        let planes = Planes {
            target,
            targets: &targets,
            boxes,
            neighbours: &neighbours,
        };
        requirements.extend(
            planes
                .requirements(&found, (&own, &paths), &mut pairs)
                .map_err(|_| too_many())?,
        );
        requirements.extend(unread_neighbours(listing, &neighbours, &indices));
        let mut requirements = combine::merge_same(requirements);
        combine::order(&mut requirements);
        // A3: a feature some requirement anchors (C6's on a shaft) is not
        // left out.
        let anchored: BTreeSet<FeatureId> =
            requirements.iter().filter_map(combine::anchor).collect();
        let left_out: Vec<LeftOut> = left_out
            .into_iter()
            .filter(|l| !anchored.contains(&l.feature))
            .collect();
        for requirement in &requirements {
            cited.extend(requirement.sources.iter().map(|s| s.neighbour.clone()));
        }
        sockets.push(model::Socket {
            id: format!("socket-{}", k + 1),
            occurrences: vec![own],
            requirements,
            left_out,
        });
    }
    let target_ref = target_of(listing, selection, target, &targets)?;
    let sockets = merge::merge(sockets);
    let (occurrences, neighbours) = placed_lists(listing, selection, &sockets, &cited);
    Ok(Extracted {
        target: target_ref,
        occurrences,
        neighbours,
        sockets,
        envelope: Envelope {
            check_set: "others_and_copies".to_owned(),
            extent: target.extent,
        },
    })
}

/// The chosen occurrences, each naming its socket, and the neighbours the
/// sources cite.
fn placed_lists(
    listing: &Listing,
    selection: &Selection,
    sockets: &[model::Socket],
    cited: &BTreeSet<Vec<u64>>,
) -> (Vec<PlacedRef>, Vec<PlacedRef>) {
    let by_path: BTreeMap<&[u64], &str> = sockets
        .iter()
        .flat_map(|s| {
            s.occurrences
                .iter()
                .map(move |o| (o.as_slice(), s.id.as_str()))
        })
        .collect();
    let socket_of = |path: &[u64]| by_path.get(path).map(|id| (*id).to_owned());
    let occurrences = selection
        .occurrences
        .iter()
        .filter_map(|&i| {
            let path = listing.occurrences().get(i)?.path.instances();
            placed_ref(listing, i, socket_of(&path))
        })
        .collect();
    let neighbours = listing
        .occurrences()
        .iter()
        .enumerate()
        .filter(|(_, o)| cited.contains(&o.path.instances()))
        .filter_map(|(i, _)| placed_ref(listing, i, None))
        .collect();
    (occurrences, neighbours)
}

/// The neighbours of target occurrence `index`, moved into its frame, with
/// their listing indices; and every occurrence's planes for J3, the
/// target's own included.
#[allow(clippy::type_complexity)]
/// What one selected occurrence's neighbours are read from.
struct Around<'l, 's> {
    listing: &'l Listing,
    selection: &'s Selection,
    elements: &'s BTreeMap<usize, Vec<axial::Element>>,
    index: usize,
    target: &'l Part,
}

/// One occurrence's neighbours, the indices of their occurrences, and
/// every placed part with its motion (for J3); `None` once `pairs`, check
/// 39's count, passes its limit, each neighbour counting one.
type Gathered<'l> = (Vec<Neighbour<'l>>, Vec<usize>, Vec<(Motion, &'l Part)>);

impl<'l> Around<'l, '_> {
    fn neighbours(&self, pairs: &mut u64) -> Result<Option<Gathered<'l>>, Refusal> {
        let internal = |m: &str| Refusal::new(RefusalCode::Internal, None, m);
        let listing = self.listing;
        let here = listing
            .occurrences()
            .get(self.index)
            .ok_or_else(|| internal("a chosen occurrence is missing"))?;
        let selected: BTreeSet<usize> = self.selection.occurrences.iter().copied().collect();
        let target_box = self.target.extent;
        let mut out = Vec::new();
        let mut indices = Vec::new();
        let mut planes = Vec::new();
        for (j, there) in listing.occurrences().iter().enumerate() {
            let (Some(part), Some(elements)) = (
                accepted_part(listing, there.definition),
                self.elements.get(&there.definition),
            ) else {
                continue;
            };
            *pairs += 1;
            if *pairs > MAX_RELATIONS {
                return Ok(None);
            }
            let motion = Motion::between(&there.transform, &here.transform)
                .ok_or_else(|| internal("a placement could not be composed"))?;
            planes.push((motion, part));
            if j == self.index {
                continue;
            }
            let floor = floor_of(here).max(floor_of(there));
            let near = match (&target_box, &part.extent) {
                (Some(t), Some(e)) => {
                    rules::boxes_meet(t, rules::moved_box(e, &motion), rules::reach(floor))
                }
                _ => true,
            };
            let faces = listing
                .definitions()
                .get(there.definition)
                .map_or(&[][..], |d| d.faces.as_slice());
            out.push(Neighbour {
                path: there.path.instances(),
                part,
                faces,
                motion,
                elements: elements
                    .iter()
                    .map(|e| axial::placed(e, &motion, part.extent.as_ref()))
                    .collect(),
                floor,
                selected: selected.contains(&j),
                near,
            });
            indices.push(j);
        }
        Ok(Some((out, indices, planes)))
    }
}

/// U4: one `unresolved` per neighbour definition in reach whose faces or
/// cylinders no rule reads.
fn unread_neighbours(
    listing: &Listing,
    neighbours: &[Neighbour<'_>],
    indices: &[usize],
) -> Vec<Requirement> {
    let mut by_definition = std::collections::BTreeMap::new();
    for (neighbour, &j) in neighbours.iter().zip(indices) {
        let unread = !neighbour.part.other_faces.is_empty()
            || neighbour
                .part
                .cylinders
                .iter()
                .any(|c| matches!(c.class, CylinderClass::Undecided { .. }));
        if !unread || !neighbour.near {
            continue;
        }
        let Some(definition) = listing
            .occurrences()
            .get(j)
            .and_then(|o| listing.definitions().get(o.definition))
            .filter(|d| d.kind == DefinitionKind::Part)
        else {
            continue;
        };
        let mut faces: Vec<u64> = neighbour.part.other_faces.iter().map(|o| o.face).collect();
        faces.extend(
            neighbour
                .part
                .cylinders
                .iter()
                .filter(|c| matches!(c.class, CylinderClass::Undecided { .. }))
                .flat_map(|c| c.faces.iter().copied()),
        );
        faces.sort_unstable();
        by_definition.insert(definition.definition.instance, faces);
    }
    by_definition
        .into_iter()
        .map(|(definition, faces)| {
            combine::unresolved(
                crate::features::FeatureId::new(crate::features::FeatureKind::Solid, definition),
                Some(definition),
                vec![CauseFaces {
                    cause: Cause::NeighbourUnread,
                    faces,
                }],
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const S01: &[u8] = include_bytes!("../../tests/assemblies/s01_bolted.stp");

    /// s01's BRACKET socket edited by `edit` and given the digest of what
    /// it now states, as anyone can: true if `from_json` refuses it as
    /// breaking a rule (9.4).
    fn refused(edit: impl FnOnce(&mut Extracted)) -> bool {
        refused_in("BRACKET", edit)
    }

    /// The same for s01's `part`.
    fn refused_in(part: &str, edit: impl FnOnce(&mut Extracted)) -> bool {
        let Ok(mut document) = SocketDocument::extract(S01, Some(part), None) else {
            return false;
        };
        let Some(extracted) = document.extracted.as_mut() else {
            return false;
        };
        edit(extracted);
        document.digest = Some(document.compute_digest());
        matches!(
            SocketDocument::from_json(&document.to_json()),
            Err(SocketFormatError::Invalid(_))
        )
    }

    /// 9.4: a clearance span's clear diameter is the strictest its sources'
    /// margins give.
    #[test]
    fn a_clearance_looser_than_its_sources_margins_is_refused() {
        // A bore's clear diameter lowered below what its bolt reaches (the
        // value is still a clearance, under the original).
        assert!(refused(|e| {
            for s in &mut e.sockets {
                for r in &mut s.requirements {
                    if let Requires::Bore { spans, .. } = &mut r.requires {
                        for span in spans {
                            span.clear_diameter_mm = span.clear_diameter_mm.map(|c| c - 0.1);
                        }
                    }
                }
            }
        }));
        // A bolt's limit raised past the room its holes leave.
        assert!(!refused_in("BOLT", |_| {}));
        assert!(refused_in("BOLT", |e| {
            for s in &mut e.sockets {
                for r in &mut s.requirements {
                    if let Requires::Shaft { spans, .. } = &mut r.requires {
                        for span in spans {
                            span.clear_diameter_mm = span.clear_diameter_mm.map(|c| c + 0.1);
                        }
                    }
                }
            }
        }));
    }

    #[test]
    fn each_structural_rule_refuses_an_edit_with_its_digest_recomputed() {
        // The document as extracted is not refused.
        assert!(!refused(|_| {}));
        // A3: a hole neither required nor left out.
        assert!(refused(|e| {
            for s in &mut e.sockets {
                s.left_out.clear();
            }
        }));
        // Y1: a floor below its sources'.
        assert!(refused(|e| {
            for s in &mut e.sockets {
                for r in &mut s.requirements {
                    r.floor_mm = 0.0;
                }
            }
        }));
        // N5: requirements out of order.
        assert!(refused(|e| {
            for s in &mut e.sockets {
                s.requirements.reverse();
            }
        }));
        // A tight fit with no label, and unpinned.
        assert!(refused(|e| {
            for s in &mut e.sockets {
                for r in &mut s.requirements {
                    if let Requires::Bore { spans, .. } = &mut r.requires {
                        for span in spans {
                            span.fit = Fit::Interference;
                        }
                    }
                }
            }
        }));
        // Spans that overlap.
        assert!(refused(|e| {
            for s in &mut e.sockets {
                for r in &mut s.requirements {
                    if let Requires::Bore { spans, .. } = &mut r.requires {
                        let copy = spans.clone();
                        spans.extend(copy);
                    }
                }
            }
        }));
        // A seat with nothing on it.
        assert!(refused(|e| {
            for s in &mut e.sockets {
                for r in &mut s.requirements {
                    if let Requires::Seat { contacts, .. } = &mut r.requires {
                        for c in contacts {
                            c.by.clear();
                        }
                    }
                }
            }
        }));
        // A grip naming one seat twice.
        assert!(refused(|e| {
            for s in &mut e.sockets {
                for r in &mut s.requirements {
                    if let Requires::Grip { seats, .. } = &mut r.requires {
                        if let Some(first) = seats.first().copied() {
                            *seats = vec![first, first];
                        }
                    }
                }
            }
        }));
        // Sources out of order.
        assert!(refused(|e| {
            for s in &mut e.sockets {
                for r in &mut s.requirements {
                    r.sources.reverse();
                }
            }
        }));
        // A conflict no source shows.
        assert!(refused(|e| {
            for s in &mut e.sockets {
                for r in &mut s.requirements {
                    r.flags = vec![Flag::Conflict];
                }
            }
        }));
        // A left-out feature with no diameter.
        assert!(refused(|e| {
            for s in &mut e.sockets {
                for l in &mut s.left_out {
                    l.diameter_mm = -1.0;
                }
            }
        }));
        // A socket misnumbered.
        assert!(refused(|e| {
            for s in &mut e.sockets {
                s.id = "socket-2".to_owned();
            }
            for o in &mut e.occurrences {
                o.socket = Some("socket-2".to_owned());
            }
        }));
        // A requirement with no source.
        assert!(refused(|e| {
            for s in &mut e.sockets {
                for r in &mut s.requirements {
                    r.sources.clear();
                }
            }
        }));
    }
}
