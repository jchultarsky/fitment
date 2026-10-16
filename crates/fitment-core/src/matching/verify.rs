//! Verification (`docs/research/m4-match.md`, section 8): one active
//! requirement against a placed candidate, with every measure evaluated
//! and every failure coded (VR1). Bores and shafts are `axial.rs`'s; seat
//! coverage is `region.rs`'s; seats, grips and conical seats are here.

use std::collections::BTreeMap;

use super::align::Placement;
use super::axial;
use super::candidate::CandidateView;
use super::evidence::{At, Band, Evidence, Failure, FailureCode, Measure, Quantity};
use super::region::{Face2, S2, Uncovered, covers};
use super::{MAX_COVERAGE_STEPS, MatchPolicy, SocketView, span_floor};
use crate::features::{Axis, FeatureId, Outline, Plane, Segment, Sense};
use crate::geom::Point3;
use crate::socket::geo::{Motion, V, cross, dot, norm, perpendiculars, scale, sub};
use crate::socket::{Flag, Requirement, Requires, SeatContact};
use crate::verdict::Check;

/// The guard on angular comparisons (VR2), in radians.
pub(crate) const ANGLE_GUARD_RAD: f64 = 1e-9;

/// The coverage evidence already computed for one socket: by placement,
/// candidate plane and contact faces.
type Covered = BTreeMap<(usize, usize, Vec<u64>), Evidence>;

/// The work a socket's coverage and core tests may do (AL11), and the
/// coverage already found, so a grip reuses its seats' and the chosen
/// placement the search's.
#[derive(Debug, Clone)]
pub(crate) struct Budget {
    /// Work done.
    pub(crate) used: u64,
    /// The most allowed.
    pub(crate) limit: u64,
    covered: Covered,
}

impl Budget {
    /// The shipped bound.
    pub(crate) fn new() -> Self {
        Self {
            used: 0,
            limit: MAX_COVERAGE_STEPS,
            covered: BTreeMap::new(),
        }
    }

    /// A budget of `limit`.
    #[cfg(test)]
    pub(crate) fn of(limit: u64) -> Self {
        Self {
            used: 0,
            limit,
            covered: BTreeMap::new(),
        }
    }
}

/// What every check of one placement reads.
#[derive(Clone)]
pub(crate) struct Ctx<'a> {
    pub(crate) view: &'a SocketView<'a>,
    pub(crate) cand: &'a CandidateView<'a>,
    /// Candidate to target.
    pub(crate) place: Motion,
    /// Target to candidate.
    pub(crate) back: Motion,
    /// VR2's u: both parts' own uncertainty.
    pub(crate) u: f64,
    /// ε = t − u.
    pub(crate) eps: f64,
    /// The angular tolerance less its guard.
    pub(crate) angle: f64,
    /// Its sine.
    pub(crate) sine: f64,
    /// D6 strict.
    pub(crate) strict: bool,
    /// The placement's index, for the coverage already found.
    pub(crate) index: usize,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(
        view: &'a SocketView<'a>,
        cand: &'a CandidateView<'a>,
        at: &Placement,
        policy: &MatchPolicy,
    ) -> Self {
        let place = Motion::of(&at.transform);
        let u = view.target_u().max(cand.uncertainty_mm());
        let angle = policy.angular_rad() - ANGLE_GUARD_RAD;
        Self {
            view,
            cand,
            place,
            back: place.inverse(),
            u,
            eps: policy.linear_mm() - u,
            angle,
            sine: libm::sin(angle.max(0.0)),
            strict: policy.clearance() == super::Clearance::Strict,
            index: at.index,
        }
    }

    /// VR4's τ over a span: the contact tolerance, the span's floor, u.
    pub(crate) fn tau(&self, requirement: &Requirement, from: f64, to: f64) -> f64 {
        self.view
            .contact_mm()
            .max(span_floor(requirement, from, to))
            .max(self.u)
    }
}

/// VR1: `requirement` of `socket` against `candidate` at `at`, under
/// `policy`, with the shipped bound on its work (AL11).
pub fn verify(
    socket: &SocketView<'_>,
    requirement: &Requirement,
    candidate: &CandidateView<'_>,
    at: &Placement,
    policy: &MatchPolicy,
) -> Check {
    check(
        socket,
        requirement,
        candidate,
        at,
        policy,
        &mut Budget::new(),
    )
}

