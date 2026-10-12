//! The canonical gate (decision D2): the checks a STEP file must pass
//! before any feature is read from it, and the reason for each refusal.
//!
//! The checklist is `docs/research/m2-features.md`, section 4. Checks run
//! in stages; a stage that refuses skips the later ones, which need what it
//! establishes, so that one fault does not cascade into many. Within a
//! stage every check runs, so a file can carry several reasons.
//!
//! This file and `features/step.rs` are the only ones that name stepq.

pub(crate) mod solid;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use stepq::geometry::Rigid;
use stepq::info::Info;
use stepq::model::{DefinitionShape, Graph, NotFollowedReason, ProductStructure, ShapeItem};

/// Why a file is not canonical.
///
/// The machine names are the `snake_case` of the variants, for example
/// `several_solids`. They never change meaning: a new check gets a new
/// code. The variants are in the order the checks run, which is the order
/// refusals are reported in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RefusalCode {
    /// The bytes are not ISO 10303-21.
    Unparseable,
    /// An instance refers to an `#id` that is not defined.
    DanglingReference,
    /// The file does not have exactly one data section.
    SeveralDataSections,
    /// The file does not name AP203, AP214 or AP242.
    SchemaUnrecognized,
    /// The file is an assembly; `features` reads one part.
    Assembly,
    /// The file holds no product definition.
    NoDefinition,
    /// The file holds more than one product definition and no assembly.
    SeveralDefinitions,
    /// A link that may hold more of the part's shape was not followed.
    ShapeIncomplete,
    /// The part's shape holds no B-rep solid.
    NoSolid,
    /// The part's shape holds more than one solid.
    SeveralSolids,
    /// The part's shape holds something other than the solid and curve or
    /// point sets: a surface model, a tessellated item, an unknown item.
    UnreadShapeItem,
    /// The map that places the solid in the part's frame cannot be read.
    SolidPlacementUnreadable,
    /// The solid's topology is valid STEP that is not read.
    TopologyUnsupported,
    /// The solid's topology contradicts the schema.
    TopologyMalformed,
    /// An edge of the solid is not used exactly twice, in opposite
    /// directions.
    SolidNotClosed,
    /// A shell of the solid is not one connected piece.
    ShellDisconnected,
    /// The solid's contexts or their units cannot be read.
    UnitsUnreadable,
    /// The length unit cannot be read.
    LengthUnitUnreadable,
    /// The plane angle unit cannot be read.
    AngleUnitUnreadable,
    /// An uncertainty cannot be read.
    UncertaintyUnreadable,
    /// No length uncertainty is stated.
    UncertaintyMissing,
    /// A supported surface, curve or point cannot be read.
    GeometryUnreadable,
    /// A vertex does not lie on its face's surface.
    VertexOffSurface,
    /// An edge's end vertex does not lie on its curve.
    VertexOffCurve,
    /// A face's stated sense disagrees with the direction its loops run.
    FaceOrientation,
    /// A value is not finite or too large to be read reliably.
    OutOfRange,
    /// The file states two things that cannot both be true.
    Contradiction,
    /// Extraction found its own invariant broken: a fitment bug, not a
    /// property of the input.
    Internal,
}

