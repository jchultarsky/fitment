//! A candidate as M4 reads it (CF1, CF3 to CF10): built once, shared by the
//! shortlist, alignment and verification. It holds the signature, every
//! hole and shaft stack with its usable profile (VR8), the planes of the
//! outer shell, the cones a walk crossed, and a tree over its faces for
//! VR22's core test, each face boxed from what the part states.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::GUARD_MM;
use super::signature::PartSignature;
use crate::features::extent::FaceBox;
use crate::features::{
    Axis, Bottom, CylinderClass, FeatureId, FeatureKind, Outline, Part, StackEnd, StackSegment,
};
use crate::socket::geo::{V, add, cross, norm, scale};
use crate::socket::own::{FaceReach, Frame, Own, loop_box};

/// One piece of a stack's usable profile, along its own stations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Piece {
    /// Where it starts.
    pub(crate) lo: f64,
    /// Where it ends.
    pub(crate) hi: f64,
    /// The radius it counts at: its cylinder's, or a zone's owner's.
    pub(crate) r: f64,
    /// The segment's cylinder; `None` for a zone.
    pub(crate) cylinder: Option<FeatureId>,
    /// A zone that is one cone: (station, radius) at its two circles.
    pub(crate) taper: Option<((f64, f64), (f64, f64))>,
    /// Pieces of one run join end to end; a zone not taken starts another.
    pub(crate) run: usize,
}

/// An end of a stack, as verification reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum EndAt {
    /// An opening, or a shaft's free end: its wall's end, and its rim where
    /// VR8 took the end zone out to it.
    Open { wall: f64, rim: Option<f64> },
    /// A bottom or a root, at the wall's end.
    Closed { at: f64 },
    /// Unknown or irregular: never open (CF5).
    Unknown,
}

/// A hole or shaft stack (CF3), with what VR7 to VR9 and VR22 read.
#[derive(Debug, Clone)]
pub(crate) struct Stack {
    /// The hole or shaft.
    pub(crate) id: FeatureId,
    /// True for a hole.
    pub(crate) hole: bool,
    /// Its axis point, in the candidate's frame.
    pub(crate) point: V,
    /// Its axis direction.
    pub(crate) dir: V,
    /// Its usable profile (VR8), ascending.
    pub(crate) pieces: Vec<Piece>,
    /// Its ends at the lowest and the highest station.
    pub(crate) ends: [EndAt; 2],
    /// The faces VR22 does not test: its cylinders and the faces of the
    /// zones VR8 took.
    pub(crate) exempt: BTreeSet<u64>,
    /// The stations of its walls with openings (CF8).
    pub(crate) openings: Vec<(f64, f64)>,
}

/// What M4 reads of one candidate.
#[non_exhaustive]
pub struct CandidateView<'p> {
    part: &'p Part,
    signature: PartSignature,
    stacks: Vec<Stack>,
    own: Own<'p>,
}

impl fmt::Debug for CandidateView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CandidateView")
            .field("signature", &self.signature)
            .field("stacks", &self.stacks)
            .finish_non_exhaustive()
    }
}

impl<'p> CandidateView<'p> {
    /// The view of `part`.
    pub fn of(part: &'p Part) -> Self {
        let own = Own::of(part, &face_boxes(part));
        let look = Lookup::of(part);
        let holes = part.holes.iter().map(|h| {
            let read = Read {
                id: h.id,
                axis: &h.axis,
                segments: &h.segments,
                transitions: &h.transitions,
                ends: &h.ends,
                hole: true,
            };
            stack(&look, &own, &read)
        });
        let shafts = part.shafts.iter().map(|s| {
            let read = Read {
                id: s.id,
                axis: &s.axis,
                segments: &s.segments,
                transitions: &s.transitions,
                ends: &s.ends,
                hole: false,
            };
            stack(&look, &own, &read)
        });
        let stacks = holes.chain(shafts).collect();
        Self {
            part,
            signature: PartSignature::of(part),
            stacks,
            own,
        }
    }

    /// Its signature.
    pub fn signature(&self) -> &PartSignature {
        &self.signature
    }

    /// `u_c`, its own uncertainty (VR2).
    pub fn uncertainty_mm(&self) -> f64 {
        self.signature.uncertainty_mm()
    }

