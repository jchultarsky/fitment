//! The API's documents for assemblies and sockets (docs/research/m6-api.md,
//! section 7.11, RA3 to RA5, DC9): `fitment.assembly`, and the items of the
//! pages of socket lines and revisions. None embeds a result document: they
//! link to `fitment.parts` and `fitment.socket` bytes (DC1). None holds a
//! time or a path (DC9).

use serde::Serialize;
use serde_json::Value;

use crate::state::{AssemblyRow, LineRow, RevisionRow};

/// `fitment.assembly`.
pub const ASSEMBLY_FORMAT: &str = "fitment.assembly";

/// Its version (DC9: may grow within v1).
pub const ASSEMBLY_FORMAT_VERSION: u32 = 1;

/// The kind of a page of socket lines.
pub const LINE_KIND: &str = "socket_line";

/// The kind of a page of socket revisions.
pub const REVISION_KIND: &str = "socket_revision";

/// `fitment.assembly`'s `parts` (RA3).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PartsLink {
    /// `/v1/assemblies/sha256:{hex}/parts`.
    pub href: String,
    /// The versions its latest reading was made with.
    pub extractor_version: Option<u32>,
    #[allow(missing_docs)]
    pub socket_rules_version: Option<u32>,
    /// A reading of this build exists: else `…/parts` is `409
    /// derived_stale` until the assembly is registered again.
    pub current: bool,
}

/// `fitment.assembly`'s `sockets`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SocketsLink {
    /// `/v1/assemblies/sha256:{hex}/sockets`.
    pub href: String,
    /// The caller's live socket lines of it.
    pub count: u64,
}

/// `fitment.assembly` v1 (section 7.11).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AssemblyDocument {
    /// `fitment.assembly`.
    pub format: &'static str,
    /// 1.
    pub format_version: u32,
    /// Its bytes' SHA-256.
    pub sha256: String,
    /// Their length.
    pub bytes: u64,
    /// The names it was registered under (RA8), sorted.
    pub file_names: Vec<String>,
    /// Its reading's status: `accepted` or `refused` (RA3).
    pub status: Option<String>,
    /// Its root definition, as `fitment.parts` states it.
    pub root: Option<Value>,
    /// Its `fitment.parts`.
    pub parts: PartsLink,
    /// Its socket lines.
    pub sockets: SocketsLink,
}

impl AssemblyDocument {
    /// The document of `row`.
    pub fn of(row: &AssemblyRow) -> Self {
        let href = format!("/v1/assemblies/sha256:{}", row.sha256);
        let parts = row.parts.as_ref();
        Self {
            format: ASSEMBLY_FORMAT,
            format_version: ASSEMBLY_FORMAT_VERSION,
            sha256: row.sha256.clone(),
            bytes: row.bytes,
            file_names: row.file_names.clone(),
            status: parts.map(|p| p.status.clone()),
            root: parts
                .and_then(|p| p.root.as_deref())
                .and_then(|r| serde_json::from_str(r).ok()),
            parts: PartsLink {
                href: format!("{href}/parts"),
                extractor_version: parts.map(|p| p.extractor_version),
                socket_rules_version: parts.map(|p| p.socket_rules_version),
                current: parts.is_some_and(current),
            },
            sockets: SocketsLink {
                href: format!("{href}/sockets"),
                count: row.sockets,
            },
        }
    }
}

/// True if `parts` is this build's reading (RA3).
pub fn current(parts: &crate::state::PartsRow) -> bool {
    parts.extractor_version == fitment_core::EXTRACTOR_VERSION
        && parts.socket_rules_version == fitment_core::SOCKET_RULES_VERSION
}

/// The versions a reading of this build records.
pub fn versions() -> (u32, u32) {
    (
        fitment_core::EXTRACTOR_VERSION,
        fitment_core::SOCKET_RULES_VERSION,
    )
}

/// A socket revision, as its line's history lists it (RA5, section 7.6).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RevisionItem {
    /// `sk_…@rN`.
    #[serde(rename = "ref")]
    pub reference: String,
    /// N.
    pub n: i64,
    /// `extract`, `review` or `import`.
    pub how: String,
    /// RA10.
    pub verified: bool,
    /// The revision it follows, `sk_…@rM`.
    pub parent: Option<String>,
    /// Its document's SHA-256 and `digest` (DC5).
    pub sha256: String,
    #[allow(missing_docs)]
    pub digest: String,
    /// Its versions.
    pub extractor_version: u32,
    #[allow(missing_docs)]
    pub socket_rules_version: u32,
    /// Its counts.
    pub unreviewed: u64,
    #[allow(missing_docs)]
    pub carried: u64,
    #[allow(missing_docs)]
    pub stale: u64,
}

impl RevisionItem {
    /// The item of `row`.
    pub fn of(row: &RevisionRow) -> Self {
        Self {
            reference: format!("{}@r{}", row.line_id, row.n),
            n: row.n,
            how: row.how.clone(),
            verified: row.verified,
            parent: row.parent.as_ref().map(|(id, n)| format!("{id}@r{n}")),
            sha256: row.document_sha256.clone(),
            digest: row.document_digest.clone(),
            extractor_version: row.extractor_version,
            socket_rules_version: row.socket_rules_version,
            unreviewed: row.unreviewed,
            carried: row.carried,
            stale: row.stale,
        }
    }
}

/// A socket line, as its lists show it (RA4, section 7.6).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LineItem {
    /// `sk_…`.
    pub id: String,
    /// `sha256:HEX`.
    pub assembly: String,
    /// The target definition, `#<n>`.
    pub target: String,
    /// The occurrence, its `#id`s joined by `/`, or `*` for all.
    pub occurrence: String,
    /// The current revision.
    pub current: Option<RevisionItem>,
}

impl LineItem {
    /// The item of `row`.
    pub fn of(row: &LineRow) -> Self {
        Self {
            id: row.id.clone(),
            assembly: format!("sha256:{}", row.assembly_sha256),
            target: row.definition.clone(),
            occurrence: row.occurrence.clone(),
            current: row.current.as_ref().map(RevisionItem::of),
        }
    }
}