impl RefusalCode {
    /// The machine name, as written in JSON.
    pub fn name(self) -> &'static str {
        match self {
            Self::Unparseable => "unparseable",
            Self::DanglingReference => "dangling_reference",
            Self::SeveralDataSections => "several_data_sections",
            Self::SchemaUnrecognized => "schema_unrecognized",
            Self::Assembly => "assembly",
            Self::NoDefinition => "no_definition",
            Self::SeveralDefinitions => "several_definitions",
            Self::ShapeIncomplete => "shape_incomplete",
            Self::NoSolid => "no_solid",
            Self::SeveralSolids => "several_solids",
            Self::UnreadShapeItem => "unread_shape_item",
            Self::SolidPlacementUnreadable => "solid_placement_unreadable",
            Self::TopologyUnsupported => "topology_unsupported",
            Self::TopologyMalformed => "topology_malformed",
            Self::SolidNotClosed => "solid_not_closed",
            Self::ShellDisconnected => "shell_disconnected",
            Self::UnitsUnreadable => "units_unreadable",
            Self::LengthUnitUnreadable => "length_unit_unreadable",
            Self::AngleUnitUnreadable => "angle_unit_unreadable",
            Self::UncertaintyUnreadable => "uncertainty_unreadable",
            Self::UncertaintyMissing => "uncertainty_missing",
            Self::GeometryUnreadable => "geometry_unreadable",
            Self::VertexOffSurface => "vertex_off_surface",
            Self::VertexOffCurve => "vertex_off_curve",
            Self::FaceOrientation => "face_orientation",
            Self::OutOfRange => "out_of_range",
            Self::Contradiction => "contradiction",
            Self::Internal => "internal",
        }
    }
}

/// One reason a file is refused.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Refusal {
    /// What failed.
    pub code: RefusalCode,
    /// The innermost `#id` at fault, if there is one.
    #[serde(deserialize_with = "Option::deserialize")]
    pub instance: Option<u64>,
    /// For people: what failed, then any detail stepq gave. It may change
    /// between versions; the code does not.
    pub message: String,
}

impl Refusal {
    pub(crate) fn new(
        code: RefusalCode,
        instance: Option<u64>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            instance,
            message: message.into(),
        }
    }
}

/// At most this many refusals of one code are listed; the last one says
/// how many more there were. A crafted file cannot make the list huge.
const LISTED_PER_CODE: usize = 32;

/// Sorts refusals and caps each code's list at [`LISTED_PER_CODE`].
pub(crate) fn finish(mut refusals: Vec<Refusal>) -> Vec<Refusal> {
    refusals.sort();
    refusals.dedup();
    let mut counts: BTreeMap<RefusalCode, usize> = BTreeMap::new();
    for refusal in &refusals {
        *counts.entry(refusal.code).or_default() += 1;
    }
    let mut kept = Vec::with_capacity(refusals.len());
    let mut seen: BTreeMap<RefusalCode, usize> = BTreeMap::new();
    for refusal in refusals {
        let n = seen.entry(refusal.code).or_default();
        *n += 1;
        if *n < LISTED_PER_CODE {
            kept.push(refusal);
        } else if *n == LISTED_PER_CODE {
            let total = counts.get(&refusal.code).copied().unwrap_or(0);
            if total > LISTED_PER_CODE {
                kept.push(Refusal::new(
                    refusal.code,
                    None,
                    format!("and {} more", total - (LISTED_PER_CODE - 1)),
                ));
            } else {
                kept.push(refusal);
            }
        }
    }
    // The summary has no instance, so it sorts first within its code.
    kept.sort();
    kept
}

/// The schema families a canonical file may name, by the name before `{`.
const SCHEMAS: &[(&str, &str)] = &[
    ("CONFIG_CONTROL_DESIGN", "ap203"),
    (
        "AP203_CONFIGURATION_CONTROLLED_3D_DESIGN_OF_MECHANICAL_PARTS_AND_ASSEMBLIES_MIM_LF",
        "ap203",
    ),
    ("AUTOMOTIVE_DESIGN", "ap214"),
    ("AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF", "ap242"),
];

/// A schema that may follow the first one.
const EXTRA_SCHEMA: &str = "GEOMETRIC_VALIDATION_PROPERTIES_MIM";

/// The name of a `FILE_SCHEMA` identifier: the text before `{`, trimmed and
/// upper-cased.
fn schema_name(identifier: &str) -> String {
    identifier
        .split('{')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase()
}

/// The schema family (`ap203`, `ap214`, `ap242`) the first identifier names.
pub(crate) fn schema_family(schemas: &[String]) -> Option<&'static str> {
    let first = schemas.first()?;
    let name = schema_name(first);
    SCHEMAS
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, family)| *family)
}