    /// The part.
    pub(crate) fn part(&self) -> &'p Part {
        self.part
    }

    /// Its stacks: holes, then shafts, each by id.
    pub(crate) fn stacks(&self) -> &[Stack] {
        &self.stacks
    }

    /// Its faces, searchable.
    pub(crate) fn own(&self) -> &Own<'p> {
        &self.own
    }

    /// The cones a walk crossed (CF7): their circles are whole.
    pub(crate) fn walked_cones(&self) -> Vec<&'p crate::features::Cone> {
        let mut walked: BTreeSet<FeatureId> = BTreeSet::new();
        for h in &self.part.holes {
            walked.extend(
                h.transitions
                    .iter()
                    .filter(|t| t.kind() == FeatureKind::Cone),
            );
            for end in &h.ends {
                if let StackEnd::Opening {
                    countersink: Some(c),
                    ..
                } = end
                {
                    walked.insert(c.cone);
                }
            }
        }
        for s in &self.part.shafts {
            walked.extend(
                s.transitions
                    .iter()
                    .filter(|t| t.kind() == FeatureKind::Cone),
            );
        }
        self.part
            .cones
            .iter()
            .filter(|c| walked.contains(&c.id))
            .collect()
    }
}

/// The part's cylinders by id, and its shafts by the face they rise from,
/// built once, so a stack's reading costs a lookup, not a scan.
struct Lookup<'p> {
    cylinders: BTreeMap<FeatureId, &'p crate::features::Cylinder>,
    posts: BTreeMap<FeatureId, Vec<&'p crate::features::Shaft>>,
}

impl<'p> Lookup<'p> {
    fn of(part: &'p Part) -> Self {
        let mut posts: BTreeMap<FeatureId, Vec<&crate::features::Shaft>> = BTreeMap::new();
        for shaft in &part.shafts {
            for end in &shaft.ends {
                if let StackEnd::Root { rises_from, .. } = end {
                    posts.entry(*rises_from).or_default().push(shaft);
                }
            }
        }
        Self {
            cylinders: part.cylinders.iter().map(|c| (c.id, c)).collect(),
            posts,
        }
    }
}

/// What a stack is read from: a hole's or a shaft's fields.
struct Read<'a> {
    id: FeatureId,
    axis: &'a Axis,
    segments: &'a [StackSegment],
    transitions: &'a [FeatureId],
    ends: &'a [StackEnd],
    hole: bool,
}

/// A wall of the profile: its stations, radius and cylinder.
#[derive(Clone, Copy)]
struct Wall {
    /// The segment's index in its stack.
    index: usize,
    lo: f64,
    hi: f64,
    r: f64,
    cylinder: FeatureId,
}

/// One transition face: its index, `#id`, feature and reach.
struct Crossed {
    face: u64,
    feature: FeatureId,
    reach: Option<FaceReach>,
}

/// VR8: the walls with both ends known, between the last segment whose
/// start is unknown and the first whose end is: nothing past a `None` is
/// known. Empty if they are not in order along the axis.
fn walls(segments: &[StackSegment]) -> Vec<Wall> {
    let low = segments
        .iter()
        .rposition(|s| s.from_mm.is_none())
        .map_or(0, |i| i + 1);
    let high = segments
        .iter()
        .position(|s| s.to_mm.is_none())
        .unwrap_or(segments.len());
    let walls: Vec<Wall> = segments
        .iter()
        .enumerate()
        .take(high)
        .skip(low)
        .filter_map(|(index, s)| {
            let (a, b) = (s.from_mm?, s.to_mm?);
            Some(Wall {
                index,
                lo: a.min(b),
                hi: a.max(b),
                r: s.diameter_mm / 2.0,
                cylinder: s.cylinder,
            })
        })
        .collect();
    let ordered = walls
        .windows(2)
        .all(|w| w[0].hi <= w[1].lo + slack(w[1].lo));
    if ordered { walls } else { Vec::new() }
}

/// The comparison slack at station or radius `x`: X7's, as M3's U11 uses.
fn slack(x: f64) -> f64 {
    1e-6 * x.abs().max(1.0)
}

