//! Signatures (`docs/research/m4-match.md`, CF and SG): what a candidate
//! offers, in values a rigid motion keeps, and what a socket demands of
//! them under a policy, as windows at least as wide as verification
//! accepts (SG6). The shortlist compares the two; it never passes.

use serde::Serialize;

use super::policy::{Clearance, MatchPolicy};
use super::{SIGNATURE_VERSION, SocketView};
use crate::features::{
    Axis, Bottom, CylinderClass, FeatureId, Outline, Part, Segment, Sense, StackEnd, StackSegment,
};
use crate::geom::Dir3;
use crate::socket::geo::{V, add, cross, dot, norm, scale, sub};
use crate::socket::{BoreSpan, Requires, ShaftSpan, SpanBasis};
use crate::verdict::RequirementId;

/// A closed interval, either end unbounded (SG6).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Window {
    /// Its least value; `None` for none.
    pub lo: Option<f64>,
    /// Its greatest.
    pub hi: Option<f64>,
}

impl Window {
    /// [`lo`, `hi`], each widened by the rounding slack 1e-9 (1 + |v|).
    pub(crate) fn new(lo: Option<f64>, hi: Option<f64>) -> Self {
        Self {
            lo: lo.map(|v| v - slack(v)),
            hi: hi.map(|v| v + slack(v)),
        }
    }

    /// True if `v` lies in it, ends included.
    pub fn holds(&self, v: f64) -> bool {
        self.lo.is_none_or(|lo| v >= lo) && self.hi.is_none_or(|hi| v <= hi)
    }

    /// Its width; infinite when unbounded.
    pub(crate) fn width(&self) -> f64 {
        match (self.lo, self.hi) {
            (Some(lo), Some(hi)) => hi - lo,
            _ => f64::INFINITY,
        }
    }
}

/// SG6's rounding slack.
pub(crate) fn slack(v: f64) -> f64 {
    1e-9 * (1.0 + v.abs())
}

/// One hole or shaft stack a candidate offers (CF3, CF5).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct SigStack {
    /// The hole or shaft.
    pub feature: FeatureId,
    /// Hole or shaft.
    pub sense: Sense,
    /// Its axis, in the candidate's frame.
    pub axis: Axis,
    /// Its segments' diameters, each a full cylinder.
    pub diameters_mm: Vec<f64>,
    /// Its extent along its axis, rim to rim (a bottom's tip or floor
    /// included); `None` on a side whose end is unknown or irregular.
    pub extent_mm: [Option<f64>; 2],
    /// Its `opening` ends.
    pub openings: u8,
    /// True if an end is unknown or irregular.
    pub unknown_end: bool,
}

/// One plane a candidate offers (CF6).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct SigPlane {
    /// The plane.
    pub feature: FeatureId,
    /// Its outward normal.
    pub normal: Dir3,
    /// Its offset along the normal.
    pub offset_mm: f64,
    /// The area of its faces of known outline.
    pub known_area_mm2: f64,
    /// Its faces of known outline.
    pub faces_known: u32,
    /// Its faces of unknown outline.
    pub faces_unknown: u32,
}

/// One cone a candidate offers (CF7).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct SigCone {
    /// The cone.
    pub feature: FeatureId,
    /// Its axis, along which it widens.
    pub axis: Axis,
    /// Its apex.
    pub apex_mm: [f64; 3],
    /// Its included angle.
    pub included_angle_rad: f64,
    /// Hole (a countersink) or shaft.
    pub sense: Sense,
    /// Its radial range; `None` if unknown, when it serves nothing (CF4).
    pub radii_mm: Option<[f64; 2]>,
}

/// What of a candidate M2 left unread (CF4): it never serves a
/// requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[non_exhaustive]
pub struct Unread {
    /// Undecided cylinders.
    pub undecided: u32,
    /// Partial cylinders.
    pub partial: u32,
    /// `other` faces.
    pub other_faces: u32,
    /// Plane faces of unknown outline.
    pub unknown_outline_faces: u32,
    /// Stacks with an unknown or irregular end.
    pub unknown_end_stacks: u32,
}

/// A candidate's signature (SG2): policy-free, so M5 stores it once.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct PartSignature {
    version: u32,
    stacks: Vec<SigStack>,
    planes: Vec<SigPlane>,
    cones: Vec<SigCone>,
    lever_mm: f64,
    uncertainty_mm: f64,
    unread: Unread,
}

impl PartSignature {
    /// The signature of `part` (CF1 to CF10).
    pub fn of(part: &Part) -> Self {
        let mut points: Vec<V> = Vec::new();
        let mut stacks: Vec<SigStack> = part
            .holes
            .iter()
            .map(|h| stack(Sense::Hole, h.id, &h.axis, &h.segments, &h.ends))
            .chain(
                part.shafts
                    .iter()
                    .map(|s| stack(Sense::Shaft, s.id, &s.axis, &s.segments, &s.ends)),
            )
            .collect();
        stacks.sort_by_key(|s| s.feature.instance());
        for s in &stacks {
            let (p, d) = (s.axis.point_mm.to_array(), s.axis.direction.to_array());
            points.extend(s.extent_mm.iter().flatten().map(|t| add(p, scale(d, *t))));
            points.push(p);
        }
        let mut unread = Unread {
            unknown_end_stacks: count(stacks.iter().filter(|s| s.unknown_end)),
            other_faces: count(part.other_faces.iter()),
            ..Unread::default()
        };
        let planes = part
            .planes
            .iter()
            .map(|plane| {
                let mut known = (0.0, 0_u32, 0_u32);
                for face in &plane.faces {
                    if let Outline::Known { loops } = &face.outline {
                        known.0 += loops.iter().map(|l| l.area_mm2).sum::<f64>();
                        known.1 += 1;
                        points.extend(outline_points(&face.outline));
                    } else {
                        known.2 += 1;
                    }
                }
                unread.unknown_outline_faces += known.2;
                SigPlane {
                    feature: plane.id,
                    normal: plane.normal,
                    offset_mm: plane.offset_mm,
                    known_area_mm2: known.0,
                    faces_known: known.1,
                    faces_unknown: known.2,
                }
            })
            .collect();
        let cones = part
            .cones
            .iter()
            .map(|c| {
                points.push(c.apex_mm.to_array());
                SigCone {
                    feature: c.id,
                    axis: c.axis,
                    apex_mm: c.apex_mm.to_array(),
                    included_angle_rad: 2.0 * c.semi_angle_rad,
                    sense: c.sense,
                    radii_mm: c.radii_mm,
                }
            })
            .collect();
        for c in &part.cylinders {
            match c.class {
                CylinderClass::Partial { .. } => unread.partial += 1,
                CylinderClass::Undecided { .. } => unread.undecided += 1,
                _ => {}
            }
        }
        if let Some(e) = &part.extent {
            points.extend([e.min_mm.to_array(), e.max_mm.to_array()]);
        }
        let uncertainty = part
            .solids
            .iter()
            .map(|s| s.max_vertex_offset_mm)
            .fold(part.max_uncertainty_mm.max(super::GUARD_MM), f64::max);
        Self {
            version: SIGNATURE_VERSION,
            stacks,
            planes,
            cones,
            lever_mm: diagonal(&points),
            uncertainty_mm: uncertainty,
            unread,
        }
    }

