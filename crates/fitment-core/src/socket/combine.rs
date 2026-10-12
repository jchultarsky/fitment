//! Relations into requirements (`docs/research/m3-socket.md`, 8.1, 8.2):
//! one per target feature (Y1), every relation kept as a source (Y2),
//! named by kind and anchor (Y3), and the features left out (A).

use std::collections::{BTreeMap, BTreeSet};

use super::axial::{Element, EndKind, Side};
use super::model::{Basis, Ring as RingRef, SeatContact, Toucher};
use super::model::{
    BoreSpan, Cause, CauseFaces, Depth, Fit, Flag, LeftOut, LeftOutReason, Open, Requirement,
    Requires, ShaftSpan, SourceRef,
};
use super::planar::{PlaneFound, TargetFace};
use super::rules::{Found, Relation};
use crate::features::{Axis, FeatureId, Part};
use crate::geom::{Dir3, Point3};

/// How strict a fit is: a span takes the strictest of its relations'.
fn strictness(fit: Fit) -> u8 {
    match fit {
        Fit::Clearance => 0,
        Fit::NotRead => 1,
        Fit::LineToLine => 2,
        Fit::Interference => 3,
    }
}

fn axis_of(element: &Element) -> Option<Axis> {
    Some(Axis {
        point_mm: Point3::new(element.point)?,
        direction: Dir3::from_unit(element.dir).or_else(|| Dir3::normalize(element.dir))?,
    })
}

fn clean(x: f64) -> f64 {
    crate::geom::finite(x).unwrap_or(0.0)
}

/// A source for `relation`, `paths` giving the neighbours' paths.
fn source(relation: &Relation, occurrence: &[u64], paths: &[Vec<u64>]) -> SourceRef {
    SourceRef {
        rule: relation.rule.to_owned(),
        occurrence: occurrence.to_vec(),
        neighbour: paths.get(relation.neighbour).cloned().unwrap_or_default(),
        feature: relation.feature,
        faces: relation.faces.clone(),
        e_max_mm: relation.e_max.map(clean),
        sine: relation.sine.map(clean),
        margin_mm: relation.margin.map(clean),
        gap_mm: relation.gap.map(clean),
        span_mm: relation.span.map(|(a, b)| [clean(a), clean(b)]),
        floor_mm: clean(relation.floor),
        flags: relation.flags.iter().copied().collect(),
    }
}

/// One elementary piece of a stack's axis, and what holds it.
struct Piece {
    from: f64,
    to: f64,
    diameter: f64,
    fit: Fit,
    clear: Option<f64>,
    pinned: bool,
}

