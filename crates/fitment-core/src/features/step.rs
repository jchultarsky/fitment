//! The stepq adapter: parse a file, run the gate, and copy what it reads
//! into fitment's own types. This file and `gate.rs` are the only ones
//! that name stepq, so stepq stays a private dependency.

use std::collections::BTreeMap;

use stepq::info::Info;
use stepq::model::{Graph, ProductStructure};
use stepq::p21::parse;

use super::primary;
use super::{
    DefinitionRef, FeatureId, FeatureKind, FileHeader, Frame, OtherItem, Part, ProductRef,
    SolidInfo,
};
use crate::gate::{self, Refusal, RefusalCode};
use crate::geom::Rigid;

/// The header facts a document records, accepted or refused.
fn header_of(info: &Info) -> FileHeader {
    let header = &info.header;
    FileHeader {
        schema: gate::schema_family(&header.schemas).map(str::to_owned),
        schema_identifiers: header.schemas.clone(),
        originating_system: header.originating_system.clone(),
        preprocessor_version: header.preprocessor_version.clone(),
        implementation_level: header.implementation_level.clone(),
    }
}

/// `x`, finite and never `-0.0` (stepq's values are finite already).
fn finite(x: f64) -> f64 {
    crate::geom::finite(x).unwrap_or(f64::MAX)
}

/// Copies stepq's transform into fitment's, which checks it again.
fn rigid(transform: &stepq::geometry::Rigid) -> Option<Rigid> {
    Rigid::new(transform.rotation, transform.translation)
}

/// The file's header facts, when it can be parsed, and its part or the
/// reasons it is refused, sorted and capped.
pub(super) fn read(step: &[u8]) -> (Option<FileHeader>, Result<Part, Vec<Refusal>>) {
    match parse(step) {
        Err(error) => (
            None,
            Err(vec![Refusal::new(
                RefusalCode::Unparseable,
                None,
                format!("the file is not ISO 10303-21: {error}"),
            )]),
        ),
        Ok(exchange) => {
            // `build`, not `new`: a dangling reference is a refusal with its
            // `#id`s, not an error that stops the reading of the header.
            let graph = Graph::build(exchange);
            let info = Info::new(&graph);
            let header = header_of(&info);
            (Some(header), part(&graph, &info).map_err(gate::finish))
        }
    }
}

/// Gate stages 1 to 3, then the part.
fn part(graph: &Graph<'_>, info: &Info) -> Result<Part, Vec<Refusal>> {
    // Stage 1: the file.
    let refusals = gate::file(graph, info);
    if !refusals.is_empty() {
        return Err(refusals);
    }

    // Stage 2: the product structure.
    let structure = ProductStructure::new(graph);
    let refusals = gate::structure(&structure);
    if !refusals.is_empty() {
        return Err(refusals);
    }
    let Some(definition) = structure.definitions().first() else {
        return Err(vec![Refusal::new(
            RefusalCode::Internal,
            None,
            "the gate passed a file with no definition",
        )]);
    };

    // Stage 3: the shape. Index 0 is in range: there is one definition.
    let shape = structure.shape(graph, 0);
    let solid = gate::shape(graph, &shape)?;
    let item = solid.item;
    let transform = match &solid.transform {
        None => None,
        Some(transform) => match rigid(transform) {
            Some(transform) => Some(transform),
            None => {
                return Err(vec![Refusal::new(
                    RefusalCode::SolidPlacementUnreadable,
                    Some(item.item),
                    format!(
                        "the map placing solid #{} is not a finite rigid transform",
                        item.item
                    ),
                )]);
            }
        },
    };

    // Stages 4 to 6: the solid's topology, units and geometry.
    let facts = gate::solid::check(graph, item.item)?;
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let solid_id = FeatureId::new(FeatureKind::Solid, item.item);
    // Features are stated in the part's frame (design, section 5.1): a
    // solid placed by a mapped item is moved there first.
    let (shell_count, face_count) = (
        count(facts.topology.shells.len()),
        count(facts.topology.faces.len()),
    );
    let moved = match &transform {
        Some(rigid) => facts.topology.moved(rigid),
        None => facts.topology,
    };
    let primary = primary::read(&moved, solid_id, facts.max_uncertainty_mm)
        .map_err(|refusal| vec![refusal])?;

    let mut others: BTreeMap<u64, OtherItem> = BTreeMap::new();
    for other in &shape.other {
        others.entry(other.item).or_insert_with(|| OtherItem {
            instance: other.item,
            entity: other.entity.clone(),
            representation: other.representation,
        });
    }

    Ok(Part {
        definition: DefinitionRef {
            instance: definition.instance,
            product: definition.product.as_ref().map(|product| ProductRef {
                instance: product.instance,
                id: product.id.clone(),
                name: product.name.clone(),
            }),
        },
        frame: Frame {
            shape_representation: item.shape_representation,
        },
        solids: vec![SolidInfo {
            id: solid_id,
            instance: item.item,
            entity: item.entity.clone(),
            representation: item.representation,
            transform,
            shells: shell_count,
            faces: face_count,
            units: facts.units,
            max_vertex_offset_mm: finite(facts.max_vertex_offset_mm),
        }],
        max_uncertainty_mm: finite(facts.max_uncertainty_mm),
        face_counts: primary.face_counts,
        planes: primary.planes,
        cylinders: primary.cylinders,
        cones: primary.cones,
        tori: primary.tori,
        other_faces: primary.other_faces,
        other_items: others.into_values().collect(),
        notes: primary.notes,
    })
}
