//! The stepq adapter: parse a file, run the gate, and copy what it reads
//! into fitment's own types. This file and `gate.rs` are the only ones
//! that name stepq, so stepq stays a private dependency.

use std::collections::BTreeMap;

use stepq::info::Info;
use stepq::model::{Graph, ProductStructure};
use stepq::p21::parse;

use super::{
    DefinitionRef, FeatureId, FeatureKind, FileHeader, Frame, OtherItem, Part, ProductRef, Refused,
    SolidInfo,
};
use crate::gate::{self, Refusal, RefusalCode};
use crate::geom::Rigid;

/// Refuses with `refusals`, sorted and capped.
fn refuse(refusals: Vec<Refusal>, header: Option<FileHeader>) -> Refused {
    Refused {
        refusals: gate::finish(refusals),
        header: header.map(Box::new),
    }
}

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

/// Copies stepq's transform into fitment's, which checks it again.
fn rigid(transform: &stepq::geometry::Rigid) -> Option<Rigid> {
    Rigid::new(transform.rotation, transform.translation)
}

pub(super) fn read(step: &[u8]) -> Result<Part, Refused> {
    // Stage 1: the file.
    let exchange = match parse(step) {
        Ok(exchange) => exchange,
        Err(error) => {
            return Err(refuse(
                vec![Refusal::new(
                    RefusalCode::Unparseable,
                    None,
                    format!("the file is not ISO 10303-21: {error}"),
                )],
                None,
            ));
        }
    };
    // `build`, not `new`: a dangling reference is a refusal with its
    // `#id`s, not an error that stops the reading of the header.
    let graph = Graph::build(exchange);
    let info = Info::new(&graph);
    let header = header_of(&info);
    let refusals = gate::file(&graph, &info);
    if !refusals.is_empty() {
        return Err(refuse(refusals, Some(header)));
    }

    // Stage 2: the product structure.
    let structure = ProductStructure::new(&graph);
    let refusals = gate::structure(&structure);
    if !refusals.is_empty() {
        return Err(refuse(refusals, Some(header)));
    }
    let Some(definition) = structure.definitions().first() else {
        return Err(refuse(
            vec![Refusal::new(
                RefusalCode::Internal,
                None,
                "the gate passed a file with no definition",
            )],
            Some(header),
        ));
    };

    // Stage 3: the shape. Index 0 is in range: there is one definition.
    let shape = structure.shape(&graph, 0);
    let solid =
        gate::shape(&graph, &shape).map_err(|refusals| refuse(refusals, Some(header.clone())))?;
    let item = solid.item;
    let transform = match &solid.transform {
        None => None,
        Some(transform) => match rigid(transform) {
            Some(transform) => Some(transform),
            None => {
                return Err(refuse(
                    vec![Refusal::new(
                        RefusalCode::SolidPlacementUnreadable,
                        Some(item.item),
                        format!(
                            "the map placing solid #{} is not a finite rigid transform",
                            item.item
                        ),
                    )],
                    Some(header),
                ));
            }
        },
    };

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
            id: FeatureId::new(FeatureKind::Solid, item.item),
            instance: item.item,
            entity: item.entity.clone(),
            representation: item.representation,
            transform,
        }],
        other_items: others.into_values().collect(),
        header,
    })
}
