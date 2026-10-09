//! The tolerances a match is judged with.

use serde::{Deserialize, Serialize};

/// A named pair of tolerances, stored with every result so it can be re-run.
///
/// The defaults are decision D9's starting values: 0.01 mm and 0.01°. Both
/// values must be finite and greater than zero; a policy read from a file is
/// checked the same way as one built in code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawPolicy", into = "RawPolicy")]
// Fields are private so that every policy, however it was made, has passed
// `TolerancePolicy::new`. That makes the struct non-exhaustive already.
#[allow(clippy::exhaustive_structs)]
pub struct TolerancePolicy {
    name: String,
    linear_mm: f64,
    angular_deg: f64,
}

/// Why a tolerance policy was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ToleranceError {
    /// The policy has no name.
    #[error("a tolerance policy needs a name")]
    EmptyName,
    /// The linear tolerance is zero, negative, infinite or not a number.
    #[error("linear tolerance must be finite and greater than zero, got {0} mm")]
    Linear(f64),
    /// The angular tolerance is zero, negative, infinite or not a number.
    #[error("angular tolerance must be finite and greater than zero, got {0}°")]
    Angular(f64),
}

impl TolerancePolicy {
    /// The name of the default policy.
    pub const DEFAULT_NAME: &'static str = "default";

    /// Builds a policy, refusing values that cannot be compared against.
    pub fn new(
        name: impl Into<String>,
        linear_mm: f64,
        angular_deg: f64,
    ) -> Result<Self, ToleranceError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(ToleranceError::EmptyName);
        }
        if !(linear_mm.is_finite() && linear_mm > 0.0) {
            return Err(ToleranceError::Linear(linear_mm));
        }
        if !(angular_deg.is_finite() && angular_deg > 0.0) {
            return Err(ToleranceError::Angular(angular_deg));
        }
        Ok(Self {
            name,
            linear_mm,
            angular_deg,
        })
    }

    /// The policy's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The linear tolerance, in millimetres.
    pub fn linear_mm(&self) -> f64 {
        self.linear_mm
    }

    /// The angular tolerance, in degrees.
    pub fn angular_deg(&self) -> f64 {
        self.angular_deg
    }
}

impl Default for TolerancePolicy {
    fn default() -> Self {
        Self {
            name: Self::DEFAULT_NAME.to_owned(),
            linear_mm: 0.01,
            angular_deg: 0.01,
        }
    }
}

/// The policy as written in a file, before it is checked.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPolicy {
    name: String,
    linear_mm: f64,
    angular_deg: f64,
}

impl TryFrom<RawPolicy> for TolerancePolicy {
    type Error = ToleranceError;

    fn try_from(raw: RawPolicy) -> Result<Self, Self::Error> {
        Self::new(raw.name, raw.linear_mm, raw.angular_deg)
    }
}

impl From<TolerancePolicy> for RawPolicy {
    fn from(policy: TolerancePolicy) -> Self {
        Self {
            name: policy.name,
            linear_mm: policy.linear_mm,
            angular_deg: policy.angular_deg,
        }
    }
}
