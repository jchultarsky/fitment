//! The rules a part's features read back from JSON must keep: what
//! [`read_part`](super::read_part) guarantees of the features it writes
//! (`docs/research/m2-features.md`, section 10.2). A document that breaks
//! one is not read, so no consumer sees, for example, a known outline
//! whose loops do not close.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::TAU;

use super::cylinder::{Cylinder, CylinderClass, Sense, UndecidedReason};
use super::outline::{Outline, OutlineLoop, Segment};
use super::stack::{Hole, HoleKind, Shaft, ShaftKind, StackEnd, StackSegment};
use super::{FeatureId, FeatureKind, Part, SolidInfo};
use crate::geom::{Dir3, Point3};

/// Relative slack on sums read back: a sum is recomputed in the order it
/// was written, but the check should not depend on that.
const SUM_REL: f64 = 1e-9;

type Checked = Result<(), String>;

/// Checks every feature list of `part`, whose one solid is `solid`. JSON
/// holds no NaN, so every number read is finite.
pub(super) fn check(part: &Part, solid: &SolidInfo) -> Checked {
    let ids: BTreeSet<FeatureId> = part
        .planes
        .iter()
        .map(|p| p.id)
        .chain(part.cylinders.iter().map(|c| c.id))
        .chain(part.cones.iter().map(|c| c.id))
        .chain(part.tori.iter().map(|t| t.id))
        .chain(part.other_faces.iter().map(|o| o.id))
        .collect();
    let owned = part
        .planes
        .iter()
        .map(|p| (p.id, p.solid, p.shell))
        .chain(part.cylinders.iter().map(|c| (c.id, c.solid, c.shell)))
        .chain(part.cones.iter().map(|c| (c.id, c.solid, c.shell)))
        .chain(part.tori.iter().map(|t| (t.id, t.solid, t.shell)))
        .chain(part.other_faces.iter().map(|o| (o.id, o.solid, o.shell)));
    for (id, of, shell) in owned {
        if of != solid.id || shell >= solid.shells {
            return Err(format!(
                "feature '{id}' names solid '{of}' and shell {shell}; the part has '{}' with {} shells",
                solid.id, solid.shells
            ));
        }
    }
    face_counts(part)?;
    if let Some(extent) = &part.extent {
        let (low, high) = (extent.min_mm.to_array(), extent.max_mm.to_array());
        if (0..3).any(|i| {
            low[i]
                .partial_cmp(&high[i])
                .is_none_or(std::cmp::Ordering::is_gt)
        }) {
            return Err("the extent's minimum is not below its maximum".to_owned());
        }
    }
    for cone in &part.cones {
        if let Some([low, high]) = cone.radii_mm {
            if !(0.0 <= low && !low.is_sign_negative() && low <= high && high.is_finite()) {
                return Err(format!("cone '{}' has radii out of order", cone.id));
            }
        }
    }
    for plane in &part.planes {
        if plane.spread_mm < 0.0 {
            return Err(format!("plane '{}' has a negative spread", plane.id));
        }
        let mut sum = Some(0.0);
        for face in &plane.faces {
            let area = outline(&face.outline, Some(&ids))
                .map_err(|problem| format!("face #{} of '{}': {problem}", face.face, plane.id))?;
            if !same(area, face.area_mm2) {
                return Err(format!(
                    "face #{} of '{}' states an area its outline does not give",
                    face.face, plane.id
                ));
            }
            sum = sum.zip(face.area_mm2).map(|(s, a)| s + a);
        }
        if !same(sum, plane.area_mm2) {
            return Err(format!(
                "plane '{}' states an area that is not its faces' sum",
                plane.id
            ));
        }
    }
    let lengths = part
        .cylinders
        .iter()
        .map(|c| (c.id, [c.radius_mm, c.spread_mm]))
        .chain(
            part.cones
                .iter()
                .map(|c| (c.id, [c.semi_angle_rad, c.spread_mm])),
        )
        .chain(part.tori.iter().map(|t| {
            (
                t.id,
                [t.minor_radius_mm.min(t.major_radius_mm), t.spread_mm],
            )
        }));
    for (id, [size, spread]) in lengths {
        if size <= 0.0 || spread < 0.0 {
            return Err(format!(
                "feature '{id}' has a size that is not positive, or a negative spread"
            ));
        }
    }
    cylinders(part, &ids)?;
    stacks(part, &ids)?;
    if !part
        .notes
        .windows(2)
        .all(|pair| (pair[0].instance, pair[0].code) <= (pair[1].instance, pair[1].code))
    {
        return Err("the notes are not sorted by #id, then code".to_owned());
    }
    Ok(())
}

