//! The `features.v1` document: what `fitment features` prints and the
//! catalog stores (`docs/research/m2-features.md`, section 10).

use serde::{Deserialize, Serialize};

use super::{Part, Refused};
use crate::EXTRACTOR_VERSION;
use crate::gate::Refusal;

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
        identity_mm: 1e-6,
        parallel_sine: 1e-9,
        vertex_side_mm: 1e-3,
        lever_min_mm: 1.0,
        sweep_rad: 1e-9,
        walk_max: 8,
        gate_slack_mm: 1e-6,
        gate_slack_rel: 1e-6,
        range_mm: 1e6,
    };
}

/// What a file's header states. Recorded for every document, never gated.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct FileHeader {
    /// The schema family the first `FILE_SCHEMA` names: `ap203`, `ap214`
    /// or `ap242`, or `None` if it is none of them.
    pub schema: Option<String>,
    /// Every `FILE_SCHEMA` identifier, as written.
    pub schema_identifiers: Vec<String>,
    /// `FILE_NAME.originating_system`.
    pub originating_system: Option<String>,
    /// `FILE_NAME.preprocessor_version`.
    pub preprocessor_version: Option<String>,
    /// `FILE_DESCRIPTION.implementation_level`.
    pub implementation_level: Option<String>,
}

/// Whether a document holds a part or the reasons it was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
// A document is accepted or refused; there is no third outcome.
#[allow(clippy::exhaustive_enums)]
pub enum Status {
    /// The file is a canonical part; `part` holds it.
    Accepted,
    /// The file is not a canonical part; `refusals` says why.
    Refused,
}

/// A features document.
///
/// Its fields serialize in this order, with every key written (absent
/// values as `null`), so that the same input gives the same bytes on every
/// operating system.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct FeaturesDocument {
    /// Always [`FEATURES_FORMAT`].
    pub format: String,
    /// The layout version, [`FEATURES_FORMAT_VERSION`].
    pub format_version: u32,
    /// The [`EXTRACTOR_VERSION`] that wrote the document.
    pub extractor_version: u32,
    /// The constants it extracted with.
    pub extraction: Extraction,
    /// The file's header facts; `None` if the file could not be parsed.
    pub header: Option<FileHeader>,
    /// Accepted or refused.
    pub status: Status,
    /// Why the file was refused, sorted; empty when accepted.
    pub refusals: Vec<Refusal>,
    /// The part; `None` when refused, so that a refused file is never
    /// mistaken for a read one.
    pub part: Option<Part>,
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
    fn envelope(
        header: Option<FileHeader>,
        status: Status,
        refusals: Vec<Refusal>,
        part: Option<Part>,
    ) -> Self {
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

    /// The document for a part that was read.
    pub fn accepted(part: Part) -> Self {
        Self::envelope(
            Some(part.header.clone()),
            Status::Accepted,
            Vec::new(),
            Some(part),
        )
    }

    /// The document for a refused file.
    pub fn refused(refused: Refused) -> Self {
        Self::envelope(
            refused.header.map(|header| *header),
            Status::Refused,
            refused.refusals,
            None,
        )
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
        let mut sorted = raw.refusals.clone();
        sorted.sort();
        if sorted != raw.refusals {
            return Err(FormatError::Invalid("refusals are not sorted".to_owned()));
        }
        let part = match (raw.status, raw.part, raw.refusals.is_empty()) {
            (Status::Accepted, Some(mut part), true) => {
                part.header = raw.header.clone().ok_or_else(|| {
                    FormatError::Invalid("an accepted document has no header".to_owned())
                })?;
                Some(part)
            }
            (Status::Refused, None, false) => None,
            (Status::Accepted, _, _) => {
                return Err(FormatError::Invalid(
                    "an accepted document must hold a part and no refusals".to_owned(),
                ));
            }
            (Status::Refused, _, _) => {
                return Err(FormatError::Invalid(
                    "a refused document must hold refusals and no part".to_owned(),
                ));
            }
        };
        Ok(Self {
            format: raw.format,
            format_version: raw.format_version,
            extractor_version: raw.extractor_version,
            extraction: raw.extraction,
            header: raw.header,
            status: raw.status,
            refusals: raw.refusals,
            part,
        })
    }

    /// True if the document was written by this crate's
    /// [`EXTRACTOR_VERSION`]; the catalog re-reads files whose documents
    /// are not.
    pub fn is_current(&self) -> bool {
        self.extractor_version == EXTRACTOR_VERSION
    }
}

/// The document as read, before its rules are checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDocument {
    format: String,
    format_version: u32,
    extractor_version: u32,
    extraction: Extraction,
    header: Option<FileHeader>,
    status: Status,
    refusals: Vec<Refusal>,
    part: Option<Part>,
}
