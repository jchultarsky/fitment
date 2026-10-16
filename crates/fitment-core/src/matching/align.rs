//! Alignment (`docs/research/m4-match.md`, section 7): where a candidate is
//! placed in a socket's frame. A placement maps the candidate's frame into
//! the target's, `p_target = R · p_candidate + t`, and says where it came
//! from (AL12, AL14).

use crate::geom::Rigid;

/// What a placement was built from.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Seed {
    /// The caller gave it (AL14): the construction's, or the identity for
    /// the original.
    Given,
}

/// One placement of a candidate in a socket's frame.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Placement {
    /// Candidate to target.
    pub transform: Rigid,
    /// Where it came from.
    pub seed: Seed,
    /// Its place in the order placements were tried.
    pub index: usize,
}

impl Placement {
    /// A placement the caller gives (AL14).
    pub fn given(transform: Rigid) -> Self {
        Self {
            transform,
            seed: Seed::Given,
            index: 0,
        }
    }
}
