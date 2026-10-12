//! Holes and shafts (`docs/research/m2-features.md`, sections 9.2 to
//! 9.5): full cylinders joined by mutual steps into stacks (K), with their
//! ends from the end walk (W) and their lengths (E).

use serde::{Deserialize, Serialize};

use super::FeatureId;
use super::cylinder::{Axis, Cylinder, CylinderClass, End};
use super::topo::{P, dot, scale, sub};
use super::walk::{self, Terminal, Walk, Wall};
use crate::geom::{Dir3, Point3, finite};

/// A hole: hole-sense cylinders joined by mutual steps, with its two ends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Hole {
    /// `hole-<smallest face #id of its cylinders>`.
    pub id: FeatureId,
    /// The solid it is in.
    pub solid: FeatureId,
    /// Through, blind, enclosed or unknown.
    pub kind: HoleKind,
    /// Its axis, from its start end (section 9.4): a blind hole points
    /// from its bottom to its opening; the others take the canonical sign.
    pub axis: Axis,
    /// Its cylinders in order along the axis.
    pub segments: Vec<StackSegment>,
    /// The cones, tori and annulus planes the walks crossed, in order along
    /// the axis, not counting the faces the ends stop at.
    pub transitions: Vec<FeatureId>,
    /// Its two ends, at the start and at the finish of the axis.
    pub ends: Vec<StackEnd>,
    /// A through hole's length, rim to rim.
    #[serde(deserialize_with = "Option::deserialize")]
    pub length_mm: Option<f64>,
    /// A blind hole's depth, from its opening's rim to the end of its
    /// deepest full-diameter wall (ISO and ASME).
    #[serde(deserialize_with = "Option::deserialize")]
    pub depth_mm: Option<f64>,
    /// Openings in its walls between their stations.
    pub wall_openings: u32,
}

/// What kind of hole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HoleKind {
    /// Open at both ends.
    Through,
    /// Open at one end, closed at the other.
    Blind,
    /// Closed at both ends.
    Enclosed,
    /// An end is irregular or unknown.
    Unknown,
}

/// A shaft: shaft-sense cylinders joined by mutual steps, with its two
/// ends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Shaft {
    /// `shaft-<smallest face #id of its cylinders>`.
    pub id: FeatureId,
    /// The solid it belongs to.
    pub solid: FeatureId,
    /// Free at both ends, rooted at one, at both, or unknown.
    pub kind: ShaftKind,
    /// Its axis, from its start end: a shaft with one root points from it
    /// to its free end; the others take the canonical sign.
    pub axis: Axis,
    /// Its cylinders in order along the axis.
    pub segments: Vec<StackSegment>,
    /// The cones, tori and annulus planes the walks crossed.
    pub transitions: Vec<FeatureId>,
    /// Its two ends.
    pub ends: Vec<StackEnd>,
    /// The length between two free ends or two roots, rim to rim.
    #[serde(deserialize_with = "Option::deserialize")]
    pub length_mm: Option<f64>,
    /// How far it rises from its root's rim to its free end's rim.
    #[serde(deserialize_with = "Option::deserialize")]
    pub protrusion_mm: Option<f64>,
    /// Openings in its walls between their stations.
    pub wall_openings: u32,
}

/// What kind of shaft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ShaftKind {
    /// Free at both ends: a pin.
    FreeFree,
    /// Rooted at one end: a boss or stub.
    RootFree,
    /// Rooted at both: a bar between two faces.
    RootRoot,
    /// An end is irregular or unknown.
    Unknown,
}

/// One cylinder of a hole or shaft.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct StackSegment {
    /// The cylinder.
    pub cylinder: FeatureId,
    /// Its diameter.
    pub diameter_mm: f64,
    /// Where it starts along the axis, from the axis point.
    pub from_mm: f64,
    /// Where it ends.
    pub to_mm: f64,
}