    /// Its stacks, by feature id.
    pub fn stacks(&self) -> &[SigStack] {
        &self.stacks
    }

    /// Its planes, in the part's order.
    pub fn planes(&self) -> &[SigPlane] {
        &self.planes
    }

    /// Its cones.
    pub fn cones(&self) -> &[SigCone] {
        &self.cones
    }

    /// The diagonal of the box of every point it uses (SG6's Λ).
    pub fn lever_mm(&self) -> f64 {
        self.lever_mm
    }

    /// `u_c`: the candidate's own uncertainty, never below the guard (VR2).
    pub fn uncertainty_mm(&self) -> f64 {
        self.uncertainty_mm
    }

    /// What M2 left unread.
    pub fn unread(&self) -> &Unread {
        &self.unread
    }

    /// `SIGNATURE_VERSION` when it was computed.
    pub fn version(&self) -> u32 {
        self.version
    }
}

fn count<T>(items: impl Iterator<Item = T>) -> u32 {
    u32::try_from(items.count()).unwrap_or(u32::MAX)
}

/// The diagonal of the box of `points`; 0 for none.
pub(crate) fn diagonal(points: &[V]) -> f64 {
    let finite: Vec<&V> = points
        .iter()
        .filter(|p| p.iter().all(|x| x.is_finite()))
        .collect();
    let Some(first) = finite.first() else {
        return 0.0;
    };
    let (mut lo, mut hi) = (**first, **first);
    for p in finite {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    norm(sub(hi, lo))
}

/// An outline's points, arcs by their circles' boxes: within the lever.
fn outline_points(outline: &Outline) -> Vec<V> {
    let mut out = Vec::new();
    if let Outline::Known { loops } = outline {
        for segment in loops.iter().flat_map(|l| l.segments.iter()) {
            match segment {
                Segment::Line { from_mm, .. } => out.push(from_mm.to_array()),
                Segment::Arc {
                    center_mm,
                    radius_mm,
                    ..
                } => {
                    let c = center_mm.to_array();
                    out.push(c.map(|x| x - radius_mm));
                    out.push(c.map(|x| x + radius_mm));
                }
            }
        }
    }
    out
}

/// CF3, CF5: a stack's diameters, extent and openings.
fn stack(
    sense: Sense,
    feature: FeatureId,
    axis: &Axis,
    segments: &[StackSegment],
    ends: &[StackEnd],
) -> SigStack {
    let known = segments
        .iter()
        .flat_map(|s| [s.from_mm, s.to_mm])
        .flatten()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), t| {
            (lo.min(t), hi.max(t))
        });
    let unknown = |e: Option<&StackEnd>| {
        matches!(
            e,
            None | Some(StackEnd::Irregular { .. } | StackEnd::Unknown { .. })
        )
    };
    let side = |e: Option<&StackEnd>, low: bool, own: f64| -> Option<f64> {
        if unknown(e) {
            return None;
        }
        let reach = e.map_or(own, |e| end_reach(e, low));
        let t = if low { own.min(reach) } else { own.max(reach) };
        t.is_finite().then_some(t)
    };
    let (start, finish) = (ends.first(), ends.get(1));
    SigStack {
        feature,
        sense,
        axis: *axis,
        diameters_mm: segments.iter().map(|s| s.diameter_mm).collect(),
        extent_mm: [side(start, true, known.0), side(finish, false, known.1)],
        openings: u8::try_from(
            ends.iter()
                .filter(|e| matches!(e, StackEnd::Opening { .. }))
                .count(),
        )
        .unwrap_or(u8::MAX),
        unknown_end: unknown(start) || unknown(finish),
    }
}

/// How far an end reaches along the axis: its rim, or a bottom's tip or
/// floor beyond its station.
fn end_reach(end: &StackEnd, low: bool) -> f64 {
    let pick = |a: f64, b: f64| if low { a.min(b) } else { a.max(b) };
    match end {
        StackEnd::Opening { at_mm, rim_mm, .. }
        | StackEnd::FreeEnd { at_mm, rim_mm, .. }
        | StackEnd::Root { at_mm, rim_mm, .. } => pick(*at_mm, *rim_mm),
        StackEnd::Bottom { at_mm, bottom, .. } => {
            let beyond = match bottom {
                Bottom::Point { tip_length_mm, .. } => {
                    if low {
                        at_mm - tip_length_mm
                    } else {
                        at_mm + tip_length_mm
                    }
                }
                Bottom::ConeThenFlat { floor_at_mm, .. } | Bottom::Annular { floor_at_mm, .. } => {
                    *floor_at_mm
                }
                _ => *at_mm,
            };
            pick(*at_mm, beyond)
        }
        _ => {
            if low {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }
        }
    }
}