/// The spans of one stack: on each segment, the maximal intervals over
/// which the same relations hold. `smallest` takes the smallest clear
/// diameter (a shaft's allowed largest) instead of the largest (a bore's
/// least).
fn spans(segments: &[(usize, &Element)], relations: &[&Relation], smallest: bool) -> Vec<Piece> {
    let normal: Vec<&&Relation> = relations
        .iter()
        .filter(|r| r.fit.is_some() && !r.unoccupied_only && r.span.is_some())
        .collect();
    let pins: Vec<&&Relation> = relations
        .iter()
        .filter(|r| r.unoccupied_only && r.fit.is_some())
        .collect();
    let mut pieces: Vec<Piece> = Vec::new();
    for &(t, element) in segments {
        // An unbounded side (an undecided cylinder with nothing to bound
        // it) ends where its relations do.
        let ends = normal
            .iter()
            .filter(|r| r.target == t)
            .filter_map(|r| r.span)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (a, b)| {
                (lo.min(a), hi.max(b))
            });
        let lo = if element.lo.is_finite() {
            element.lo
        } else {
            ends.0
        };
        let hi = if element.hi.is_finite() {
            element.hi
        } else {
            ends.1
        };
        if !(lo.is_finite() && hi.is_finite()) {
            continue;
        }
        // X7: the cylinder's own ends cut too, for a zone it stands for is
        // never pinned.
        let mut cuts: Vec<f64> = vec![lo, hi];
        cuts.extend(
            [element.own_lo, element.own_hi]
                .into_iter()
                .filter(|x| *x > lo && *x < hi),
        );
        for r in normal.iter().filter(|r| r.target == t) {
            if let Some((a, b)) = r.span {
                cuts.extend([a, b].into_iter().filter(|x| *x > lo && *x < hi));
            }
        }
        cuts.sort_by(f64::total_cmp);
        // Ends that differ by rounding alone are one: no sliver of a piece
        // between a segment's end and a span's end at the same station.
        cuts.dedup_by(|b, a| (*b - *a).abs() <= super::MERGE_MM);
        let covering = |x0: f64, x1: f64| -> Vec<&&Relation> {
            normal
                .iter()
                .filter(|r| r.target == t)
                .filter(|r| {
                    r.span.is_some_and(|(a, b)| {
                        let e = super::MERGE_MM;
                        if x0.total_cmp(&x1).is_eq() {
                            a <= x0 + e && x0 <= b + e
                        } else {
                            a < b && a <= x0 + e && x1 <= b + e
                        }
                    })
                })
                .copied()
                .collect()
        };
        let pinned_only = !pins.is_empty();
        let make = |x0: f64, x1: f64, holding: &[&&Relation]| {
            let zone = x1 <= element.own_lo || x0 >= element.own_hi;
            let mut piece = piece(
                x0,
                x1,
                2.0 * element.radius,
                holding,
                pinned_only && !zone,
                smallest,
            )?;
            if zone {
                piece.pinned = false;
            }
            Some(piece)
        };
        for pair in cuts.windows(2) {
            let holding = covering(pair[0], pair[1]);
            if let Some(piece) = make(pair[0], pair[1], &holding) {
                push(&mut pieces, piece);
            }
        }
        // A touch holds one station: a span of its own where no longer span
        // covers it.
        for r in normal.iter().filter(|r| r.target == t) {
            if let Some((a, b)) = r.span {
                if a.total_cmp(&b).is_eq()
                    && !pieces
                        .iter()
                        .any(|p| p.from <= a && a <= p.to && p.from < p.to)
                {
                    let holding = covering(a, a);
                    if let Some(piece) = make(a, a, &holding) {
                        push(&mut pieces, piece);
                    }
                }
            }
        }
    }
    pieces.sort_by(|x, y| x.from.total_cmp(&y.from).then(x.to.total_cmp(&y.to)));
    pieces
}

/// The piece `x0` to `x1` of a segment of `diameter`, from the relations
/// `holding` it; with none, J's pin if `pinned_only`, else nothing.
fn piece(
    x0: f64,
    x1: f64,
    diameter: f64,
    holding: &[&&Relation],
    pinned_only: bool,
    smallest: bool,
) -> Option<Piece> {
    if holding.is_empty() {
        return pinned_only.then_some(Piece {
            from: x0,
            to: x1,
            diameter,
            fit: Fit::NotRead,
            clear: None,
            pinned: true,
        });
    }
    let fit = holding
        .iter()
        .filter_map(|r| r.fit)
        .max_by_key(|f| strictness(*f))
        .unwrap_or(Fit::NotRead);
    let clears = holding.iter().filter_map(|r| r.clear);
    let clear = if smallest {
        clears.reduce(f64::min)
    } else {
        clears.reduce(f64::max)
    };
    let pinned =
        fit != Fit::Clearance || holding.iter().any(|r| r.flags.contains(&Flag::SelfPaired));
    Some(Piece {
        from: x0,
        to: x1,
        diameter,
        fit,
        clear: if pinned && smallest { None } else { clear },
        pinned,
    })
}

/// Adds `piece`, joined to the last one when nothing differs but by
/// rounding (`MERGE_MM`): a value read through another frame differs in
/// its last digits, and must not split a span (P2).
fn push(pieces: &mut Vec<Piece>, piece: Piece) {
    let same = |a: f64, b: f64| (a - b).abs() <= super::MERGE_MM;
    match pieces.last_mut() {
        Some(last)
            if same(last.to, piece.from)
                && last.fit == piece.fit
                && last.pinned == piece.pinned
                && match (last.clear, piece.clear) {
                    (Some(a), Some(b)) => same(a, b),
                    (None, None) => true,
                    _ => false,
                }
                && same(last.diameter, piece.diameter)
                && last.from < last.to
                && piece.from < piece.to =>
        {
            last.to = piece.to;
        }
        _ => pieces.push(piece),
    }
}

