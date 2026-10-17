//! The stepq adapter for properties (PR1, PR2): the one file in
//! `properties/` that names stepq.
//!
//! The walk is core's own, over stepq's records, so that PR2's bounds hold
//! while reading and not only after: properties are chosen before any
//! value is read, values are read in `#id` order until the budget is
//! spent, and an item too long to keep is measured, never copied. A file
//! with many properties sharing one large representation costs what is
//! kept, not their product.

use std::collections::BTreeSet;

use stepq::model::{Graph, ProductStructure};
use stepq::p21::{Exchange, Instance, Literal, Param, Record, parse};

use super::{MAX_STATED, MAX_STATED_BYTES, StatedProperties, StatedProperty};

/// Items looked at, kept or not, before the capture stops as truncated.
const MAX_ITEMS: usize = 16 * MAX_STATED;

/// An item's text longer than this is not read at all: every field kept
/// from it lies in that text.
const MAX_ITEM_BYTES: usize = 3 * MAX_STATED_BYTES;

/// The part-level properties of `step`: none if it does not parse or does
/// not hold exactly one product definition.
pub(super) fn read(step: &[u8]) -> StatedProperties {
    match parse(step) {
        Ok(exchange) => of_graph(&Graph::build(exchange)),
        Err(_) => StatedProperties::default(),
    }
}

/// What the walk has taken, and whether it left anything out.
struct Taken {
    found: Vec<StatedProperty>,
    items: usize,
    truncated: bool,
}

impl Taken {
    /// True once the budget is spent: more values than are kept, or more
    /// items looked at than the walk allows. Either truncates the set.
    fn spent(&mut self) -> bool {
        let over = self.found.len() > MAX_STATED || self.items >= MAX_ITEMS;
        self.truncated |= over;
        over
    }
}

/// The part-level properties stated in `graph`'s file (PR2).
pub(crate) fn of_graph(graph: &Graph<'_>) -> StatedProperties {
    let structure = ProductStructure::new(graph);
    let [definition] = structure.definitions() else {
        return StatedProperties::default();
    };
    let part = definition.instance;
    let exchange = graph.exchange();
    let mut taken = Taken {
        found: Vec::new(),
        items: 0,
        truncated: false,
    };
    for node in properties_of(exchange, part) {
        if taken.spent() {
            break;
        }
        user_values(graph, node, &mut taken);
    }
    designations(exchange, part, &mut taken);
    let truncated = taken.truncated;
    let mut stated = StatedProperties::bounded(taken.found);
    stated.truncated |= truncated;
    stated
}

/// A simple instance's one record.
fn simple<'e>(exchange: &'e Exchange<'_>, instance: &Instance) -> Option<Record<'e>> {
    if instance.is_complex() {
        return None;
    }
    exchange.records(instance).next()
}

fn reference(param: Option<Param<'_>>) -> Option<u64> {
    param.and_then(|p| p.reference())
}

/// The nodes of every simple `PROPERTY_DEFINITION` whose definition is the
/// part's product definition itself, or a simple `PRODUCT_DEFINITION_SHAPE`
/// whose definition is it; never a shape aspect's, and never through one
/// (PR2). In `#id` order.
fn properties_of(exchange: &Exchange<'_>, part: u64) -> Vec<usize> {
    let instances = exchange.instances();
    let mut subjects = BTreeSet::from([part]);
    for index in exchange.instances_of("PRODUCT_DEFINITION_SHAPE") {
        let Some(instance) = instances.get(index) else {
            continue;
        };
        if simple(exchange, instance).is_some_and(|r| reference(r.param(2)) == Some(part)) {
            subjects.insert(instance.id);
        }
    }
    let mut found: Vec<(u64, usize)> = exchange
        .instances_of("PROPERTY_DEFINITION")
        .filter_map(|index| {
            let instance = instances.get(index)?;
            let subject = reference(simple(exchange, instance)?.param(2))?;
            subjects.contains(&subject).then_some((instance.id, index))
        })
        .collect();
    found.sort_unstable();
    found.into_iter().map(|(_, node)| node).collect()
}

/// A string literal, decoded.
fn text(param: Option<Param<'_>>) -> Option<String> {
    let literal = param?.literal()?;
    if literal.text().first() != Some(&b'\'') {
        return None;
    }
    literal.decode().ok().map(std::borrow::Cow::into_owned)
}