/// [`verify`] within a socket's shared bound.
pub(crate) fn check(
    socket: &SocketView<'_>,
    requirement: &Requirement,
    candidate: &CandidateView<'_>,
    at: &Placement,
    policy: &MatchPolicy,
    budget: &mut Budget,
) -> Check {
    Check::of(evidence(socket, requirement, candidate, at, policy, budget))
}

fn evidence(
    socket: &SocketView<'_>,
    requirement: &Requirement,
    candidate: &CandidateView<'_>,
    at: &Placement,
    policy: &MatchPolicy,
    budget: &mut Budget,
) -> Evidence {
    // VR14: what no candidate can pass fails every one.
    if !requirement.verifiable() {
        return Evidence::failed(FailureCode::Unverifiable, reason(requirement));
    }
    let ctx = Ctx::new(socket, candidate, at, policy);
    // VR2: the parts' own geometry is not consistent to the tolerance.
    if ctx.eps.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return Evidence::failed(
            FailureCode::FloorAboveTolerance,
            format!(
                "the parts' own uncertainty, {} mm, leaves nothing of the tolerance, {} mm",
                ctx.u,
                policy.linear_mm()
            ),
        );
    }
    match &requirement.requires {
        Requires::Bore {
            axis, spans, open, ..
        } => axial::bore(&ctx, requirement, axis, spans, *open, budget),
        Requires::Shaft { axis, spans, .. } => axial::shaft(&ctx, requirement, axis, spans, budget),
        Requires::Seat {
            normal,
            offset_mm,
            contacts,
            ..
        } => seat(&ctx, (normal.to_array(), *offset_mm), contacts, budget),
        Requires::Grip {
            seats, length_mm, ..
        } => grip(&ctx, seats, *length_mm, budget),
        Requires::ConicalSeat {
            axis,
            apex_mm,
            included_angle_rad,
            radii_mm: Some(radii),
            sense,
            ..
        } => conical(&ctx, axis, (*apex_mm, *included_angle_rad), *radii, *sense).0,
        _ => Evidence::failed(FailureCode::Unverifiable, reason(requirement)),
    }
}

/// Why no candidate can pass `requirement` (VR14): its kind, or the flag
/// or piece that makes it so.
fn reason(requirement: &Requirement) -> String {
    let flag = |f: Flag, why: &str| requirement.flags.contains(&f).then(|| why.to_owned());
    match &requirement.requires {
        Requires::Unresolved { .. } => "unresolved: something no rule reads".to_owned(),
        Requires::PartialBore { .. } => "partial_bore: a partial hole cylinder".to_owned(),
        Requires::PartialShaft { .. } => "partial_shaft: a partial shaft cylinder".to_owned(),
        Requires::Seat { .. } => "a seat contact whose region is unknown".to_owned(),
        Requires::Grip { .. } => "seat_unverifiable: a grip on an unverifiable seat".to_owned(),
        Requires::ConicalSeat { .. } => "a conical seat whose radial range is unknown".to_owned(),
        Requires::Bore { .. } | Requires::Shaft { .. } => flag(
            Flag::TargetUndecided,
            "target_undecided: the target's cylinder is undecided",
        )
        .or_else(|| {
            flag(
                Flag::CoreNotClear,
                "core_not_clear: the original's own material lies in a span's core",
            )
        })
        .or_else(|| {
            flag(
                Flag::OpenAtUnknownEnd,
                "open_at_unknown_end: an opening at an end the target leaves unknown",
            )
        })
        .unwrap_or_else(|| {
            "a span piece past a known end, or a zone the socket states nothing of".to_owned()
        }),
    }
}

/// A candidate plane, placed: its index among the part's planes, its
/// normal and offset in the target's frame, its sine to the seat's normal
/// and its largest departure from the seat's plane over the region.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlaneAt {
    pub(crate) index: usize,
    offset: f64,
    sine: f64,
    dev: f64,
}

/// The vertices of a seat's regions, and each arc's centre with its
/// radius (VR3: a plane is compared over every vertex and arc).
fn region_points(contacts: &[SeatContact]) -> Vec<(V, f64)> {
    let mut pts = Vec::new();
    for c in contacts {
        let Outline::Known { loops } = &c.outline else {
            continue;
        };
        for segment in loops.iter().flat_map(|l| l.segments.iter()) {
            match segment {
                Segment::Line { from_mm, to_mm, .. } => {
                    pts.push((from_mm.to_array(), 0.0));
                    pts.push((to_mm.to_array(), 0.0));
                }
                Segment::Arc {
                    center_mm,
                    radius_mm,
                    ..
                } => pts.push((center_mm.to_array(), *radius_mm)),
            }
        }
    }
    pts
}

