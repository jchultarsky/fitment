//! Alignment (`docs/research/m4-match.md`, section 7): where a candidate is
//! placed in a socket's frame. A placement maps the candidate's frame into
//! the target's, `p_target = R · p_candidate + t`, and says where it came
//! from. Placements are built from pairs of candidate features that match
//! the socket's lines, seats and cones, ranked but never filtered by
//! diameter (AL3), in a fixed order (AL12), at most `MAX_PLACEMENTS`.

use std::collections::BTreeSet;

use super::candidate::{CandidateView, Stack};
use super::shortlist::Kept;
use super::{MatchPolicy, SocketView};
use crate::features::{FeatureId, Outline, Segment};
use crate::geom::Rigid;
use crate::socket::geo::{V, add, cross, dot, norm, perpendiculars, scale, sub};
use crate::socket::{Requirement, Requires};
use crate::verdict::RequirementId;

/// The most placements built for one candidate and socket (AL11).
pub const MAX_PLACEMENTS: usize = 4_096;

/// The most pairs of socket lines tried (AL4).
pub const MAX_ANCHOR_PAIRS: usize = 16;

/// The most pairs of candidate stacks compared for one socket (AL11).
const MAX_PAIR_CHECKS: u64 = 4_000_000;

/// Lines this close are one (section 7): in mm, and in sine.
const SAME_LINE: f64 = 1e-6;

/// What a placement was built from.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Seed {
    /// Two socket lines and the candidate stacks on them (AL5, AL6).
    AxisPair {
        /// The first line's requirement and stack.
        a: (RequirementId, FeatureId),
        /// The second's.
        b: (RequirementId, FeatureId),
    },
    /// One socket line and the stack on it (AL8).
    OneAxis {
        /// The line's requirement and stack.
        axis: (RequirementId, FeatureId),
    },
    /// A conical seat and the cone on it (AL9).
    Cone {
        /// The seat and the cone.
        cone: (RequirementId, FeatureId),
    },
    /// A seat and a candidate plane, by a landmark of each (AL9).
    Landmark {
        /// The seat.
        seat: RequirementId,
        /// The plane.
        plane: FeatureId,
    },
    /// The caller gave it (AL14): the construction's, or the identity for
    /// the original.
    Given,
}

/// One placement of a candidate in a socket's frame.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Placement {
    /// Candidate to target.
    pub transform: Rigid,
    /// Where it came from.
    pub seed: Seed,
    /// Its place in the order placements were built.
    pub index: usize,
}

impl Placement {
    /// A placement the caller gives (AL14).
    pub fn given(transform: Rigid) -> Self {
        Self {
            transform,
            seed: Seed::Given,
            index: 0,
        }
    }
}

/// What alignment built for one candidate and socket.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Placements {
    /// The placements, in the order they are verified.
    pub ordered: Vec<Placement>,
    /// How many were built, those past the bound included.
    pub tried: usize,
    /// True if a bound stopped the work (AL11).
    pub exhausted: bool,
    /// The requirements no candidate feature could anchor (KH3).
    pub unanchored: Vec<RequirementId>,
}

impl Placements {
    /// No placement, nothing tried.
    pub(crate) fn none() -> Self {
        Self {
            ordered: Vec::new(),
            tried: 0,
            exhausted: false,
            unanchored: Vec::new(),
        }
    }
}

/// A rotation and a translation, as alignment builds them.
#[derive(Debug, Clone, Copy)]
struct Pose {
    r: [[f64; 3]; 3],
    t: V,
}

impl Pose {
    fn dir(&self, d: V) -> V {
        self.r.map(|row| dot(row, d))
    }

    fn point(&self, p: V) -> V {
        add(self.dir(p), self.t)
    }

    /// The rotation taking orthonormal frame `from` onto `to`, with point
    /// `p` of the candidate onto `q` of the target.
    fn between(from: [V; 3], to: [V; 3], p: V, q: V) -> Self {
        let mut r = [[0.0; 3]; 3];
        for (k, (f, e)) in to.iter().zip(&from).enumerate() {
            let _ = k;
            for (i, row) in r.iter_mut().enumerate() {
                for (j, x) in row.iter_mut().enumerate() {
                    *x += f[i] * e[j];
                }
            }
        }
        let turned = Self { r, t: [0.0; 3] };
        Self {
            r,
            t: sub(q, turned.point(p)),
        }
    }