/// A string literal decoded, or any other literal's source text.
fn literal_text(literal: Literal<'_>) -> String {
    let source = literal.text();
    if source.first() == Some(&b'\'') {
        if let Ok(decoded) = literal.decode() {
            return decoded.into_owned();
        }
    }
    String::from_utf8_lossy(source).into_owned()
}

fn upper(name: &[u8]) -> String {
    String::from_utf8_lossy(name).to_ascii_uppercase()
}

/// stepq's rule for a user-defined attribute: associated with a general
/// property, or described as one; never a validation property.
fn is_user_defined(graph: &Graph<'_>, node: usize, record: Record<'_>, id: u64) -> bool {
    let is = |field: Option<&str>, expected: &str| {
        field.is_some_and(|f| f.replace('_', " ").eq_ignore_ascii_case(expected))
    };
    let name = text(record.param(0));
    if is(name.as_deref(), "geometric validation property")
        || is(name.as_deref(), "attribute validation property")
    {
        return false;
    }
    let exchange = graph.exchange();
    let general = graph.referenced_by(node).iter().any(|&user| {
        simple(exchange, graph.instance(user)).is_some_and(|link| {
            link.is("GENERAL_PROPERTY_ASSOCIATION") && reference(link.param(3)) == Some(id)
        })
    });
    general || is(text(record.param(1)).as_deref(), "user defined attribute")
}

/// The items of every representation tied to property `id`, in order.
fn items_of<'g>(graph: &'g Graph<'_>, node: usize, id: u64) -> impl Iterator<Item = u64> + 'g {
    let exchange = graph.exchange();
    graph
        .referenced_by(node)
        .iter()
        .filter_map(move |&user| {
            let link = simple(exchange, graph.instance(user))?;
            let ties = (link.is("PROPERTY_DEFINITION_REPRESENTATION")
                || link.is("SHAPE_DEFINITION_REPRESENTATION"))
                && reference(link.param(0)) == Some(id);
            let instance = exchange.get(reference(link.param(1)).filter(|_| ties)?)?;
            let mut records = exchange.records(instance);
            let record = if instance.is_complex() {
                records.find(|r| r.is("REPRESENTATION"))
            } else {
                records.next()
            };
            record.and_then(|r| r.param(1)).and_then(|p| p.list())
        })
        .flatten()
        .filter_map(|p| p.reference())
}

/// Property `node`'s values, if it is user-defined, within the budget.
fn user_values(graph: &Graph<'_>, node: usize, taken: &mut Taken) {
    let exchange = graph.exchange();
    let instance = graph.instance(node);
    let Some(record) = simple(exchange, instance) else {
        return;
    };
    let id = instance.id;
    if !is_user_defined(graph, node, record, id) {
        return;
    }
    let named = text(record.param(0)).filter(|n| !n.is_empty());
    // One value takes its property's name; several each keep their own,
    // so no value is read under another's name.
    let single = items_of(graph, node, id).take(2).count() == 1;
    for item in items_of(graph, node, id) {
        if taken.spent() {
            return;
        }
        taken.items += 1;
        let Some(value) = value(exchange, item) else {
            // Too long to keep: measured, not copied.
            taken.truncated = true;
            continue;
        };
        let own = value.name.filter(|n| !n.is_empty());
        let name = if single {
            named.clone().or(own)
        } else {
            own.or_else(|| named.clone())
        };
        taken.found.push(StatedProperty {
            instance: id,
            name: name.unwrap_or_default(),
            value: value.value,
            measure: value.measure,
            unit: value.unit.map(|u| format!("#{u}")),
        });
    }
}

/// One item read as a value.
struct Item {
    name: Option<String>,
    value: String,
    measure: Option<String>,
    unit: Option<u64>,
}

/// A typed value such as `VOLUME_MEASURE(96858.9)`: its type and its
/// value's source text.
fn typed(param: Option<Param<'_>>) -> (Option<String>, String) {
    let Some(param) = param else {
        return (None, String::new());
    };
    match param.typed() {
        Some(record) => (
            Some(upper(record.name())),
            record
                .param(0)
                .and_then(|p| p.literal())
                .map(literal_text)
                .unwrap_or_default(),
        ),
        None => (None, param.literal().map(literal_text).unwrap_or_default()),
    }
}

