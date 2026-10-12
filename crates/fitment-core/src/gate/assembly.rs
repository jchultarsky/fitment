//! The assembly gate (`docs/research/m3-socket.md`, section 4): checks 31
//! to 35, which a file must pass before a socket is read from it. Checks 32
//! to 34 are also what a parts list needs: without one root, placed usages
//! and a bounded number of occurrences, quantities and paths are unknown.

use std::collections::{BTreeMap, BTreeSet};

use stepq::model::{Graph, Placements, ProductStructure};
use stepq::model::{Occurrence, PlacementError};

use super::{Refusal, RefusalCode};
use crate::geom::Rigid;

/// At most this many occurrences are read under the root (check 34). A
/// crafted file of a few kilobytes can reach one sub-assembly by 2²⁰ paths.
pub(crate) const MAX_OCCURRENCES: usize = 100_000;

/// At most this many usages on all occurrences' paths together (check 34):
/// a parts list writes every path, so a deep chain above a wide diamond
/// must not grow it past the file by orders of magnitude.
pub(crate) const MAX_PATH_SEGMENTS: usize = 2_000_000;

/// At most this many bytes of usage ids on all occurrences' paths together
/// (check 34), for the same reason: a parts list labels every segment.
pub(crate) const MAX_LABEL_BYTES: usize = 16 * 1024 * 1024;