/// A required line (SG3, SG5): one or more bores or shafts of one sense a
/// single candidate stack may serve.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReqLine {
    pub(crate) ids: Vec<RequirementId>,
    pub(crate) sense: Sense,
    pub(crate) point: V,
    pub(crate) dir: V,
    /// Each span's diameter window; pieces that bound no diameter give
    /// none.
    pub(crate) windows: Vec<Window>,
    pub(crate) from: f64,
    pub(crate) to: f64,
    pub(crate) openings: u8,
}

impl ReqLine {
    pub(crate) fn mid(&self) -> V {
        add(
            self.point,
            scale(self.dir, f64::midpoint(self.from, self.to)),
        )
    }
}

/// A required seat (SG3, SG5): a plane with at least `needed` known area.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReqSeat {
    pub(crate) ids: Vec<RequirementId>,
    pub(crate) normal: V,
    pub(crate) offset: f64,
    pub(crate) needed: f64,
}

/// A required cone (SG3).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReqCone {
    pub(crate) id: RequirementId,
    pub(crate) apex: V,
    pub(crate) dir: V,
    pub(crate) angle: f64,
    pub(crate) radii: [f64; 2],
    pub(crate) sense: Sense,
}

/// A socket's signature under a policy (SG3 to SG8).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SocketSignature {
    pub(crate) lines: Vec<ReqLine>,
    pub(crate) seats: Vec<ReqSeat>,
    pub(crate) cones: Vec<ReqCone>,
    blocked_by: Vec<RequirementId>,
    removed: Vec<RequirementId>,
    /// The active requirements a candidate can pass.
    demands: usize,
    pub(crate) lever_mm: f64,
    /// σ, the policy's linear tolerance.
    pub(crate) sigma: f64,
    /// `σ_ang`, its angular tolerance in radians.
    pub(crate) sigma_ang: f64,
}

impl SocketSignature {
    /// The signature of `socket` under `policy`.
    pub fn of(socket: &SocketView<'_>, policy: &MatchPolicy) -> Self {
        let sigma = policy.linear_mm();
        let mut sig = Self {
            lines: Vec::new(),
            seats: Vec::new(),
            cones: Vec::new(),
            blocked_by: Vec::new(),
            demands: 0,
            removed: socket
                .removed()
                .iter()
                .map(|r| RequirementId::new(r.id.clone()))
                .collect(),
            lever_mm: 0.0,
            sigma,
            sigma_ang: policy.angular_rad(),
        };
        let mut points: Vec<V> = Vec::new();
        for requirement in socket.active() {
            let id = RequirementId::new(requirement.id.clone());
            if !requirement.verifiable() || socket.blocked(requirement, policy) {
                sig.blocked_by.push(id);
                continue;
            }
            sig.demands += 1;
            match &requirement.requires {
                Requires::Bore {
                    axis, spans, open, ..
                } => {
                    let windows = spans
                        .iter()
                        .filter_map(|s| bore_window(s, policy))
                        .collect();
                    let openings = u8::from(open.start) + u8::from(open.finish);
                    sig.lines.push(line(
                        id,
                        Sense::Hole,
                        axis,
                        spans_of(spans.iter().map(|s| (s.from_mm, s.to_mm))),
                        windows,
                        openings,
                    ));
                }
                Requires::Shaft { axis, spans, .. } => {
                    let windows = spans
                        .iter()
                        .filter_map(|s| shaft_window(s, policy))
                        .collect();
                    sig.lines.push(line(
                        id,
                        Sense::Shaft,
                        axis,
                        spans_of(spans.iter().map(|s| (s.from_mm, s.to_mm))),
                        windows,
                        0,
                    ));
                }
                Requires::Seat {
                    normal,
                    offset_mm,
                    contacts,
                    ..
                } => {
                    let needed = seat_area(contacts, sigma, &mut points);
                    sig.seats.push(ReqSeat {
                        ids: vec![id],
                        normal: normal.to_array(),
                        offset: *offset_mm,
                        needed: needed - slack(needed),
                    });
                }
                Requires::ConicalSeat {
                    axis,
                    apex_mm,
                    included_angle_rad,
                    radii_mm: Some(radii),
                    sense,
                    ..
                } => {
                    points.push(apex_mm.to_array());
                    sig.cones.push(ReqCone {
                        id,
                        apex: apex_mm.to_array(),
                        dir: axis.direction.to_array(),
                        angle: *included_angle_rad,
                        radii: *radii,
                        sense: *sense,
                    });
                }
                // A grip binds two seats already in the signature (SG4):
                // their separation is a pair. Nothing else demands an
                // element.
                _ => {}
            }
        }
        for l in &sig.lines {
            points.push(add(l.point, scale(l.dir, l.from)));
            points.push(add(l.point, scale(l.dir, l.to)));
        }
        sig.lever_mm = diagonal(&points);
        sig
    }

    /// The requirements that fail every candidate under the policy for the
    /// target's own uncertainty, or that no candidate can pass (SG7).
    pub fn blocked_by(&self) -> &[RequirementId] {
        &self.blocked_by
    }

    /// The requirements removed in review.
    pub fn removed(&self) -> &[RequirementId] {
        &self.removed
    }

    /// True if it demands nothing: no active requirement a candidate can
    /// pass (SL8). A grip alone demands its seats' planes.
    pub fn is_empty(&self) -> bool {
        self.demands == 0
    }

    /// `W_L` for a lever of `lever`: two offsets of σ and two tilts of `σ_ang`
    /// over it (SG6).
    pub(crate) fn linear_window(&self, lever: f64) -> f64 {
        2.0 * self.sigma + 4.0 * lever * self.sigma_ang
    }

    /// `W_A`.
    pub(crate) fn angular_window(&self) -> f64 {
        2.0 * self.sigma_ang
    }