/// Item `#id` read as stepq's `props` reads a value: a measure, a
/// descriptive or literal item, or failing those its whole text with its
/// entity; `None` if it is missing or too long to keep.
fn value(exchange: &Exchange<'_>, id: u64) -> Option<Item> {
    let instance = exchange.get(id)?;
    if exchange.text(instance).len() > MAX_ITEM_BYTES {
        return None;
    }
    let records: Vec<Record<'_>> = exchange.records(instance).collect();
    if instance.is_complex() {
        let find = |name: &str| records.iter().copied().find(|r| r.is(name));
        if let (Some(item), Some(measure)) =
            (find("REPRESENTATION_ITEM"), find("MEASURE_WITH_UNIT"))
        {
            let (measure_type, value) = typed(measure.param(0));
            return Some(Item {
                name: text(item.param(0)),
                value,
                measure: measure_type,
                unit: reference(measure.param(1)),
            });
        }
    } else if let Some(record) = records.first().copied() {
        let entity = upper(record.name());
        let pair = record.params().count() == 2;
        if record.is("MEASURE_REPRESENTATION_ITEM") || record.is("VALUE_REPRESENTATION_ITEM") {
            let (measure_type, value) = typed(record.param(1));
            return Some(Item {
                name: text(record.param(0)),
                value,
                measure: measure_type,
                unit: reference(record.param(2)),
            });
        }
        if entity.ends_with("MEASURE_WITH_UNIT") && pair {
            let (measure_type, value) = typed(record.param(0));
            return Some(Item {
                name: None,
                value,
                measure: measure_type,
                unit: reference(record.param(1)),
            });
        }
        if record.is("DESCRIPTIVE_REPRESENTATION_ITEM") {
            return Some(Item {
                name: text(record.param(0)),
                value: text(record.param(1)).unwrap_or_default(),
                measure: None,
                unit: None,
            });
        }
        let literal = record.param(1).and_then(|p| p.literal());
        if let (true, Some(literal)) = (entity.ends_with("_REPRESENTATION_ITEM") && pair, literal) {
            return Some(Item {
                name: text(record.param(0)),
                value: literal_text(literal),
                measure: None,
                unit: None,
            });
        }
    }
    // Not a value item: a centroid point, a placement, … Kept whole.
    let entity: Vec<String> = records.iter().map(|r| upper(r.name())).collect();
    Some(Item {
        name: records.first().and_then(|r| text(r.param(0))),
        value: String::from_utf8_lossy(exchange.text(instance)).into_owned(),
        measure: Some(entity.join("+")),
        unit: None,
    })
}

/// Each `MATERIAL_DESIGNATION` whose definitions include `part` or a
/// `PRODUCT_DEFINITION_SHAPE` of it (PR2), within the bound.
fn designations(exchange: &Exchange<'_>, part: u64, taken: &mut Taken) {
    let is_part = |id: u64| {
        id == part
            || exchange.get(id).is_some_and(|instance| {
                simple(exchange, instance).is_some_and(|record| {
                    record.is("PRODUCT_DEFINITION_SHAPE")
                        && reference(record.param(2)) == Some(part)
                })
            })
    };
    // Counted apart from the user values: the bound keeps the first by
    // `#id` of both together.
    let mut kept = 0;
    for index in exchange.instances_of("MATERIAL_DESIGNATION") {
        let Some(instance) = exchange.instances().get(index) else {
            continue;
        };
        let Some(record) = simple(exchange, instance) else {
            continue;
        };
        let names_part = record
            .param(1)
            .and_then(|p| p.list())
            .is_some_and(|mut items| items.any(|item| item.reference().is_some_and(is_part)));
        if !names_part {
            continue;
        }
        if kept > MAX_STATED || exchange.text(instance).len() > MAX_ITEM_BYTES {
            taken.truncated = true;
            continue;
        }
        let Some(name) = text(record.param(0)) else {
            continue;
        };
        kept += 1;
        taken.found.push(StatedProperty::new(
            instance.id,
            "MATERIAL_DESIGNATION",
            &name,
        ));
    }
}