    fn shifted(&self, by: V) -> Self {
        Self {
            r: self.r,
            t: add(self.t, by),
        }
    }

    /// This pose, then a turn by `angle` about the line through `c` along
    /// unit `axis`.
    fn turned(&self, c: V, axis: V, angle: f64) -> Self {
        let (s, co) = (libm::sin(angle), libm::cos(angle));
        let rot = |v: V| -> V {
            let k = dot(axis, v);
            let x = cross(axis, v);
            [0, 1, 2].map(|i| v[i] * co + x[i] * s + axis[i] * k * (1.0 - co))
        };
        let columns = [0, 1, 2].map(|j| rot([self.r[0][j], self.r[1][j], self.r[2][j]]));
        let r = [0, 1, 2].map(|i| [columns[0][i], columns[1][i], columns[2][i]]);
        Self {
            r,
            t: add(rot(sub(self.t, c)), c),
        }
    }
}

/// A unit frame whose first axis is `e1` and whose second lies in the
/// plane of `e1` and `x`.
fn frame(e1: V, x: V) -> Option<[V; 3]> {
    let w = sub(x, scale(e1, dot(x, e1)));
    let n = norm(w);
    (n > 1e-12 && n.is_finite()).then(|| {
        let e2 = scale(w, 1.0 / n);
        [e1, e2, cross(e1, e2)]
    })
}

/// A unit frame whose first axis is `e1`.
fn any_frame(e1: V) -> [V; 3] {
    let (u, w) = perpendiculars(e1);
    [e1, u, w]
}

/// The distance of `q` from the line through `p` along unit `d`.
fn off_line(q: V, p: V, d: V) -> f64 {
    let w = sub(q, p);
    norm(sub(w, scale(d, dot(w, d))))
}

/// A socket line (section 7): the axis of active bores and shafts M4 can
/// verify, one per distinct line, whatever their sense (a tube's bore and
/// outside are one line), with the diameter windows its spans allow by
/// sense (AL3) and their stations (AL7 (c)).
#[derive(Debug, Clone)]
struct Line {
    p: V,
    d: V,
    ids: Vec<RequirementId>,
    /// (hole, lo, hi): a window on a hole's or a shaft's diameters.
    windows: Vec<(bool, f64, f64)>,
    stations: Vec<f64>,
}

impl Line {
    /// True if a stack of this sense may anchor it.
    fn takes(&self, hole: bool) -> bool {
        self.windows.iter().any(|w| w.0 == hole)
    }
}

/// The lines of the requirements alignment may anchor on.
fn lines(active: &[&Requirement], policy: &MatchPolicy) -> Vec<Line> {
    let t = policy.linear_mm();
    let strict = policy.clearance() == super::Clearance::Strict;
    let mut out: Vec<Line> = Vec::new();
    for r in active {
        let (axis, hole, spans) = match &r.requires {
            Requires::Bore { axis, spans, .. } => (
                axis,
                true,
                spans
                    .iter()
                    .map(|s| {
                        let d = s.hole_diameter_mm;
                        let window = match (s.pinned, s.clear_diameter_mm) {
                            (true, _) => (d - t, d + t),
                            (false, Some(c)) => (c, if strict { d + t } else { f64::INFINITY }),
                            (false, None) => (0.0, f64::INFINITY),
                        };
                        (window, [s.from_mm, s.to_mm])
                    })
                    .collect::<Vec<_>>(),
            ),
            Requires::Shaft { axis, spans, .. } => (
                axis,
                false,
                spans
                    .iter()
                    .map(|s| {
                        let d = s.shaft_diameter_mm;
                        let window = match (s.pinned, s.clear_diameter_mm) {
                            (true, _) => (d - t, d + t),
                            (false, Some(c)) => (if strict { d - t } else { 0.0 }, c),
                            (false, None) => (0.0, f64::INFINITY),
                        };
                        (window, [s.from_mm, s.to_mm])
                    })
                    .collect(),
            ),
            _ => continue,
        };
        let (p, d) = (axis.point_mm.to_array(), axis.direction.to_array());
        let id = RequirementId::new(r.id.clone());
        let same = out
            .iter_mut()
            .find(|l| norm(cross(l.d, d)) < SAME_LINE && off_line(p, l.p, l.d) < SAME_LINE);
        let line = if let Some(l) = same {
            l
        } else {
            out.push(Line {
                p,
                d,
                ids: Vec::new(),
                windows: Vec::new(),
                stations: Vec::new(),
            });
            let Some(last) = out.last_mut() else {
                continue;
            };
            last
        };
        let k = dot(d, line.d);
        let shift = dot(sub(p, line.p), line.d);
        line.ids.push(id);
        for ((lo, hi), stations) in spans {
            line.windows.push((hole, lo, hi));
            line.stations.extend(stations.map(|s| shift + k * s));
        }
    }
    out
}