    /// SG5: which required lines, and which seats, one candidate element
    /// may serve together: lines of one sense, seats facing one way, within
    /// `wl` and `wa` of each other, joined transitively. Each requirement
    /// keeps its own domain (a group's members may lie `W_L` apart, and
    /// verification passes each on its own witness); distinct witnesses
    /// are asked only of distinct groups (SL5). Offsets of seats are
    /// compared only where their normals are exactly parallel: otherwise
    /// the offset depends on the frame, and the seats are grouped.
    pub(crate) fn groups(&self, wl: f64, wa: f64) -> (Vec<usize>, Vec<usize>) {
        let lines = transitive(self.lines.len(), |a, b| {
            let (Some(x), Some(y)) = (self.lines.get(a), self.lines.get(b)) else {
                return false;
            };
            let near = |p: &ReqLine, q: &ReqLine| {
                [q.from, q.to]
                    .iter()
                    .all(|t| point_line(add(q.point, scale(q.dir, *t)), p.point, p.dir) <= wl)
            };
            x.sense == y.sense && line_angle(x.dir, y.dir) <= wa && (near(x, y) || near(y, x))
        });
        let seats = transitive(self.seats.len(), |a, b| {
            let (Some(x), Some(y)) = (self.seats.get(a), self.seats.get(b)) else {
                return false;
            };
            let parallel = angle(x.normal, y.normal);
            parallel <= wa && (parallel > EXACT || (x.offset - y.offset).abs() <= wl)
        });
        (lines, seats)
    }
}

/// Normals within this are the same direction: a plane's offset compared
/// across them does not depend on the frame (M2's direction classes).
pub(crate) const EXACT: f64 = 1e-12;

/// Each of `n` items' group under `near`, joined transitively (union by
/// the smaller index): the group is its least member.
fn transitive(n: usize, near: impl Fn(usize, usize) -> bool) -> Vec<usize> {
    let mut root: Vec<usize> = (0..n).collect();
    let find = |root: &[usize], mut k: usize| {
        while let Some(&up) = root.get(k) {
            if up == k {
                break;
            }
            k = up;
        }
        k
    };
    for a in 0..n {
        for b in (a + 1)..n {
            if near(a, b) {
                let (ra, rb) = (find(&root, a), find(&root, b));
                if let Some(r) = root.get_mut(ra.max(rb)) {
                    *r = ra.min(rb);
                }
            }
        }
    }
    (0..n).map(|k| find(&root, k)).collect()
}

/// SG6: the known area a seat's contacts need, less the band of width σ
/// about each loop that verification may leave uncovered (twice its
/// perimeter times σ, and π σ² a loop); their points join the lever's.
fn seat_area(contacts: &[crate::socket::SeatContact], sigma: f64, points: &mut Vec<V>) -> f64 {
    let mut needed = 0.0;
    for c in contacts {
        if let Outline::Known { loops } = &c.outline {
            let area: f64 = loops.iter().map(|l| l.area_mm2).sum();
            let perimeter: f64 = loops
                .iter()
                .flat_map(|l| l.segments.iter())
                .map(length)
                .sum();
            let count = f64::from(u32::try_from(loops.len()).unwrap_or(u32::MAX));
            needed +=
                area - (2.0 * perimeter * sigma + std::f64::consts::PI * sigma * sigma * count);
            points.extend(outline_points(&c.outline));
        }
    }
    needed
}

/// The stations a requirement's spans cover.
fn spans_of(spans: impl Iterator<Item = (f64, f64)>) -> (f64, f64) {
    let (from, to) = spans.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (a, b)| {
        (lo.min(a.min(b)), hi.max(a.max(b)))
    });
    if from <= to { (from, to) } else { (0.0, 0.0) }
}

fn line(
    id: RequirementId,
    sense: Sense,
    axis: &Axis,
    (from, to): (f64, f64),
    windows: Vec<Window>,
    openings: u8,
) -> ReqLine {
    ReqLine {
        ids: vec![id],
        sense,
        point: axis.point_mm.to_array(),
        dir: axis.direction.to_array(),
        windows,
        from,
        to,
        openings,
    }
}

/// SG3: a bore span's diameter window, from `pinned` and the clear
/// diameter alone; a piece in a zone or past a known end bounds none.
fn bore_window(span: &BoreSpan, policy: &MatchPolicy) -> Option<Window> {
    let sigma = policy.linear_mm();
    let d = span.hole_diameter_mm;
    if span.basis != SpanBasis::Cylinder {
        return None;
    }
    if span.pinned {
        return Some(Window::new(Some(d - sigma), Some(d + sigma)));
    }
    let c = span.clear_diameter_mm?;
    Some(match policy.clearance() {
        Clearance::Strict => Window::new(Some(c - sigma), Some(d + sigma)),
        Clearance::Functional => Window::new(Some(c - sigma), None),
    })
}

/// SG3: a shaft span's.
fn shaft_window(span: &ShaftSpan, policy: &MatchPolicy) -> Option<Window> {
    let sigma = policy.linear_mm();
    let d = span.shaft_diameter_mm;
    if span.basis != SpanBasis::Cylinder {
        return None;
    }
    if span.pinned {
        return Some(Window::new(Some(d - sigma), Some(d + sigma)));
    }
    let c = span.clear_diameter_mm?;
    Some(match policy.clearance() {
        Clearance::Strict => Window::new(Some(d - sigma), Some(c + sigma)),
        Clearance::Functional => Window::new(Some(0.0), Some(c + sigma)),
    })
}

/// A segment's length.
fn length(segment: &Segment) -> f64 {
    match segment {
        Segment::Line { from_mm, to_mm, .. } => norm(sub(to_mm.to_array(), from_mm.to_array())),
        Segment::Arc {
            radius_mm,
            sweep_rad,
            ..
        } => radius_mm * sweep_rad.abs(),
    }
}

/// The acute angle between two lines (SG9: atan2, never acos near 1).
pub(crate) fn line_angle(a: V, b: V) -> f64 {
    libm::atan2(norm(cross(a, b)), dot(a, b).abs())
}

/// The angle between two directions, 0 to π.
pub(crate) fn angle(a: V, b: V) -> f64 {
    libm::atan2(norm(cross(a, b)), dot(a, b))
}

