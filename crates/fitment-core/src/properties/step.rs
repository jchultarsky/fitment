//! The stepq adapter for properties (PR1, PR2): the one file in
//! `properties/` that names stepq.

use stepq::model::{Graph, ProductStructure};
use stepq::p21::{Exchange, parse};
use stepq::props::{PropertyKind, properties};

use super::{StatedProperties, StatedProperty};

/// The part-level properties of `step`: none if it does not parse or does
/// not hold exactly one product definition.
pub(super) fn read(step: &[u8]) -> StatedProperties {
    match parse(step) {
        Ok(exchange) => of_graph(&Graph::build(exchange)),
        Err(_) => StatedProperties::default(),
    }
}

/// The part-level properties stated in `graph`'s file (PR2).
pub(crate) fn of_graph(graph: &Graph<'_>) -> StatedProperties {
    let structure = ProductStructure::new(graph);
    let [definition] = structure.definitions() else {
        return StatedProperties::default();
    };
    let part = definition.instance;
    let mut found = Vec::new();
    for property in properties(graph, &structure).properties {
        // Never a shape aspect's, and never another definition's.
        let of_part = matches!(
            property.subject.entity.as_str(),
            "PRODUCT_DEFINITION" | "PRODUCT_DEFINITION_SHAPE"
        ) && property.subject.product_definition == Some(part);
        if property.kind != PropertyKind::UserDefined || !of_part {
            continue;
        }
        let named = property.name.as_deref().filter(|n| !n.is_empty());
        // One value takes its property's name; several each keep their
        // own, so no value is read under another's name.
        let single = property.values.len() == 1;
        for value in property.values {
            let own = value.name.as_deref().filter(|n| !n.is_empty());
            let name = if single { named.or(own) } else { own.or(named) };
            found.push(StatedProperty {
                instance: property.instance,
                name: name.unwrap_or_default().to_owned(),
                value: value.value,
                measure: value.measure,
                unit: value.unit.map(|u| format!("#{u}")),
            });
        }
    }
    found.extend(designations(graph.exchange(), part));
    StatedProperties::bounded(found)
}

/// Each `MATERIAL_DESIGNATION` whose definitions include `part` or a
/// `PRODUCT_DEFINITION_SHAPE` of it (PR2).
fn designations(exchange: &Exchange<'_>, part: u64) -> Vec<StatedProperty> {
    let is_part = |id: u64| {
        id == part
            || exchange.get(id).is_some_and(|instance| {
                !instance.is_complex()
                    && exchange.records(instance).next().is_some_and(|record| {
                        record.is("PRODUCT_DEFINITION_SHAPE")
                            && record.param(2).and_then(|p| p.reference()) == Some(part)
                    })
            })
    };
    let mut found = Vec::new();
    for index in exchange.instances_of("MATERIAL_DESIGNATION") {
        let Some(instance) = exchange.instances().get(index) else {
            continue;
        };
        if instance.is_complex() {
            continue;
        }
        let Some(record) = exchange.records(instance).next() else {
            continue;
        };
        let Some(name) = record
            .param(0)
            .and_then(|p| p.literal())
            .and_then(|l| l.decode().ok())
        else {
            continue;
        };
        let names_part = record
            .param(1)
            .and_then(|p| p.list())
            .is_some_and(|mut items| items.any(|item| item.reference().is_some_and(is_part)));
        if names_part {
            found.push(StatedProperty::new(
                instance.id,
                "MATERIAL_DESIGNATION",
                &name,
            ));
        }
    }
    found
}