/// The candidate's outer-shell planes facing the seat `(n, o)`, placed,
/// nearest first. A plane deviates over the region by its distance at each
/// vertex, and over each arc by its centre's distance plus the radius
/// times its tilt (VR3).
fn planes_at(ctx: &Ctx<'_>, (n, o): (V, f64), pts: &[(V, f64)]) -> Vec<PlaneAt> {
    let anchor = [(scale(n, o), 0.0)];
    let pts = if pts.is_empty() { &anchor[..] } else { pts };
    let mut out: Vec<PlaneAt> = ctx
        .cand
        .part()
        .planes
        .iter()
        .enumerate()
        .filter(|(_, p)| p.shell == 0)
        .filter_map(|(index, plane)| {
            let normal = ctx.place.dir(plane.normal.to_array());
            if dot(normal, n) <= 0.0 {
                return None;
            }
            let on = ctx
                .place
                .point(scale(plane.normal.to_array(), plane.offset_mm));
            let offset = dot(normal, on);
            let sine = norm(cross(normal, n));
            let dev = pts
                .iter()
                .map(|(q, r)| (dot(normal, *q) - offset).abs() + r * sine)
                .fold(0.0, f64::max);
            Some(PlaneAt {
                index,
                offset,
                sine,
                dev,
            })
        })
        .collect();
    out.sort_by(|a, b| a.dev.total_cmp(&b.dev).then(a.index.cmp(&b.index)));
    out
}

/// A plane's measures: its tilt and its departure (VR3).
fn plane_measures(ctx: &Ctx<'_>, at: &PlaneAt, nominal: f64) -> [Measure; 2] {
    [
        Measure::number(
            Quantity::PlaneSine,
            At::whole(),
            (at.sine, Some(0.0)),
            (Band::Angle, ctx.angle),
            (None, Some(ctx.sine)),
        ),
        Measure::number(
            Quantity::PlaneOffsetMm,
            At::whole(),
            (at.dev, Some(nominal)),
            (Band::Epsilon, ctx.eps),
            (None, Some(ctx.eps)),
        ),
    ]
}

/// The planes within the tolerance of a seat.
/// Past the bound, `None`: each plane costs a test of every region point
/// (AL11).
fn matching(
    ctx: &Ctx<'_>,
    seat: (V, f64),
    pts: &[(V, f64)],
    budget: &mut Budget,
) -> Option<(Vec<PlaneAt>, Option<PlaneAt>)> {
    let planes = ctx.cand.part().planes.len().max(1);
    let work = u64::try_from(planes.saturating_mul(pts.len().max(1))).unwrap_or(u64::MAX);
    budget.used = budget.used.saturating_add(work);
    if budget.used > budget.limit {
        return None;
    }
    let all = planes_at(ctx, seat, pts);
    let nearest = all.first().copied();
    Some((
        all.into_iter()
            .filter(|p| p.sine < ctx.sine && p.dev < ctx.eps)
            .collect(),
        nearest,
    ))
}

/// VR10: a plane of the candidate's shell at the seat, whose faces of
/// known outline cover every contact's region (VR11).
fn seat(
    ctx: &Ctx<'_>,
    (n, o): (V, f64),
    contacts: &[SeatContact],
    budget: &mut Budget,
) -> Evidence {
    let pts = region_points(contacts);
    let Some((planes, nearest)) = matching(ctx, (n, o), &pts, budget) else {
        return exhausted();
    };
    if planes.is_empty() {
        let measures = nearest.map_or_else(Vec::new, |p| plane_measures(ctx, &p, 0.0).to_vec());
        let detail = nearest.map_or_else(
            || "no candidate plane faces the seat".to_owned(),
            |p| {
                format!(
                    "no candidate plane at the seat: the nearest lies {} mm off",
                    p.dev
                )
            },
        );
        return Evidence::new(
            Vec::new(),
            measures,
            vec![Failure::new(FailureCode::NoFeature, At::whole(), detail)],
        );
    }
    let mut first = None;
    for plane in &planes {
        let e = coverage(ctx, plane, (n, o), contacts, budget);
        if e.passes() {
            return e;
        }
        first.get_or_insert(e);
    }
    first.unwrap_or_else(|| {
        Evidence::failed(FailureCode::NoFeature, "no candidate plane was judged")
    })
}