/// The distance from `q` to the line through `p` along unit `d`.
pub(crate) fn point_line(q: V, p: V, d: V) -> f64 {
    norm(cross(sub(q, p), d))
}

#[cfg(test)]
mod tests {
    use super::{PartSignature, SocketSignature, Window};
    use crate::assembly::{ComponentRead, read_assembly};
    use crate::features::{Axis, Part};
    use crate::geom::{Dir3, Point3};
    use crate::matching::{MatchPolicy, SocketView, shortlist};
    use crate::socket::SocketDocument;

    const S01: &[u8] = include_bytes!("../../tests/assemblies/s01_bolted.stp");

    fn bracket() -> Option<Part> {
        let read = read_assembly(S01);
        let listing = read.listing()?;
        let selection = listing.select(Some("BRACKET"), None).ok()?;
        match &listing.definitions().get(selection.definition)?.read {
            Some(ComponentRead::Accepted(part)) => Some((**part).clone()),
            _ => None,
        }
    }

    /// The bracket and its socket's signature under the default policy;
    /// asserted to be read.
    fn bracket_and_socket() -> Option<(Part, SocketSignature)> {
        let document = SocketDocument::extract(S01, Some("BRACKET"), None).ok();
        let found = bracket().zip(document);
        assert!(found.is_some(), "s01's bracket is read");
        let (part, document) = found?;
        let view = SocketView::new(&document, "socket-1");
        assert!(view.is_some(), "socket-1");
        Some((part, SocketSignature::of(&view?, &MatchPolicy::default())))
    }

    /// `point` turned a quarter about z and moved: a proper rigid motion.
    fn moved(p: [f64; 3]) -> [f64; 3] {
        [-p[1] + 17.0, p[0] - 3.0, p[2] + 40.0]
    }

    fn turned(d: [f64; 3]) -> [f64; 3] {
        [-d[1], d[0], d[2]]
    }

    /// The signature moved rigidly: every invariant kept.
    fn rigidly(sig: &PartSignature) -> Option<PartSignature> {
        let mut out = sig.clone();
        for s in &mut out.stacks {
            s.axis = Axis {
                point_mm: Point3::new(moved(s.axis.point_mm.to_array()))?,
                direction: Dir3::from_unit(turned(s.axis.direction.to_array()))?,
            };
        }
        for p in &mut out.planes {
            let n = p.normal.to_array();
            // The plane n·x = o moved: n' = R n, o' = o + n'·t.
            let t = moved([0.0, 0.0, 0.0]);
            let m = turned(n);
            p.normal = Dir3::from_unit(m)?;
            p.offset_mm += m[0] * t[0] + m[1] * t[1] + m[2] * t[2];
        }
        for c in &mut out.cones {
            c.apex_mm = moved(c.apex_mm);
            c.axis = Axis {
                point_mm: Point3::new(moved(c.axis.point_mm.to_array()))?,
                direction: Dir3::from_unit(turned(c.axis.direction.to_array()))?,
            };
        }
        Some(out)
    }

    /// Section 20.3: the original's signature mutated, against its own
    /// socket: whatever stays within the tolerance is kept, and a missing,
    /// shrunk or moved hole is dropped.
    #[test]
    fn mutations_within_the_tolerance_are_kept_and_beyond_dropped() {
        let Some((part, socket)) = bracket_and_socket() else {
            return;
        };
        let original = PartSignature::of(&part);
        let kept = |sig: &PartSignature| shortlist(&socket, sig).is_ok();
        assert!(kept(&original));
        assert!(rigidly(&original).is_some_and(|s| kept(&s)));
        let edit = |f: &dyn Fn(&mut PartSignature)| {
            let mut s = original.clone();
            f(&mut s);
            s
        };
        let bore = |s: &mut PartSignature| -> Option<usize> {
            s.stacks
                .iter()
                .position(|x| x.diameters_mm.iter().any(|d| (d - 6.6).abs() < 1e-9))
        };
        // Within tolerance: a diameter 0.005 larger, a hole 0.005 off.
        assert!(kept(&edit(&|s| {
            if let Some(k) = bore(s) {
                s.stacks[k]
                    .diameters_mm
                    .iter_mut()
                    .for_each(|d| *d += 0.005);
            }
        })));
        let shift = |by: f64| {
            edit(&|s| {
                if let Some(k) = bore(s) {
                    let p = s.stacks[k].axis.point_mm.to_array();
                    if let Some(q) = Point3::new([p[0] + by, p[1], p[2]]) {
                        s.stacks[k].axis.point_mm = q;
                    }
                }
            })
        };
        assert!(kept(&shift(0.005)));
        // Beyond: a hole missing, a hole too small for its bolt (Ø5.6 for
        // a clear Ø6; Ø6.1 would still clear it), a hole 0.5 off.
        assert!(!kept(&edit(&|s| {
            if let Some(k) = bore(s) {
                s.stacks.remove(k);
            }
        })));
        assert!(!kept(&edit(&|s| {
            if let Some(k) = bore(s) {
                s.stacks[k].diameters_mm.iter_mut().for_each(|d| *d -= 1.0);
            }
        })));
        assert!(!kept(&shift(0.5)));
    }

    /// SL4: a candidate of 10,000 holes is judged within the budget, and
    /// kept when it runs out: work bounded, never a false drop.
    #[test]
    fn a_hostile_candidate_is_judged_within_the_budget() {
        let Some((part, socket)) = bracket_and_socket() else {
            return;
        };
        let mut many = PartSignature::of(&part);
        let first = many.stacks.first().cloned();
        assert!(first.is_some(), "a stack");
        let Some(first) = first else {
            return;
        };
        for k in 0..10_000_u32 {
            let mut s = first.clone();
            let p = s.axis.point_mm.to_array();
            if let Some(q) = Point3::new([p[0] + 0.001 * f64::from(k), p[1], p[2]]) {
                s.axis.point_mm = q;
            }
            many.stacks.push(s);
        }
        assert!(shortlist(&socket, &many).is_ok_and(|k| k.budget_exhausted));
    }

