//! A candidate as M4 reads it (CF1, CF10): built once, shared by the
//! shortlist, alignment and verification. PR 2 holds its signature and
//! uncertainty; alignment's stacks and planes follow in PR 3.

use super::signature::PartSignature;
use crate::features::Part;

/// What M4 reads of one candidate.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CandidateView {
    signature: PartSignature,
}

impl CandidateView {
    /// The view of `part`.
    pub fn of(part: &Part) -> Self {
        Self {
            signature: PartSignature::of(part),
        }
    }

    /// Its signature.
    pub fn signature(&self) -> &PartSignature {
        &self.signature
    }

    /// `u_c`, its own uncertainty (VR2).
    pub fn uncertainty_mm(&self) -> f64 {
        self.signature.uncertainty_mm()
    }
}