/// A zone between stations `a` and `b`, its owner's radius `r` and the
/// widest radius beside it `wide`: the crossed faces it holds, and whether
/// VR8 takes it (every one of them outside a hole's cylinder at `r`,
/// inside a shaft's).
fn zone(
    crossed: &[Crossed],
    (a, b): (f64, f64),
    (r, wide): (f64, f64),
    hole: bool,
) -> (Vec<&Crossed>, bool) {
    let (along, s) = (slack(a.abs().max(b.abs())), slack(wide));
    let held: Vec<&Crossed> = crossed
        .iter()
        .filter(|c| {
            c.reach.is_none_or(|x| {
                x.to >= a - along && x.from <= b + along && (hole || x.near <= wide + s)
            })
        })
        .collect();
    let safe = held.iter().all(|c| {
        c.reach.is_some_and(|x| {
            if hole {
                x.near >= r - s
            } else {
                x.far <= r + s
            }
        })
    });
    // A zone of some length crosses some face; one that crosses none is
    // not read.
    let taken = safe && (b - a <= along || !held.is_empty());
    (held, taken)
}

/// A zone's cone, if its faces are one cone's.
fn taper(held: &[&Crossed]) -> Option<((f64, f64), (f64, f64))> {
    let first = held.first()?;
    let one = held.iter().all(|c| c.feature == first.feature);
    let t = first.reach?.taper?;
    (one && first.feature.kind() == FeatureKind::Cone).then_some(t)
}

/// CF3, VR8: one stack, with its usable profile, ends, exempt faces and
/// wall openings.
fn stack(look: &Lookup<'_>, own: &Own<'_>, read: &Read<'_>) -> Stack {
    let (point, dir) = (
        read.axis.point_mm.to_array(),
        read.axis.direction.to_array(),
    );
    let frame = Frame::new(point, dir);
    let walls = walls(read.segments);
    let mut features: Vec<FeatureId> = read.transitions.to_vec();
    for end in read.ends {
        if let StackEnd::Opening {
            countersink: Some(c),
            ..
        } = end
        {
            features.push(c.cone);
        }
    }
    features.sort_unstable_by_key(|f| (f.instance(), f.kind()));
    features.dedup();
    let crossed: Vec<Crossed> = features
        .iter()
        .flat_map(|f| {
            own.faces_of(*f).iter().filter_map(|k| {
                Some(Crossed {
                    face: own.face(*k)?,
                    feature: *f,
                    reach: own.reach(*k, &frame),
                })
            })
        })
        .collect();
    let mut exempt: BTreeSet<u64> = read
        .segments
        .iter()
        .flat_map(|s| own.faces_of(s.cylinder))
        .filter_map(|k| own.face(*k))
        .collect();
    let mut pieces = Vec::new();
    let mut run = 0;
    for (j, w) in walls.iter().enumerate() {
        pieces.push(Piece {
            lo: w.lo,
            hi: w.hi,
            r: w.r,
            cylinder: Some(w.cylinder),
            taper: None,
            run,
        });
        let Some(next) = walls.get(j + 1) else {
            continue;
        };
        let (narrow, wide) = (w.r.min(next.r), w.r.max(next.r));
        let owner = if read.hole { narrow } else { wide };
        let (held, taken) = zone(&crossed, (w.hi, next.lo), (owner, wide), read.hole);
        if !taken {
            run += 1;
            continue;
        }
        exempt.extend(held.iter().map(|c| c.face));
        if next.lo > w.hi {
            pieces.push(Piece {
                lo: w.hi,
                hi: next.lo,
                r: owner,
                cylinder: None,
                taper: taper(&held),
                run,
            });
        }
    }
    let ends = end_zones(read, &walls, &crossed, (run, &mut pieces), &mut exempt);
    bound_by_posts(look, read, &frame, &mut pieces);
    Stack {
        id: read.id,
        hole: read.hole,
        point,
        dir,
        pieces,
        ends,
        exempt,
        openings: openings(look, read.segments, &frame),
    }
}