/// Stage 1: the file as a whole. `graph` is built with
/// [`Graph::build`], so dangling references are counted, not fatal.
pub(crate) fn file(graph: &Graph<'_>, info: &Info) -> Vec<Refusal> {
    let mut refusals = Vec::new();
    // Only the first few become messages: a crafted file can hold millions
    // of dangling references, and the pairs are cheap where strings are not.
    let mut dangling: Vec<(u64, u64)> = graph
        .unresolved()
        .iter()
        .map(|&(node, missing)| (graph.instance(node).id, missing))
        .collect();
    dangling.sort_unstable();
    dangling.dedup();
    let listed = if dangling.len() > LISTED_PER_CODE {
        LISTED_PER_CODE - 1
    } else {
        dangling.len()
    };
    for &(from, missing) in dangling.iter().take(listed) {
        refusals.push(Refusal::new(
            RefusalCode::DanglingReference,
            Some(from),
            format!("#{from} refers to #{missing}, which is not defined"),
        ));
    }
    if dangling.len() > listed {
        refusals.push(Refusal::new(
            RefusalCode::DanglingReference,
            None,
            format!("and {} more", dangling.len() - listed),
        ));
    }
    if info.sections != 1 {
        refusals.push(Refusal::new(
            RefusalCode::SeveralDataSections,
            None,
            format!(
                "the file has {} data sections; a canonical part has exactly one",
                info.sections
            ),
        ));
    }
    let schemas = &info.header.schemas;
    if schema_family(schemas).is_none() {
        let named = schemas
            .first()
            .map_or_else(|| "none".to_owned(), |s| format!("'{s}'"));
        refusals.push(Refusal::new(
            RefusalCode::SchemaUnrecognized,
            None,
            format!("the file schema is {named}; a canonical part names AP203, AP214 or AP242"),
        ));
    }
    for extra in schemas.iter().skip(1) {
        if schema_name(extra) != EXTRA_SCHEMA {
            refusals.push(Refusal::new(
                RefusalCode::SchemaUnrecognized,
                None,
                format!(
                    "the file also names schema '{extra}'; only {EXTRA_SCHEMA} may follow the first"
                ),
            ));
        }
    }
    refusals
}

/// Stage 2: the product structure. At M2 a file is one part.
pub(crate) fn structure(structure: &ProductStructure) -> Vec<Refusal> {
    let mut refusals = Vec::new();
    let usages = structure.usages();
    if !usages.is_empty() {
        refusals.push(Refusal::new(
            RefusalCode::Assembly,
            None,
            format!(
                "the file is an assembly with {} usages of {} product definitions; features reads one part",
                usages.len(),
                structure.definitions().len()
            ),
        ));
    }
    let definitions = structure.definitions();
    match definitions.len() {
        0 => refusals.push(Refusal::new(
            RefusalCode::NoDefinition,
            None,
            "the file holds no product definition",
        )),
        1 => {}
        n if usages.is_empty() => {
            let listed: Vec<String> = definitions
                .iter()
                .take(8)
                .map(|definition| format!("#{}", definition.instance))
                .collect();
            refusals.push(Refusal::new(
                RefusalCode::SeveralDefinitions,
                None,
                format!(
                    "the file holds {n} product definitions ({}{}) and no assembly; a canonical part holds one",
                    listed.join(", "),
                    if n > 8 { ", ..." } else { "" }
                ),
            ));
        }
        _ => {}
    }
    refusals
}