/// The requirements and left-out features of one target occurrence.
pub(crate) fn requirements(
    part: &Part,
    targets: &[Element],
    found: &Found,
    occurrence: &[u64],
    paths: &[Vec<u64>],
) -> (Vec<Requirement>, Vec<LeftOut>) {
    let mut by_feature: BTreeMap<FeatureId, Vec<usize>> = BTreeMap::new();
    for (t, element) in targets.iter().enumerate() {
        by_feature.entry(element.feature).or_default().push(t);
    }
    let mut out = Vec::new();
    for (feature, indices) in &by_feature {
        let segments: Vec<(usize, &Element)> = indices
            .iter()
            .filter_map(|&t| targets.get(t).map(|e| (t, e)))
            .collect();
        let relations: Vec<&Relation> = found
            .relations
            .iter()
            .filter(|r| indices.contains(&r.target))
            .collect();
        if relations.is_empty() {
            continue;
        }
        if let Some(requirement) =
            stack_requirement(*feature, &segments, &relations, occurrence, paths)
        {
            out.push(requirement);
        }
    }
    for (feature, faces) in &found.skew {
        out.push(unresolved(
            *feature,
            None,
            vec![CauseFaces {
                cause: Cause::Skew,
                faces: faces.iter().copied().collect(),
            }],
        ));
    }
    for other in &part.other_faces {
        out.push(unresolved(
            other.id,
            None,
            vec![CauseFaces {
                cause: Cause::TargetUnreadFace,
                faces: vec![other.face],
            }],
        ));
    }
    let anchored: BTreeSet<FeatureId> = out.iter().filter_map(anchor).collect();
    let mut left_out = Vec::new();
    for (feature, indices) in &by_feature {
        if anchored.contains(feature) {
            continue;
        }
        let Some(first) = indices.first().and_then(|&t| targets.get(t)) else {
            continue;
        };
        // A partial cylinder with nothing near it (a fillet) is no anchor.
        if first.partial.is_some() {
            continue;
        }
        let Some(axis) = axis_of(first) else {
            continue;
        };
        let diameter = indices
            .iter()
            .filter_map(|&t| targets.get(t))
            .map(|e| 2.0 * e.radius)
            .fold(0.0, f64::max);
        let walls: Vec<&Element> = indices.iter().filter_map(|&t| targets.get(t)).collect();
        let from = walls.iter().map(|e| e.lo).fold(f64::INFINITY, f64::min);
        let to = walls.iter().map(|e| e.hi).fold(f64::NEG_INFINITY, f64::max);
        left_out.push(LeftOut {
            feature: *feature,
            from_mm: clean(from),
            to_mm: clean(to),
            reason: if found.blocked.contains(feature) {
                LeftOutReason::BlockedGap
            } else {
                LeftOutReason::NothingOnAxis
            },
            axis,
            diameter_mm: clean(diameter),
        });
    }
    (out, left_out)
}

/// The target feature a requirement is anchored on first, if one.
pub(crate) fn anchor(requirement: &Requirement) -> Option<FeatureId> {
    match &requirement.requires {
        Requires::Bore { hole, .. } => Some(*hole),
        Requires::Shaft { shaft, .. } => Some(*shaft),
        Requires::Seat { plane, .. } => Some(*plane),
        Requires::Grip { seats, .. } => seats.iter().min_by_key(|f| f.instance()).copied(),
        Requires::ConicalSeat { cone, .. } => Some(*cone),
        Requires::PartialBore { cylinder, .. } | Requires::PartialShaft { cylinder, .. } => {
            Some(*cylinder)
        }
        Requires::Unresolved { feature, .. } => *feature,
    }
}

/// A requirement's kind, as its id and order name it (section 9.2).
pub(crate) fn kind(requires: &Requires) -> (&'static str, u8) {
    match requires {
        Requires::Bore { .. } => ("bore", 0),
        Requires::Shaft { .. } => ("shaft", 1),
        Requires::Seat { .. } => ("seat", 2),
        Requires::ConicalSeat { .. } => ("conical_seat", 3),
        Requires::Grip { .. } => ("grip", 4),
        Requires::PartialBore { .. } => ("partial_bore", 5),
        Requires::PartialShaft { .. } => ("partial_shaft", 6),
        Requires::Unresolved { .. } => ("unresolved", 9),
    }
}

