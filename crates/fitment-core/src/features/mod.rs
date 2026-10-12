//! The features of one part, read from a STEP file.
//!
//! [`read_part`] runs the canonical gate and reads the part.
//! [`FeaturesDocument`] is the `features.v1` JSON that `fitment features`
//! prints and the catalog stores. The design is
//! `docs/research/m2-features.md`; features arrive one kind per pull
//! request, and each one that changes output raises
//! [`EXTRACTOR_VERSION`](crate::EXTRACTOR_VERSION).

mod cylinder;
mod document;
mod full;
mod group;
mod other;
mod outline;
mod plane;
mod primary;
mod readback;
mod stack;
mod step;
pub(crate) use step::header_of;
pub(crate) mod topo;
mod walk;

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::gate::Refusal;
use crate::geom::Rigid;

pub use cylinder::{Axis, Cylinder, CylinderClass, End, Sense, UndecidedReason};
pub use other::{Cone, OtherFace, OtherReason, Torus};
pub use outline::{Outline, OutlineLoop, OutlineProblem, Segment};
pub use plane::{Plane, PlaneFace};
pub use stack::{
    Bottom, Countersink, Hole, HoleKind, Shaft, ShaftKind, StackEnd, StackSegment, UnknownReason,
};

pub use document::{
    Extraction, FEATURES_FORMAT, FEATURES_FORMAT_VERSION, FeaturesDocument, FileHeader,
    FormatError, Status,
};

/// Reads the one part a STEP file holds. Bytes in, values out: no I/O.
/// [`FeaturesDocument::read`] gives the same result with the file's header
/// facts, as `fitment features` prints it.
///
/// # Errors
///
/// Every way the input can fail is a [`Refused`] with its reasons, sorted;
/// nothing panics.
pub fn read_part(step: &[u8]) -> Result<Part, Refused> {
    step::read(step).1.map_err(|refusals| Refused { refusals })
}

/// Why a file was refused: its reasons, never empty.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", summary(refusals))]
// The field is private; `refusals` reads it.
#[allow(clippy::exhaustive_structs)]
pub struct Refused {
    refusals: Vec<Refusal>,
}

impl Refused {
    /// The reasons, sorted by code, then instance, then message.
    pub fn refusals(&self) -> &[Refusal] {
        &self.refusals
    }
}

fn summary(refusals: &[Refusal]) -> String {
    match refusals {
        [] => "refused".to_owned(),
        [only] => format!("refused: {}: {}", only.code.name(), only.message),
        [first, rest @ ..] => format!(
            "refused: {}: {} (and {} more)",
            first.code.name(),
            first.message,
            rest.len()
        ),
    }
}

/// One part, as read from a canonical file.
///
/// Every length is in millimetres. Every value traces to a STEP `#id`.
/// Other crates can read the fields but not build a `Part`: one comes only
/// from [`read_part`] or from a checked [`FeaturesDocument`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Part {
    /// The product definition read as the part.
    pub definition: DefinitionRef,
    /// The frame every feature is stated in.
    pub frame: Frame,
    /// The part's solids. A canonical part has exactly one.
    pub solids: Vec<SolidInfo>,
    /// The largest length uncertainty the solids' contexts state, in mm:
    /// the matcher's tolerance floor.
    pub max_uncertainty_mm: f64,
    /// True if every cylinder is decided and every hole and shaft has two
    /// known ends (section 9.6): information, not a refusal.
    pub complete: bool,
    /// How many faces of each surface kind the solids have.
    pub face_counts: FaceCounts,
    /// The planes, by id. With the other primary features they hold every
    /// face exactly once.
    pub planes: Vec<Plane>,
    /// The cylinders, by id.
    pub cylinders: Vec<Cylinder>,
    /// The cones, by id.
    pub cones: Vec<Cone>,
    /// The tori, by id.
    pub tori: Vec<Torus>,
    /// The faces no rule reads, by id.
    pub other_faces: Vec<OtherFace>,
    /// The holes, by id: compound features over the cylinders.
    pub holes: Vec<Hole>,
    /// The shafts, by id.
    pub shafts: Vec<Shaft>,
    /// Items of the part's shape that are not solids: curve and point
    /// sets, sorted by `#id`.
    pub other_items: Vec<OtherItem>,
    /// What reading the part noticed that is not wrong, by `#id`, then
    /// code.
    pub notes: Vec<Note>,
}

