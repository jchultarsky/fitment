//! The assembly gate (`docs/research/m3-socket.md`, section 4): checks 31
//! to 35, which a file must pass before a socket is read from it. Checks 32
//! to 34 are also what a parts list needs: without one root, placed usages
//! and a bounded number of occurrences, quantities and paths are unknown.

use std::collections::BTreeMap;

use stepq::model::{Graph, Placements, ProductStructure};
use stepq::model::{Occurrence, PlacementError};

use super::{Refusal, RefusalCode};
use crate::geom::Rigid;

/// At most this many occurrences are read under the root (check 34). A
/// crafted file of a few kilobytes can reach one sub-assembly by 2²⁰ paths.
pub(crate) const MAX_OCCURRENCES: usize = 100_000;

/// Check 31: `socket` reads an assembly, so a file with no usage refuses.
/// `parts` lists such a file's one part instead.
pub(crate) fn assembly(structure: &ProductStructure) -> Vec<Refusal> {
    if structure.usages().is_empty() {
        vec![Refusal::new(
            RefusalCode::NotAnAssembly,
            None,
            "the file has no assembly usage: it is a part, and a socket is read from an assembly",
        )]
    } else {
        Vec::new()
    }
}

/// Check 32: exactly one root. An unused definition is a root too, so an
/// orphan refuses: its place in the assembly is unknown.
pub(crate) fn root(structure: &ProductStructure) -> Result<usize, Vec<Refusal>> {
    let definitions = structure.definitions();
    match structure.roots() {
        [] => Err(vec![Refusal::new(
            RefusalCode::NoRoot,
            None,
            if definitions.is_empty() {
                "the file holds no product definition".to_owned()
            } else {
                format!(
                    "every one of the {} product definitions is used by another: no root",
                    definitions.len()
                )
            },
        )]),
        &[root] => Ok(root),
        roots => {
            let listed: Vec<String> = roots
                .iter()
                .take(8)
                .filter_map(|&index| definitions.get(index))
                .map(|definition| format!("#{}", definition.instance))
                .collect();
            Err(vec![Refusal::new(
                RefusalCode::SeveralRoots,
                roots
                    .get(1)
                    .and_then(|&index| definitions.get(index))
                    .map(|definition| definition.instance),
                format!(
                    "{} product definitions are roots ({}{}); an assembly has exactly one",
                    roots.len(),
                    listed.join(", "),
                    if roots.len() > 8 { ", ..." } else { "" }
                ),
            )])
        }
    }
}

/// The machine name of a placement error's kind, first in a refusal's
/// message so that a reader can tell them apart without parsing.
fn kind(error: &PlacementError) -> &'static str {
    match error {
        PlacementError::Unplaced { .. } => "unplaced",
        PlacementError::Quantified { .. } => "quantified",
        PlacementError::NoTransformation { .. } => "no_transformation",
        PlacementError::Unsupported { .. } => "unsupported",
        PlacementError::Inconsistent { .. } => "inconsistent",
        PlacementError::Divergent { .. } => "divergent",
        PlacementError::Frame { .. } => "frame",
        PlacementError::Cycle { .. } => "cycle",
        PlacementError::TooDeep { .. } => "too_deep",
        _ => "other",
    }
}

/// The usage a placement error names.
fn usage_of(error: &PlacementError) -> Option<u64> {
    match error {
        PlacementError::Unplaced { usage }
        | PlacementError::Quantified { usage, .. }
        | PlacementError::NoTransformation { usage, .. }
        | PlacementError::Unsupported { usage, .. }
        | PlacementError::Inconsistent { usage, .. }
        | PlacementError::Divergent { usage, .. }
        | PlacementError::Frame { usage, .. }
        | PlacementError::Cycle { usage }
        | PlacementError::TooDeep { usage } => Some(*usage),
        _ => None,
    }
}

