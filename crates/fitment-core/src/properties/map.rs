//! What a stated property means (PR4): a row per evidenced reading, each
//! citing the sample file and record that exercise it. A row with no real
//! sample is not added.

use super::StatedProperty;

/// What a mapped property says something about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Field {
    /// The part's material.
    Material,
    /// A thread.
    Thread,
    /// A surface finish.
    Finish,
}

impl Field {
    /// The name `get` prints and the `fitment.catalog` document holds.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Material => "material",
            Self::Thread => "thread",
            Self::Finish => "finish",
        }
    }
}

/// What a mapped property says about its field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Meaning {
    /// The value designates it, as written.
    Designation,
    /// The model states that none is assigned.
    Unassigned,
}

impl Meaning {
    /// The name `get` prints and the `fitment.catalog` document holds.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Designation => "designation",
            Self::Unassigned => "unassigned",
        }
    }
}

/// A stated property's meaning under a row of the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Mapped {
    /// What it is about.
    pub field: Field,
    /// What it says.
    pub meaning: Meaning,
    /// The sample files and records the row is evidenced by.
    pub citation: &'static str,
}

/// One row: a name, compared trimmed and ASCII case-folded, and a value as
/// written or any value.
#[derive(Debug, Clone, Copy)]
struct Row {
    name: &'static str,
    value: Option<&'static str>,
    field: Field,
    meaning: Meaning,
    citation: &'static str,
}

/// The rows M5 ships (PR4).
const SHIPPED: &[Row] = &[Row {
    name: "materialmissingassignments",
    value: Some("TRUE"),
    field: Field::Material,
    meaning: Meaning::Unassigned,
    citation: "Siemens NX: nist_ctc_05_asme1_ap242-e1.stp #1095, nist_stc_09_asme1_ap242-e3.stp #7270",
}];

/// Properties considered and not mapped, with the reason (PR4).
pub const NOT_MAPPED: &[(&str, &str)] = &[
    (
        "Density",
        "1000.0 in three different parts (nist_ctc_04 #18079, nist_ftc_07, nist_ftc_10): a CAD default, not a material",
    ),
    (
        "MaterialMultipleAssigned",
        "says nothing about which material",
    ),
    (
        "Part Number, Description, CAD_SOURCE, Part File Format",
        "identification",
    ),
    ("MATERIAL_DESIGNATION", "captured, but in no sample yet"),
];

/// The mapping table (PR4), applied when a stored set is shown, so a new
/// row needs no capture again.
#[derive(Debug, Clone)]
pub struct PropertyMap {
    rows: &'static [Row],
}

impl PropertyMap {
    /// The rows this build ships.
    pub fn shipped() -> Self {
        Self { rows: SHIPPED }
    }

    /// What `property` means under the first row that applies, if any.
    pub fn meaning(&self, property: &StatedProperty) -> Option<Mapped> {
        let name = property.name.trim().to_ascii_lowercase();
        self.rows
            .iter()
            .find(|row| row.name == name && row.value.is_none_or(|v| v == property.value))
            .map(|row| Mapped {
                field: row.field,
                meaning: row.meaning,
                citation: row.citation,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Field, Meaning, PropertyMap};
    use crate::properties::StatedProperty;

    #[test]
    fn the_shipped_row_and_nothing_else() {
        let map = PropertyMap::shipped();
        let mapped = map
            .meaning(&StatedProperty::new(
                1095,
                " MaterialMissingAssignments ",
                "TRUE",
            ))
            .unwrap();
        assert_eq!(
            (mapped.field, mapped.meaning),
            (Field::Material, Meaning::Unassigned)
        );
        for (name, value) in [
            ("MaterialMissingAssignments", "FALSE"),
            ("MaterialMissingAssignments", "true"),
            ("MaterialMultipleAssigned", "FALSE"),
            ("Density", "1000.0"),
            ("MATERIAL", "304 SST"),
            ("Part Number", "M6x20"),
        ] {
            assert_eq!(
                map.meaning(&StatedProperty::new(1, name, value)),
                None,
                "{name}"
            );
        }
    }
}
