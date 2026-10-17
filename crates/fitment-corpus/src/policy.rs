//! The policies the corpus is judged under (`docs/research/m4-match.md`,
//! KC3; `docs/research/m5-catalog.md`, CT1): M4's two, D6's strict and
//! functional clearance at D9's tolerances, and the catalog's wider
//! window, `--tolerance-mm 0.05`. Each as the library builds it, as the
//! CLI's flags ask for it and as the API's request states it (DC7).

use fitment_core::{Clearance, Grip, MatchPolicy, TolerancePolicy};
use serde_json::{Value, json};

/// A policy of the corpus's runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Policy {
    /// D9's tolerances, D6 strict: `default` in the manifest.
    Default,
    /// D9's tolerances, D6 functional.
    Functional,
    /// A linear tolerance of 0.05 mm, D6 strict (CT1, the review's F2).
    Tolerance005,
}

impl Policy {
    /// The two M4 offers, in the report's order (KH6).
    pub const M4: [Policy; 2] = [Policy::Default, Policy::Functional];

    /// Its name in the manifest's expectations and the report.
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Functional => "functional",
            Self::Tolerance005 => "tolerance_mm_0.05",
        }
    }

    /// D6's clearance under it.
    pub fn clearance(self) -> &'static str {
        match self {
            Self::Functional => "functional",
            Self::Default | Self::Tolerance005 => "strict",
        }
    }

    /// The policy as the library builds it, and as `cmd/match_.rs` does
    /// from the same flags (DC7): a tolerance other than D9's is `cli`.
    ///
    /// # Panics
    ///
    /// Never: 0.05 mm is a valid tolerance.
    pub fn match_policy(self) -> MatchPolicy {
        let tolerance = match self {
            Self::Default | Self::Functional => TolerancePolicy::default(),
            Self::Tolerance005 => {
                TolerancePolicy::new("cli", 0.05, TolerancePolicy::default().angular_deg())
                    .expect("0.05 mm is a valid tolerance")
            }
        };
        let clearance = match self {
            Self::Functional => Clearance::Functional,
            Self::Default | Self::Tolerance005 => Clearance::Strict,
        };
        MatchPolicy::new(tolerance, clearance, Grip::Hard)
    }

    /// `fitment match`'s flags for it.
    pub fn cli_args(self) -> &'static [&'static str] {
        match self {
            Self::Default => &["--clearance", "strict"],
            Self::Functional => &["--clearance", "functional"],
            Self::Tolerance005 => &["--clearance", "strict", "--tolerance-mm", "0.05"],
        }
    }

    /// The API's `policy` object for it (DC7).
    pub fn request(self) -> Value {
        match self {
            Self::Tolerance005 => json!({"clearance": "strict", "tolerance_mm": 0.05}),
            _ => json!({"clearance": self.clearance()}),
        }
    }
}