/// VR11 on one candidate plane, as `covering` finds it, once per
/// placement, plane and contacts.
fn coverage(
    ctx: &Ctx<'_>,
    at: &PlaneAt,
    seat: (V, f64),
    contacts: &[SeatContact],
    budget: &mut Budget,
) -> Evidence {
    let key = (
        ctx.index,
        at.index,
        contacts.iter().map(|c| c.face).collect(),
    );
    if let Some(found) = budget.covered.get(&key) {
        return found.clone();
    }
    let found = covering(ctx, at, seat, contacts, budget);
    budget.covered.insert(key, found.clone());
    found
}

/// VR11 on one candidate plane: each contact's region covered by the
/// plane's faces of known outline. An edge is a boundary of their union
/// unless both faces it joins are in it, so with a face of unknown outline
/// on the plane every edge is one (RS4).
fn covering(
    ctx: &Ctx<'_>,
    at: &PlaneAt,
    (n, o): (V, f64),
    contacts: &[SeatContact],
    budget: &mut Budget,
) -> Evidence {
    let Some(plane) = ctx.cand.part().planes.get(at.index) else {
        return Evidence::failed(FailureCode::NoFeature, "the plane judged is not the part's");
    };
    let frame = SeatFrame::new(n, o);
    let unknown = plane
        .faces
        .iter()
        .any(|f| matches!(f.outline, Outline::Unknown { .. }));
    let cover: Vec<Face2> = plane
        .faces
        .iter()
        .filter_map(|f| match &f.outline {
            Outline::Known { loops } => Some(frame.face(
                loops.iter().flat_map(|l| l.segments.iter()),
                &|p| ctx.place.point(p),
                &|s| !unknown && across(s) == plane.id,
            )),
            Outline::Unknown { .. } => None,
        })
        .collect();
    let mut measures = plane_measures(ctx, at, o).to_vec();
    let mut failures = Vec::new();
    for contact in contacts {
        let Outline::Known { loops } = &contact.outline else {
            continue;
        };
        let region = frame.face(
            loops.iter().flat_map(|l| l.segments.iter()),
            &|p| p,
            &|_| false,
        );
        let spot = At {
            contact_face: Some(contact.face),
            ..At::default()
        };
        let result = covers(&region, &cover, ctx.eps, budget);
        measures.push(Measure::flag(
            Quantity::Covers,
            spot.clone(),
            result.is_ok(),
            true,
        ));
        if let Err(why) = result {
            failures.push(uncovered(why, unknown, spot, contact.face, &frame));
        }
    }
    Evidence::new(vec![plane.id], measures, failures)
}

/// The failure an uncovered region gives.
fn uncovered(why: Uncovered, unknown: bool, at: At, face: u64, frame: &SeatFrame) -> Failure {
    let code = if unknown {
        FailureCode::UnknownOutline
    } else {
        FailureCode::ContactNotCovered
    };
    match why {
        Uncovered::Exhausted => Failure::new(
            FailureCode::CoverageExhausted,
            at,
            "the coverage test reached its bound",
        ),
        Uncovered::NoTolerance => Failure::new(
            FailureCode::FloorAboveTolerance,
            at,
            "no tolerance left to cover within",
        ),
        Uncovered::EdgeInside { at: q, depth } => Failure::new(
            code,
            at,
            format!(
                "a candidate edge lies {depth} mm inside contact face #{face} at {:?}",
                frame.point(q)
            ),
        ),
        Uncovered::NotCovered { at: q } => Failure::new(
            code,
            at,
            format!(
                "contact face #{face} is not covered at {:?}",
                frame.point(q)
            ),
        ),
    }
}

/// The feature across an edge.
fn across(segment: &Segment) -> FeatureId {
    match segment {
        Segment::Line { across, .. } | Segment::Arc { across, .. } => *across,
    }
}

/// A seat's plane frame: its point nearest the origin, and two axes in it
/// with u × w its normal.
struct SeatFrame {
    origin: V,
    u: V,
    w: V,
}

impl SeatFrame {
    fn new(n: V, o: f64) -> Self {
        let (u, w) = perpendiculars(n);
        Self {
            origin: scale(n, o),
            u,
            w,
        }
    }

    fn flat(&self, p: V) -> [f64; 2] {
        let d = sub(p, self.origin);
        [dot(d, self.u), dot(d, self.w)]
    }