    /// SG3: a span's window follows `pinned` and its clear diameter, never
    /// its fit; a zone or a piece past a known end bounds no diameter; D6
    /// functional drops a bore's upper bound and a shaft's lower one.
    #[test]
    fn windows_follow_the_pin_and_the_clear_diameter() {
        use super::{bore_window, shaft_window};
        use crate::matching::{Clearance, Grip};
        use crate::socket::{BoreSpan, Fit, ShaftSpan, SpanBasis};
        use crate::tolerance::TolerancePolicy;
        let strict = MatchPolicy::default();
        let functional = MatchPolicy::new(
            TolerancePolicy::default(),
            Clearance::Functional,
            Grip::Hard,
        );
        let bore = |pinned: bool, clear: Option<f64>, basis: SpanBasis| BoreSpan {
            from_mm: 0.0,
            to_mm: 5.0,
            hole_diameter_mm: 6.6,
            clear_diameter_mm: clear,
            fit: Fit::NotRead,
            pinned,
            basis,
            profile_mm: None,
        };
        let ends = |w: Option<Window>| {
            w.map(|w| {
                (
                    w.lo.map(|v| (v * 1e6).round() / 1e6),
                    w.hi.map(|v| (v * 1e6).round() / 1e6),
                )
            })
        };
        let cylinder = SpanBasis::Cylinder;
        assert_eq!(
            ends(bore_window(&bore(true, None, cylinder), &strict)),
            Some((Some(6.59), Some(6.61)))
        );
        assert_eq!(
            ends(bore_window(&bore(false, Some(6.0), cylinder), &strict)),
            Some((Some(5.99), Some(6.61)))
        );
        assert_eq!(
            ends(bore_window(&bore(false, Some(6.0), cylinder), &functional)),
            Some((Some(5.99), None))
        );
        assert_eq!(bore_window(&bore(false, None, cylinder), &strict), None);
        assert_eq!(
            bore_window(&bore(false, None, SpanBasis::Zone), &strict),
            None
        );
        assert_eq!(
            bore_window(&bore(true, None, SpanBasis::PastKnownEnd), &strict),
            None
        );
        let shaft = |pinned: bool, clear: Option<f64>| ShaftSpan {
            from_mm: 0.0,
            to_mm: 5.0,
            shaft_diameter_mm: 6.0,
            clear_diameter_mm: clear,
            fit: Fit::Clearance,
            pinned,
            basis: SpanBasis::Cylinder,
            profile_mm: None,
        };
        assert_eq!(
            ends(shaft_window(&shaft(true, None), &strict)),
            Some((Some(5.99), Some(6.01)))
        );
        assert_eq!(
            ends(shaft_window(&shaft(false, Some(6.6)), &strict)),
            Some((Some(5.99), Some(6.61)))
        );
        assert_eq!(
            ends(shaft_window(&shaft(false, Some(6.6)), &functional)),
            Some((Some(0.0), Some(6.61)))
        );
    }

    /// SG5: lines of one sense within `W_L` and `W_A` group (a clevis's
    /// two holes, a chain of them transitively); lines farther apart, or
    /// of the other sense, stay apart; coplanar seats group, and seats
    /// whose normals differ within `W_A` group whatever their offsets.
    #[test]
    fn near_requirements_group() {
        use super::{ReqLine, ReqSeat};
        use crate::features::Sense;
        use crate::verdict::RequirementId;
        let line = |id: &str, x: f64, sense: Sense, from: f64, to: f64| ReqLine {
            ids: vec![RequirementId::new(id)],
            sense,
            point: [x, 0.0, 0.0],
            dir: [0.0, 0.0, 1.0],
            windows: Vec::new(),
            from,
            to,
            openings: 1,
        };
        let seat = |id: &str, normal: [f64; 3], offset: f64| ReqSeat {
            ids: vec![RequirementId::new(id)],
            normal,
            offset,
            needed: 10.0,
        };
        let tilted = [0.0, libm::sin(1e-5), libm::cos(1e-5)];
        let sig = SocketSignature {
            lines: vec![
                line("a", 0.0, Sense::Hole, 0.0, 5.0),
                line("b", 0.015, Sense::Hole, 20.0, 25.0),
                line("c", 0.03, Sense::Hole, 0.0, 5.0),
                line("d", 1.0, Sense::Hole, 0.0, 5.0),
                line("e", 0.0, Sense::Shaft, 0.0, 5.0),
            ],
            seats: vec![
                seat("s", [0.0, 0.0, 1.0], 0.0),
                seat("t", [0.0, 0.0, 1.0], 0.015),
                seat("u", [0.0, 0.0, 1.0], 1.0),
                seat("v", tilted, 50.0),
                seat("w", [0.0, 0.0, -1.0], 0.0),
            ],
            cones: Vec::new(),
            blocked_by: Vec::new(),
            removed: Vec::new(),
            demands: 9,
            lever_mm: 0.0,
            sigma: 0.01,
            sigma_ang: 0.01_f64.to_radians(),
        };
        let (wl, wa) = (sig.linear_window(0.0), sig.angular_window());
        let (lines, seats) = sig.groups(wl, wa);
        assert_eq!(lines, [0, 0, 0, 3, 4]);
        // u is a millimetre off s, but v, tilted, may share a plane with
        // either: one group, transitively; w faces the other way.
        assert_eq!(seats, [0, 0, 0, 0, 4]);
    }

