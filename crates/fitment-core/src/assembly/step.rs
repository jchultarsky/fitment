//! The stepq adapter for assemblies: parse a file, run the assembly gate's
//! structure checks, and copy the occurrences into fitment's own types.
//! With `gate/` and `features/step.rs`, the only files that name stepq.

use std::collections::{BTreeMap, BTreeSet};

use stepq::info::Info;
use stepq::model::{Graph, Placements, ProductStructure};
use stepq::p21::parse;

use super::{
    AssemblyRead, DefinitionKind, ListedDefinition, ListedUsage, Listing, Occurrence,
    OccurrencePath, PathForm, UsageRef,
};
use crate::features::{DefinitionRef, ProductRef, header_of};
use crate::gate::{self, Refusal, RefusalCode};

/// A refused read: no listing.
fn refused(header: Option<crate::features::FileHeader>, refusals: Vec<Refusal>) -> AssemblyRead {
    AssemblyRead {
        header,
        refusals: gate::finish(refusals),
        listing: None,
        gate: Vec::new(),
    }
}

/// A text attribute, or `None` if the file leaves it blank.
fn stated(text: Option<&String>) -> Option<String> {
    text.filter(|text| !text.trim().is_empty()).cloned()
}

/// Gate stage 1, checks 31 to 35, and the listing.
pub(super) fn read(step: &[u8]) -> AssemblyRead {
    let exchange = match parse(step) {
        Ok(exchange) => exchange,
        Err(error) => {
            return refused(
                None,
                vec![Refusal::new(
                    RefusalCode::Unparseable,
                    None,
                    format!("the file is not ISO 10303-21: {error}"),
                )],
            );
        }
    };
    let graph = Graph::build(exchange);
    let info = Info::new(&graph);
    let header = Some(header_of(&info));
    let refusals = gate::file(&graph, &info);
    if !refusals.is_empty() {
        return refused(header, refusals);
    }
    let structure = ProductStructure::new(&graph);
    let root = match gate::assembly::root(&structure) {
        Ok(root) => root,
        Err(refusals) => return refused(header, refusals),
    };
    let placements = Placements::new(&graph, &structure);
    let placed = match gate::assembly::occurrences(&placements, &structure, root) {
        Ok(placed) => placed,
        Err(refusals) => return refused(header, refusals),
    };
    let listing = listing(&structure, root, placed);
    let mut refusals = gate::assembly::assembly(&structure);
    for listed in &listing.definitions {
        if listed.kind == DefinitionKind::Assembly {
            if let Some(index) = structure
                .definitions()
                .iter()
                .position(|definition| definition.instance == listed.definition.instance)
            {
                refusals.extend(gate::assembly::assembly_shape(&graph, &structure, index));
            }
        }
    }
    AssemblyRead {
        header,
        refusals: Vec::new(),
        listing: Some(listing),
        gate: gate::finish(refusals),
    }
}

/// O2: usage ids are the written form only if every usage has one that
/// is not blank, has no `/`, does not begin with `#`, and is unique among
/// its siblings.
fn path_form(structure: &ProductStructure) -> PathForm {
    let mut seen: BTreeSet<(usize, &str)> = BTreeSet::new();
    for usage in structure.usages() {
        let Some(id) = usage.id.as_deref() else {
            return PathForm::Instance;
        };
        if id.trim().is_empty()
            || id.contains('/')
            || id.starts_with('#')
            || !seen.insert((usage.parent, id))
        {
            return PathForm::Instance;
        }
    }
    PathForm::UsageId
}

/// The definitions reached from the root, by `#id`, and the occurrences,
/// by path (O4).
fn listing(
    structure: &ProductStructure,
    root: usize,
    placed: Vec<gate::assembly::Placed>,
) -> Listing {
    let definitions = structure.definitions();
    let usages = structure.usages();
    // Structure index to listing index, by `#id`.
    let reached: BTreeMap<u64, usize> = placed
        .iter()
        .filter_map(|placed| {
            definitions
                .get(placed.occurrence.definition)
                .map(|definition| (definition.instance, placed.occurrence.definition))
        })
        .collect();
    let index_of: BTreeMap<usize, usize> = reached
        .values()
        .enumerate()
        .map(|(listed, &index)| (index, listed))
        .collect();
    let mut listed: Vec<ListedDefinition> = reached
        .values()
        .filter_map(|&index| {
            let definition = definitions.get(index)?;
            Some(ListedDefinition {
                definition: DefinitionRef {
                    instance: definition.instance,
                    product: definition.product.as_ref().map(|product| ProductRef {
                        instance: product.instance,
                        id: product.id.clone(),
                        name: product.name.clone(),
                    }),
                },
                kind: if structure.children(index).next().is_none() {
                    DefinitionKind::Part
                } else {
                    DefinitionKind::Assembly
                },
                occurrences: Vec::new(),
            })
        })
        .collect();
    let mut occurrences: Vec<Occurrence> = placed
        .into_iter()
        .filter_map(|placed| {
            let path = placed
                .occurrence
                .path
                .iter()
                .filter_map(|&index| usages.get(index))
                .map(|usage| UsageRef {
                    instance: usage.instance,
                    id: stated(usage.id.as_ref()),
                })
                .collect();
            Some(Occurrence {
                path: OccurrencePath(path),
                definition: *index_of.get(&placed.occurrence.definition)?,
                representation: placed.occurrence.representation,
                transform: placed.transform,
            })
        })
        .collect();
    occurrences.sort_by(|a, b| a.path.cmp(&b.path));
    for (index, occurrence) in occurrences.iter().enumerate() {
        if let Some(definition) = listed.get_mut(occurrence.definition) {
            definition.occurrences.push(index);
        }
    }
    let listed_usages = usages
        .iter()
        .filter_map(|usage| {
            Some(ListedUsage {
                instance: usage.instance,
                id: stated(usage.id.as_ref()),
                parent: *index_of.get(&usage.parent)?,
            })
        })
        .collect();
    Listing {
        root: index_of.get(&root).copied().unwrap_or(0),
        definitions: listed,
        occurrences,
        usages: listed_usages,
        path_form: path_form(structure),
    }
}