/// VR8's end zones: from an opening's or a free end's wall out to its rim,
/// counted at the end segment's radius, taken when every face crossed
/// there stays outside it (a hole) or inside it (a shaft). The stack's
/// ends as verification reads them.
fn end_zones(
    read: &Read<'_>,
    walls: &[Wall],
    crossed: &[Crossed],
    (run, pieces): (usize, &mut Vec<Piece>),
    exempt: &mut BTreeSet<u64>,
) -> [EndAt; 2] {
    // The wall an end zone joins must be the stack's own end segment.
    let last = read.segments.len().saturating_sub(1);
    let whole = |side: usize, wall: &Wall| match side {
        0 => wall.index == 0,
        _ => wall.index == last,
    };
    let mut ends = [EndAt::Unknown; 2];
    for (side, end) in ends.iter_mut().enumerate() {
        // An opening counts only through a zone VR8 takes: one it does not
        // take may hold material back inside the hole (a lip), so the end
        // is not open (CF5, VR7). Bottoms and roots are what they are.
        *end = match end_at(read.ends.get(side), None) {
            EndAt::Open { .. } => EndAt::Unknown,
            other => other,
        };
        let edge = if side == 0 {
            walls.first()
        } else {
            walls.last()
        };
        let (Some(edge), Some((at, rim))) = (
            edge.filter(|w| whole(side, w)),
            open_end(read.ends.get(side)),
        ) else {
            continue;
        };
        let (lo, hi) = (at.min(rim), at.max(rim));
        let (held, taken) = zone(crossed, (lo, hi), (edge.r, edge.r), read.hole);
        if !taken {
            continue;
        }
        exempt.extend(held.iter().map(|c| c.face));
        *end = EndAt::Open {
            wall: at,
            rim: Some(rim),
        };
        if hi > lo {
            let piece = Piece {
                lo,
                hi,
                r: edge.r,
                cylinder: None,
                taper: taper(&held),
                run: if side == 0 { 0 } else { run },
            };
            if side == 0 {
                pieces.insert(0, piece);
            } else {
                pieces.push(piece);
            }
        }
    }
    ends
}

/// An open end's wall end and rim: a hole's opening, a shaft's free end.
fn open_end(end: Option<&StackEnd>) -> Option<(f64, f64)> {
    match end? {
        StackEnd::Opening { at_mm, rim_mm, .. } | StackEnd::FreeEnd { at_mm, rim_mm, .. } => {
            Some((*at_mm, *rim_mm))
        }
        _ => None,
    }
}

/// An end as verification reads it, its zone not yet taken.
fn end_at(end: Option<&StackEnd>, rim: Option<f64>) -> EndAt {
    match end {
        Some(StackEnd::Opening { at_mm, .. } | StackEnd::FreeEnd { at_mm, .. }) => {
            EndAt::Open { wall: *at_mm, rim }
        }
        Some(StackEnd::Bottom { at_mm, .. } | StackEnd::Root { at_mm, .. }) => {
            EndAt::Closed { at: *at_mm }
        }
        _ => EndAt::Unknown,
    }
}

/// VR8: an annular bottom bounds a hole's profile at its post's free end;
/// a post whose free end is unknown, or that is not found on the axis,
/// leaves nothing of the hole usable.
fn bound_by_posts(look: &Lookup<'_>, read: &Read<'_>, frame: &Frame, pieces: &mut Vec<Piece>) {
    if !read.hole {
        return;
    }
    for (side, end) in read.ends.iter().enumerate() {
        let StackEnd::Bottom {
            bottom: Bottom::Annular { .. },
            closed_by,
            ..
        } = end
        else {
            continue;
        };
        let Some(free) = post_end(look, *closed_by, frame) else {
            pieces.clear();
            return;
        };
        if side == 0 {
            pieces.retain(|p| p.hi > free);
            for p in pieces.iter_mut() {
                p.lo = p.lo.max(free);
            }
        } else {
            pieces.retain(|p| p.lo < free);
            for p in pieces.iter_mut() {
                p.hi = p.hi.min(free);
            }
        }
    }
}

/// The station, in `frame`, of the free end's rim of the shaft on the
/// frame's axis rising from `floor`; `None` if there is none, or its end
/// is not known.
fn post_end(look: &Lookup<'_>, floor: FeatureId, frame: &Frame) -> Option<f64> {
    let post = look.posts.get(&floor)?.iter().find(|s| {
        let (p, d) = (s.axis.point_mm.to_array(), s.axis.direction.to_array());
        norm(cross(d, frame.dir())) <= 1e-9 && frame.distance(p) <= 1e-6
    })?;
    let (p, d) = (
        post.axis.point_mm.to_array(),
        post.axis.direction.to_array(),
    );
    post.ends.iter().find_map(|e| match e {
        StackEnd::FreeEnd { rim_mm, .. } => Some(frame.station(add(p, scale(d, *rim_mm)))),
        _ => None,
    })
}