    /// CF4, CF5, CF10: what M2 left unread is counted and serves nothing;
    /// a stack with an unknown end is unbounded on that side; the version
    /// is recorded.
    #[test]
    fn unread_features_are_counted_and_unknown_ends_unbounded() {
        let read = |bytes: &[u8]| {
            crate::features::FeaturesDocument::read(bytes)
                .part()
                .cloned()
        };
        let angled = read(include_bytes!("../../tests/data/p25_angled.stp"));
        assert!(angled.is_some(), "p25 is read");
        let Some(angled) = angled else {
            return;
        };
        let sig = PartSignature::of(&angled);
        assert!(sig.unread().undecided >= 1);
        assert_eq!(sig.version(), crate::matching::SIGNATURE_VERSION);
        assert!(sig.uncertainty_mm() >= crate::matching::GUARD_MM);
        let undercut = read(include_bytes!("../../tests/data/u01_undercut_floor.stp"));
        assert!(undercut.is_some(), "u01 is read");
        let Some(undercut) = undercut else {
            return;
        };
        let sig = PartSignature::of(&undercut);
        assert!(sig.unread().unknown_end_stacks >= 1);
        assert!(
            sig.stacks()
                .iter()
                .filter(|s| s.unknown_end)
                .all(|s| s.extent_mm.iter().any(Option::is_none))
        );
    }

    /// SG9: the same part gives the same signature, byte for byte, with no
    /// −0.0 in it.
    #[test]
    fn a_signature_is_deterministic() {
        let Some((part, _)) = bracket_and_socket() else {
            return;
        };
        let once = serde_json::to_string(&PartSignature::of(&part)).unwrap_or_default();
        let again = serde_json::to_string(&PartSignature::of(&part)).unwrap_or_default();
        assert_eq!(once, again);
        assert!(!once.contains("-0.0"));
        assert!(once.len() > 100);
    }

    /// SL7: a part with more pair tokens than are kept is marked, and the
    /// index lets it through the pair filter.
    #[test]
    fn a_part_with_too_many_pairs_is_marked_incomplete() {
        let Some((part, _)) = bracket_and_socket() else {
            return;
        };
        let mut many = PartSignature::of(&part);
        let first = many.stacks.first().cloned();
        assert!(first.is_some(), "a stack");
        let Some(first) = first else {
            return;
        };
        for k in 0..200_u32 {
            let mut s = first.clone();
            s.feature = crate::features::FeatureId::new(s.feature.kind(), 1_000_000 + u64::from(k));
            let p = s.axis.point_mm.to_array();
            if let Some(q) = Point3::new([p[0] + f64::from(k), p[1], p[2]]) {
                s.axis.point_mm = q;
            }
            many.stacks.push(s);
        }
        let tokens = crate::matching::IndexTokens::of(&many);
        assert!(tokens.pairs_incomplete);
        let plain = crate::matching::IndexTokens::of(&PartSignature::of(&part));
        assert!(!plain.pairs_incomplete);
    }

    /// SG6: a window holds its ends and the rounding slack beyond them.
    #[test]
    fn a_window_holds_its_ends() {
        let w = Window::new(Some(6.0), Some(6.02));
        assert!(w.holds(6.0) && w.holds(6.02) && w.holds(6.01));
        assert!(!w.holds(5.99) && !w.holds(6.03));
        assert!(Window::new(Some(6.0), None).holds(1e9));
    }

    /// The review of PR 2 constructed these: each a candidate verification
    /// passes (by VR's text) that SG5's merging, or an offset compared
    /// across a tilt, had dropped. Each must be kept, at both levels.
    mod review {
        use super::super::{
            PartSignature, ReqLine, ReqSeat, SigPlane, SigStack, SocketSignature, Unread, Window,
            diagonal,
        };
        use crate::features::{Axis, FeatureId, FeatureKind, Sense};
        use crate::geom::{Dir3, Point3};
        use crate::matching::{IndexTokens, ShortlistIndex, shortlist};
        use crate::socket::geo::{V, add, dot, scale};
        use crate::verdict::RequirementId;

        fn hole(id: u64, p: [f64; 3], dia: f64, lo: f64, hi: f64) -> Option<SigStack> {
            Some(SigStack {
                feature: FeatureId::new(FeatureKind::Hole, id),
                sense: Sense::Hole,
                axis: Axis {
                    point_mm: Point3::new(p)?,
                    direction: Dir3::from_unit([0.0, 0.0, 1.0])?,
                },
                diameters_mm: vec![dia],
                extent_mm: [Some(lo), Some(hi)],
                openings: 2,
                unknown_end: false,
            })
        }

        fn plane(id: u64, n: [f64; 3], offset: f64, area: f64) -> Option<SigPlane> {
            Some(SigPlane {
                feature: FeatureId::new(FeatureKind::Plane, id),
                normal: Dir3::normalize(n)?,
                offset_mm: offset,
                known_area_mm2: area,
                faces_known: 1,
                faces_unknown: 0,
            })
        }

        fn part(stacks: Vec<SigStack>, planes: Vec<SigPlane>, extra: &[V]) -> PartSignature {
            let mut points: Vec<V> = extra.to_vec();
            for s in &stacks {
                let (p, d) = (s.axis.point_mm.to_array(), s.axis.direction.to_array());
                points.extend(s.extent_mm.iter().flatten().map(|t| add(p, scale(d, *t))));
                points.push(p);
            }
            PartSignature {
                version: 1,
                stacks,
                planes,
                cones: Vec::new(),
                lever_mm: diagonal(&points),
                uncertainty_mm: 1e-6,
                unread: Unread::default(),
            }
        }

        fn line(id: &str, x: f64, (from, to): (f64, f64), window: Window) -> ReqLine {
            ReqLine {
                ids: vec![RequirementId::new(id)],
                sense: Sense::Hole,
                point: [x, 0.0, 0.0],
                dir: [0.0, 0.0, 1.0],
                windows: vec![window],
                from,
                to,
                openings: 0,
            }
        }

        fn seat(id: &str, normal: [f64; 3], offset: f64, needed: f64) -> ReqSeat {
            ReqSeat {
                ids: vec![RequirementId::new(id)],
                normal,
                offset,
                needed,
            }
        }

