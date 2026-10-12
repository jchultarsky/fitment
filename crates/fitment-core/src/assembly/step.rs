//! The stepq adapter for assemblies: parse a file, run the assembly gate's
//! structure checks, and copy the occurrences into fitment's own types.
//! With `gate/` and `features/step.rs`, the only files that name stepq.

use std::collections::{BTreeMap, BTreeSet};

use stepq::info::Info;
use stepq::model::{Graph, Placements, ProductStructure};
use stepq::p21::parse;

use super::{
    AssemblyNote, AssemblyNoteCode, AssemblyRead, ComponentRead, DefinitionKind, ListedDefinition,
    ListedUsage, Listing, Occurrence, OccurrencePath, PathForm, UsageRef,
};
use crate::features::{DefinitionRef, ProductRef, header_of, part_of_definition};
use crate::gate::{self, Refusal, RefusalCode};
use crate::geom::Rigid;

/// A refused read: no listing.
fn refused(header: Option<crate::features::FileHeader>, refusals: Vec<Refusal>) -> AssemblyRead {
    AssemblyRead {
        header,
        refusals: gate::finish(refusals),
        listing: None,
        gate: Vec::new(),
        notes: Vec::new(),
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
    let mut refusals = gate::assembly::unread_usages(&graph, &structure);
    let placed = match gate::assembly::occurrences(&placements, &structure, root) {
        Ok(placed) => placed,
        Err(more) => {
            refusals.extend(more);
            return refused(header, refusals);
        }
    };
    let reached: BTreeSet<usize> = placed.iter().map(|p| p.occurrence.definition).collect();
    refusals.extend(gate::assembly::unreached(&structure, &reached));
    if !refusals.is_empty() {
        return refused(header, refusals);
    }
    let mut refusals = gate::assembly::assembly(&structure, root);
    for &index in &reached {
        if structure.children(index).next().is_some() {
            refusals.extend(gate::assembly::assembly_shape(&graph, &structure, index));
        }
    }
    let (mut listing, indices) = listing(&structure, root, placed);
    refusals.extend(components(&graph, &structure, &mut listing, &indices));
    let mut notes = uncertainties(&graph, &mut listing);
    notes.extend(coincident(&listing));
    notes.sort();
    notes.dedup();
    AssemblyRead {
        header,
        refusals: Vec::new(),
        listing: Some(listing),
        gate: gate::finish(refusals),
        notes,
    }
}

/// Check 36, then 37 and 38: every distinct part read once (P1); every
/// occurrence of an accepted part placing a frame its solid is listed
/// from (P3); every placed coordinate within range. `indices` holds each
/// listed definition's index in `structure`.
fn components(
    graph: &Graph<'_>,
    structure: &ProductStructure,
    listing: &mut Listing,
    indices: &[usize],
) -> Vec<Refusal> {
    let mut refusals = Vec::new();
    let mut frames: BTreeMap<usize, (Vec<u64>, f64)> = BTreeMap::new();
    for (listed_index, (listed, &index)) in listing.definitions.iter_mut().zip(indices).enumerate()
    {
        if listed.kind != DefinitionKind::Part {
            continue;
        }
        match part_of_definition(graph, structure, index) {
            Ok(read) => {
                frames.insert(listed_index, (read.frames, read.reach_mm));
                listed.read = Some(ComponentRead::Accepted(Box::new(read.part)));
            }
            Err(found) => {
                let found = gate::finish(found);
                let mut codes: Vec<&str> = found.iter().map(|r| r.code.name()).collect();
                codes.dedup();
                let product = listed.definition.product.as_ref();
                refusals.push(Refusal::new(
                    RefusalCode::ComponentRefused,
                    Some(listed.definition.instance),
                    format!(
                        "component #{} ('{}', {} occurrences) is not a canonical part: {}",
                        listed.definition.instance,
                        product.and_then(|p| p.id.as_deref()).unwrap_or(""),
                        listed.occurrences.len(),
                        codes.join(", ")
                    ),
                ));
                listed.read = Some(ComponentRead::Refused(found));
            }
        }
    }
    for occurrence in &listing.occurrences {
        let Some((allowed, reach)) = frames.get(&occurrence.definition) else {
            continue;
        };
        let last = occurrence.path.usages().last().map(|usage| usage.instance);
        if last.is_some()
            && !occurrence
                .representation
                .is_some_and(|r| allowed.contains(&r))
        {
            refusals.push(Refusal::new(
                RefusalCode::SolidFrameUnplaced,
                last,
                format!(
                    "usage #{} places representation {}, which the part's solid is not listed \
                     from ({allowed:?})",
                    last.unwrap_or(0),
                    occurrence
                        .representation
                        .map_or_else(|| "none".to_owned(), |r| format!("#{r}"))
                ),
            ));
        }
        if !in_range(&occurrence.transform, *reach) {
            refusals.push(Refusal::new(
                RefusalCode::OutOfRange,
                last,
                format!(
                    "an occurrence placed by usage #{} carries its part beyond {} mm",
                    last.unwrap_or(0),
                    gate::geometry::RANGE_MM
                ),
            ));
        }
    }
    refusals
}

/// Check 38: a part reaching `reach` mm from its origin, moved by
/// `transform`, stays within range: a rotation keeps distances, so no
/// placed point lies farther than `reach + |t|`.
fn in_range(transform: &Rigid, reach: f64) -> bool {
    let t = transform.translation();
    let far = reach + (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
    far.is_finite() && far < gate::geometry::RANGE_MM
}

/// Z1: each occurrence's part and placement uncertainties. A placement's
/// is the largest any context on its path states: the root's
/// representation and each enclosing occurrence's.
fn uncertainties(graph: &Graph<'_>, listing: &mut Listing) -> Vec<AssemblyNote> {
    let mut notes = Vec::new();
    let mut of_representation: BTreeMap<u64, Option<f64>> = BTreeMap::new();
    let mut context = |representation: u64, notes: &mut Vec<AssemblyNote>| -> f64 {
        let stated = *of_representation
            .entry(representation)
            .or_insert_with(|| gate::assembly::context_uncertainty(graph, representation));
        if stated.is_none() {
            notes.push(AssemblyNote {
                code: AssemblyNoteCode::AssemblyUncertaintyMissing,
                instance: Some(representation),
                message: format!(
                    "the context of representation #{representation} states no length uncertainty"
                ),
            });
        }
        stated.unwrap_or(0.0)
    };
    // Occurrences are sorted by path, so an enclosing occurrence comes
    // before those inside it: its own placement floor is known by then.
    let mut by_path: BTreeMap<Vec<u64>, f64> = BTreeMap::new();
    for index in 0..listing.occurrences.len() {
        let Some(occurrence) = listing.occurrences.get(index) else {
            continue;
        };
        let instances = occurrence.path.instances();
        let enclosing = instances.split_last().map(|(_, parent)| parent.to_vec());
        let inherited = enclosing
            .and_then(|p| by_path.get(&p).copied())
            .unwrap_or(0.0);
        let is_assembly = listing
            .definitions
            .get(occurrence.definition)
            .is_some_and(|d| d.kind == DefinitionKind::Assembly);
        let own = match (is_assembly, occurrence.representation) {
            (true, Some(representation)) => context(representation, &mut notes),
            _ => 0.0,
        };
        let (part, offset) = match listing
            .definitions
            .get(occurrence.definition)
            .and_then(|d| d.read.as_ref())
        {
            Some(ComponentRead::Accepted(part)) => (
                part.max_uncertainty_mm,
                part.solids.first().map_or(0.0, |s| s.max_vertex_offset_mm),
            ),
            _ => (0.0, 0.0),
        };
        by_path.insert(instances, inherited.max(own));
        if let Some(occurrence) = listing.occurrences.get_mut(index) {
            occurrence.placement_uncertainty_mm = inherited;
            occurrence.part_uncertainty_mm = part;
            occurrence.vertex_offset_mm = offset;
        }
    }
    notes
}

/// Two occurrences of one definition at one place (within `IDENTITY_MM`),
/// found through a grid of their translations, so the work stays linear.
fn coincident(listing: &Listing) -> Vec<AssemblyNote> {
    let tolerance = gate::geometry::IDENTITY_MM;
    let cell = |x: f64| (x / tolerance).floor();
    let mut grid: BTreeMap<(usize, [i64; 3]), Vec<usize>> = BTreeMap::new();
    let mut notes = Vec::new();
    for (index, occurrence) in listing.occurrences.iter().enumerate() {
        let t = occurrence.transform.translation();
        // Saturating: a cell index far out of i64's range is still one cell.
        #[allow(clippy::cast_possible_truncation)]
        let key = [cell(t[0]) as i64, cell(t[1]) as i64, cell(t[2]) as i64];
        let mut found = None;
        'cells: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let near = [key[0] + dx, key[1] + dy, key[2] + dz];
                    for &other in grid
                        .get(&(occurrence.definition, near))
                        .into_iter()
                        .flatten()
                    {
                        if same_place(
                            &listing.occurrences[other].transform,
                            &occurrence.transform,
                            tolerance,
                        ) {
                            found = Some(other);
                            break 'cells;
                        }
                    }
                }
            }
        }
        if let Some(other) = found {
            let first = &listing.occurrences[other];
            notes.push(AssemblyNote {
                code: AssemblyNoteCode::CoincidentOccurrences,
                instance: occurrence.path.usages().last().map(|u| u.instance),
                message: format!(
                    "occurrences {} and {} of one definition are at the same place",
                    first.path.display(PathForm::Instance),
                    occurrence.path.display(PathForm::Instance)
                ),
            });
        } else {
            grid.entry((occurrence.definition, key))
                .or_default()
                .push(index);
        }
    }
    notes
}

/// True if two transforms agree entry by entry within `tolerance`.
fn same_place(a: &Rigid, b: &Rigid, tolerance: f64) -> bool {
    let (ra, rb) = (a.rotation(), b.rotation());
    let (ta, tb) = (a.translation(), b.translation());
    (0..3).all(|i| {
        (ta[i] - tb[i]).abs() <= tolerance
            && (0..3).all(|j| (ra[i][j] - rb[i][j]).abs() <= tolerance)
    })
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
) -> (Listing, Vec<usize>) {
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
                read: None,
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
                part_uncertainty_mm: 0.0,
                vertex_offset_mm: 0.0,
                placement_uncertainty_mm: 0.0,
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
    let listing = Listing {
        root: index_of.get(&root).copied().unwrap_or(0),
        definitions: listed,
        occurrences,
        usages: listed_usages,
        path_form: path_form(structure),
    };
    (listing, reached.into_values().collect())
}