/// Why a link was not followed, in words.
fn not_followed_reason(reason: &NotFollowedReason) -> String {
    match reason {
        NotFollowedReason::External {
            document: Some(document),
        } => {
            format!("the shape is in another file (document #{document})")
        }
        NotFollowedReason::External { document: None } => "the shape is in another file".to_owned(),
        NotFollowedReason::TransformedRelationship => {
            "a representation relationship with a transformation that no assembly usage names"
                .to_owned()
        }
        NotFollowedReason::NotShapeRelationship => {
            "a representation relationship that is not a plain shape_representation_relationship"
                .to_owned()
        }
        NotFollowedReason::NotPlainMappedItem => {
            "an item that is a mapped_item and more".to_owned()
        }
        NotFollowedReason::DifferentContext { contexts } => format!(
            "a relationship between representations in two contexts, #{} and #{}",
            contexts[0], contexts[1]
        ),
        NotFollowedReason::OtherDefinition { .. } => {
            "a representation another product definition reaches too".to_owned()
        }
        NotFollowedReason::UnexpectedTarget { expected } => {
            format!("a link whose far side is not {expected}")
        }
        NotFollowedReason::Cycle => "a mapped item that leads back to itself".to_owned(),
        NotFollowedReason::TooLarge => "a shape too large or deep to read".to_owned(),
        NotFollowedReason::Malformed => {
            "a reference that is undefined or of the wrong type".to_owned()
        }
        _ => "a link stepq does not follow".to_owned(),
    }
}

/// Entities a curve or point set may hold: points and curves, simple, or
/// the partials of a complex curve such as a rational B-spline.
const SET_ELEMENTS: &[&str] = &[
    "CARTESIAN_POINT",
    "POINT_ON_CURVE",
    "POINT_ON_SURFACE",
    "LINE",
    "CIRCLE",
    "ELLIPSE",
    "HYPERBOLA",
    "PARABOLA",
    "POLYLINE",
    "TRIMMED_CURVE",
    "COMPOSITE_CURVE",
    "COMPOSITE_CURVE_ON_SURFACE",
    "B_SPLINE_CURVE",
    "B_SPLINE_CURVE_WITH_KNOTS",
    "BEZIER_CURVE",
    "UNIFORM_CURVE",
    "QUASI_UNIFORM_CURVE",
    "RATIONAL_B_SPLINE_CURVE",
    "OFFSET_CURVE_3D",
    "SURFACE_CURVE",
    "SEAM_CURVE",
    "INTERSECTION_CURVE",
    "PCURVE",
];

/// Supertypes that appear only as partials of a complex point or curve.
const SET_ELEMENT_PARTIALS: &[&str] = &[
    "BOUNDED_CURVE",
    "CURVE",
    "POINT",
    "GEOMETRIC_REPRESENTATION_ITEM",
    "REPRESENTATION_ITEM",
];

/// The upper-cased entity names of instance `id`, one per partial.
fn entity_names(graph: &Graph<'_>, id: u64) -> Option<Vec<String>> {
    let node = graph.node(id)?;
    let exchange = graph.exchange();
    Some(
        exchange
            .records(graph.instance(node))
            .map(|record| String::from_utf8_lossy(record.name()).to_ascii_uppercase())
            .collect(),
    )
}

/// True if `id` is a point or a curve.
fn is_point_or_curve(graph: &Graph<'_>, id: u64) -> bool {
    let Some(names) = entity_names(graph, id) else {
        return false;
    };
    names
        .iter()
        .any(|name| SET_ELEMENTS.contains(&name.as_str()))
        && names.iter().all(|name| {
            SET_ELEMENTS.contains(&name.as_str()) || SET_ELEMENT_PARTIALS.contains(&name.as_str())
        })
}

/// Why a curve or point set is not one.
enum SetProblem {
    /// It holds `#id`, which is not a point or a curve.
    Element(u64),
    /// Its elements cannot be read.
    Unreadable,
}

/// What, if anything, keeps a `geometric_set` or `geometric_curve_set`
/// from being a set of points and curves only.
fn set_problem(graph: &Graph<'_>, set: u64) -> Option<SetProblem> {
    let Some(node) = graph.node(set) else {
        return Some(SetProblem::Unreadable);
    };
    let exchange = graph.exchange();
    let mut records = exchange.records(graph.instance(node));
    let Some(elements) = records
        .next()
        .and_then(|record| record.param(1))
        .and_then(|param| param.list())
    else {
        return Some(SetProblem::Unreadable);
    };
    for element in elements {
        match element.reference() {
            Some(id) if is_point_or_curve(graph, id) => {}
            Some(id) => return Some(SetProblem::Element(id)),
            None => return Some(SetProblem::Unreadable),
        }
    }
    None
}