/// The candidate stacks that may lie on `line` (AL2): of its sense, within
/// the shortlist's domains when it kept the candidate, those whose
/// diameters could meet every span first (AL3), each group by id.
fn anchors<'c>(line: &Line, cand: &'c CandidateView<'_>, kept: Option<&Kept>) -> Vec<&'c Stack> {
    let allowed: Option<BTreeSet<FeatureId>> = kept.and_then(|k| {
        let named: Vec<&Vec<FeatureId>> = k
            .domains
            .iter()
            .filter(|(id, _)| line.ids.contains(id))
            .map(|(_, d)| d)
            .collect();
        (!named.is_empty()).then(|| named.into_iter().flatten().copied().collect())
    });
    let of_line: Vec<&Stack> = cand
        .stacks()
        .iter()
        .filter(|s| line.takes(s.hole))
        .filter(|s| allowed.as_ref().is_none_or(|a| a.contains(&s.id)))
        .collect();
    let fits = |s: &Stack| {
        line.windows
            .iter()
            .filter(|w| w.0 == s.hole)
            .all(|(_, lo, hi)| {
                s.pieces
                    .iter()
                    .filter(|p| p.cylinder.is_some())
                    .any(|p| 2.0 * p.r > *lo && 2.0 * p.r < *hi)
            })
    };
    let (mut first, rest): (Vec<&Stack>, Vec<&Stack>) = of_line.into_iter().partition(|s| fits(s));
    first.extend(rest);
    first
}

/// What placements are built from, and where they go.
struct Builder<'a, 'c> {
    cand: &'a CandidateView<'c>,
    policy: &'a MatchPolicy,
    /// The verifiable requirements alignment reads.
    active: Vec<&'a Requirement>,
    out: Vec<Placement>,
    tried: usize,
    pairs: u64,
    exhausted: bool,
}

impl Builder<'_, '_> {
    fn push(&mut self, pose: &Pose, seed: &Seed) {
        self.tried += 1;
        if self.out.len() >= MAX_PLACEMENTS {
            self.exhausted = true;
            return;
        }
        if let Some(transform) = Rigid::new(pose.r, pose.t) {
            let index = self.out.len();
            self.out.push(Placement {
                transform,
                seed: seed.clone(),
                index,
            });
        }
    }

    fn full(&self) -> bool {
        self.exhausted || self.out.len() >= MAX_PLACEMENTS
    }

    /// One pair comparison; false past the bound.
    fn compare(&mut self) -> bool {
        self.charge(1)
    }

    /// `n` comparisons; false past the bound, which ends alignment.
    fn charge(&mut self, n: usize) -> bool {
        self.pairs = self
            .pairs
            .saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        if self.pairs > MAX_PAIR_CHECKS {
            self.exhausted = true;
        }
        !self.exhausted
    }

    fn sine(&self) -> f64 {
        libm::sin(self.policy.angular_rad())
    }
}

/// AL1 to AL12: every placement worth verifying for `cand` in `socket`,
/// in a fixed order, at most `MAX_PLACEMENTS`. `kept` is the shortlist's
/// verdict when it kept the candidate: its domains bound the anchors.
pub fn align(
    socket: &SocketView<'_>,
    cand: &CandidateView<'_>,
    kept: Option<&Kept>,
    policy: &MatchPolicy,
) -> Placements {
    let active: Vec<&Requirement> = socket
        .active()
        .iter()
        .copied()
        .filter(|r| r.verifiable() && !socket.blocked(r, policy))
        .collect();
    let lines = lines(&active, policy);
    let anchored: Vec<Vec<&Stack>> = lines.iter().map(|l| anchors(l, cand, kept)).collect();
    let unanchored = lines
        .iter()
        .zip(&anchored)
        .filter(|(_, a)| a.is_empty())
        .flat_map(|(l, _)| l.ids.iter().cloned())
        .collect();
    let mut b = Builder {
        cand,
        policy,
        active,
        out: Vec::new(),
        tried: 0,
        pairs: 0,
        exhausted: false,
    };
    match lines.len() {
        0 => no_line(&mut b),
        1 => {
            if let (Some(line), Some(anchors)) = (lines.first(), anchored.first()) {
                one_line(&mut b, line, anchors);
            }
        }
        _ => line_pairs(&mut b, &lines, &anchored),
    }
    Placements {
        ordered: b.out,
        tried: b.tried,
        exhausted: b.exhausted,
        unanchored,
    }
}

