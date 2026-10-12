//! Relations into requirements (`docs/research/m3-socket.md`, 8.1, 8.2):
//! one per target feature (Y1), every relation kept as a source (Y2),
//! named by kind and anchor (Y3), and the features left out (A).

use std::collections::{BTreeMap, BTreeSet};

use super::axial::{Element, EndKind, Side};
use super::model::{
    BoreSpan, Cause, CauseFaces, Depth, Fit, Flag, LeftOut, LeftOutReason, Open, Requirement,
    Requires, ShaftSpan, SourceRef,
};
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
        cuts.dedup();
        let covering = |x0: f64, x1: f64| -> Vec<&&Relation> {
            normal
                .iter()
                .filter(|r| r.target == t)
                .filter(|r| {
                    r.span.is_some_and(|(a, b)| {
                        if x0.total_cmp(&x1).is_eq() {
                            a <= x0 && x0 <= b
                        } else {
                            a < b && a <= x0 && x1 <= b
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

/// Adds `piece`, joined to the last one when nothing differs.
fn push(pieces: &mut Vec<Piece>, piece: Piece) {
    match pieces.last_mut() {
        Some(last)
            if last.to.total_cmp(&piece.from).is_eq()
                && last.fit == piece.fit
                && last.pinned == piece.pinned
                && last.clear.map(f64::to_bits) == piece.clear.map(f64::to_bits)
                && last.diameter.total_cmp(&piece.diameter).is_eq()
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

/// The target feature a requirement is anchored on, if one.
pub(crate) fn anchor(requirement: &Requirement) -> Option<FeatureId> {
    match &requirement.requires {
        Requires::Bore { hole, .. } => Some(*hole),
        Requires::Shaft { shaft, .. } => Some(*shaft),
        Requires::Unresolved { feature, .. } => *feature,
    }
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
    let requires = match first.side {
        Side::Hole => {
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
        Side::Shaft => {
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
                max_extra_mm: None,
            }
        }
    };
    let kind = match requires {
        Requires::Bore { .. } => "bore",
        Requires::Shaft { .. } => "shaft",
        Requires::Unresolved { .. } => "unresolved",
    };
    Some(Requirement {
        id: format!("{kind}:{feature}"),
        requires,
        floor_mm: clean(floor),
        flags: flags.into_iter().collect(),
        sources,
    })
}

/// The order of requirements within a socket (section 9.2): by smallest
/// anchor `#id`, then kind; a neighbour definition's `unresolved` last.
pub(crate) fn order(requirements: &mut [Requirement]) {
    let key = |r: &Requirement| -> (u8, u64, u8) {
        let kind = match r.requires {
            Requires::Bore { .. } => 0,
            Requires::Shaft { .. } => 1,
            Requires::Unresolved { .. } => 9,
        };
        match &r.requires {
            Requires::Unresolved {
                definition: Some(n),
                ..
            } => (1, *n, kind),
            _ => (0, anchor(r).map_or(0, FeatureId::instance), kind),
        }
    };
    requirements.sort_by_key(key);
}