/// CF8: the stations of the segments whose walls have openings; with an
/// end unknown, the cylinder's own stations carried into the stack's
/// frame, or the whole axis with none.
fn openings(look: &Lookup<'_>, segments: &[StackSegment], frame: &Frame) -> Vec<(f64, f64)> {
    segments
        .iter()
        .filter_map(|s| {
            let cylinder = look.cylinders.get(&s.cylinder)?;
            if cylinder.wall_openings == 0 {
                return None;
            }
            let (p, d) = (
                cylinder.axis.point_mm.to_array(),
                cylinder.axis.direction.to_array(),
            );
            let stations = cylinder
                .stations_mm
                .iter()
                .map(|x| frame.station(add(p, scale(d, *x))));
            let (lo, hi) = stations.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| {
                (a.min(x), b.max(x))
            });
            Some(match (s.from_mm, s.to_mm) {
                (Some(a), Some(b)) => (a.min(b), a.max(b)),
                _ if lo <= hi => (lo, hi),
                _ => (f64::NEG_INFINITY, f64::INFINITY),
            })
        })
        .collect()
}

/// A sound box of every face, from what the part states: a plane face's
/// outer loops, a whole cylinder's end circles, a cone's circles at its
/// radii, a torus's tube; none for the rest, which every query returns.
/// Grown by the largest vertex offset, as M2's boxes are.
fn face_boxes(part: &Part) -> Vec<FaceBox> {
    let grow = part
        .solids
        .iter()
        .map(|s| s.max_vertex_offset_mm)
        .fold(GUARD_MM, f64::max);
    let mut out = Vec::new();
    let mut put = |face: u64, bounds: Option<(V, V)>| {
        let bounds = bounds.map(|(lo, hi)| (lo.map(|x| x - grow), hi.map(|x| x + grow)));
        out.push(FaceBox { face, bounds });
    };
    for plane in &part.planes {
        for face in &plane.faces {
            let bounds = match &face.outline {
                Outline::Known { loops } => loops
                    .iter()
                    .filter(|l| l.outer)
                    .filter_map(|l| loop_box(&l.segments))
                    .reduce(union),
                Outline::Unknown { .. } => None,
            };
            put(face.face, bounds);
        }
    }
    let extent = part
        .extent
        .map(|e| (e.min_mm.to_array(), e.max_mm.to_array()));
    for c in &part.cylinders {
        let bounds = cylinder_box(c).or_else(|| extent.and_then(|e| surface_box(c, e)));
        for face in &c.faces {
            put(*face, bounds);
        }
    }
    for c in &part.cones {
        let bounds = cone_box(c);
        for face in &c.faces {
            put(*face, bounds);
        }
    }
    for t in &part.tori {
        let (c, d) = (t.axis.point_mm.to_array(), t.axis.direction.to_array());
        let r = t.major_radius_mm + t.minor_radius_mm;
        let e = [0, 1, 2]
            .map(|k| r * (1.0 - d[k] * d[k]).max(0.0).sqrt() + t.minor_radius_mm * d[k].abs());
        let bounds = (
            [0, 1, 2].map(|k| c[k] - e[k]),
            [0, 1, 2].map(|k| c[k] + e[k]),
        );
        for face in &t.faces {
            put(*face, Some(bounds));
        }
    }
    for o in &part.other_faces {
        put(o.face, None);
    }
    out
}

fn union(a: (V, V), b: (V, V)) -> (V, V) {
    (
        [0, 1, 2].map(|k| a.0[k].min(b.0[k])),
        [0, 1, 2].map(|k| a.1[k].max(b.1[k])),
    )
}

/// The box of a circle about `c` with unit normal `d` and radius `r`.
fn circle_box(c: V, d: V, r: f64) -> (V, V) {
    let e = [0, 1, 2].map(|k| r * (1.0 - d[k] * d[k]).max(0.0).sqrt());
    (
        [0, 1, 2].map(|k| c[k] - e[k]),
        [0, 1, 2].map(|k| c[k] + e[k]),
    )
}