/// AL4: pairs of lines, non-parallel by decreasing sine, then parallel by
/// decreasing separation; the first pair that yields a placement is used.
fn line_pairs(b: &mut Builder<'_, '_>, lines: &[Line], anchored: &[Vec<&Stack>]) {
    let mut pairs: Vec<(usize, usize, bool, f64)> = Vec::new();
    let t = b.policy.linear_mm();
    for (i, l1) in lines.iter().enumerate() {
        for (j, l2) in lines.iter().enumerate().skip(i + 1) {
            let sine = norm(cross(l1.d, l2.d));
            let skew = sine > b.sine() * 2.0 + 1e-9;
            let score = if skew {
                sine
            } else {
                off_line(l2.p, l1.p, l1.d)
            };
            // Parallel lines closer than the tolerance fix no turn.
            if skew || score > 2.0 * t {
                pairs.push((i, j, skew, score));
            }
        }
    }
    if pairs.is_empty() {
        if let (Some(line), Some(anchors)) = (lines.first(), anchored.first()) {
            one_line(b, line, anchors);
        }
        return;
    }
    pairs.sort_by(|x, y| {
        y.2.cmp(&x.2)
            .then(y.3.total_cmp(&x.3))
            .then(x.0.cmp(&y.0))
            .then(x.1.cmp(&y.1))
    });
    for (i, j, skew, _) in pairs.into_iter().take(MAX_ANCHOR_PAIRS) {
        let (Some(l1), Some(l2), Some(a1), Some(a2)) =
            (lines.get(i), lines.get(j), anchored.get(i), anchored.get(j))
        else {
            continue;
        };
        if skew {
            skew_pair(b, (l1, a1), (l2, a2));
        } else {
            parallel_pair(b, (l1, a1), (l2, a2));
        }
        if !b.out.is_empty() || b.exhausted {
            return;
        }
    }
}

/// AL5: candidate pairs parallel within twice the angular tolerance whose
/// separation is within 2t of the lines'; both signs; the midpoint of the
/// candidate's feet onto the socket's, then slides (AL7).
fn parallel_pair(
    b: &mut Builder<'_, '_>,
    (l1, a1): (&Line, &[&Stack]),
    (l2, a2): (&Line, &[&Stack]),
) {
    let t = b.policy.linear_mm();
    let sine = 2.0 * b.sine() + 1e-12;
    let ws = sub(sub(l2.p, l1.p), scale(l1.d, dot(sub(l2.p, l1.p), l1.d)));
    let separation = norm(ws);
    for a in a1 {
        for c in a2 {
            if !b.compare() {
                return;
            }
            if a.id == c.id || norm(cross(a.dir, c.dir)) >= sine {
                continue;
            }
            let wc = sub(
                sub(c.point, a.point),
                scale(a.dir, dot(sub(c.point, a.point), a.dir)),
            );
            if (norm(wc) - separation).abs() >= 2.0 * t {
                continue;
            }
            for sign in [1.0, -1.0] {
                let ds = scale(l1.d, sign);
                let (Some(from), Some(to)) = (frame(a.dir, wc), frame(ds, ws)) else {
                    continue;
                };
                let pose = Pose::between(
                    from,
                    to,
                    add(a.point, scale(wc, 0.5)),
                    add(l1.p, scale(ws, 0.5)),
                );
                let seed = Seed::AxisPair {
                    a: (first_id(l1), a.id),
                    b: (first_id(l2), c.id),
                };
                slides(b, &pose, ds, &[(l1, *a), (l2, *c)], &seed);
                if b.full() {
                    return;
                }
            }
        }
    }
}

fn first_id(line: &Line) -> RequirementId {
    line.ids
        .first()
        .cloned()
        .unwrap_or_else(|| RequirementId::new(""))
}