    /// A point of the plane, for people.
    fn point(&self, q: [f64; 2]) -> V {
        let [a, b] = q;
        [0, 1, 2].map(|k| self.origin[k] + a * self.u[k] + b * self.w[k])
    }

    /// A face's edges, each point moved by `place`, projected onto the
    /// plane; `internal` says which edges join two faces of the union.
    fn face<'s>(
        &self,
        segments: impl Iterator<Item = &'s Segment>,
        place: &dyn Fn(V) -> V,
        internal: &dyn Fn(&Segment) -> bool,
    ) -> Face2 {
        let mut out = Face2::default();
        for s in segments {
            match s {
                Segment::Line { from_mm, to_mm, .. } => out.segs.push(S2::Line {
                    a: self.flat(place(from_mm.to_array())),
                    b: self.flat(place(to_mm.to_array())),
                }),
                Segment::Arc {
                    from_mm,
                    center_mm,
                    radius_mm,
                    sweep_rad,
                    ..
                } => {
                    let c = self.flat(place(center_mm.to_array()));
                    let a = self.flat(place(from_mm.to_array()));
                    out.segs.push(S2::Arc {
                        c,
                        r: *radius_mm,
                        a0: libm::atan2(a[1] - c[1], a[0] - c[0]),
                        sw: *sweep_rad,
                    });
                }
            }
            out.internal.push(internal(s));
        }
        out
    }
}

/// The seat or conical seat on target feature `feature`, among every
/// requirement of the socket, removed ones included (VR12).
fn seat_on<'r>(ctx: &Ctx<'r>, feature: FeatureId) -> Option<&'r Requirement> {
    ctx.view
        .socket()
        .requirements
        .iter()
        .find(|r| match &r.requires {
            Requires::Seat { plane, .. } => *plane == feature,
            Requires::ConicalSeat { cone, .. } => *cone == feature,
            _ => false,
        })
}

/// VR12: both seats' planes found as VR10 finds them (or a conical side's
/// cone, L2), their separation within ε of the target's. D7 is hard.
fn grip(ctx: &Ctx<'_>, seats: &[FeatureId], length: f64, budget: &mut Budget) -> Evidence {
    let (mut target, mut found) = (0.0, 0.0);
    let mut features = Vec::new();
    let mut failures = Vec::new();
    let mut normal: Option<V> = None;
    // Planes first: a conical side is measured along a plane's normal.
    for feature in seats {
        let Some(requirement) = seat_on(ctx, *feature) else {
            failures.push(missing_seat(&feature.to_string()));
            continue;
        };
        if let Requires::Seat {
            normal: n,
            offset_mm,
            contacts,
            ..
        } = &requirement.requires
        {
            let n = n.to_array();
            normal.get_or_insert(n);
            let Some((planes, _)) =
                matching(ctx, (n, *offset_mm), &region_points(contacts), budget)
            else {
                return exhausted();
            };
            // The seat's own plane: the first that covers it (VR10, VR12),
            // so the grip measures the plane the seat passes on.
            let covering = planes
                .iter()
                .find(|p| coverage(ctx, p, (n, *offset_mm), contacts, budget).passes());
            match covering.and_then(|p| plane_id(ctx, p)) {
                Some((id, offset)) => {
                    target += offset_mm;
                    found += offset;
                    features.push(id);
                }
                None => failures.push(missing_seat(&requirement.id)),
            }
        }
    }
    for feature in seats {
        let Some(Requires::ConicalSeat {
            axis,
            apex_mm,
            included_angle_rad,
            radii_mm: Some(radii),
            sense,
            ..
        }) = seat_on(ctx, *feature).map(|r| &r.requires)
        else {
            continue;
        };
        let along = normal.unwrap_or_else(|| axis.direction.to_array());
        let (e, apex) = conical(ctx, axis, (*apex_mm, *included_angle_rad), *radii, *sense);
        match apex.filter(|_| e.passes()) {
            Some((id, apex)) => {
                target -= dot(along, apex_mm.to_array());
                found -= dot(along, apex);
                features.push(id);
            }
            None => failures.push(missing_seat(&feature.to_string())),
        }
    }
    if !failures.is_empty() {
        return Evidence::new(features, Vec::new(), failures);
    }
    let value = length + (found - target);
    let measure = Measure::number(
        Quantity::GripLengthMm,
        At::whole(),
        (value, Some(length)),
        (Band::Epsilon, ctx.eps),
        (Some(length - ctx.eps), Some(length + ctx.eps)),
    );
    if !measure.passed() {
        failures.push(Failure::new(
            FailureCode::GripLength,
            At::whole(),
            format!("the seats lie {value} mm apart, the target's {length} mm"),
        ));
    }
    Evidence::new(features, vec![measure], failures)
}