/// How many faces of each surface kind a part has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct FaceCounts {
    /// Faces on planes.
    pub plane: u32,
    /// Faces on cylinders.
    pub cylindrical: u32,
    /// Faces on cones.
    pub conical: u32,
    /// Faces on spheres.
    pub spherical: u32,
    /// Faces on tori.
    pub toroidal: u32,
    /// Faces on surfaces stepq does not read.
    pub other: u32,
    /// All faces.
    pub total: u32,
}

/// Something reading the part noticed that is not wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Note {
    /// What kind of note.
    pub code: NoteCode,
    /// The `#id` it is about.
    pub instance: u64,
    /// The details.
    pub message: String,
}

/// What kind of [`Note`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NoteCode {
    /// A planar face's `face_outer_bound` is not its loop with positive
    /// area, which fitment takes as the outer one (section 5.3).
    StatedOuterBoundDiffers,
    /// Two cylinder faces that share this edge lie on one surface within
    /// the stated uncertainty, but not within 1e-6 mm, so they are two
    /// features (section 5.2).
    NearMiss,
}

/// The product definition a part was read from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DefinitionRef {
    /// The `product_definition`'s `#id`.
    pub instance: u64,
    /// The product it defines, if the file states one.
    #[serde(deserialize_with = "Option::deserialize")]
    pub product: Option<ProductRef>,
}

/// A `product`, as the file states it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ProductRef {
    /// The `product`'s `#id`.
    pub instance: u64,
    /// Its `id`, usually a part number.
    #[serde(deserialize_with = "Option::deserialize")]
    pub id: Option<String>,
    /// Its `name`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub name: Option<String>,
}

/// The frame features are stated in: that of the part's shape
/// representation, the space an assembly placement of the part maps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Frame {
    /// The shape representation's `#id`.
    pub shape_representation: u64,
}

/// A solid of the part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SolidInfo {
    /// `solid-<#id>`.
    pub id: FeatureId,
    /// The solid's `#id`.
    pub instance: u64,
    /// Its upper-cased entity types, joined with `+`.
    pub entity: String,
    /// The representation that lists it.
    pub representation: u64,
    /// The map from the solid's representation into the part's frame, if
    /// a mapped item places it; `None` if its coordinates are the frame's.
    #[serde(deserialize_with = "Option::deserialize")]
    pub transform: Option<Rigid>,
    /// How many shells it has: the outer one, then its voids.
    pub shells: u32,
    /// How many faces it has.
    pub faces: u32,
    /// The units its context states.
    pub units: SolidUnits,
    /// The largest distance of a vertex from a surface or curve it lies on,
    /// as measured, in mm. Values read from vertices cannot be trusted
    /// below the larger of this and the stated uncertainty.
    pub max_vertex_offset_mm: f64,
}

/// The units a solid's context states.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SolidUnits {
    /// The representation context's `#id`.
    pub context: u64,
    /// Its length unit; every length is reported in mm.
    pub length: StatedUnit,
    /// Its plane angle unit; every angle is reported in radians.
    pub plane_angle: StatedUnit,
    /// Its length uncertainties, in mm. At least one.
    pub uncertainty: Vec<StatedUncertainty>,
}

/// A unit as the file states it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct StatedUnit {
    /// The named unit's `#id`.
    pub instance: u64,
    /// Its name as stepq labels it, such as `millimetre` or `inch`.
    pub name: String,
    /// A value in this unit times `factor` is in mm (or radians).
    pub factor: f64,
}

/// An uncertainty as the file states it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct StatedUncertainty {
    /// The `uncertainty_measure_with_unit`'s `#id`.
    pub instance: u64,
    /// Its name as written, such as `distance_accuracy_value`.
    pub name: String,
    /// Its value, in mm.
    pub value_mm: f64,
}