/// The closest points of two lines that are not parallel.
fn closest(p1: V, d1: V, p2: V, d2: V) -> Option<(V, V)> {
    let between = sub(p1, p2);
    let (aa, ab, bb) = (dot(d1, d1), dot(d1, d2), dot(d2, d2));
    let (wa, wb) = (dot(d1, between), dot(d2, between));
    let den = aa * bb - ab * ab;
    if den.abs() <= 1e-18 {
        return None;
    }
    let along1 = (ab * wb - bb * wa) / den;
    let along2 = (aa * wb - ab * wa) / den;
    Some((add(p1, scale(d1, along1)), add(p2, scale(d2, along2))))
}

/// AL6: candidate pairs whose angle matches within twice the angular
/// tolerance for some signs, and whose common perpendicular's length
/// within 2t; the frame of the two directions onto the lines', the
/// midpoint of the closest points onto the socket's. Nothing slides.
fn skew_pair(b: &mut Builder<'_, '_>, (l1, a1): (&Line, &[&Stack]), (l2, a2): (&Line, &[&Stack])) {
    let t = b.policy.linear_mm();
    let sine = 2.0 * b.sine() + 1e-12;
    let Some((q1, q2)) = closest(l1.p, l1.d, l2.p, l2.d) else {
        return;
    };
    let gap = norm(sub(q2, q1));
    for a in a1 {
        for c in a2 {
            if !b.compare() {
                return;
            }
            if a.id == c.id {
                continue;
            }
            let Some((k1, k2)) = closest(a.point, a.dir, c.point, c.dir) else {
                continue;
            };
            if (norm(sub(k2, k1)) - gap).abs() >= 2.0 * t {
                continue;
            }
            for (s1, s2) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
                let (e1, e2) = (scale(l1.d, s1), scale(l2.d, s2));
                if (dot(a.dir, c.dir) - dot(e1, e2)).abs() >= sine {
                    continue;
                }
                let (Some(from), Some(to)) = (frame(a.dir, c.dir), frame(e1, e2)) else {
                    continue;
                };
                let mid = |x: V, y: V| scale(add(x, y), 0.5);
                let pose = Pose::between(from, to, mid(k1, k2), mid(q1, q2));
                let seed = Seed::AxisPair {
                    a: (first_id(l1), a.id),
                    b: (first_id(l2), c.id),
                };
                b.push(&pose, &seed);
                if b.full() {
                    return;
                }
            }
        }
    }
}

/// AL7: the slides along `ds` worth trying from `pose`: (a) the most
/// selective seat across the axis, a slide per candidate plane facing it;
/// (b) a conical seat on the axis, apex onto apex; (c) otherwise the
/// breakpoints that put each span end of the anchors' requirements on
/// each end of their profile pieces, and the midpoints between them.
fn slides(b: &mut Builder<'_, '_>, pose: &Pose, ds: V, anchors: &[(&Line, &Stack)], seed: &Seed) {
    let mut lams = seat_slides(b, pose, ds);
    if lams.is_empty() {
        lams = cone_slides(b, pose, ds, anchors);
    }
    if lams.is_empty() {
        lams = breakpoints(pose, ds, anchors);
    }
    for lam in lams {
        b.push(&pose.shifted(scale(ds, lam)), seed);
        if b.full() {
            return;
        }
    }
}

/// AL7 (a): of the seats across the axis that some candidate plane faces,
/// the one with fewest such planes; a slide per plane.
fn seat_slides(b: &mut Builder<'_, '_>, pose: &Pose, ds: V) -> Vec<f64> {
    let cos = libm::cos(b.policy.angular_rad());
    let mut best: Option<Vec<f64>> = None;
    let planes = b.cand.part().planes.len();
    for r in b.active.clone() {
        let Requires::Seat {
            normal, offset_mm, ..
        } = &r.requires
        else {
            continue;
        };
        let n = normal.to_array();
        let along = dot(n, ds);
        if along.abs() < cos || !b.charge(planes) {
            continue;
        }
        let lams: Vec<f64> = shell_planes(b.cand)
            .filter_map(|(cn, co)| {
                let placed = pose.dir(cn);
                (dot(placed, n) > cos).then(|| {
                    let on = pose.point(scale(cn, co));
                    (offset_mm - dot(placed, on)) / along
                })
            })
            .collect();
        if !lams.is_empty() && best.as_ref().is_none_or(|x| lams.len() < x.len()) {
            best = Some(lams);
        }
    }
    best.unwrap_or_default()
}