/// A whole cylinder lies between its end circles.
fn cylinder_box(c: &crate::features::Cylinder) -> Option<(V, V)> {
    if !matches!(c.class, CylinderClass::Full { .. }) {
        return None;
    }
    let (p, d) = (c.axis.point_mm.to_array(), c.axis.direction.to_array());
    let lo = c.stations_mm.iter().copied().reduce(f64::min)?;
    let hi = c.stations_mm.iter().copied().reduce(f64::max)?;
    Some(union(
        circle_box(add(p, scale(d, lo)), d, c.radius_mm),
        circle_box(add(p, scale(d, hi)), d, c.radius_mm),
    ))
}

/// Any cylinder's faces lie on its surface: within the part's box, the
/// stretch of the surface whose axis stations keep it in the box. Along
/// each coordinate a point at station `t` lies within the radius times
/// `√(1 − dₖ²)` of the axis's, which bounds `t`; the box follows from the
/// stations left.
fn surface_box(cylinder: &crate::features::Cylinder, (lo, hi): (V, V)) -> Option<(V, V)> {
    let point = cylinder.axis.point_mm.to_array();
    let dir = cylinder.axis.direction.to_array();
    let across = dir.map(|x| cylinder.radius_mm * (1.0 - x * x).max(0.0).sqrt());
    let (mut first, mut last) = (f64::NEG_INFINITY, f64::INFINITY);
    for k in 0..3 {
        if dir[k] == 0.0 {
            continue;
        }
        let below = (lo[k] - across[k] - point[k]) / dir[k];
        let above = (hi[k] + across[k] - point[k]) / dir[k];
        first = first.max(below.min(above));
        last = last.min(below.max(above));
    }
    if !(first.is_finite() && last.is_finite()) || first > last {
        return None;
    }
    let low =
        [0, 1, 2].map(|k| (point[k] + (first * dir[k]).min(last * dir[k]) - across[k]).max(lo[k]));
    let high =
        [0, 1, 2].map(|k| (point[k] + (first * dir[k]).max(last * dir[k]) + across[k]).min(hi[k]));
    (0..3).all(|k| low[k] <= high[k]).then_some((low, high))
}

/// A cone with known radii lies between its circles at them.
fn cone_box(c: &crate::features::Cone) -> Option<(V, V)> {
    let [inner, outer] = c.radii_mm?;
    let tan = libm::tan(c.semi_angle_rad);
    if !(tan > 0.0 && tan.is_finite()) {
        return None;
    }
    let (apex, d) = (c.apex_mm.to_array(), c.axis.direction.to_array());
    let at = |r: f64| circle_box(add(apex, scale(d, r / tan)), d, r);
    Some(union(at(inner), at(outer)))
}

#[cfg(test)]
mod tests {
    use super::{CandidateView, EndAt, Stack, surface_box, union};
    use crate::features::{FeaturesDocument, Part};

    fn part(bytes: &[u8]) -> Part {
        let read = FeaturesDocument::read(bytes);
        read.part().cloned().unwrap_or_else(|| unreachable!())
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9
    }

    fn pieces(s: &Stack) -> Vec<(f64, f64, f64, bool, usize)> {
        s.pieces
            .iter()
            .map(|p| (p.lo, p.hi, p.r, p.cylinder.is_some(), p.run))
            .collect()
    }