/// Each cylinder keeps what rule F writes: stations that ascend from 0,
/// as many as its class has; the canonical sign on its axis; a sweep within
/// 2π; near misses that are other cylinders of the part, sorted, each
/// naming it back; and wall openings only when full.
fn cylinders(part: &Part, ids: &BTreeSet<FeatureId>) -> Checked {
    let mut pairs: Vec<(FeatureId, FeatureId)> = Vec::new();
    for cylinder in &part.cylinders {
        let id = cylinder.id;
        let stations = &cylinder.stations_mm;
        let counted = match &cylinder.class {
            CylinderClass::Partial { sweep_rad: None }
            | CylinderClass::Undecided {
                reason: UndecidedReason::NoEndCircle,
            } => stations.is_empty(),
            CylinderClass::Full { .. } => stations.len() >= 2,
            CylinderClass::Partial {
                sweep_rad: Some(sweep),
            } => !stations.is_empty() && *sweep >= 0.0 && *sweep <= TAU,
            CylinderClass::FullOneEndIrregular { .. } | CylinderClass::Undecided { .. } => {
                !stations.is_empty()
            }
        };
        let ascending =
            stations.first().is_none_or(|s| *s == 0.0) && stations.windows(2).all(|w| w[0] < w[1]);
        let near = cylinder.near_misses.windows(2).all(|w| w[0] < w[1])
            && cylinder
                .near_misses
                .iter()
                .all(|n| *n != id && n.kind() == FeatureKind::Cylinder && ids.contains(n));
        pairs.extend(cylinder.near_misses.iter().map(|&n| (id, n)));
        let openings = cylinder.wall_openings == 0 || cylinder.class.is_full();
        if !(counted && ascending && near && openings && canonical(cylinder.axis.direction)) {
            return Err(format!(
                "cylinder '{id}' breaks a rule of its class, stations, axis sign, near misses or wall openings"
            ));
        }
    }
    let mut mirrored: Vec<(FeatureId, FeatureId)> = pairs.iter().map(|&(a, b)| (b, a)).collect();
    pairs.sort_unstable();
    mirrored.sort_unstable();
    if pairs != mirrored {
        return Err("a near miss is not named back".to_owned());
    }
    Ok(())
}

/// One hole or shaft, as the read-back rules see it.
struct StackView<'a> {
    id: FeatureId,
    kind: FeatureKind,
    sense: Sense,
    segments: &'a [StackSegment],
    ends: &'a [StackEnd],
    transitions: &'a [FeatureId],
    /// Its kind as K2 names it from its ends' classes (1 open, 2 closed).
    named: (u8, u8),
    /// Its lengths are the ones its kind has.
    lengths: bool,
}

fn hole_view(h: &Hole) -> StackView<'_> {
    let lengths = match h.kind {
        HoleKind::Through => h.length_mm.is_some() && h.depth_mm.is_none(),
        HoleKind::Blind => h.depth_mm.is_some() && h.length_mm.is_none(),
        _ => h.length_mm.is_none() && h.depth_mm.is_none(),
    };
    let named = match h.kind {
        HoleKind::Through => (1, 1),
        HoleKind::Blind => (2, 1),
        HoleKind::Enclosed => (2, 2),
        _ => (0, 0),
    };
    StackView {
        id: h.id,
        kind: FeatureKind::Hole,
        sense: Sense::Hole,
        segments: &h.segments,
        ends: &h.ends,
        transitions: &h.transitions,
        named,
        lengths,
    }
}

fn shaft_view(s: &Shaft) -> StackView<'_> {
    let lengths = match s.kind {
        ShaftKind::RootFree => s.protrusion_mm.is_some() && s.length_mm.is_none(),
        ShaftKind::FreeFree | ShaftKind::RootRoot => {
            s.length_mm.is_some() && s.protrusion_mm.is_none()
        }
        _ => s.length_mm.is_none() && s.protrusion_mm.is_none(),
    };
    let named = match s.kind {
        ShaftKind::FreeFree => (1, 1),
        ShaftKind::RootFree => (2, 1),
        ShaftKind::RootRoot => (2, 2),
        _ => (0, 0),
    };
    StackView {
        id: s.id,
        kind: FeatureKind::Shaft,
        sense: Sense::Shaft,
        segments: &s.segments,
        ends: &s.ends,
        transitions: &s.transitions,
        named,
        lengths,
    }
}

/// An end's class, anchor and named features (see [`end_view`]).
type EndView = (u8, Option<f64>, Vec<(FeatureId, Option<FeatureKind>)>);

