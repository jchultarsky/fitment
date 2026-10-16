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
        sig.merge();
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

    /// True if it demands nothing (SL8).
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.seats.is_empty() && self.cones.is_empty()
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

    /// SG5: required lines of one sense within `W_L` and `W_A` of each other
    /// over their spans are one; seats likewise, their areas added.
    fn merge(&mut self) {
        let (wl, wa) = (self.linear_window(self.lever_mm), self.angular_window());
        let mut lines: Vec<ReqLine> = Vec::new();
        for l in std::mem::take(&mut self.lines) {
            let ends = [
                add(l.point, scale(l.dir, l.from)),
                add(l.point, scale(l.dir, l.to)),
            ];
            let near = lines.iter_mut().find(|o| {
                o.sense == l.sense
                    && line_angle(o.dir, l.dir) <= wa
                    && ends.iter().all(|e| point_line(*e, o.point, o.dir) <= wl)
            });
            match near {
                Some(o) => {
                    let (a, b) = (
                        dot(sub(ends[0], o.point), o.dir),
                        dot(sub(ends[1], o.point), o.dir),
                    );
                    o.from = o.from.min(a.min(b));
                    o.to = o.to.max(a.max(b));
                    o.windows.extend(l.windows);
                    o.openings = o.openings.max(l.openings);
                    o.ids.extend(l.ids);
                }
                None => lines.push(l),
            }
        }
        self.lines = lines;
        let mut seats: Vec<ReqSeat> = Vec::new();
        for s in std::mem::take(&mut self.seats) {
            let near = seats.iter_mut().find(|o| {
                line_angle(o.normal, s.normal) <= wa
                    && dot(o.normal, s.normal) > 0.0
                    && (o.offset - s.offset).abs() <= wl
            });
            match near {
                Some(o) => {
                    o.needed += s.needed;
                    o.ids.extend(s.ids);
                }
                None => seats.push(s),
            }
        }
        self.seats = seats;
    }
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

    /// SG5: lines of one sense within `W_L` and `W_A` are one (a clevis's two
    /// holes); lines farther apart, or of the other sense, stay apart;
    /// coplanar seats merge, their areas added.
    #[test]
    fn near_lines_and_coplanar_seats_merge() {
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
        let seat = |id: &str, offset: f64| ReqSeat {
            ids: vec![RequirementId::new(id)],
            normal: [0.0, 0.0, 1.0],
            offset,
            needed: 10.0,
        };
        let mut sig = SocketSignature {
            lines: vec![
                line("a", 0.0, Sense::Hole, 0.0, 5.0),
                line("b", 0.005, Sense::Hole, 20.0, 25.0),
                line("c", 0.015 + 0.01, Sense::Hole, 0.0, 5.0),
                line("d", 0.0, Sense::Shaft, 0.0, 5.0),
            ],
            seats: vec![seat("s", 0.0), seat("t", 0.004), seat("u", 1.0)],
            cones: Vec::new(),
            blocked_by: Vec::new(),
            removed: Vec::new(),
            lever_mm: 0.0,
            sigma: 0.01,
            sigma_ang: 0.01_f64.to_radians(),
        };
        sig.merge();
        let ids: Vec<Vec<&str>> = sig
            .lines
            .iter()
            .map(|l| l.ids.iter().map(RequirementId::as_str).collect())
            .collect();
        assert_eq!(ids, [vec!["a", "b"], vec!["c"], vec!["d"]]);
        assert!(
            sig.lines
                .first()
                .is_some_and(|l| l.from.abs() < 1e-12 && (l.to - 25.0).abs() < 1e-12)
        );
        let seats: Vec<f64> = sig.seats.iter().map(|s| s.needed).collect();
        assert_eq!(seats, [20.0, 10.0]);
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

    /// SG6: a window holds its ends and the rounding slack beyond them.
    #[test]
    fn a_window_holds_its_ends() {
        let w = Window::new(Some(6.0), Some(6.02));
        assert!(w.holds(6.0) && w.holds(6.02) && w.holds(6.01));
        assert!(!w.holds(5.99) && !w.holds(6.03));
        assert!(Window::new(Some(6.0), None).holds(1e9));
    }
}