/// The id a requirement must have (Y3): its kind and anchors.
pub(crate) fn id_of(requires: &Requires) -> String {
    let (name, _) = kind(requires);
    match requires {
        Requires::Unresolved {
            definition: Some(n),
            ..
        } => format!("unresolved:part-{n}"),
        Requires::Grip { seats, .. } => {
            let mut sorted = seats.clone();
            sorted.sort_by_key(|f| f.instance());
            let names: Vec<String> = sorted.iter().map(ToString::to_string).collect();
            format!("grip:{}", names.join("+"))
        }
        other => anchor_of(other).map_or_else(String::new, |f| format!("{name}:{f}")),
    }
}

fn anchor_of(requires: &Requires) -> Option<FeatureId> {
    match requires {
        Requires::Bore { hole, .. } => Some(*hole),
        Requires::Shaft { shaft, .. } => Some(*shaft),
        Requires::Seat { plane, .. } => Some(*plane),
        Requires::ConicalSeat { cone, .. } => Some(*cone),
        Requires::PartialBore { cylinder, .. } | Requires::PartialShaft { cylinder, .. } => {
            Some(*cylinder)
        }
        Requires::Unresolved { feature, .. } => *feature,
        Requires::Grip { .. } => None,
    }
}

/// Requirements with one id become one: their causes, sources and flags
/// joined (an `unresolved` on a plane both C6 and U5 would name).
pub(crate) fn merge_same(requirements: Vec<Requirement>) -> Vec<Requirement> {
    let mut by_id: BTreeMap<String, Requirement> = BTreeMap::new();
    for requirement in requirements {
        match by_id.get_mut(&requirement.id) {
            None => {
                by_id.insert(requirement.id.clone(), requirement);
            }
            Some(kept) => {
                if let (
                    Requires::Unresolved { causes: a, .. },
                    Requires::Unresolved { causes: b, .. },
                ) = (&mut kept.requires, requirement.requires)
                {
                    a.extend(b);
                    a.sort();
                    a.dedup();
                }
                kept.sources.extend(requirement.sources);
                kept.flags.extend(requirement.flags);
                kept.flags.sort();
                kept.flags.dedup();
                kept.floor_mm = kept.floor_mm.max(requirement.floor_mm);
            }
        }
    }
    by_id.into_values().collect()
}

/// An `unresolved` requirement on a target feature or a neighbour
/// definition.
pub(crate) fn unresolved(
    feature: FeatureId,
    definition: Option<u64>,
    causes: Vec<CauseFaces>,
) -> Requirement {
    let (id, feature) = match definition {
        Some(n) => (format!("unresolved:part-{n}"), None),
        None => (format!("unresolved:{feature}"), Some(feature)),
    };
    Requirement {
        id,
        requires: Requires::Unresolved {
            feature,
            definition,
            causes,
        },
        floor_mm: 0.0,
        flags: Vec::new(),
        sources: Vec::new(),
    }
}

/// U1: a partial cylinder's requirement, over what its relations span.
fn partial(
    cylinder: FeatureId,
    first: &Element,
    axis: Axis,
    sweep: f64,
    relations: &[&Relation],
) -> Requires {
    let ends = relations
        .iter()
        .filter_map(|r| r.span)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (a, b)| {
            (lo.min(a), hi.max(b))
        });
    let clears = relations.iter().filter_map(|r| r.clear);
    let diameter = 2.0 * first.radius;
    if first.side == Side::Hole {
        Requires::PartialBore {
            cylinder,
            axis,
            diameter_mm: clean(diameter),
            clear_diameter_mm: clears.reduce(f64::max).map(clean),
            sweep_rad: clean(sweep),
            from_mm: clean(ends.0),
            to_mm: clean(ends.1),
        }
    } else {
        Requires::PartialShaft {
            cylinder,
            axis,
            diameter_mm: clean(diameter),
            clear_diameter_mm: clears.reduce(f64::min).map(clean),
            sweep_rad: clean(sweep),
            from_mm: clean(ends.0),
            to_mm: clean(ends.1),
        }
    }
}