/// A check stopped by the bound on its work (AL11).
fn exhausted() -> Evidence {
    Evidence::failed(
        FailureCode::CoverageExhausted,
        "the bound on this socket's work was reached",
    )
}

fn missing_seat(seat: &str) -> Failure {
    Failure::new(
        FailureCode::SeatMissing,
        At::whole(),
        format!("no candidate plane at seat {seat}"),
    )
}

/// A placed plane's id and offset.
fn plane_id(ctx: &Ctx<'_>, at: &PlaneAt) -> Option<(FeatureId, f64)> {
    let plane: &Plane = ctx.cand.part().planes.get(at.index)?;
    Some((plane.id, at.offset))
}

/// VR13: a walked cone of the candidate along the seat's axis, widening
/// the same way, its apex within ε, its included angle within the angular
/// tolerance, its radial range covering the seat's to within ε, and the
/// target's sense. Its evidence, and the cone and its placed apex when one
/// was judged.
fn conical(
    ctx: &Ctx<'_>,
    axis: &Axis,
    (apex, angle): (Point3, f64),
    [low, high]: [f64; 2],
    sense: Sense,
) -> (Evidence, Option<(FeatureId, V)>) {
    let (d, apex) = (axis.direction.to_array(), apex.to_array());
    let mut first: Option<(Evidence, Option<(FeatureId, V)>)> = None;
    let mut cones: Vec<(f64, &crate::features::Cone, V, f64)> = ctx
        .cand
        .walked_cones()
        .into_iter()
        .filter_map(|c| {
            let dir = ctx.place.dir(c.axis.direction.to_array());
            let sine = norm(cross(dir, d));
            (dot(dir, d) > 0.0 && sine < ctx.sine).then(|| {
                let a = ctx.place.point(c.apex_mm.to_array());
                (norm(sub(a, apex)), c, a, sine)
            })
        })
        .collect();
    cones.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (distance, cone, placed, sine) in cones {
        let range = cone
            .radii_mm
            .map_or(f64::NAN, |[a, b]| (a - low).max(high - b));
        let measures = vec![
            Measure::number(
                Quantity::AxisSine,
                At::whole(),
                (sine, Some(0.0)),
                (Band::Angle, ctx.angle),
                (None, Some(ctx.sine)),
            ),
            Measure::number(
                Quantity::ApexOffsetMm,
                At::whole(),
                (distance, Some(0.0)),
                (Band::Epsilon, ctx.eps),
                (None, Some(ctx.eps)),
            ),
            Measure::number(
                Quantity::ConeAngleRad,
                At::whole(),
                (2.0 * cone.semi_angle_rad, Some(angle)),
                (Band::Angle, ctx.angle),
                (Some(angle - ctx.angle), Some(angle + ctx.angle)),
            ),
            Measure::number(
                Quantity::ConeRangeMm,
                At::whole(),
                (range, Some(0.0)),
                (Band::Epsilon, ctx.eps),
                (None, Some(ctx.eps)),
            ),
        ];
        let coded = [
            FailureCode::AxisAngle,
            FailureCode::ConeApex,
            FailureCode::ConeAngle,
            FailureCode::ConeRange,
        ];
        let mut failures: Vec<Failure> = measures
            .iter()
            .zip(coded)
            .filter(|(m, _)| !m.passed())
            .map(|(m, code)| Failure::new(code, At::whole(), format!("{:?}", m.value())))
            .collect();
        if cone.sense != sense {
            failures.push(Failure::new(
                FailureCode::ConeSense,
                At::whole(),
                format!("a {:?} cone where the seat is {sense:?}", cone.sense),
            ));
        }
        let e = Evidence::new(vec![cone.id], measures, failures);
        if e.passes() {
            return (e, Some((cone.id, placed)));
        }
        first.get_or_insert((e, Some((cone.id, placed))));
    }
    first.unwrap_or_else(|| {
        (
            Evidence::failed(
                FailureCode::NoFeature,
                "no candidate cone along the seat's axis",
            ),
            None,
        )
    })
}
