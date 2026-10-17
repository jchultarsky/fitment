//! The `features.v1` document: what `fitment features` prints and the
//! catalog stores (`docs/research/m2-features.md`, section 10).

use serde::{Deserialize, Serialize};

use super::{FeatureId, FeatureKind, Part, SolidInfo};
use crate::EXTRACTOR_VERSION;
use crate::gate::Refusal;
use crate::properties::StatedProperties;

/// The `format` of every features document.
pub const FEATURES_FORMAT: &str = "fitment.features";

/// The document layout this crate writes and reads. It is frozen at the
/// first publish; before then it changes together with
/// [`EXTRACTOR_VERSION`].
pub const FEATURES_FORMAT_VERSION: u32 = 1;

/// The extraction constants that decide which faces the file states as one
/// surface (section 5.2 of the design). They are written into every
/// document; changing one raises [`EXTRACTOR_VERSION`]. They are not the
/// matching tolerance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Extraction {
    /// "The file states the same value twice", in mm.
    pub identity_mm: f64,
    /// Plane normals in one direction class.
    pub parallel_sine: f64,
    /// Which side of the stations a vertex lies on, in mm.
    pub vertex_side_mm: f64,
    /// The smallest lever arm for an angular test, in mm.
    pub lever_min_mm: f64,
    /// How close a station's summed sweep must be to 2π, in rad.
    pub sweep_rad: f64,
    /// The most transition faces one end walk crosses.
    pub walk_max: u32,
    /// The gate's floor for a vertex lying on its surface or curve, in mm.
    pub gate_slack_mm: f64,
    /// The gate's relative slack on that bound.
    pub gate_slack_rel: f64,
    /// The largest coordinate magnitude read, in mm.
    pub range_mm: f64,
}

impl Extraction {
    /// The constants this version extracts with.
    pub const CURRENT: Self = Self {
        identity_mm: crate::gate::geometry::IDENTITY_MM,
        parallel_sine: super::group::PARALLEL_SINE,
        vertex_side_mm: super::full::VERTEX_SIDE_MM,
        lever_min_mm: super::group::LEVER_MIN_MM,
        sweep_rad: super::full::SWEEP_RAD,
        walk_max: super::walk::WALK_MAX,
        gate_slack_mm: crate::gate::geometry::GATE_SLACK_MM,
        gate_slack_rel: crate::gate::geometry::GATE_SLACK_REL,
        range_mm: crate::gate::geometry::RANGE_MM,
    };
}

/// What a file's header states. Recorded for every document, never gated.
///
/// Every key is written, with `null` for a value the file does not state,
/// and reading a document requires every key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct FileHeader {
    /// The schema family the first `FILE_SCHEMA` names: `ap203`, `ap214`
    /// or `ap242`, or `None` if it is none of them.
    #[serde(deserialize_with = "Option::deserialize")]
    pub schema: Option<String>,
    /// The `FILE_SCHEMA` identifiers, as stepq reads them: blank ones are
    /// left out.
    pub schema_identifiers: Vec<String>,
    /// `FILE_NAME.originating_system`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub originating_system: Option<String>,
    /// `FILE_NAME.preprocessor_version`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub preprocessor_version: Option<String>,
    /// `FILE_DESCRIPTION.implementation_level`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub implementation_level: Option<String>,
}

/// Whether a document holds a part or the reasons it was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
// A document is accepted or refused; there is no third outcome.
#[allow(clippy::exhaustive_enums)]
pub enum Status {
    /// The file is a canonical part; [`FeaturesDocument::part`] holds it.
    Accepted,
    /// The file is not a canonical part; [`FeaturesDocument::refusals`]
    /// says why.
    Refused,
}