/// The candidate's outer-shell planes: (unit normal, offset).
fn shell_planes<'c>(cand: &'c CandidateView<'_>) -> impl Iterator<Item = (V, f64)> + 'c {
    cand.part()
        .planes
        .iter()
        .filter(|p| p.shell == 0)
        .map(|p| (p.normal.to_array(), p.offset_mm))
}

/// AL7 (b): a conical seat on the axis; a slide per candidate cone whose
/// placed apex lies within 2t of the line.
fn cone_slides(b: &Builder<'_, '_>, pose: &Pose, ds: V, anchors: &[(&Line, &Stack)]) -> Vec<f64> {
    let t = b.policy.linear_mm();
    let Some((line, _)) = anchors.first() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for r in &b.active {
        let Requires::ConicalSeat { apex_mm, .. } = &r.requires else {
            continue;
        };
        let apex = apex_mm.to_array();
        if off_line(apex, line.p, line.d) >= 2.0 * t {
            continue;
        }
        for cone in b.cand.walked_cones() {
            let placed = pose.point(cone.apex_mm.to_array());
            if off_line(placed, line.p, line.d) < 2.0 * t {
                out.push(dot(sub(apex, placed), ds));
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

/// AL7 (c): span ends against profile piece ends, and the midpoints.
fn breakpoints(pose: &Pose, ds: V, anchors: &[(&Line, &Stack)]) -> Vec<f64> {
    let mut lams = Vec::new();
    for (line, stack) in anchors {
        let (p, d) = (pose.point(stack.point), pose.dir(stack.dir));
        let sign = dot(ds, line.d);
        for piece in &stack.pieces {
            for e in [piece.lo, piece.hi] {
                let at = dot(sub(add(p, scale(d, e)), line.p), line.d);
                lams.extend(line.stations.iter().map(|s| sign * (s - at)));
            }
        }
    }
    lams.retain(|x| x.is_finite());
    lams.sort_by(f64::total_cmp);
    lams.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
    let mids: Vec<f64> = lams.windows(2).map(|w| f64::midpoint(w[0], w[1])).collect();
    lams.extend(mids);
    if lams.is_empty() {
        lams.push(0.0);
    }
    lams
}

/// AL8: one line. Each anchor and sign: the candidate's axis onto the
/// line, its point onto the line's, then the turns worth trying and the
/// slides.
fn one_line(b: &mut Builder<'_, '_>, line: &Line, anchors: &[&Stack]) {
    for a in anchors {
        for sign in [1.0, -1.0] {
            let ds = scale(line.d, sign);
            let base = Pose::between(any_frame(a.dir), any_frame(ds), a.point, line.p);
            let seed = Seed::OneAxis {
                axis: (first_id(line), a.id),
            };
            for angle in turns(b, &base, line.p, ds) {
                let pose = base.turned(line.p, ds, angle);
                slides(b, &pose, ds, &[(line, *a)], &seed);
                if b.full() {
                    return;
                }
            }
        }
    }
}

/// True if every requirement alignment reads is symmetric about the line
/// through `p` along `d` (AL8): bores, shafts and conical seats on it,
/// seats square to it whose contacts are whole circles about it.
fn symmetric(b: &Builder<'_, '_>, p: V, d: V) -> bool {
    let on = |q: V, e: V| norm(cross(e, d)) < 1e-9 && off_line(q, p, d) < SAME_LINE;
    b.active.iter().all(|r| match &r.requires {
        Requires::Bore { axis, .. }
        | Requires::Shaft { axis, .. }
        | Requires::ConicalSeat { axis, .. } => {
            on(axis.point_mm.to_array(), axis.direction.to_array())
        }
        Requires::Seat {
            normal, contacts, ..
        } => {
            norm(cross(normal.to_array(), d)) < 1e-9
                && contacts.iter().all(|c| match &c.outline {
                    Outline::Known { loops } => loops.iter().flat_map(|l| &l.segments).all(|s| {
                        matches!(s, Segment::Arc { center_mm, sweep_rad, .. }
                            if sweep_rad.abs() > 2.0 * std::f64::consts::PI - 1e-9
                                && off_line(center_mm.to_array(), p, d) < SAME_LINE)
                    }),
                    Outline::Unknown { .. } => false,
                })
        }
        _ => true,
    })
}

/// The signed angle about unit `axis` from `from` to `to`, both projected
/// across it.
fn angle_about(axis: V, from: V, to: V) -> f64 {
    let f = sub(from, scale(axis, dot(from, axis)));
    let t = sub(to, scale(axis, dot(to, axis)));
    libm::atan2(dot(cross(f, t), axis), dot(f, t))
}

/// AL8's turns about the placed axis: none when the socket is symmetric
/// about it; else from seats across it (a target line edge against a
/// candidate line edge on a plane facing the same way), from seats along
/// it (candidate planes at the same angle to it), and from off-axis arcs
/// of one radius at one distance from it. Ascending, each once.
fn turns(b: &mut Builder<'_, '_>, base: &Pose, p: V, axis: V) -> Vec<f64> {
    if symmetric(b, p, axis) {
        return vec![0.0];
    }
    let cos = libm::cos(b.policy.angular_rad());
    let t = b.policy.linear_mm();
    let mut out = Vec::new();
    let cand = b.cand;
    for r in b.active.clone() {
        let Requires::Seat {
            normal, contacts, ..
        } = &r.requires
        else {
            continue;
        };
        let n = normal.to_array();
        let target = edges(contacts.iter().map(|c| &c.outline), &|x| x);
        let facing = cand
            .part()
            .planes
            .iter()
            .filter(|cp| cp.shell == 0 && dot(base.dir(cp.normal.to_array()), n) > cos);
        if norm(cross(n, axis)) < 1e-9 {
            for cp in facing {
                let placed = edges(cp.faces.iter().map(|f| &f.outline), &|x| base.point(x));
                // Each pair of edges is a comparison (AL11).
                if !b.charge(placed.len().saturating_mul(target.len()).max(1)) {
                    break;
                }
                for (dc, arc_c) in &placed {
                    for (dt, arc_t) in &target {
                        match (arc_c, arc_t) {
                            (None, None) => out.push(angle_about(axis, *dc, *dt)),
                            (Some((cc, rc)), Some((ct, rt))) if (rc - rt).abs() < 2.0 * t => {
                                let (ac, at) = (sub(*cc, p), sub(*ct, p));
                                let (fc, ft) = (
                                    norm(sub(ac, scale(axis, dot(ac, axis)))),
                                    norm(sub(at, scale(axis, dot(at, axis)))),
                                );
                                if fc > t && (fc - ft).abs() < 2.0 * t {
                                    out.push(angle_about(axis, ac, at));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        } else if b.charge(cand.part().planes.len()) {
            for (cn, _) in shell_planes(cand) {
                let placed = base.dir(cn);
                if (dot(placed, axis) - dot(n, axis)).abs() < 1e-6 {
                    out.push(angle_about(axis, placed, n));
                }
            }
        }
    }
    out.retain(|x| x.is_finite());
    out.sort_by(f64::total_cmp);
    out.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
    if out.is_empty() {
        out.push(0.0);
    }
    out
}

/// The edges of `outlines`, moved by `place`: a line edge by its direction;
/// an arc by its centre and radius.
fn edges<'o>(
    outlines: impl Iterator<Item = &'o Outline>,
    place: &dyn Fn(V) -> V,
) -> Vec<(V, Option<(V, f64)>)> {
    let mut out = Vec::new();
    for outline in outlines {
        let Outline::Known { loops } = outline else {
            continue;
        };
        for s in loops.iter().flat_map(|l| &l.segments) {
            match s {
                Segment::Line { from_mm, to_mm, .. } => {
                    out.push((
                        sub(place(to_mm.to_array()), place(from_mm.to_array())),
                        None,
                    ));
                }
                Segment::Arc {
                    center_mm,
                    radius_mm,
                    ..
                } => out.push(([0.0; 3], Some((place(center_mm.to_array()), *radius_mm)))),
            }
        }
    }
    out
}

/// AL9: no line. A conical seat fixes its axis and apex, then turns as
/// AL8; otherwise landmarks of the seat with the fewest outline segments.
fn no_line(b: &mut Builder<'_, '_>) {
    let tolerance = b.policy.angular_rad();
    let cones: Vec<(RequirementId, V, V, f64)> = b
        .active
        .iter()
        .filter_map(|r| match &r.requires {
            Requires::ConicalSeat {
                axis,
                apex_mm,
                included_angle_rad,
                ..
            } => Some((
                RequirementId::new(r.id.clone()),
                axis.direction.to_array(),
                apex_mm.to_array(),
                *included_angle_rad,
            )),
            _ => None,
        })
        .collect();
    for (id, d, apex, angle) in cones {
        for cone in b.cand.walked_cones() {
            if (2.0 * cone.semi_angle_rad - angle).abs() >= 2.0 * tolerance {
                continue;
            }
            let base = Pose::between(
                any_frame(cone.axis.direction.to_array()),
                any_frame(d),
                cone.apex_mm.to_array(),
                apex,
            );
            let seed = Seed::Cone {
                cone: (id.clone(), cone.id),
            };
            for angle in turns(b, &base, apex, d) {
                b.push(&base.turned(apex, d, angle), &seed);
                if b.full() {
                    return;
                }
            }
        }
        if !b.out.is_empty() {
            return;
        }
    }
    landmarks(b);
}

/// One landmark: a vertex with its outgoing line edge, or an arc's centre
/// with its radius.
enum Mark {
    Vertex { at: V, along: V },
    Centre { at: V, radius: f64, to_start: V },
}

fn marks<'o>(outlines: impl Iterator<Item = &'o Outline>) -> Vec<Mark> {
    let mut out = Vec::new();
    for outline in outlines {
        let Outline::Known { loops } = outline else {
            continue;
        };
        for s in loops.iter().flat_map(|l| &l.segments) {
            match s {
                Segment::Line { from_mm, to_mm, .. } => out.push(Mark::Vertex {
                    at: from_mm.to_array(),
                    along: sub(to_mm.to_array(), from_mm.to_array()),
                }),
                Segment::Arc {
                    from_mm,
                    center_mm,
                    radius_mm,
                    ..
                } => out.push(Mark::Centre {
                    at: center_mm.to_array(),
                    radius: *radius_mm,
                    to_start: sub(from_mm.to_array(), center_mm.to_array()),
                }),
            }
        }
    }
    out
}

/// AL9's landmark pairs on the seat with the fewest outline segments:
/// normals together, a candidate vertex with its edge onto a target
/// vertex with its edge, or an arc's centre onto one of the same radius
/// within 2t; pairs whose edge lengths or radii agree first.
fn landmarks(b: &mut Builder<'_, '_>) {
    let t = b.policy.linear_mm();
    let seat = b
        .active
        .iter()
        .filter_map(|r| match &r.requires {
            Requires::Seat {
                normal, contacts, ..
            } => {
                let target = marks(contacts.iter().map(|c| &c.outline));
                Some((RequirementId::new(r.id.clone()), normal.to_array(), target))
            }
            _ => None,
        })
        .min_by_key(|s| s.2.len());
    let Some((id, n, target)) = seat else {
        return;
    };
    let mut ranked: Vec<(f64, usize, Pose, FeatureId)> = Vec::new();
    for plane in b.cand.part().planes.iter().filter(|p| p.shell == 0) {
        let cn = plane.normal.to_array();
        let candidate = marks(plane.faces.iter().map(|f| &f.outline));
        // Each pair of landmarks is a comparison (AL11).
        if !b.charge(candidate.len().saturating_mul(target.len()).max(1)) {
            break;
        }
        for c in &candidate {
            for m in &target {
                let (rank, from, to, at_c, at_t) = match (c, m) {
                    (Mark::Vertex { at, along }, Mark::Vertex { at: ta, along: tl }) => (
                        (norm(*along) - norm(*tl)).abs(),
                        frame(cn, *along),
                        frame(n, *tl),
                        *at,
                        *ta,
                    ),
                    (
                        Mark::Centre {
                            at,
                            radius,
                            to_start,
                        },
                        Mark::Centre {
                            at: ta,
                            radius: tr,
                            to_start: ts,
                        },
                    ) if (radius - tr).abs() < 2.0 * t => (
                        (radius - tr).abs(),
                        frame(cn, *to_start),
                        frame(n, *ts),
                        *at,
                        *ta,
                    ),
                    _ => continue,
                };
                let (Some(from), Some(to)) = (from, to) else {
                    continue;
                };
                let order = ranked.len();
                ranked.push((rank, order, Pose::between(from, to, at_c, at_t), plane.id));
            }
        }
        if ranked.len() > 4 * MAX_PLACEMENTS {
            break;
        }
    }
    ranked.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
    for (_, _, pose, plane) in ranked {
        let seed = Seed::Landmark {
            seat: id.clone(),
            plane,
        };
        b.push(&pose, &seed);
        if b.full() {
            return;
        }
    }
}