fn stack_requirement(
    feature: FeatureId,
    segments: &[(usize, &Element)],
    relations: &[&Relation],
    occurrence: &[u64],
    paths: &[Vec<u64>],
) -> Option<Requirement> {
    let (_, first) = segments.first()?;
    let axis = axis_of(first)?;
    let mut flags: BTreeSet<Flag> = relations
        .iter()
        .flat_map(|r| r.flags.iter().copied())
        .collect();
    let floor = relations.iter().map(|r| r.floor).fold(0.0, f64::max);
    let mut sources: Vec<SourceRef> = relations
        .iter()
        .map(|r| source(r, occurrence, paths))
        .collect();
    sources.sort_by(|a, b| {
        (&a.neighbour, &a.rule, a.feature.map(FeatureId::instance)).cmp(&(
            &b.neighbour,
            &b.rule,
            b.feature.map(FeatureId::instance),
        ))
    });
    let requires = match (first.side, first.partial) {
        (_, Some(sweep)) => partial(feature, first, axis, sweep, relations),
        (Side::Hole, None) => {
            let pieces = spans(segments, relations, false);
            if first.undecided {
                flags.insert(Flag::TargetUndecided);
            }
            let last = segments.last().map_or(*first, |(_, e)| *e);
            Requires::Bore {
                hole: feature,
                axis,
                spans: pieces
                    .into_iter()
                    .map(|p| BoreSpan {
                        from_mm: clean(p.from),
                        to_mm: clean(p.to),
                        hole_diameter_mm: clean(p.diameter),
                        clear_diameter_mm: p.clear.map(clean),
                        fit: p.fit,
                        pinned: p.pinned,
                    })
                    .collect(),
                open: Open {
                    start: relations.iter().any(|r| r.open_start),
                    finish: relations.iter().any(|r| r.open_finish),
                },
                depth: if first.start == EndKind::Unknown
                    || last.finish == EndKind::Unknown
                    || first.undecided
                {
                    Depth::UnknownEnd
                } else {
                    Depth::Known
                },
            }
        }
        (Side::Shaft, None) => {
            if first.undecided {
                flags.insert(Flag::TargetUndecided);
            }
            Requires::Shaft {
                shaft: feature,
                axis,
                spans: spans(segments, relations, true)
                    .into_iter()
                    .map(|p| ShaftSpan {
                        from_mm: clean(p.from),
                        to_mm: clean(p.to),
                        shaft_diameter_mm: clean(p.diameter),
                        clear_diameter_mm: p.clear.map(clean),
                        fit: p.fit,
                        pinned: p.pinned,
                    })
                    .collect(),
                max_extra_mm: relations
                    .iter()
                    .filter_map(|r| r.extra)
                    .reduce(f64::min)
                    .map(clean),
            }
        }
    };
    Some(Requirement {
        id: id_of(&requires),
        requires,
        floor_mm: clean(floor),
        flags: flags.into_iter().collect(),
        sources,
    })
}

/// The order of requirements within a socket (section 9.2): by smallest
/// anchor `#id`, then kind; a neighbour definition's `unresolved` last.
pub(crate) fn order(requirements: &mut [Requirement]) {
    requirements.sort_by_key(order_key);
}

/// A requirement's place in that order.
pub(crate) fn order_key(r: &Requirement) -> (u8, u64, u8) {
    let (_, kind) = kind(&r.requires);
    match &r.requires {
        Requires::Unresolved {
            definition: Some(n),
            ..
        } => (1, *n, kind),
        _ => (0, anchor(r).map_or(0, FeatureId::instance), kind),
    }
}

/// A seat's sources: its contacts (C1, or C5 in the near band) and its
/// rings (H1).
fn seat_sources(
    contacts: &[&super::planar::Contact],
    rings: &[&super::rules::Ring],
    occurrence: &[u64],
    paths: &[Vec<u64>],
) -> Vec<SourceRef> {
    let path = |n: usize| paths.get(n).cloned().unwrap_or_default();
    let mut sources: Vec<SourceRef> = contacts
        .iter()
        .map(|c| SourceRef {
            rule: if c.flags.contains(&Flag::NearGap) || c.flags.contains(&Flag::Interference) {
                "C5".to_owned()
            } else {
                "C1".to_owned()
            },
            occurrence: occurrence.to_vec(),
            neighbour: path(c.neighbour),
            feature: None,
            faces: vec![c.neighbour_face],
            e_max_mm: None,
            sine: None,
            margin_mm: None,
            gap_mm: Some(clean(c.gap)),
            span_mm: None,
            floor_mm: clean(c.floor),
            flags: c.flags.iter().copied().collect(),
        })
        .collect();
    sources.extend(rings.iter().map(|r| SourceRef {
        rule: "H1".to_owned(),
        occurrence: occurrence.to_vec(),
        neighbour: path(r.neighbour),
        feature: Some(r.hole),
        faces: r.faces.clone(),
        e_max_mm: None,
        sine: None,
        margin_mm: None,
        gap_mm: None,
        span_mm: None,
        floor_mm: clean(r.floor),
        flags: Vec::new(),
    }));
    sources
}

