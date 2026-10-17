//! What a match is judged under (`docs/research/m4-match.md`, M4-7, M4-8,
//! M4-37): D9's tolerances, D6's clearance and D7's grip, recorded in every
//! result.

use serde::Serialize;

use crate::tolerance::TolerancePolicy;

/// D6: how a clearance span's diameter is bounded.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Clearance {
    /// No larger than the original's (the default).
    #[default]
    Strict,
    /// Any larger: only the occupant's clear diameter bounds it.
    Functional,
}

/// D7: how a grip binds. M4 offers `Hard` only (M4-8); M7 adds a warning.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Grip {
    /// A grip is a requirement like any other.
    #[default]
    Hard,
}

/// The policy a match is judged under.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[non_exhaustive]
pub struct MatchPolicy {
    tolerance: TolerancePolicy,
    clearance: Clearance,
    grip: Grip,
}

impl MatchPolicy {
    /// A policy of these parts.
    pub fn new(tolerance: TolerancePolicy, clearance: Clearance, grip: Grip) -> Self {
        Self {
            tolerance,
            clearance,
            grip,
        }
    }

    /// D9's tolerances.
    pub fn tolerance(&self) -> &TolerancePolicy {
        &self.tolerance
    }

    /// D6.
    pub fn clearance(&self) -> Clearance {
        self.clearance
    }

    /// D7.
    pub fn grip(&self) -> Grip {
        self.grip
    }

    /// The linear tolerance t, in mm.
    pub(crate) fn linear_mm(&self) -> f64 {
        self.tolerance.linear_mm()
    }

    /// The angular tolerance, in radians.
    pub(crate) fn angular_rad(&self) -> f64 {
        self.tolerance.angular_deg().to_radians()
    }
}

/// What a run asks beyond the policy.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[non_exhaustive]
pub struct MatchOptions {
    require_fit: bool,
    require_reviewed: bool,
}

impl MatchOptions {
    /// `--require-fit` and `--require-reviewed`.
    pub fn new(require_fit: bool, require_reviewed: bool) -> Self {
        Self {
            require_fit,
            require_reviewed,
        }
    }

    /// An `InterfaceMatch` counts as a reject (MF4).
    pub fn require_fit(&self) -> bool {
        self.require_fit
    }

    /// A socket with unreviewed requirements is refused (MF2).
    pub fn require_reviewed(&self) -> bool {
        self.require_reviewed
    }
}