/// A features document: a part, or the reasons a file was refused.
///
/// A document comes only from [`read`](Self::read) or
/// [`from_json`](Self::from_json), and its fields are read through
/// methods, so every document this crate writes follows the rules
/// `from_json` checks. Its JSON keys are in a fixed order, every one is
/// written (absent values as `null`), and the same input gives the same
/// bytes on every operating system.
#[derive(Debug, Clone, PartialEq, Serialize)]
// The fields are private; the methods below read them.
#[allow(clippy::exhaustive_structs)]
pub struct FeaturesDocument {
    format: String,
    format_version: u32,
    extractor_version: u32,
    extraction: Extraction,
    header: Option<FileHeader>,
    status: Status,
    refusals: Vec<Refusal>,
    part: Option<Part>,
}

/// Why a features document could not be read back.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FormatError {
    /// The text is not JSON, or not this document's shape.
    #[error("not a features document: {message} (line {line}, column {column})")]
    Json {
        /// What `serde_json` reported.
        message: String,
        /// 1-based line.
        line: usize,
        /// 1-based column.
        column: usize,
    },
    /// The document has another `format`.
    #[error("format is '{found}', not '{FEATURES_FORMAT}'")]
    Format {
        /// The format found.
        found: String,
    },
    /// The document has a layout version this crate does not read.
    #[error("format version {found} is not {FEATURES_FORMAT_VERSION}")]
    Version {
        /// The version found.
        found: u32,
    },
    /// The document breaks one of its own rules.
    #[error("invalid features document: {0}")]
    Invalid(String),
}

impl FeaturesDocument {
    /// Reads one part from a STEP file: the document for it, or for the
    /// reasons it was refused. Bytes in, values out: no I/O, and nothing
    /// panics.
    pub fn read(step: &[u8]) -> Self {
        let (header, result) = super::step::read(step);
        Self::of(header, result)
    }

    /// The document of a read.
    fn of(header: Option<FileHeader>, result: Result<Part, Vec<Refusal>>) -> Self {
        let (status, refusals, part) = match result {
            Ok(part) => (Status::Accepted, Vec::new(), Some(part)),
            Err(refusals) => (Status::Refused, refusals, None),
        };
        Self {
            format: FEATURES_FORMAT.to_owned(),
            format_version: FEATURES_FORMAT_VERSION,
            extractor_version: EXTRACTOR_VERSION,
            extraction: Extraction::CURRENT,
            header,
            status,
            refusals,
            part,
        }
    }

    /// [`FeaturesDocument::read`] and [`StatedProperties::read`] from one
    /// parse of `step`, as a catalog's ingest reads both (PR1).
    pub fn read_with_properties(step: &[u8]) -> (Self, StatedProperties) {
        let (header, result, stated) = super::step::read_with(step, true);
        (Self::of(header, result), stated)
    }

    /// Always [`FEATURES_FORMAT`].
    pub fn format(&self) -> &str {
        &self.format
    }

    /// The layout version, [`FEATURES_FORMAT_VERSION`].
    pub fn format_version(&self) -> u32 {
        self.format_version
    }

    /// The [`EXTRACTOR_VERSION`] that wrote the document.
    pub fn extractor_version(&self) -> u32 {
        self.extractor_version
    }

    /// The constants it extracted with.
    pub fn extraction(&self) -> &Extraction {
        &self.extraction
    }

    /// The file's header facts; `None` if the file could not be parsed.
    pub fn header(&self) -> Option<&FileHeader> {
        self.header.as_ref()
    }

    /// Accepted or refused.
    pub fn status(&self) -> Status {
        self.status
    }

    /// Why the file was refused, sorted by code, then instance, then
    /// message; empty when accepted.
    pub fn refusals(&self) -> &[Refusal] {
        &self.refusals
    }

    /// The part; `None` when refused, so that a refused file is never
    /// mistaken for a read one.
    pub fn part(&self) -> Option<&Part> {
        self.part.as_ref()
    }

    /// The document as pretty JSON, ending with a newline.
    pub fn to_json(&self) -> String {
        // Serializing these types cannot fail: every map key is a string
        // and every float is finite by construction.
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }

    /// The document as compact JSON: the catalog's stored form (M5's GT2).
    /// It reads back with [`FeaturesDocument::from_json`] to an equal
    /// document, every float exactly (`float_roundtrip`).
    pub fn to_compact_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The SHA-256 (hex) of the compact JSON of the part alone, without the
    /// header, the source or the versions; `None` for a refused document.
    /// Two files whose parts read the same have one digest, so a catalog
    /// judges a part once and reuses the result for each (M5's QR6).
    pub fn part_digest(&self) -> Option<String> {
        let part = self.part.as_ref()?;
        let text = serde_json::to_string(part).ok()?;
        Some(crate::sha256_hex(text.as_bytes()))
    }

    /// Reads a document back, checking its format, version and rules.
    ///
    /// # Errors
    ///
    /// A [`FormatError`] if the text is not a valid features document.
    pub fn from_json(text: &str) -> Result<Self, FormatError> {
        let raw: RawDocument = serde_json::from_str(text).map_err(|error| FormatError::Json {
            message: error.to_string(),
            line: error.line(),
            column: error.column(),
        })?;
        if raw.format != FEATURES_FORMAT {
            return Err(FormatError::Format { found: raw.format });
        }
        if raw.format_version != FEATURES_FORMAT_VERSION {
            return Err(FormatError::Version {
                found: raw.format_version,
            });
        }
        let invalid = |message: &str| Err(FormatError::Invalid(message.to_owned()));
        if !raw.refusals.windows(2).all(|pair| pair[0] < pair[1]) {
            return invalid("refusals are not sorted, or repeat");
        }
        match (raw.status, &raw.part, raw.refusals.is_empty()) {
            (Status::Accepted, Some(part), true) => {
                if raw.header.is_none() {
                    return invalid("an accepted document has no header");
                }
                check_part(part)?;
            }
            (Status::Refused, None, false) => {}
            (Status::Accepted, _, _) => {
                return invalid("an accepted document must hold a part and no refusals");
            }
            (Status::Refused, _, _) => {
                return invalid("a refused document must hold refusals and no part");
            }
        }
        Ok(Self {
            format: raw.format,
            format_version: raw.format_version,
            extractor_version: raw.extractor_version,
            extraction: raw.extraction,
            header: raw.header,
            status: raw.status,
            refusals: raw.refusals,
            part: raw.part,
        })
    }

    /// True if the document was written by this crate's
    /// [`EXTRACTOR_VERSION`]; the catalog re-reads files whose documents
    /// are not.
    pub fn is_current(&self) -> bool {
        self.extractor_version == EXTRACTOR_VERSION
    }
}

/// The rules a part read back must keep, beyond its shape.
fn check_part(part: &Part) -> Result<(), FormatError> {
    let invalid = |message: String| Err(FormatError::Invalid(message));
    let [solid] = part.solids.as_slice() else {
        return invalid(format!("a part has one solid, not {}", part.solids.len()));
    };
    if solid.id.kind() != FeatureKind::Solid || solid.id.instance() != solid.instance {
        return invalid(format!(
            "solid #{} has the id '{}', not 'solid-{}'",
            solid.instance, solid.id, solid.instance
        ));
    }
    check_solid(solid)?;
    check_partition(part, solid)?;
    super::readback::check(part, solid).map_err(FormatError::Invalid)?;
    let largest = solid
        .units
        .uncertainty
        .iter()
        .map(|u| u.value_mm)
        .fold(0.0, f64::max);
    if part.max_uncertainty_mm.to_bits() != largest.to_bits() {
        return invalid(format!(
            "max_uncertainty_mm is {}, not the largest stated uncertainty, {largest}",
            part.max_uncertainty_mm
        ));
    }
    if !part
        .other_items
        .windows(2)
        .all(|pair| pair[0].instance < pair[1].instance)
    {
        return invalid("other items are not sorted by #id, or repeat".to_owned());
    }
    Ok(())
}