/// Check 31: `socket` reads an assembly, so a root that uses nothing
/// refuses. `parts` lists such a file's one part instead.
pub(crate) fn assembly(structure: &ProductStructure, root: usize) -> Vec<Refusal> {
    if structure.children(root).next().is_none() {
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

/// Check 32, after the walk: every definition is reached from the root.
/// Definitions that use each other in a cycle are nobody's root, yet they
/// may place material at an unknown place.
pub(crate) fn unreached(structure: &ProductStructure, reached: &BTreeSet<usize>) -> Vec<Refusal> {
    let definitions = structure.definitions();
    let missing: Vec<u64> = (0..definitions.len())
        .filter(|index| !reached.contains(index))
        .filter_map(|index| definitions.get(index))
        .map(|definition| definition.instance)
        .collect();
    let Some(&first) = missing.first() else {
        return Vec::new();
    };
    let listed: Vec<String> = missing.iter().take(8).map(|id| format!("#{id}")).collect();
    vec![Refusal::new(
        RefusalCode::SeveralRoots,
        Some(first),
        format!(
            "{} product definitions are not reached from the root ({}{}): they use each \
             other, and nothing places them in the assembly",
            missing.len(),
            listed.join(", "),
            if missing.len() > 8 { ", ..." } else { "" }
        ),
    )]
}

/// Check 33, beside stepq: every assembly usage in the file is one stepq
/// reads. A usage whose parent or child is not a reference is skipped by
/// the product structure, which would silently lower a quantity.
pub(crate) fn unread_usages(graph: &Graph<'_>, structure: &ProductStructure) -> Vec<Refusal> {
    let exchange = graph.exchange();
    let read: BTreeSet<u64> = structure.usages().iter().map(|u| u.instance).collect();
    let stated: BTreeSet<u64> = exchange
        .instances_of("NEXT_ASSEMBLY_USAGE_OCCURRENCE")
        .chain(exchange.instances_of("QUANTIFIED_ASSEMBLY_COMPONENT_USAGE"))
        .filter_map(|position| exchange.instances().get(position))
        .map(|instance| instance.id)
        .collect();
    stated
        .difference(&read)
        .map(|&id| {
            Refusal::new(
                RefusalCode::PlacementUnreadable,
                Some(id),
                format!(
                    "unread: usage #{id} cannot be read as a usage of one definition by another"
                ),
            )
        })
        .collect()
}

/// An occurrence under the root whose transform reads and is rigid.
pub(crate) struct Placed {
    pub(crate) occurrence: Occurrence,
    pub(crate) transform: Rigid,
}

/// The refusal of check 34, with what was counted.
fn too_many(what: &str) -> Refusal {
    Refusal::new(
        RefusalCode::TooManyOccurrences,
        None,
        format!(
            "the assembly has more than {what} under its root; fitment reads at most that many"
        ),
    )
}

/// Check 34's budget, counted while enumerating: occurrences, the usages
/// on their paths, and the bytes of those usages' ids.
#[derive(Default)]
struct Budget {
    occurrences: usize,
    segments: usize,
    label_bytes: usize,
}

impl Budget {
    /// Counts `occurrence`; the refusal if the budget is spent.
    fn spend(&mut self, occurrence: &Occurrence, structure: &ProductStructure) -> Option<Refusal> {
        self.occurrences += 1;
        self.segments = self.segments.saturating_add(occurrence.path.len());
        self.label_bytes = occurrence
            .path
            .iter()
            .filter_map(|&index| structure.usages().get(index))
            .filter_map(|usage| usage.id.as_ref())
            .fold(self.label_bytes, |sum, id| sum.saturating_add(id.len()));
        if self.occurrences > MAX_OCCURRENCES {
            Some(too_many(&format!("{MAX_OCCURRENCES} occurrences")))
        } else if self.segments > MAX_PATH_SEGMENTS {
            Some(too_many(&format!(
                "{MAX_PATH_SEGMENTS} usages on its occurrences' paths"
            )))
        } else if self.label_bytes > MAX_LABEL_BYTES {
            Some(too_many(&format!(
                "{MAX_LABEL_BYTES} bytes of usage ids on its occurrences' paths"
            )))
        } else {
            None
        }
    }
}

/// Records why `occurrence` cannot be placed, once per usage.
fn unplaced(
    failing: &mut BTreeMap<u64, String>,
    occurrence: &Occurrence,
    structure: &ProductStructure,
    error: Option<&PlacementError>,
) {
    let last = occurrence
        .path
        .last()
        .and_then(|&index| structure.usages().get(index))
        .map(|usage| usage.instance);
    let (usage, message) = match error {
        Some(error) => (
            usage_of(error).or(last),
            format!("{}: {error}", kind(error)),
        ),
        None => (
            last,
            format!(
                "not_rigid: usage #{}: the composed transform is not finite and rigid",
                last.unwrap_or(0)
            ),
        ),
    };
    if let Some(usage) = usage {
        failing.entry(usage).or_insert(message);
    }
}

/// Checks 33 and 34: every usage places, every occurrence under `root`
/// composes to a rigid transform, and the occurrences stay within
/// [`MAX_OCCURRENCES`], [`MAX_PATH_SEGMENTS`] and [`MAX_LABEL_BYTES`].
/// Returns the occurrences, the root's first, in stepq's order.
///
/// A usage that cannot be placed is reported once, however many
/// occurrences lie below it, and also when check 34 ends the walk.
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
    let refusals = |failing: BTreeMap<u64, String>| -> Vec<Refusal> {
        failing
            .into_iter()
            .map(|(usage, message)| {
                Refusal::new(RefusalCode::PlacementUnreadable, Some(usage), message)
            })
            .collect()
    };
    let mut placed = Vec::new();
    // Every occurrence counts, placed or not: a diamond of unplaced usages
    // must not run on.
    let mut budget = Budget::default();
    for occurrence in placements.occurrences(root) {
        if let Some(refusal) = budget.spend(&occurrence, structure) {
            let mut all = refusals(failing);
            all.push(refusal);
            return Err(all);
        }
        let transform = match &occurrence.transform {
            Ok(transform) => Rigid::new(transform.rotation, transform.translation),
            Err(error) => {
                unplaced(&mut failing, &occurrence, structure, Some(error));
                continue;
            }
        };
        match transform {
            Some(transform) => placed.push(Placed {
                occurrence,
                transform,
            }),
            None => unplaced(&mut failing, &occurrence, structure, None),
        }
    }
    if failing.is_empty() {
        Ok(placed)
    } else {
        Err(refusals(failing))
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