/// An end's class toward its stack's kind (K2): 1 open, 2 closed, 0
/// neither; `None` if a stack of `sense` cannot have it. Also its anchor
/// station, and the features it names with the kinds they must have.
fn end_view(end: &StackEnd, sense: Sense) -> Option<EndView> {
    let hole = sense == Sense::Hole;
    Some(match end {
        StackEnd::Opening {
            rim_mm,
            opens_into,
            countersink,
            ..
        } if hole => {
            let mut named = vec![(*opens_into, None)];
            named.extend(
                countersink
                    .iter()
                    .map(|c| (c.cone, Some(FeatureKind::Cone))),
            );
            (1, Some(*rim_mm), named)
        }
        StackEnd::Bottom {
            at_mm, closed_by, ..
        } if hole => (2, Some(*at_mm), vec![(*closed_by, None)]),
        StackEnd::FreeEnd {
            rim_mm, opens_into, ..
        } if !hole => (1, Some(*rim_mm), vec![(*opens_into, None)]),
        StackEnd::Root {
            rim_mm, rises_from, ..
        } if !hole => (2, Some(*rim_mm), vec![(*rises_from, None)]),
        StackEnd::Irregular { .. } | StackEnd::Unknown { .. } => (0, None, Vec::new()),
        _ => return None,
    })
}

/// A stack's segments: each a cylinder of the part of the stack's sense
/// and diameter whose length its stations give, none in two stacks.
/// Returns the smallest face `#id` among them.
fn stack_segments(
    view: &StackView<'_>,
    cylinders: &BTreeMap<FeatureId, &Cylinder>,
    used: &mut BTreeSet<FeatureId>,
) -> Result<Option<u64>, String> {
    let id = view.id;
    let mut smallest: Option<u64> = None;
    for segment in view.segments {
        let Some(cylinder) = cylinders.get(&segment.cylinder) else {
            return Err(format!(
                "'{id}' names '{}', which is no cylinder of the part",
                segment.cylinder
            ));
        };
        let span = cylinder
            .stations_mm
            .last()
            .zip(cylinder.stations_mm.first())
            .map(|(a, b)| a - b);
        let length_agrees = match (segment.from_mm, segment.to_mm, span) {
            (Some(from), Some(to), Some(span)) => {
                (to - from - span).abs() <= SUM_REL * span.abs().max(1.0)
            }
            _ => true,
        };
        if cylinder.sense != view.sense
            || !used.insert(segment.cylinder)
            || (segment.diameter_mm - 2.0 * cylinder.radius_mm).abs() > 0.0
            || !length_agrees
        {
            return Err(format!(
                "'{id}' holds '{}' with another sense, diameter or length, or a second time",
                segment.cylinder
            ));
        }
        let first = cylinder.faces.first().copied();
        smallest = match (smallest, first) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }
    Ok(smallest)
}

/// Holes and shafts keep what the stacks write: sorted ids named after
/// their smallest face; segments in order, each a cylinder of the part of
/// the right sense and diameter whose length its stations give, none in two
/// stacks; two ends a stack of its sense can have, naming features the part
/// has, of the kind its kind says, the start end at 0, a blind hole and a
/// rooted shaft starting at their closed end; transitions the part has;
/// and the lengths its kind has, and no others.
fn stacks(part: &Part, ids: &BTreeSet<FeatureId>) -> Checked {
    let cylinders: BTreeMap<FeatureId, &Cylinder> =
        part.cylinders.iter().map(|c| (c.id, c)).collect();
    let mut used = BTreeSet::new();
    let mut last: Option<FeatureId> = None;
    let views = part
        .holes
        .iter()
        .map(hole_view)
        .chain(part.shafts.iter().map(shaft_view));
    for view in views {
        let id = view.id;
        if last.is_some_and(|l| l.kind() == view.kind && l >= id) {
            return Err(format!(
                "the {} features are not sorted by id",
                view.kind.name()
            ));
        }
        last = Some(id);
        let smallest = stack_segments(&view, &cylinders, &mut used)?;
        let key = |s: &StackSegment| s.from_mm.or(s.to_mm).unwrap_or(f64::INFINITY);
        let ordered = view.segments.windows(2).all(|w| key(&w[0]) <= key(&w[1]));
        let named = id.kind() == view.kind && smallest == Some(id.instance());
        let [start, finish] = view.ends else {
            return Err(format!("'{id}' has {} ends, not two", view.ends.len()));
        };
        let (Some(a), Some(b)) = (end_view(start, view.sense), end_view(finish, view.sense)) else {
            return Err(format!(
                "'{id}' has an end a {} cannot have",
                view.kind.name()
            ));
        };
        let refs =
            a.2.iter()
                .chain(&b.2)
                .all(|(f, kind)| ids.contains(f) && kind.is_none_or(|k| f.kind() == k));
        // K2: the kind its ends name; the closed end first when one is.
        let kind_agrees = if view.named == (0, 0) {
            !matches!((a.0, b.0), (1 | 2, 1 | 2))
        } else {
            (a.0, b.0) == view.named || (view.named.0 == view.named.1 && (b.0, a.0) == view.named)
        };
        let at_zero = a.1.is_none_or(|t| t == 0.0);
        if !(ordered
            && named
            && view.lengths
            && refs
            && kind_agrees
            && at_zero
            && view.transitions.iter().all(|t| ids.contains(t)))
        {
            return Err(format!(
                "'{id}' breaks a rule of its segments, name, ends, transitions or lengths"
            ));
        }
    }
    let complete = part
        .cylinders
        .iter()
        .all(|c| !matches!(c.class, CylinderClass::Undecided { .. }))
        && part.holes.iter().all(|h| h.kind != HoleKind::Unknown)
        && part.shafts.iter().all(|s| s.kind != ShaftKind::Unknown);
    if part.complete != complete {
        return Err(format!(
            "complete is {}, but the features say {complete}",
            part.complete
        ));
    }
    Ok(())
}