/// A feature's id and its faces' `#id`s.
type Faces = (FeatureId, Vec<u64>);

/// The primary features read back must hold every face of the solid
/// exactly once; each list is sorted by id, each id names its kind and its
/// smallest face, and each feature's faces are sorted.
fn check_partition(part: &Part, solid: &SolidInfo) -> Result<(), FormatError> {
    let lists: [(FeatureKind, Vec<Faces>); 5] = [
        (
            FeatureKind::Plane,
            part.planes
                .iter()
                .map(|p| (p.id, p.faces.iter().map(|f| f.face).collect()))
                .collect(),
        ),
        (
            FeatureKind::Cylinder,
            part.cylinders
                .iter()
                .map(|c| (c.id, c.faces.clone()))
                .collect(),
        ),
        (
            FeatureKind::Cone,
            part.cones.iter().map(|c| (c.id, c.faces.clone())).collect(),
        ),
        (
            FeatureKind::Torus,
            part.tori.iter().map(|t| (t.id, t.faces.clone())).collect(),
        ),
        (
            FeatureKind::Other,
            part.other_faces
                .iter()
                .map(|o| (o.id, vec![o.face]))
                .collect(),
        ),
    ];
    let invalid = |message: String| Err(FormatError::Invalid(message));
    let mut all = Vec::new();
    for (kind, features) in &lists {
        if !features.windows(2).all(|pair| pair[0].0 < pair[1].0) {
            return invalid(format!("the {} features are not sorted by id", kind.name()));
        }
        for (id, faces) in features {
            let sorted = faces.windows(2).all(|pair| pair[0] < pair[1]);
            if id.kind() != *kind || faces.first() != Some(&id.instance()) || !sorted {
                return invalid(format!(
                    "feature '{id}' is not a {} named after its smallest face, with its faces sorted",
                    kind.name()
                ));
            }
            all.extend(faces.iter().copied());
        }
    }
    let count = all.len();
    all.sort_unstable();
    all.dedup();
    let total = part.face_counts.total;
    if all.len() != count || u32::try_from(count).ok() != Some(total) || total != solid.faces {
        return invalid(format!(
            "the primary features hold {count} faces, {} distinct, of the solid's {}",
            all.len(),
            solid.faces
        ));
    }
    Ok(())
}

/// The rules a solid read back must keep: what the gate established.
fn check_solid(solid: &SolidInfo) -> Result<(), FormatError> {
    let positive = |x: f64| x.is_finite() && x > 0.0;
    let units = &solid.units;
    let problem = if solid.shells == 0 || solid.faces == 0 {
        Some("a solid has at least one shell and one face")
    } else if !positive(units.length.factor) || !positive(units.plane_angle.factor) {
        Some("a unit's factor is positive")
    } else if units.uncertainty.is_empty() {
        Some("a solid states at least one uncertainty")
    } else if !units.uncertainty.iter().all(|u| positive(u.value_mm)) {
        Some("an uncertainty is positive")
    } else if !(solid.max_vertex_offset_mm.is_finite() && solid.max_vertex_offset_mm >= 0.0) {
        Some("a vertex offset is a distance")
    } else {
        None
    };
    match problem {
        Some(rule) => Err(FormatError::Invalid(format!(
            "solid #{} breaks a rule: {rule}",
            solid.instance
        ))),
        None => Ok(()),
    }
}

/// The document as read, before its rules are checked. Every key is
/// required, `null` ones included.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDocument {
    format: String,
    format_version: u32,
    extractor_version: u32,
    extraction: Extraction,
    #[serde(deserialize_with = "Option::deserialize")]
    header: Option<FileHeader>,
    status: Status,
    refusals: Vec<Refusal>,
    #[serde(deserialize_with = "Option::deserialize")]
    part: Option<Part>,
}
