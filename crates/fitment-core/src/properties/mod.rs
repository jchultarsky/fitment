//! Part-level properties as a STEP file states them (D10;
//! `docs/research/m5-catalog.md`, section 4.13, PR1 to PR7): captured,
//! bounded and versioned, never read for a verdict. What they mean is
//! [`PropertyMap`]'s, and only rows with a real sample behind them.

mod map;
mod step;

use serde::{Deserialize, Serialize};

use crate::features::FormatError;

pub use map::{Field, Mapped, Meaning, NOT_MAPPED, PropertyMap};
pub(crate) use step::of_graph;

/// The version of the capture: it rises whenever what [`StatedProperties::read`]
/// keeps, or how, changes (RX11), so a catalog knows to capture again.
pub const PROPERTIES_VERSION: u32 = 1;

/// At most this many values are kept (PR2): `nist_stc_06` states 462
/// values of 441 properties.
pub const MAX_STATED: usize = 1024;

/// A name or value longer than this, in bytes, is not kept (PR2).
pub const MAX_STATED_BYTES: usize = 4096;

/// The part-level properties a file states, as written (PR2): user-defined
/// properties of the part's own product definition or its shape, and any
/// `MATERIAL_DESIGNATION` of either.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct StatedProperties {
    /// True if a value was left out for PR2's bounds.
    pub truncated: bool,
    /// Each value kept, by its property's `#id`, then in file order.
    pub stated: Vec<StatedProperty>,
}

/// One stated value.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct StatedProperty {
    /// The `#id` of the `property_definition` or `material_designation`.
    pub instance: u64,
    /// The property's name where it has one value (or that value's name
    /// where the property has none); each value's own name where it has
    /// several; `MATERIAL_DESIGNATION` for a material designation.
    pub name: String,
    /// The value as written: a number's source text or a decoded string.
    pub value: String,
    /// The measure type, such as `POSITIVE_RATIO_MEASURE`, if stated.
    pub measure: Option<String>,
    /// The unit's `#id`, as `#18077`, if stated.
    pub unit: Option<String>,
}

impl StatedProperty {
    /// A value, for tests and for constructing a [`StatedProperties`].
    pub fn new(instance: u64, name: &str, value: &str) -> Self {
        Self {
            instance,
            name: name.to_owned(),
            value: value.to_owned(),
            measure: None,
            unit: None,
        }
    }
}

impl StatedProperties {
    /// The part-level properties of a STEP file holding one product
    /// definition; none for a file that does not parse or holds several
    /// (PR1, PR2). Bytes in, values out: no I/O, and nothing panics.
    pub fn read(step: &[u8]) -> Self {
        step::read(step)
    }

    /// PR2's bounds over values found, in their order.
    pub(crate) fn bounded(mut found: Vec<StatedProperty>) -> Self {
        found.sort_by_key(|p| p.instance);
        let fits = |p: &StatedProperty| {
            p.name.len() <= MAX_STATED_BYTES
                && p.value.len() <= MAX_STATED_BYTES
                && p.measure
                    .as_ref()
                    .is_none_or(|m| m.len() <= MAX_STATED_BYTES)
        };
        let before = found.len();
        let mut stated: Vec<StatedProperty> = found.into_iter().filter(fits).collect();
        let mut truncated = stated.len() < before;
        if stated.len() > MAX_STATED {
            stated.truncate(MAX_STATED);
            truncated = true;
        }
        Self { truncated, stated }
    }

    /// The compact JSON a store keeps, the same bytes for the same set.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Reads [`StatedProperties::to_json`]'s text back, checking PR2's
    /// bounds and the order.
    ///
    /// # Errors
    ///
    /// [`FormatError::Json`] if it is not this shape, and
    /// [`FormatError::Invalid`] if it breaks a bound or the order.
    pub fn from_json(text: &str) -> Result<Self, FormatError> {
        let read: Self = serde_json::from_str(text).map_err(|error| FormatError::Json {
            message: error.to_string(),
            line: error.line(),
            column: error.column(),
        })?;
        let invalid = |message: &str| Err(FormatError::Invalid(message.to_owned()));
        if read.stated.len() > MAX_STATED {
            return invalid("more stated properties than are kept");
        }
        let long = |s: &str| s.len() > MAX_STATED_BYTES;
        if read
            .stated
            .iter()
            .any(|p| long(&p.name) || long(&p.value) || p.measure.as_deref().is_some_and(long))
        {
            return invalid("a stated property longer than is kept");
        }
        if !read
            .stated
            .windows(2)
            .all(|w| w[0].instance <= w[1].instance)
        {
            return invalid("stated properties out of order");
        }
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_STATED, MAX_STATED_BYTES, StatedProperties, StatedProperty};

    #[test]
    fn bounds_mark_the_set_truncated() {
        let many: Vec<StatedProperty> = (0..2000)
            .map(|i| StatedProperty::new(i, "p", "1"))
            .collect();
        let kept = StatedProperties::bounded(many);
        assert!(kept.truncated);
        assert_eq!(kept.stated.len(), MAX_STATED);
        assert_eq!(kept.stated[0].instance, 0);

        let long = "x".repeat(MAX_STATED_BYTES + 1);
        let kept = StatedProperties::bounded(vec![
            StatedProperty::new(2, "a", &long),
            StatedProperty::new(1, "b", "2"),
        ]);
        assert!(kept.truncated);
        assert_eq!(kept.stated, vec![StatedProperty::new(1, "b", "2")]);

        let kept = StatedProperties::bounded(vec![StatedProperty::new(1, "b", "2")]);
        assert!(!kept.truncated);
    }

    #[test]
    fn json_reads_back_and_checks_its_bounds() {
        let set = StatedProperties::bounded(vec![
            StatedProperty::new(7270, "MaterialMissingAssignments", "TRUE"),
            StatedProperty::new(18079, "Density", "1000.0"),
        ]);
        let text = set.to_json();
        assert_eq!(
            text,
            r#"{"truncated":false,"stated":[{"instance":7270,"name":"MaterialMissingAssignments","value":"TRUE","measure":null,"unit":null},{"instance":18079,"name":"Density","value":"1000.0","measure":null,"unit":null}]}"#
        );
        assert_eq!(StatedProperties::from_json(&text).unwrap(), set);
        let swapped = text.replace("7270", "99999");
        assert!(StatedProperties::from_json(&swapped).is_err());
        let extra = text.replacen("{\"truncated\"", "{\"x\":1,\"truncated\"", 1);
        assert!(StatedProperties::from_json(&extra).is_err());
        let long = format!(
            r#"{{"truncated":true,"stated":[{{"instance":1,"name":"a","value":"{}","measure":null,"unit":null}}]}}"#,
            "x".repeat(MAX_STATED_BYTES + 1)
        );
        assert!(StatedProperties::from_json(&long).is_err());
    }
}