/// An occurrence under the root whose transform reads and is rigid.
pub(crate) struct Placed {
    pub(crate) occurrence: Occurrence,
    pub(crate) transform: Rigid,
}

/// Checks 33 and 34: every usage places, every occurrence under `root`
/// composes to a rigid transform, and there are at most
/// [`MAX_OCCURRENCES`]. Returns the occurrences, the root's first, in
/// stepq's order.
///
/// A usage that cannot be placed is reported once, however many
/// occurrences lie below it.
pub(crate) fn occurrences(
    placements: &Placements<'_>,
    structure: &ProductStructure,
    root: usize,
) -> Result<Vec<Placed>, Vec<Refusal>> {
    // One message per failing usage, the first found: the usage's own
    // error, then one an occurrence below it reports.
    let mut failing: BTreeMap<u64, String> = BTreeMap::new();
    for (index, usage) in structure.usages().iter().enumerate() {
        if let Err(error) = placements.usage(index) {
            failing
                .entry(usage.instance)
                .or_insert_with(|| format!("{}: {error}", kind(&error)));
        }
    }
    let mut placed = Vec::new();
    // Every occurrence counts, placed or not: a diamond of unplaced usages
    // must not run on.
    for (count, occurrence) in placements.occurrences(root).enumerate() {
        if count >= MAX_OCCURRENCES {
            return Err(vec![Refusal::new(
                RefusalCode::TooManyOccurrences,
                None,
                format!(
                    "the assembly has more than {MAX_OCCURRENCES} occurrences under its root; \
                     fitment reads at most that many"
                ),
            )]);
        }
        let transform = match &occurrence.transform {
            Ok(transform) => Rigid::new(transform.rotation, transform.translation),
            Err(error) => {
                let usage = usage_of(error).or_else(|| {
                    occurrence
                        .path
                        .last()
                        .and_then(|&index| structure.usages().get(index))
                        .map(|usage| usage.instance)
                });
                if let Some(usage) = usage {
                    failing
                        .entry(usage)
                        .or_insert_with(|| format!("{}: {error}", kind(error)));
                }
                continue;
            }
        };
        match transform {
            Some(transform) => placed.push(Placed {
                occurrence,
                transform,
            }),
            None => {
                if let Some(usage) = occurrence
                    .path
                    .last()
                    .and_then(|&index| structure.usages().get(index))
                {
                    failing.entry(usage.instance).or_insert_with(|| {
                        format!(
                            "not_rigid: usage #{}: the composed transform is not finite and \
                             rigid",
                            usage.instance
                        )
                    });
                }
            }
        }
    }
    if failing.is_empty() {
        Ok(placed)
    } else {
        Err(failing
            .into_iter()
            .map(|(usage, message)| {
                Refusal::new(RefusalCode::PlacementUnreadable, Some(usage), message)
            })
            .collect())
    }
}

/// Check 35: the own shape of assembly definition `definition` holds only
/// curve and point sets, and can be read whole. Material there would be a
/// neighbour that belongs to no part.
pub(crate) fn assembly_shape(
    graph: &Graph<'_>,
    structure: &ProductStructure,
    definition: usize,
) -> Vec<Refusal> {
    let Some(stated) = structure.definitions().get(definition) else {
        return Vec::new();
    };
    let shape = structure.shape(graph, definition);
    let mut found = super::not_followed(&shape);
    for solid in &shape.solids {
        found.push(Refusal::new(
            RefusalCode::AssemblyShapeItem,
            Some(solid.item),
            format!("#{} ({}) is a solid", solid.item, solid.entity),
        ));
    }
    found.extend(super::other_items(graph, &shape));
    found
        .into_iter()
        .map(|refusal| {
            Refusal::new(
                RefusalCode::AssemblyShapeItem,
                refusal.instance,
                format!(
                    "assembly #{}'s own shape: {}; an assembly's shape holds only its \
                     components, and curve and point sets",
                    stated.instance, refusal.message
                ),
            )
        })
        .collect()
}