/// Check 11: every item of the shape other than the solid is a curve or
/// point set.
fn other_items(graph: &Graph<'_>, shape: &DefinitionShape) -> Vec<Refusal> {
    let mut others: BTreeMap<u64, &ShapeItem> = BTreeMap::new();
    for item in &shape.other {
        others.entry(item.item).or_insert(item);
    }
    let mut refusals = Vec::new();
    for (&id, item) in &others {
        let entity = &item.entity;
        let message = if entity == "GEOMETRIC_CURVE_SET" || entity == "GEOMETRIC_SET" {
            match set_problem(graph, id) {
                None => continue,
                Some(SetProblem::Element(element)) => {
                    format!("#{id} ({entity}) holds #{element}, which is not a point or a curve")
                }
                Some(SetProblem::Unreadable) => {
                    format!("#{id} ({entity}) has elements that cannot be read")
                }
            }
        } else {
            format!(
                "#{id} ({entity}) is in the part's shape but is not read; a canonical part's shape is one solid, plus curve and point sets"
            )
        };
        refusals.push(Refusal::new(
            RefusalCode::UnreadShapeItem,
            Some(id),
            message,
        ));
    }
    refusals
}

/// Check 8: one refusal per link not followed.
fn not_followed(shape: &DefinitionShape) -> Vec<Refusal> {
    shape
        .not_followed
        .iter()
        .map(|link| {
            let entity = if link.entity.is_empty() {
                "undefined"
            } else {
                &link.entity
            };
            Refusal::new(
                RefusalCode::ShapeIncomplete,
                Some(link.instance),
                format!(
                    "#{} ({entity}) was not followed: {}",
                    link.instance,
                    not_followed_reason(&link.reason)
                ),
            )
        })
        .collect()
}

/// The one solid a part's shape holds, and the map into the part's frame.
pub(crate) struct ShapeSolid<'s> {
    pub(crate) item: &'s ShapeItem,
    /// `None` if no mapped item places the solid.
    pub(crate) transform: Option<Rigid>,
}

/// Every placement of every solid in the shape: stepq lists a solid once
/// for each path that reaches it. Paths that place it the same way are one
/// placement; a mapped item that places it elsewhere is another copy of
/// the solid, so placements, not `#id`s, are what a part may have one of.
/// Listings are ordered by solid, then shape representation, so the first
/// one of a placement decides the frame the same way for every file.
fn placements<'s>(
    graph: &Graph<'_>,
    shape: &'s DefinitionShape,
) -> Result<Vec<ShapeSolid<'s>>, Vec<Refusal>> {
    let mut listings: Vec<&ShapeItem> = shape.solids.iter().collect();
    listings.sort_by_key(|item| (item.item, item.shape_representation, item.representation));
    let mut placed: Vec<ShapeSolid<'s>> = Vec::new();
    let mut refusals = Vec::new();
    for item in listings {
        let transform = if item.is_transformed() {
            match item.transform(graph) {
                Ok(transform) => Some(transform),
                Err(error) => {
                    refusals.push(Refusal::new(
                        RefusalCode::SolidPlacementUnreadable,
                        Some(item.item),
                        format!(
                            "the map placing solid #{} cannot be read: {error}",
                            item.item
                        ),
                    ));
                    continue;
                }
            }
        } else {
            None
        };
        let same = |other: &ShapeSolid<'_>| {
            other.item.item == item.item
                && other.transform.unwrap_or(Rigid::IDENTITY)
                    == transform.unwrap_or(Rigid::IDENTITY)
        };
        if !placed.iter().any(same) {
            placed.push(ShapeSolid { item, transform });
        }
    }
    if refusals.is_empty() {
        Ok(placed)
    } else {
        Err(refusals)
    }
}