/// One end of a hole or shaft (W5). Stations are along the axis from its
/// point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum StackEnd {
    /// A hole end with air beyond.
    #[non_exhaustive]
    Opening {
        /// The end of the wall.
        at_mm: f64,
        /// The plane it opens onto (W8).
        rim_mm: f64,
        /// The face it opens onto.
        opens_into: FeatureId,
        /// The widening cone it opens through, if it does.
        #[serde(deserialize_with = "Option::deserialize")]
        countersink: Option<Countersink>,
    },
    /// A hole end with material beyond.
    #[non_exhaustive]
    Bottom {
        /// The end of the wall.
        at_mm: f64,
        /// How it is closed.
        bottom: Bottom,
        /// The face that closes it.
        closed_by: FeatureId,
    },
    /// A shaft end with air beyond.
    #[non_exhaustive]
    FreeEnd {
        /// The end of the wall.
        at_mm: f64,
        /// The plane it ends at (W8).
        rim_mm: f64,
        /// The face beyond it.
        opens_into: FeatureId,
    },
    /// A shaft end that rises from material.
    #[non_exhaustive]
    Root {
        /// The end of the wall.
        at_mm: f64,
        /// The plane it rises from (W8).
        rim_mm: f64,
        /// The face it rises from.
        rises_from: FeatureId,
    },
    /// The wall runs past its last end circle onto curves no rule reads.
    #[non_exhaustive]
    Irregular {},
    /// The walk could not tell.
    #[non_exhaustive]
    Unknown {
        /// The end of the wall.
        at_mm: f64,
        /// Why.
        reason: UnknownReason,
    },
}

/// How a hole's bottom is closed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Bottom {
    /// A flat disk across the wall's end.
    #[non_exhaustive]
    Flat {},
    /// A cone, then a flat disk.
    #[non_exhaustive]
    ConeThenFlat {
        /// The cone's included angle.
        angle_rad: f64,
        /// The disk's station.
        floor_at_mm: f64,
    },
    /// A cone closing at its apex: a drill point.
    #[non_exhaustive]
    Point {
        /// The included angle.
        angle_rad: f64,
        /// From the wall's end to the apex: `r / tan α`.
        tip_length_mm: f64,
    },
    /// A floor around a post rising on the axis.
    #[non_exhaustive]
    Annular {
        /// The post's radius.
        post_radius_mm: f64,
        /// The floor's station.
        floor_at_mm: f64,
    },
}

/// A countersink an opening is reached through.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Countersink {
    /// The cone.
    pub cone: FeatureId,
    /// Its diameter where it meets the face.
    pub outer_diameter_mm: f64,
    /// Its included angle.
    pub angle_rad: f64,
}

/// Why an end is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UnknownReason {
    /// The faces beyond belong to no group, or to several (W1).
    GroupsBeyond,
    /// The first face beyond is neither a perpendicular plane, a coaxial
    /// cone nor a coaxial torus (W3).
    FirstFace,
    /// A plane crossed says the opposite of the first face (W3).
    AirThenMaterial,
    /// A cone, torus or annulus has edges that do not lead on (W4).
    Transition,
    /// A face no rule reads lies beyond a cone or torus (W4).
    AfterCurve,
    /// The walk crossed too many faces, or came back (W7).
    TooLong,
    /// A plane met lies back along the axis from one met before: past a
    /// pocket's floor, its wall climbs back (added in PR 6).
    TurnsBack,
    /// The cylinder it steps to does not step back (W6).
    StepNotMutual,
    /// What lies beyond and where the walk stopped do not make an end (W5).
    Combination,
    /// The steps do not chain the cylinders in a line (K1).
    Stack,
}

/// A full cylinder as the stacks see it.
pub(crate) struct Group<'a> {
    pub(crate) cylinder: &'a Cylinder,
    pub(crate) wall: &'a Wall,
    /// Its end stations and their arcs, as rule F found them, along the
    /// wall's axis.
    pub(crate) stations: &'a [f64],
    pub(crate) arcs: &'a [Vec<usize>],
}

/// One end of one cylinder: its walk, or irregular.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Side {
    Irregular,
    Walked(Walk),
}

/// The ends of a group, at its lowest and highest station along its axis.
struct Ends {
    low: Side,
    high: Side,
}