    /// VR8 on the committed parts: a counterbore's step joins its two
    /// walls in one run, its annulus exempt from the core test; a
    /// countersink is an end zone counted at the hole's radius, its cone
    /// kept for a zone's profile; a torus fillet that stays outside the
    /// narrower cylinder is taken; an unknown end is never open.
    #[test]
    fn a_profile_takes_the_zones_whose_faces_stay_outside() {
        let p05 = part(include_bytes!("../../tests/data/p05_cbore.stp"));
        let view = CandidateView::of(&p05);
        let [hole] = view.stacks() else {
            unreachable!()
        };
        assert_eq!(
            pieces(hole),
            [(0.0, 8.5, 3.3, true, 0), (8.5, 15.0, 5.5, true, 0)]
        );
        assert_eq!(hole.exempt.len(), 3);

        let p06 = part(include_bytes!("../../tests/data/p06_csink.stp"));
        let view = CandidateView::of(&p06);
        let [hole] = view.stacks() else {
            unreachable!()
        };
        assert!(
            matches!(hole.ends[1], EndAt::Open { wall, rim: Some(rim) } if close(wall, 7.1) && close(rim, 10.0))
        );
        let zone = hole.pieces.last().copied();
        assert!(zone.is_some_and(|z| z.cylinder.is_none() && close(z.r, 3.3) && z.run == 0));
        let taper = zone.and_then(|z| z.taper);
        assert!(
            taper.is_some_and(|((_, r1), (s2, r2))| (r1 - 3.3).abs() < 1e-9
                && close(s2, 10.0)
                && (r2 - 6.2).abs() < 1e-9)
        );

        let p32 = part(include_bytes!("../../tests/data/p32_torus_cbore.stp"));
        let view = CandidateView::of(&p32);
        let [hole] = view.stacks() else {
            unreachable!()
        };
        assert_eq!(hole.pieces.len(), 4);
        assert!(hole.pieces.iter().all(|p| p.run == 0));

        let u01 = part(include_bytes!("../../tests/data/u01_undercut_floor.stp"));
        let view = CandidateView::of(&u01);
        let unknown = view.stacks().iter().find(|s| s.ends[0] == EndAt::Unknown);
        assert!(unknown.is_some_and(|s| s.pieces.iter().all(|p| p.lo >= 0.0)));

        let p10 = part(include_bytes!("../../tests/data/p10_boss_hole.stp"));
        let view = CandidateView::of(&p10);
        let boss = view.stacks().iter().find(|s| !s.hole);
        assert!(boss.is_some_and(|s| s.ends[0] == EndAt::Closed { at: 0.0 }));
    }

    /// VR22's test on p10, a Ø12 boss rising 10 from a plate with a Ø6.6
    /// hole through both: about the hole's axis, a core of radius 4 meets
    /// nothing but the hole's own wall, which is exempt; one of radius 7
    /// meets the boss's shaft-sense wall; with nothing exempt, the hole's
    /// wall meets the first; and the work stops at its bound.
    #[test]
    fn a_core_meets_every_face_inside_it_but_the_stack_s_own() {
        use crate::socket::own::{Core, Frame};
        use std::collections::BTreeSet;
        let p10 = part(include_bytes!("../../tests/data/p10_boss_hole.stp"));
        let view = CandidateView::of(&p10);
        let Some(hole) = view.stacks().iter().find(|s| s.hole) else {
            unreachable!()
        };
        let frame = Frame::new(hole.point, hole.dir);
        let core = |rho: f64| Core {
            rho,
            hole: true,
            from: (1.0, false),
            to: (9.0, false),
        };
        let mut pairs = 0;
        let mut ask = |rho: f64, exempt: &BTreeSet<u64>, limit: u64| {
            view.own()
                .intrusion(&frame, &core(rho), exempt, (&mut pairs, limit))
                .map(|hit| hit.map(|h| h.1))
        };
        assert!(matches!(ask(4.0, &hole.exempt, u64::MAX), Ok(None)));
        assert!(matches!(ask(7.0, &hole.exempt, u64::MAX), Ok(Some(r)) if close(r, 6.0)));
        assert!(matches!(ask(4.0, &BTreeSet::new(), u64::MAX), Ok(Some(r)) if close(r, 3.3)));
        assert!(ask(4.0, &hole.exempt, 0).is_err());
    }

    /// A box union holds both; a cylinder's surface within a box is boxed
    /// about its axis, along it as far as the box allows.
    #[test]
    fn a_cylinder_s_surface_is_boxed_within_the_part() {
        let u = union(
            ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
            ([-1.0, 2.0, 0.5], [0.0, 3.0, 0.7]),
        );
        assert_eq!(u, ([-1.0, 0.0, 0.0], [1.0, 3.0, 1.0]));
        let p10 = part(include_bytes!("../../tests/data/p10_boss_hole.stp"));
        let extent = ([-100.0, -100.0, -5.0], [100.0, 100.0, 5.0]);
        for c in &p10.cylinders {
            let Some((lo, hi)) = surface_box(c, extent) else {
                unreachable!()
            };
            let (p, r) = (c.axis.point_mm.to_array(), c.radius_mm);
            assert!((lo[0] - (p[0] - r)).abs() < 1e-9 && (hi[1] - (p[1] + r)).abs() < 1e-9);
            assert!(close(lo[2], -5.0) && close(hi[2], 5.0));
        }
    }
}