/// Stage 3: the part's shape. Returns the solid if the stage passes.
///
/// An incomplete shape (check 8) ends the stage: what it holds beyond the
/// links not followed is unknown, so the other checks would judge part of
/// it.
pub(crate) fn shape<'s>(
    graph: &Graph<'_>,
    shape: &'s DefinitionShape,
) -> Result<ShapeSolid<'s>, Vec<Refusal>> {
    if !shape.is_complete() {
        return Err(not_followed(shape));
    }
    let mut placed = placements(graph, shape)?;
    let mut refusals = Vec::new();
    if placed.is_empty() {
        refusals.push(Refusal::new(
            RefusalCode::NoSolid,
            None,
            "the part's shape holds no manifold_solid_brep or brep_with_voids",
        ));
    } else if placed.len() > 1 {
        let listed: Vec<String> = placed
            .iter()
            .take(8)
            .map(|solid| match &solid.transform {
                Some(_) => format!("#{} (placed by a mapped item)", solid.item.item),
                None => format!("#{}", solid.item.item),
            })
            .collect();
        refusals.push(Refusal::new(
            RefusalCode::SeveralSolids,
            placed.get(1).map(|solid| solid.item.item),
            format!(
                "the part's shape holds {} solids ({}{}); a canonical part holds one",
                placed.len(),
                listed.join(", "),
                if placed.len() > 8 { ", ..." } else { "" }
            ),
        ));
    }
    refusals.extend(other_items(graph, shape));
    if !refusals.is_empty() {
        return Err(refusals);
    }
    match placed.pop() {
        Some(solid) => Ok(solid),
        None => Err(vec![Refusal::new(
            RefusalCode::Internal,
            None,
            "the gate passed a shape with no solid",
        )]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_names_ignore_versions_spacing_and_case() {
        let named = |s: &str| schema_family(&[s.to_owned()]);
        assert_eq!(
            named("AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }"),
            Some("ap214")
        );
        assert_eq!(named(" config_control_design "), Some("ap203"));
        assert_eq!(
            named("AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF { 1 0 10303 442 3 1 4 }"),
            Some("ap242")
        );
        assert_eq!(named("IFC2X3"), None);
        assert_eq!(schema_family(&[]), None);
    }

    #[test]
    fn refusals_are_sorted_and_capped() {
        let many: Vec<Refusal> = (0..40u64)
            .rev()
            .map(|i| Refusal::new(RefusalCode::DanglingReference, Some(i), format!("#{i}")))
            .chain([Refusal::new(RefusalCode::Unparseable, None, "x")])
            .collect();
        let kept = finish(many);
        assert_eq!(kept[0].code, RefusalCode::Unparseable);
        assert_eq!(kept.len(), 1 + LISTED_PER_CODE);
        // The summary has no instance, so it sorts first within its code,
        // and the list stays sorted: a document can be read back.
        assert_eq!(kept[1].instance, None);
        assert_eq!(kept[1].message, "and 9 more");
        assert_eq!(kept[2].instance, Some(0));
        assert!(kept.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn every_code_name_is_its_serde_name() {
        use RefusalCode::*;
        let all = [
            Unparseable,
            DanglingReference,
            SeveralDataSections,
            SchemaUnrecognized,
            Assembly,
            NoDefinition,
            SeveralDefinitions,
            ShapeIncomplete,
            NoSolid,
            SeveralSolids,
            UnreadShapeItem,
            SolidPlacementUnreadable,
            TopologyUnsupported,
            TopologyMalformed,
            SolidNotClosed,
            ShellDisconnected,
            UnitsUnreadable,
            LengthUnitUnreadable,
            AngleUnitUnreadable,
            UncertaintyUnreadable,
            UncertaintyMissing,
            GeometryUnreadable,
            VertexOffSurface,
            VertexOffCurve,
            FaceOrientation,
            OutOfRange,
            Contradiction,
            Internal,
        ];
        assert!(
            all.windows(2).all(|pair| pair[0] < pair[1]),
            "in check order"
        );
        for code in all {
            assert_eq!(
                serde_json::to_string(&code).unwrap(),
                format!("\"{}\"", code.name())
            );
        }
    }
}