/// The neighbour faces on each face of `plane`: its contacts' and, around
/// each ring's hole, the ring's (H1).
fn touchers_by_face(
    part: &Part,
    plane: &crate::features::Plane,
    contacts: &[&super::planar::Contact],
    ours: &[&super::rules::Ring],
    paths: &[Vec<u64>],
) -> BTreeMap<u64, Vec<Toucher>> {
    let path = |n: usize| paths.get(n).cloned().unwrap_or_default();
    let mut by_face: BTreeMap<u64, Vec<Toucher>> = BTreeMap::new();
    for contact in contacts {
        by_face.entry(contact.face).or_default().push(Toucher {
            neighbour: path(contact.neighbour),
            face: contact.neighbour_face,
            gap_mm: clean(contact.gap),
            flags: contact.flags.iter().copied().collect(),
        });
    }
    // A ring requires the target faces around its hole on this plane.
    for ring in ours {
        for face in ring_faces(part, plane, ring.hole) {
            by_face.entry(face).or_default().push(Toucher {
                neighbour: path(ring.neighbour),
                face: ring.faces.first().copied().unwrap_or(0),
                gap_mm: 0.0,
                flags: Vec::new(),
            });
        }
    }
    by_face
}

/// A ring on a plane with no face found around its hole: what touches
/// there is no rule's (C6).
fn ringless(plane: FeatureId, rings: &[&super::rules::Ring]) -> Requirement {
    let mut faces: Vec<u64> = rings.iter().flat_map(|r| r.faces.iter().copied()).collect();
    faces.sort_unstable();
    faces.dedup();
    unresolved(
        plane,
        None,
        vec![CauseFaces {
            cause: Cause::ContactNoRule,
            faces,
        }],
    )
}

/// The faces of `plane` whose outline borders a cylinder of `hole` (an
/// unknown outline may).
fn ring_faces(part: &Part, plane: &crate::features::Plane, hole: FeatureId) -> Vec<u64> {
    // Its cylinders and transitions: a chamfered or countersunk rim
    // borders the plane through its cone.
    let cylinders: BTreeSet<FeatureId> = part
        .holes
        .iter()
        .filter(|h| h.id == hole)
        .flat_map(|h| {
            h.segments
                .iter()
                .map(|s| s.cylinder)
                .chain(h.transitions.iter().copied())
        })
        .collect();
    plane
        .faces
        .iter()
        .filter(|face| match &face.outline {
            crate::features::Outline::Known { loops } => {
                loops.iter().flat_map(|l| l.segments.iter()).any(|s| {
                    let across = match s {
                        crate::features::Segment::Line { across, .. }
                        | crate::features::Segment::Arc { across, .. } => across,
                    };
                    cylinders.contains(across)
                })
            }
            crate::features::Outline::Unknown { .. } => true,
        })
        .map(|face| face.face)
        .collect()
}