        fn socket(
            lines: Vec<ReqLine>,
            seats: Vec<ReqSeat>,
            (sigma, deg): (f64, f64),
            extra: &[V],
        ) -> SocketSignature {
            let mut points: Vec<V> = extra.to_vec();
            for l in &lines {
                points.push(add(l.point, scale(l.dir, l.from)));
                points.push(add(l.point, scale(l.dir, l.to)));
            }
            SocketSignature {
                demands: lines.len() + seats.len(),
                lines,
                seats,
                cones: Vec::new(),
                blocked_by: Vec::new(),
                removed: Vec::new(),
                lever_mm: diagonal(&points),
                sigma,
                sigma_ang: deg.to_radians(),
            }
        }

        /// Kept by level 2, and returned by level 1.
        fn kept(s: &SocketSignature, p: &PartSignature) -> bool {
            let mut index = ShortlistIndex::new();
            index.insert(7, &IndexTokens::of(p));
            shortlist(s, p).is_ok() && index.query(s).is_ok_and(|ids| ids == [7])
        }

        fn pinned(d: f64, sigma: f64) -> Window {
            Window::new(Some(d - sigma), Some(d + sigma))
        }

        /// P1: two near-coaxial bores of different diameters, each served
        /// by its own stack 0.009 off it.
        #[test]
        fn near_bores_served_by_two_stacks() {
            let s = socket(
                vec![
                    line("A", 0.0, (0.0, 5.0), pinned(6.6, 0.01)),
                    line("B", 0.03, (10.0, 15.0), pinned(8.0, 0.01)),
                ],
                Vec::new(),
                (0.01, 0.01),
                &[],
            );
            let stacks = [
                hole(1, [-0.009, 0.0, 0.0], 6.6, 0.0, 5.0),
                hole(2, [0.039, 0.0, 0.0], 8.0, 10.0, 15.0),
            ];
            assert!(stacks.iter().all(Option::is_some));
            assert!(kept(
                &s,
                &part(stacks.into_iter().flatten().collect(), Vec::new(), &[])
            ));
        }

        /// P2: two parallel seats; the candidate 10 m from its origin, its
        /// second face tilted 1e-4 rad about its contact.
        #[test]
        fn a_tilted_seat_far_from_the_origin() {
            let s = socket(
                Vec::new(),
                vec![
                    seat("S1", [0.0, 0.0, 1.0], 0.0, 50.0),
                    seat("S2", [0.0, 0.0, 1.0], 5.0, 50.0),
                ],
                (0.01, 0.01),
                &[[-5.0, -5.0, 0.0], [25.0, 5.0, 5.0]],
            );
            let n2 = [libm::sin(1e-4), 0.0, libm::cos(1e-4)];
            let planes = [
                plane(1, [0.0, 0.0, 1.0], 0.0, 100.0),
                plane(2, n2, dot(n2, [10_020.0, 0.0, 5.0]), 100.0),
            ];
            assert!(planes.iter().all(Option::is_some));
            let p = part(
                Vec::new(),
                planes.into_iter().flatten().collect(),
                &[[9_995.0, -5.0, 0.0], [10_025.0, 5.0, 5.0]],
            );
            assert!(kept(&s, &p));
        }

        /// P3b: a genuine 0.5 mm step between two seat pads under 0.1 mm
        /// and 0.1°, each pad 0.09 off its seat.
        #[test]
        fn a_stepped_seat_under_a_coarse_policy() {
            let s = socket(
                Vec::new(),
                vec![
                    seat("S1", [0.0, 0.0, 1.0], 0.0, 90.0),
                    seat("S2", [0.0, 0.0, 1.0], 0.5, 90.0),
                ],
                (0.1, 0.1),
                &[[0.0, 0.0, 0.0], [50.0, 10.0, 0.5]],
            );
            let planes = [
                plane(1, [0.0, 0.0, 1.0], -0.09, 100.0),
                plane(2, [0.0, 0.0, 1.0], 0.59, 100.0),
            ];
            assert!(planes.iter().all(Option::is_some));
            assert!(kept(
                &s,
                &part(
                    Vec::new(),
                    planes.into_iter().flatten().collect(),
                    &[[0.0, 0.0, 0.0], [50.0, 10.0, 0.5]]
                )
            ));
        }

        /// P4: one plane serving two seats tilted 1e-4 rad from each other,
        /// 10 m out, coincident near their contacts.
        #[test]
        fn one_plane_for_two_slightly_tilted_seats() {
            let n2 = [libm::sin(1e-4), 0.0, libm::cos(1e-4)];
            let s = socket(
                Vec::new(),
                vec![
                    seat("S1", [0.0, 0.0, 1.0], 0.0, 90.0),
                    seat("S2", n2, dot(n2, [10_020.0, 0.0, 0.0]), 90.0),
                ],
                (0.01, 0.01),
                &[[9_995.0, -5.0, 0.0], [10_025.0, 5.0, 0.0]],
            );
            let one = plane(1, [0.0, 0.0, 1.0], 0.0, 300.0);
            assert!(one.is_some());
            assert!(kept(
                &s,
                &part(
                    Vec::new(),
                    one.into_iter().collect(),
                    &[[9_995.0, -5.0, 0.0], [10_025.0, 5.0, 0.0]]
                )
            ));
        }

        /// P5: level 1 against a line whose near neighbour has the
        /// narrowest window, under 0.1 mm.
        #[test]
        fn level_one_holds_near_lines() {
            let s = socket(
                vec![
                    line("A", 0.0, (0.0, 5.0), Window::new(Some(5.0), Some(7.0))),
                    line("B", 0.2, (10.0, 15.0), pinned(8.0, 0.1)),
                    line("C", 30.0, (0.0, 15.0), pinned(10.0, 0.1)),
                ],
                Vec::new(),
                (0.1, 0.01),
                &[],
            );
            let stacks = [
                hole(1, [0.09, 0.0, 0.0], 6.6, 0.0, 5.0),
                hole(2, [0.29, 0.0, 0.0], 8.0, 10.0, 15.0),
                hole(3, [30.0, 0.0, 0.0], 10.0, 0.0, 15.0),
            ];
            assert!(stacks.iter().all(Option::is_some));
            assert!(kept(
                &s,
                &part(stacks.into_iter().flatten().collect(), Vec::new(), &[])
            ));
        }
    }
}
