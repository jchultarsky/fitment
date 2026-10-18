//! One component of an assembly, read and chosen as the socket extractor
//! chooses its target (`docs/research/m6-api.md`, CO5): a caller names a
//! leaf as `--part` and `--occurrence` name one, or a socket document names
//! its own target, and gets the leaf's part in its own frame with the
//! assembly's SHA-256 and the definition's `#id`, or why it cannot be read.
//! The server judges a component through this alone, so it never selects
//! or reads a part itself (SN1).

use super::{ComponentRead, SelectError, read_assembly};
use crate::features::Part;
use crate::gate::Refusal;
use crate::matching::CandidateSource;
use crate::sha256::sha256_hex;
use crate::socket::SocketDocument;

/// A leaf of an assembly, read as a part (CF1, CO5). The name
/// `ComponentRead` is the listing's own reading of each definition.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SelectedComponent {
    /// The assembly's SHA-256, hex.
    pub assembly_sha256: String,
    /// The definition's `#id`.
    pub definition: u64,
    /// The part, in its own frame.
    pub part: Box<Part>,
}

/// Why no component could be read.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ComponentError {
    /// The selection names no part: a usage error, since the file was
    /// read (T6).
    #[error(transparent)]
    Select(#[from] SelectError),
    /// The assembly gate refuses the assembly, or the definition is no
    /// canonical part: its refusals, sorted. Nothing is judged from a
    /// partial reading.
    #[error("the component cannot be read: {} refusals", .0.len())]
    Refused(Vec<Refusal>),
    /// No definition of the assembly is the one asked for: a socket's
    /// target whose bytes are another file's, or a document that names
    /// none.
    #[error("the assembly holds no such definition")]
    NotFound,
}

impl SelectedComponent {
    /// The component `part` and `occurrence` name in `step`, chosen by the
    /// code `SocketDocument::extract` chooses its target with (section 5),
    /// the assembly gate's refusals kept. Bytes in, values out.
    ///
    /// # Errors
    ///
    /// A [`ComponentError`]: the selection's, or the gate's refusals.
    pub fn read(
        step: &[u8],
        part: Option<&str>,
        occurrence: Option<&str>,
    ) -> Result<Self, ComponentError> {
        let read = read_assembly(step);
        let Some(listing) = read.listing() else {
            return Err(ComponentError::Refused(read.refusals().to_vec()));
        };
        let selection = listing.select(part, occurrence)?;
        if !read.gate_refusals().is_empty() {
            return Err(ComponentError::Refused(read.gate_refusals().to_vec()));
        }
        let listed = listing
            .definitions()
            .get(selection.definition)
            .ok_or(ComponentError::NotFound)?;
        Self::of(step, listed.definition.instance, listed.read.as_ref())
    }

    /// The target `document` was extracted from, read from `step`, its
    /// source (VD15's original): the definition the document names.
    ///
    /// # Errors
    ///
    /// [`ComponentError::NotFound`] when `step` is not the document's
    /// source or the document names no target; the gate's refusals.
    pub fn target(step: &[u8], document: &SocketDocument) -> Result<Self, ComponentError> {
        let instance = document
            .extracted()
            .map(|e| e.target.definition.instance)
            .ok_or(ComponentError::NotFound)?;
        if sha256_hex(step) != document.source().sha256 {
            return Err(ComponentError::NotFound);
        }
        let read = read_assembly(step);
        let Some(listing) = read.listing() else {
            return Err(ComponentError::Refused(read.refusals().to_vec()));
        };
        if !read.gate_refusals().is_empty() {
            return Err(ComponentError::Refused(read.gate_refusals().to_vec()));
        }
        let listed = listing
            .definitions()
            .iter()
            .find(|d| d.definition.instance == instance)
            .ok_or(ComponentError::NotFound)?;
        Self::of(step, instance, listed.read.as_ref())
    }

    fn of(
        step: &[u8],
        definition: u64,
        read: Option<&ComponentRead>,
    ) -> Result<Self, ComponentError> {
        match read {
            Some(ComponentRead::Accepted(part)) => Ok(Self {
                assembly_sha256: sha256_hex(step),
                definition,
                part: part.clone(),
            }),
            Some(ComponentRead::Refused(refusals)) => {
                Err(ComponentError::Refused(refusals.clone()))
            }
            None => Err(ComponentError::NotFound),
        }
    }

    /// Where it comes from (MF6's `source`): the assembly and the
    /// definition.
    pub fn source(&self) -> CandidateSource {
        CandidateSource::component(self.assembly_sha256.clone(), self.definition)
    }
}