/// Seats (C1 to C5, H1), grips (L1, L2), conical seats (Q1) and C6's
/// unresolved contacts of one target occurrence.
pub(crate) fn planar(
    part: &Part,
    faces: &[TargetFace<'_>],
    found: &PlaneFound,
    rings: &[super::rules::Ring],
    occurrence: &[u64],
    paths: &[Vec<u64>],
) -> Vec<Requirement> {
    let mut out = Vec::new();
    let path = |n: usize| paths.get(n).cloned().unwrap_or_default();
    let mut seated: BTreeSet<FeatureId> = found.contacts.iter().map(|c| c.plane).collect();
    seated.extend(rings.iter().map(|r| r.plane));
    for plane in part.planes.iter().filter(|p| seated.contains(&p.id)) {
        let contacts: Vec<&super::planar::Contact> = found
            .contacts
            .iter()
            .filter(|c| c.plane == plane.id)
            .collect();
        let ours: Vec<&super::rules::Ring> = rings.iter().filter(|r| r.plane == plane.id).collect();
        let mut flags: BTreeSet<Flag> = contacts
            .iter()
            .flat_map(|c| c.flags.iter().copied())
            .collect();
        let mut by_face = touchers_by_face(part, plane, &contacts, &ours, paths);
        let mut seat_contacts = Vec::new();
        for face in &plane.faces {
            let Some(mut by) = by_face.remove(&face.face) else {
                continue;
            };
            by.sort_by(|a, b| (&a.neighbour, a.face).cmp(&(&b.neighbour, b.face)));
            by.dedup_by(|a, b| a.neighbour == b.neighbour && a.face == b.face);
            let known = matches!(face.outline, crate::features::Outline::Known { .. });
            if !known {
                flags.insert(Flag::UnknownOutline);
            }
            seat_contacts.push(SeatContact {
                face: face.face,
                basis: if known {
                    Basis::TargetFace
                } else {
                    Basis::Unknown
                },
                outline: face.outline.clone(),
                by,
            });
        }
        // A ring on a plane with no face found around its hole requires no
        // face: what touches there is no rule's (C6), never an empty seat.
        if seat_contacts.is_empty() {
            out.push(ringless(plane.id, &ours));
            continue;
        }
        let mut sources = seat_sources(&contacts, &ours, occurrence, paths);
        sources.sort_by(|a, b| {
            (&a.neighbour, &a.rule, &a.faces).cmp(&(&b.neighbour, &b.rule, &b.faces))
        });
        let floor = sources.iter().map(|s| s.floor_mm).fold(0.0, f64::max);
        let requires = Requires::Seat {
            plane: plane.id,
            normal: plane.normal,
            offset_mm: clean(plane.offset_mm),
            contacts: seat_contacts,
            rings: ours
                .iter()
                .map(|r| RingRef {
                    hole: r.hole,
                    neighbour: path(r.neighbour),
                    inner_diameter_mm: clean(r.inner),
                    outer_diameter_mm: clean(r.outer),
                })
                .collect(),
        };
        out.push(Requirement {
            id: id_of(&requires),
            requires,
            floor_mm: clean(floor),
            flags: flags.into_iter().collect(),
            sources,
        });
    }
    let conical = cones(part, found, occurrence, paths);
    out.extend(grips(part, faces, &out));
    out.extend(cone_grips(part, &out, &conical));
    out.extend(conical);
    for (plane, faces) in &found.unread_contacts {
        out.push(unresolved(
            *plane,
            None,
            vec![CauseFaces {
                cause: Cause::ContactNoRule,
                faces: faces.iter().copied().collect(),
            }],
        ));
    }
    out
}

/// L1: two seats of opposed planes whose required regions may overlap
/// along the normal fix the thickness between them.
fn grips(part: &Part, faces: &[TargetFace<'_>], seats: &[Requirement]) -> Vec<Requirement> {
    let cos_limit = (1.0 - super::PARALLEL_SINE_REL * super::PARALLEL_SINE_REL).sqrt();
    let planes: Vec<(&crate::features::Plane, f64, Vec<u64>)> = seats
        .iter()
        .filter_map(|r| match &r.requires {
            Requires::Seat {
                plane, contacts, ..
            } => part
                .planes
                .iter()
                .find(|p| p.id == *plane)
                .map(|p| (p, r.floor_mm, contacts.iter().map(|c| c.face).collect())),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    for (i, (a, floor_a, faces_a)) in planes.iter().enumerate() {
        for (b, floor_b, faces_b) in planes.iter().skip(i + 1) {
            let (na, nb) = (a.normal.to_array(), b.normal.to_array());
            if na[0] * nb[0] + na[1] * nb[1] + na[2] * nb[2] > -cos_limit {
                continue;
            }
            let tau = super::CONTACT_MM.max(floor_a.max(*floor_b));
            let overlap = faces
                .iter()
                .filter(|f| faces_a.contains(&f.face))
                .any(|fa| {
                    faces
                        .iter()
                        .filter(|f| faces_b.contains(&f.face))
                        .any(|fb| super::planar::faces_may_overlap(fa, fb, tau))
                });
            if !overlap {
                continue;
            }
            // The holes with a rim on each plane.
            let holes: Vec<FeatureId> = part
                .holes
                .iter()
                .filter(|h| {
                    let (p, d) = (h.axis.point_mm.to_array(), h.axis.direction.to_array());
                    let on = |plane: &crate::features::Plane| {
                        h.ends.iter().any(|e| match e {
                            crate::features::StackEnd::Opening { rim_mm, .. } => {
                                let x = super::geo::add(p, super::geo::scale(d, *rim_mm));
                                (super::geo::dot(plane.normal.to_array(), x) - plane.offset_mm)
                                    .abs()
                                    <= super::CONTACT_MM
                            }
                            _ => false,
                        })
                    };
                    on(a) && on(b)
                })
                .map(|h| h.id)
                .collect();
            let requires = Requires::Grip {
                seats: vec![a.id, b.id],
                length_mm: clean(a.offset_mm + b.offset_mm),
                holes,
            };
            out.push(Requirement {
                id: id_of(&requires),
                requires,
                floor_mm: clean(floor_a.max(*floor_b)),
                flags: Vec::new(),
                sources: Vec::new(),
            });
        }
    }
    out
}

/// L2: a conical seat and an opposed plane seat give a grip from the
/// cone's apex to the plane, the cone opening away from the plane; its
/// length is signed, negative where the apex lies beyond the plane.
fn cone_grips(part: &Part, seats: &[Requirement], conical: &[Requirement]) -> Vec<Requirement> {
    let cos_limit = (1.0 - super::PARALLEL_SINE_REL * super::PARALLEL_SINE_REL).sqrt();
    let mut out = Vec::new();
    for c in conical {
        let Requires::ConicalSeat { cone, .. } = &c.requires else {
            continue;
        };
        let Some(cone) = part.cones.iter().find(|x| x.id == *cone) else {
            continue;
        };
        let apex = cone.apex_mm.to_array();
        // From the apex into the cone's mouth: the way it widens.
        let mouth = cone.axis.direction.to_array();
        for seat in seats {
            let Requires::Seat {
                plane,
                normal,
                offset_mm,
                ..
            } = &seat.requires
            else {
                continue;
            };
            let n = normal.to_array();
            let behind = offset_mm - super::geo::dot(n, apex);
            // Signed: negative where the apex lies beyond the plane (a
            // countersink through a thin plate).
            if super::geo::dot(n, mouth) > -cos_limit {
                continue;
            }
            let requires = Requires::Grip {
                seats: vec![cone.id, *plane],
                length_mm: clean(behind),
                holes: Vec::new(),
            };
            out.push(Requirement {
                id: id_of(&requires),
                requires,
                floor_mm: clean(c.floor_mm.max(seat.floor_mm)),
                flags: Vec::new(),
                sources: Vec::new(),
            });
        }
    }
    out
}

/// Q1: one conical seat per target cone.
fn cones(
    part: &Part,
    found: &PlaneFound,
    occurrence: &[u64],
    paths: &[Vec<u64>],
) -> Vec<Requirement> {
    let mut out = Vec::new();
    for cone in &part.cones {
        let seats: Vec<&super::planar::ConeSeat> =
            found.cones.iter().filter(|c| c.cone == cone.id).collect();
        let Some(first) = seats.first() else {
            continue;
        };
        let requires = Requires::ConicalSeat {
            cone: cone.id,
            axis: cone.axis,
            apex_mm: cone.apex_mm,
            included_angle_rad: clean(2.0 * cone.semi_angle_rad),
            radii_mm: first.radii.map(|[a, b]| [clean(a), clean(b)]),
        };
        out.push(Requirement {
            id: id_of(&requires),
            requires,
            floor_mm: clean(seats.iter().map(|s| s.floor).fold(0.0, f64::max)),
            flags: Vec::new(),
            sources: seats
                .iter()
                .map(|s| SourceRef {
                    rule: "Q1".to_owned(),
                    occurrence: occurrence.to_vec(),
                    neighbour: paths.get(s.neighbour).cloned().unwrap_or_default(),
                    feature: Some(s.neighbour_cone),
                    faces: Vec::new(),
                    e_max_mm: None,
                    sine: None,
                    margin_mm: None,
                    gap_mm: None,
                    span_mm: None,
                    floor_mm: clean(s.floor),
                    flags: Vec::new(),
                })
                .collect(),
        });
    }
    out
}