/// Walks both ends of every group.
pub(crate) fn walk_all(
    part: &walk::Part<'_>,
    groups: &[Group<'_>],
) -> Result<Vec<(Side, Side)>, walk::Contradiction> {
    groups
        .iter()
        .map(|g| {
            let ends = ends(part, g)?;
            Ok((ends.low, ends.high))
        })
        .collect()
}

fn ends(part: &walk::Part<'_>, g: &Group<'_>) -> Result<Ends, walk::Contradiction> {
    let first = g.stations.first().copied().zip(g.arcs.first());
    let last = g.stations.last().copied().zip(g.arcs.last());
    let walked = |end: Option<(f64, &Vec<usize>)>, u: f64| -> Result<Side, walk::Contradiction> {
        match end {
            Some((at, arcs)) => Ok(Side::Walked(walk::walk(part, g.wall, (at, arcs), u)?)),
            None => Ok(Side::Irregular),
        }
    };
    Ok(match g.cylinder.class {
        CylinderClass::FullOneEndIrregular {
            irregular: End::Start,
            ..
        } => Ends {
            low: Side::Irregular,
            high: walked(last, 1.0)?,
        },
        CylinderClass::FullOneEndIrregular {
            irregular: End::Finish,
            ..
        } => Ends {
            low: walked(first, -1.0)?,
            high: Side::Irregular,
        },
        _ => Ends {
            low: walked(first, -1.0)?,
            high: walked(last, 1.0)?,
        },
    })
}

/// A side of a group: (group index, high end).
type At = (usize, bool);

/// A group's step to another, if mutual (W6).
fn step(sides: &[(Side, Side)], groups: &[Group<'_>], at: At) -> Option<At> {
    let (i, high) = at;
    let (low_side, high_side) = sides.get(i)?;
    let Side::Walked(walk) = (if high { high_side } else { low_side }) else {
        return None;
    };
    let Ok((_, Terminal::Step(to))) = walk.outcome else {
        return None;
    };
    let j = groups.iter().position(|g| g.cylinder.id == to)?;
    let me = groups.get(i)?.cylinder.id;
    let (jl, jh) = sides.get(j)?;
    let back = |side: &Side| matches!(side, Side::Walked(w) if matches!(w.outcome, Ok((_, Terminal::Step(t))) if t == me));
    if back(jl) {
        Some((j, false))
    } else if back(jh) {
        Some((j, true))
    } else {
        None
    }
}

/// A stack's cylinders in order, and its two outer ends; or `None` if the
/// steps do not chain in a line (K1).
fn chain(
    sides: &[(Side, Side)],
    groups: &[Group<'_>],
    start: usize,
) -> (Vec<usize>, Option<(At, At)>) {
    // Every group linked to `start`, through mutual steps.
    let mut members = vec![start];
    let mut queue = vec![start];
    while let Some(i) = queue.pop() {
        for high in [false, true] {
            if let Some((j, _)) = step(sides, groups, (i, high)) {
                if !members.contains(&j) {
                    members.push(j);
                    queue.push(j);
                }
            }
        }
    }
    members.sort_unstable();
    // A line: walk from an end that steps nowhere.
    let free = |i: usize, high: bool| step(sides, groups, (i, high)).is_none();
    let Some(&(first, high)) = members
        .iter()
        .flat_map(|&i| [(i, false), (i, true)])
        .find(|&(i, high)| free(i, high))
        .as_ref()
    else {
        return (members, None);
    };
    let mut order = vec![first];
    let mut at = (first, !high);
    while let Some((j, entered)) = step(sides, groups, at) {
        if order.contains(&j) {
            return (members, None);
        }
        order.push(j);
        at = (j, !entered);
    }
    if order.len() != members.len() {
        return (members, None);
    }
    (order, Some(((first, high), at)))
}

/// One end in the stack's frame: its kind, and the station it anchors the
/// axis at (the rim of an opening, free end or root; the wall's end of a
/// bottom or unknown end; none if irregular).
struct Placed {
    end: StackEnd,
    anchor: Option<f64>,
    /// Where it lies along the stack's axis: its anchor, or for an
    /// irregular end, infinitely far on its own side.
    order: f64,
}

/// An end of group `g` (W5) at its `high` or low station, its stations
/// mapped by `map` into the stack's frame. `None` on a value that is not
/// finite.
fn place(side: &Side, g: &Group<'_>, high: bool, map: &dyn Fn(f64) -> f64) -> Option<Placed> {
    let Side::Walked(walk) = side else {
        let outward = if high {
            f64::INFINITY
        } else {
            f64::NEG_INFINITY
        };
        return Some(Placed {
            end: StackEnd::Irregular {},
            anchor: None,
            order: if map(1.0) >= map(0.0) {
                outward
            } else {
                -outward
            },
        });
    };
    let at = finite(map(walk.at))?;
    let rim = finite(map(walk.rim))?;
    let unknown = |reason| Placed {
        end: StackEnd::Unknown { at_mm: at, reason },
        anchor: Some(at),
        order: at,
    };
    let (air, terminal) = match walk.outcome {
        Ok(found) => found,
        Err(reason) => return Some(unknown(reason)),
    };
    let anchored = |end: StackEnd, anchor: f64| Placed {
        end,
        anchor: Some(anchor),
        order: anchor,
    };
    Some(match (g.wall.shaft, air, terminal) {
        (_, _, Terminal::Step(_)) => unknown(UnknownReason::StepNotMutual),
        (false, true, Terminal::Face(id) | Terminal::Post(id, _)) => anchored(
            StackEnd::Opening {
                at_mm: at,
                rim_mm: rim,
                opens_into: id,
                countersink: countersink(walk).ok()?,
            },
            rim,
        ),
        (false, false, Terminal::Disk(id) | Terminal::Apex(id, _) | Terminal::Post(id, _)) => {
            anchored(
                StackEnd::Bottom {
                    at_mm: at,
                    bottom: bottom(walk, g, terminal, map)?,
                    closed_by: id,
                },
                at,
            )
        }
        (
            true,
            true,
            Terminal::Face(id) | Terminal::Post(id, _) | Terminal::Disk(id) | Terminal::Apex(id, _),
        ) => anchored(
            StackEnd::FreeEnd {
                at_mm: at,
                rim_mm: rim,
                opens_into: id,
            },
            rim,
        ),
        (true, false, Terminal::Face(id) | Terminal::Post(id, _)) => anchored(
            StackEnd::Root {
                at_mm: at,
                rim_mm: rim,
                rises_from: id,
            },
            rim,
        ),
        _ => unknown(UnknownReason::Combination),
    })
}

/// A value that is not finite.
struct NotFinite;

/// The widening cone an opening is reached through, if any (K3).
fn countersink(walk: &Walk) -> Result<Option<Countersink>, NotFinite> {
    let Some(c) = walk.cones.iter().find(|c| c.widens) else {
        return Ok(None);
    };
    Ok(Some(Countersink {
        cone: c.id,
        outer_diameter_mm: finite(2.0 * c.far_radius).ok_or(NotFinite)?,
        angle_rad: finite(2.0 * c.semi_angle).ok_or(NotFinite)?,
    }))
}

/// How a hole's bottom closes (W5): on a disk, flat or after a cone; at a
/// cone's apex, a point; around a post, annular.
fn bottom(
    walk: &Walk,
    g: &Group<'_>,
    terminal: Terminal,
    map: &dyn Fn(f64) -> f64,
) -> Option<Bottom> {
    Some(match terminal {
        Terminal::Apex(_, semi) => Bottom::Point {
            angle_rad: finite(2.0 * semi)?,
            tip_length_mm: finite(g.wall.radius / libm::tan(semi))?,
        },
        Terminal::Post(_, radius) => Bottom::Annular {
            post_radius_mm: finite(radius)?,
            floor_at_mm: finite(map(walk.plane.unwrap_or(walk.at)))?,
        },
        _ => match walk.cones.first() {
            Some(c) => Bottom::ConeThenFlat {
                angle_rad: finite(2.0 * c.semi_angle)?,
                floor_at_mm: finite(map(walk.rim))?,
            },
            None => Bottom::Flat {},
        },
    })
}

/// The holes and shafts: each chain of groups joined
/// by mutual steps (K1), with its kind (K2), ends (W5), frame (section 9.4)
/// and lengths (E). `None` on a value that is not finite.
pub(crate) fn stacks(
    groups: &[Group<'_>],
    sides: &[(Side, Side)],
) -> Option<(Vec<Hole>, Vec<Shaft>)> {
    let mut done = vec![false; groups.len()];
    let mut holes = Vec::new();
    let mut shafts = Vec::new();
    for start in 0..groups.len() {
        if done.get(start).copied().unwrap_or(true) {
            continue;
        }
        let (order, outer) = chain(sides, groups, start);
        for &i in &order {
            if let Some(slot) = done.get_mut(i) {
                *slot = true;
            }
        }
        let stack = build(groups, sides, &order, outer)?;
        match stack {
            Built::Hole(hole) => holes.push(hole),
            Built::Shaft(shaft) => shafts.push(shaft),
        }
    }
    holes.sort_by_key(|h: &Hole| h.id);
    shafts.sort_by_key(|s: &Shaft| s.id);
    Some((holes, shafts))
}

enum Built {
    Hole(Hole),
    Shaft(Shaft),
}

/// A station `s` of group `g`, along the axis through `origin` along `z`.
fn mapper(g: &Group<'_>, origin: P, z: P) -> impl Fn(f64) -> f64 {
    let (p, w) = (g.wall.point, g.wall.z);
    let base = dot(sub(p, origin), z);
    let sign = if dot(w, z) >= 0.0 { 1.0 } else { -1.0 };
    move |s: f64| base + sign * s
}

/// The two outer ends of a stack, placed along the axis through `origin`
/// along `z`; both unknown if its steps do not chain.
fn outer_ends(
    groups: &[Group<'_>],
    sides: &[(Side, Side)],
    outer: Option<(At, At)>,
    (origin, z): (P, P),
) -> Option<(Placed, Placed)> {
    let side = |(i, high): At| -> Option<Placed> {
        let (low, hi) = sides.get(i)?;
        let g = groups.get(i)?;
        place(if high { hi } else { low }, g, high, &mapper(g, origin, z))
    };
    if let Some((x, y)) = outer {
        return Some((side(x)?, side(y)?));
    }
    let unknown = |order| Placed {
        end: StackEnd::Unknown {
            at_mm: 0.0,
            reason: UnknownReason::Stack,
        },
        anchor: None,
        order,
    };
    Some((unknown(f64::NEG_INFINITY), unknown(f64::INFINITY)))
}

/// What a stack's groups hold, along its final axis (`sign` times the
/// first group's): segments as (cylinder, diameter, from, to), the
/// transitions crossed with their stations, its faces and wall openings.
struct Laid {
    segments: Vec<(FeatureId, f64, f64, f64)>,
    transitions: Vec<(FeatureId, f64)>,
    smallest_face: Option<u64>,
    wall_openings: u32,
}

fn lay(
    groups: &[Group<'_>],
    sides: &[(Side, Side)],
    order: &[usize],
    (origin, z, sign): (P, P, f64),
) -> Laid {
    let mut laid = Laid {
        segments: Vec::new(),
        transitions: Vec::new(),
        smallest_face: None,
        wall_openings: 0,
    };
    for &i in order {
        let (Some(g), Some((low, high))) = (groups.get(i), sides.get(i)) else {
            continue;
        };
        let map = mapper(g, origin, z);
        let stations: Vec<f64> = g.stations.iter().map(|&s| sign * map(s)).collect();
        let lo = stations.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = stations.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        laid.segments
            .push((g.cylinder.id, 2.0 * g.cylinder.radius_mm, lo, hi));
        let smallest = g.cylinder.faces.iter().copied().min();
        laid.smallest_face = match (laid.smallest_face, smallest) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        laid.wall_openings = laid.wall_openings.saturating_add(g.cylinder.wall_openings);
        for side in [low, high] {
            if let Side::Walked(w) = side {
                laid.transitions
                    .extend(w.transitions.iter().map(|&(id, s)| (id, sign * map(s))));
            }
        }
    }
    laid.segments.sort_by(|x, y| x.2.total_cmp(&y.2));
    laid.transitions
        .sort_by(|x, y| x.1.total_cmp(&y.1).then(x.0.cmp(&y.0)));
    let mut seen = Vec::new();
    laid.transitions.retain(|t| {
        let new = !seen.contains(&t.0);
        seen.push(t.0);
        new
    });
    laid
}

/// How an end counts toward a stack's kind (K2): open (an opening or free
/// end), closed (a bottom or root), or neither.
fn class_of(end: &StackEnd) -> u8 {
    match end {
        StackEnd::Opening { .. } | StackEnd::FreeEnd { .. } => 1,
        StackEnd::Bottom { .. } | StackEnd::Root { .. } => 2,
        _ => 0,
    }
}

/// The stack of groups `order` (in order along their chain), whose outer
/// ends are `outer`, or both unknown if they do not chain.
fn build(
    groups: &[Group<'_>],
    sides: &[(Side, Side)],
    order: &[usize],
    outer: Option<(At, At)>,
) -> Option<Built> {
    let first = groups.get(*order.first()?)?;
    let (origin, z) = (first.wall.point, first.wall.z);
    let (mut a, mut b) = outer_ends(groups, sides, outer, (origin, z))?;
    let kinds = (class_of(&a.end), class_of(&b.end));
    // Section 9.4: a blind hole points from its bottom, a shaft from its
    // root; the others keep the canonical sign.
    let mut sign = 1.0;
    if matches!(kinds, (1, 2) | (2, 1)) {
        let (closed, open) = if kinds.0 == 2 { (&a, &b) } else { (&b, &a) };
        if closed.order > open.order {
            sign = -1.0;
        }
    }
    if sign * a.order > sign * b.order {
        std::mem::swap(&mut a, &mut b);
    }
    let laid = lay(groups, sides, order, (origin, z, sign));
    // The axis point is at the start end's anchor, or where the segments
    // start if it has none.
    let base = a.anchor.map_or_else(
        || {
            laid.segments
                .iter()
                .map(|s| s.2)
                .fold(f64::INFINITY, f64::min)
        },
        |t| sign * t,
    );
    let relative = |t: f64| finite(sign * t - base);
    let (start, finish) = (relocate(a.end, &relative)?, relocate(b.end, &relative)?);
    let direction = scale(z, sign);
    let axis = Axis {
        point_mm: Point3::new(add_scaled(origin, z, sign * base))?,
        direction: Dir3::from_unit(direction).or_else(|| Dir3::normalize(direction))?,
    };
    let segments = laid
        .segments
        .iter()
        .map(|&(cylinder, d, lo, hi)| {
            Some(StackSegment {
                cylinder,
                diameter_mm: finite(d)?,
                from_mm: finite(lo - base)?,
                to_mm: finite(hi - base)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(assemble(
        first,
        Stack {
            axis,
            segments,
            transitions: laid.transitions.iter().map(|t| t.0).collect(),
            ends: [start, finish],
            smallest_face: laid.smallest_face?,
            wall_openings: laid.wall_openings,
        },
    ))
}

/// A stack laid out, before it is a hole or a shaft.
struct Stack {
    axis: Axis,
    segments: Vec<StackSegment>,
    transitions: Vec<FeatureId>,
    ends: [StackEnd; 2],
    smallest_face: u64,
    wall_openings: u32,
}

/// The hole or shaft a stack is (K2), with its lengths (E2 to E4).
fn assemble(first: &Group<'_>, stack: Stack) -> Built {
    let [start, finish] = stack.ends;
    let kinds = (class_of(&start), class_of(&finish));
    let rim = |e: &StackEnd| match e {
        StackEnd::Opening { rim_mm, .. }
        | StackEnd::FreeEnd { rim_mm, .. }
        | StackEnd::Root { rim_mm, .. } => Some(*rim_mm),
        StackEnd::Bottom { at_mm, .. } => Some(*at_mm),
        _ => None,
    };
    let span = rim(&start)
        .zip(rim(&finish))
        .and_then(|(x, y)| finite((y - x).abs()));
    let solid = first.cylinder.solid;
    if first.wall.shaft {
        let kind = match kinds {
            (1, 1) => ShaftKind::FreeFree,
            (1, 2) | (2, 1) => ShaftKind::RootFree,
            (2, 2) => ShaftKind::RootRoot,
            _ => ShaftKind::Unknown,
        };
        Built::Shaft(Shaft {
            id: FeatureId::new(super::FeatureKind::Shaft, stack.smallest_face),
            solid,
            kind,
            axis: stack.axis,
            segments: stack.segments,
            transitions: stack.transitions,
            ends: vec![start, finish],
            length_mm: matches!(kind, ShaftKind::FreeFree | ShaftKind::RootRoot)
                .then_some(span)
                .flatten(),
            protrusion_mm: (kind == ShaftKind::RootFree).then_some(span).flatten(),
            wall_openings: stack.wall_openings,
        })
    } else {
        let kind = match kinds {
            (1, 1) => HoleKind::Through,
            (1, 2) | (2, 1) => HoleKind::Blind,
            (2, 2) => HoleKind::Enclosed,
            _ => HoleKind::Unknown,
        };
        Built::Hole(Hole {
            id: FeatureId::new(super::FeatureKind::Hole, stack.smallest_face),
            solid,
            kind,
            axis: stack.axis,
            segments: stack.segments,
            transitions: stack.transitions,
            ends: vec![start, finish],
            length_mm: (kind == HoleKind::Through).then_some(span).flatten(),
            depth_mm: (kind == HoleKind::Blind).then_some(span).flatten(),
            wall_openings: stack.wall_openings,
        })
    }
}

fn add_scaled(p: P, d: P, k: f64) -> P {
    [p[0] + k * d[0], p[1] + k * d[1], p[2] + k * d[2]]
}

/// `end` with every station moved by `shift`.
fn relocate(end: StackEnd, shift: &dyn Fn(f64) -> Option<f64>) -> Option<StackEnd> {
    Some(match end {
        StackEnd::Opening {
            at_mm,
            rim_mm,
            opens_into,
            countersink,
        } => StackEnd::Opening {
            at_mm: shift(at_mm)?,
            rim_mm: shift(rim_mm)?,
            opens_into,
            countersink,
        },
        StackEnd::Bottom {
            at_mm,
            bottom,
            closed_by,
        } => StackEnd::Bottom {
            at_mm: shift(at_mm)?,
            bottom: match bottom {
                Bottom::ConeThenFlat {
                    angle_rad,
                    floor_at_mm,
                } => Bottom::ConeThenFlat {
                    angle_rad,
                    floor_at_mm: shift(floor_at_mm)?,
                },
                Bottom::Annular {
                    post_radius_mm,
                    floor_at_mm,
                } => Bottom::Annular {
                    post_radius_mm,
                    floor_at_mm: shift(floor_at_mm)?,
                },
                other => other,
            },
            closed_by,
        },
        StackEnd::FreeEnd {
            at_mm,
            rim_mm,
            opens_into,
        } => StackEnd::FreeEnd {
            at_mm: shift(at_mm)?,
            rim_mm: shift(rim_mm)?,
            opens_into,
        },
        StackEnd::Root {
            at_mm,
            rim_mm,
            rises_from,
        } => StackEnd::Root {
            at_mm: shift(at_mm)?,
            rim_mm: shift(rim_mm)?,
            rises_from,
        },
        StackEnd::Unknown { at_mm, reason } => StackEnd::Unknown {
            at_mm: shift(at_mm).unwrap_or(0.0),
            reason,
        },
        StackEnd::Irregular {} => StackEnd::Irregular {},
    })
}