/// An item of the part's shape that is not a solid. The gate lets only
/// curve and point sets through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct OtherItem {
    /// The item's `#id`.
    pub instance: u64,
    /// Its upper-cased entity types, joined with `+`.
    pub entity: String,
    /// The representation that lists it.
    pub representation: u64,
}

/// What kind of feature an id names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum FeatureKind {
    /// A solid.
    Solid,
    /// A plane.
    Plane,
    /// A cylinder.
    Cylinder,
    /// A cone.
    Cone,
    /// A torus.
    Torus,
    /// A face no rule reads.
    Other,
    /// A hole: a stack of hole cylinders.
    Hole,
    /// A shaft: a stack of shaft cylinders.
    Shaft,
}

impl FeatureKind {
    const ALL: [Self; 8] = [
        Self::Solid,
        Self::Plane,
        Self::Cylinder,
        Self::Cone,
        Self::Torus,
        Self::Other,
        Self::Hole,
        Self::Shaft,
    ];

    /// The prefix of the id, such as `plane`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Solid => "solid",
            Self::Plane => "plane",
            Self::Cylinder => "cylinder",
            Self::Cone => "cone",
            Self::Torus => "torus",
            Self::Other => "other",
            Self::Hole => "hole",
            Self::Shaft => "shaft",
        }
    }
}

/// A feature's id, `<kind>-<#id>`, such as `plane-213`: the kind and the
/// smallest STEP `#id` of the feature's faces (for a solid, its own).
///
/// Ids are ordered by number, then kind, never as strings: as strings,
/// `plane-137` would sort before `plane-17`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
// Both fields are needed and fixed by the format; new kinds go in FeatureKind.
#[allow(clippy::exhaustive_structs)]
pub struct FeatureId {
    kind: FeatureKind,
    instance: u64,
}

impl FeatureId {
    /// The id of a feature of `kind` whose smallest `#id` is `instance`.
    pub fn new(kind: FeatureKind, instance: u64) -> Self {
        Self { kind, instance }
    }

    /// The kind of feature.
    pub fn kind(self) -> FeatureKind {
        self.kind
    }

    /// The `#id` in the id.
    pub fn instance(self) -> u64 {
        self.instance
    }
}

impl Ord for FeatureId {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.instance, self.kind).cmp(&(other.instance, other.kind))
    }
}

impl PartialOrd for FeatureId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for FeatureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.kind.name(), self.instance)
    }
}

/// A string that is not a feature id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("'{0}' is not a feature id such as 'plane-213'")]
#[non_exhaustive]
pub struct FeatureIdError(pub String);

impl FromStr for FeatureId {
    type Err = FeatureIdError;

    fn from_str(text: &str) -> Result<Self, FeatureIdError> {
        let error = || FeatureIdError(text.to_owned());
        let (prefix, number) = text.rsplit_once('-').ok_or_else(error)?;
        let kind = FeatureKind::ALL
            .into_iter()
            .find(|kind| kind.name() == prefix)
            .ok_or_else(error)?;
        // Only plain decimal digits: no sign, no leading zero.
        if number.is_empty()
            || !number.bytes().all(|b| b.is_ascii_digit())
            || (number.len() > 1 && number.starts_with('0'))
        {
            return Err(error());
        }
        let instance = number.parse().map_err(|_| error())?;
        Ok(Self::new(kind, instance))
    }
}

impl Serialize for FeatureId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for FeatureId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_ids_round_trip_and_sort_by_number() {
        let id: FeatureId = "plane-213".parse().unwrap();
        assert_eq!(id, FeatureId::new(FeatureKind::Plane, 213));
        assert_eq!(id.to_string(), "plane-213");
        let mut ids = [
            FeatureId::new(FeatureKind::Plane, 137),
            FeatureId::new(FeatureKind::Plane, 17),
        ];
        ids.sort();
        assert_eq!(ids[0].instance(), 17);
    }

    #[test]
    fn malformed_feature_ids_are_refused() {
        for bad in [
            "plane", "plane-", "plane-+1", "plane-01", "slab-3", "-3", "plane-1x",
        ] {
            assert!(bad.parse::<FeatureId>().is_err(), "{bad}");
        }
    }
}