/// True if `d` has the canonical sign of section 9.4: its largest absolute
/// component positive, ties broken in the order x, y, z.
fn canonical(d: Dir3) -> bool {
    let d = d.to_array();
    let mut largest = 0;
    for (i, c) in d.iter().enumerate() {
        if d.get(largest).is_some_and(|l| c.abs() > l.abs()) {
            largest = i;
        }
    }
    d.get(largest).is_some_and(|c| *c > 0.0)
}

/// True if two optional values are both absent, or agree within
/// [`SUM_REL`].
fn same(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => (a - b).abs() <= SUM_REL * a.abs().max(b.abs()).max(1.0),
        _ => false,
    }
}

/// The face counts by surface kind agree with the feature lists.
fn face_counts(part: &Part) -> Checked {
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let counts = &part.face_counts;
    let listed = [
        (
            "plane",
            counts.plane,
            count(part.planes.iter().map(|p| p.faces.len()).sum()),
        ),
        (
            "cylindrical",
            counts.cylindrical,
            count(part.cylinders.iter().map(|c| c.faces.len()).sum()),
        ),
        (
            "conical",
            counts.conical,
            count(part.cones.iter().map(|c| c.faces.len()).sum()),
        ),
        (
            "toroidal",
            counts.toroidal,
            count(part.tori.iter().map(|t| t.faces.len()).sum()),
        ),
        (
            "spherical and other",
            counts.spherical.saturating_add(counts.other),
            count(part.other_faces.len()),
        ),
    ];
    for (kind, stated, found) in listed {
        if stated != found {
            return Err(format!(
                "face_counts states {stated} {kind} faces, the features hold {found}"
            ));
        }
    }
    Ok(())
}

/// A face's outline is consistent, and gives this area: `None` if it is
/// unknown, else the sum of its loops'.
pub(crate) fn outline(
    outline: &Outline,
    ids: Option<&BTreeSet<FeatureId>>,
) -> Result<Option<f64>, String> {
    let Outline::Known { loops } = outline else {
        return Ok(None);
    };
    let Some((outer, inner)) = loops.split_first() else {
        return Err("a known outline has no loop".to_owned());
    };
    if !outer.outer || outer.area_mm2 <= 0.0 {
        return Err("the first loop is not an outer loop of positive area".to_owned());
    }
    if inner.iter().any(|l| l.outer || l.area_mm2 > 0.0) {
        return Err("a loop after the first is outer, or has positive area".to_owned());
    }
    for lp in loops {
        segments(lp, ids)?;
    }
    Ok(Some(loops.iter().map(|l| l.area_mm2).sum()))
}

/// A loop's segments chain end to start and close, each arc has a positive
/// radius and a sweep in [−2π, 2π], and each names a feature of the part
/// across it.
fn segments(lp: &OutlineLoop, ids: Option<&BTreeSet<FeatureId>>) -> Checked {
    let ends = |segment: &Segment| -> (Point3, Point3, FeatureId) {
        match segment {
            Segment::Line {
                from_mm,
                to_mm,
                across,
                ..
            }
            | Segment::Arc {
                from_mm,
                to_mm,
                across,
                ..
            } => (*from_mm, *to_mm, *across),
        }
    };
    let bound = lp.bound;
    let Some(last) = lp.segments.last() else {
        return Err(format!("loop #{bound} has no segment"));
    };
    let mut previous = ends(last).1;
    for segment in &lp.segments {
        let (from, to, across) = ends(segment);
        if from != previous {
            return Err(format!("loop #{bound}'s segments do not chain"));
        }
        if ids.is_some_and(|ids| !ids.contains(&across)) {
            return Err(format!(
                "loop #{bound} names '{across}' across an edge, which the part does not have"
            ));
        }
        if let Segment::Arc {
            radius_mm,
            sweep_rad,
            ..
        } = segment
        {
            if *radius_mm <= 0.0 || sweep_rad.abs() > TAU {
                return Err(format!(
                    "loop #{bound} has an arc of radius {radius_mm} sweeping {sweep_rad} rad"
                ));
            }
        }
        previous = to;
    }
    Ok(())
}
