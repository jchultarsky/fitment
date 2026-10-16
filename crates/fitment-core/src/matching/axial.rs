//! Bores and shafts (`docs/research/m4-match.md`, VR7 to VR9, CF8, VR22):
//! a candidate stack on the requirement's axis whose usable profile covers
//! every span with the diameters the span asks, open where the bore must
//! be open, crossing no wall opening, with every span's core clear.

use super::candidate::{EndAt, Piece, Stack};
use super::evidence::{At, Band, EndSide, Evidence, Failure, FailureCode, Measure, Quantity};
use super::verify::{Budget, Ctx};
use crate::features::{Axis, Sense};
use crate::socket::geo::{V, add, cross, dot, norm, scale, sub};
use crate::socket::own::{Core, Frame};
use crate::socket::{BoreSpan, Open, Requirement, ShaftSpan, SpanBasis};

/// How far a piece may meet a span and still only touch it, in mm: a
/// fixed numeric slack (a counterbore's step read 1e-14 mm into the bore
/// below it), never the candidate's own stated uncertainty, which would let
/// a part buy its own room.
const TOUCH_MM: f64 = 1e-9;

/// How far off its axis a stack is still named as the one meant, in mm.
const NEAR_MM: f64 = 1.0;

/// A span of a bore or a shaft.
#[derive(Debug, Clone, Copy)]
struct Span {
    from: f64,
    to: f64,
    /// The original's diameter there.
    d: f64,
    clear: Option<f64>,
    pinned: bool,
    basis: SpanBasis,
    profile: Option<[f64; 2]>,
}

/// VR7: a bore.
pub(crate) fn bore(
    ctx: &Ctx<'_>,
    requirement: &Requirement,
    axis: &Axis,
    spans: &[BoreSpan],
    open: Open,
    budget: &mut Budget,
) -> Evidence {
    let spans: Vec<Span> = spans
        .iter()
        .map(|s| Span {
            from: s.from_mm,
            to: s.to_mm,
            d: s.hole_diameter_mm,
            clear: s.clear_diameter_mm,
            pinned: s.pinned,
            basis: s.basis,
            profile: s.profile_mm,
        })
        .collect();
    axial(ctx, requirement, (true, axis), &spans, Some(open), budget)
}

/// VR9: a shaft.
pub(crate) fn shaft(
    ctx: &Ctx<'_>,
    requirement: &Requirement,
    axis: &Axis,
    spans: &[ShaftSpan],
    budget: &mut Budget,
) -> Evidence {
    let spans: Vec<Span> = spans
        .iter()
        .map(|s| Span {
            from: s.from_mm,
            to: s.to_mm,
            d: s.shaft_diameter_mm,
            clear: s.clear_diameter_mm,
            pinned: s.pinned,
            basis: s.basis,
            profile: s.profile_mm,
        })
        .collect();
    axial(ctx, requirement, (false, axis), &spans, None, budget)
}

/// The requirement's axis against one placed stack: stations of one in the
/// other's, and how far apart they lie.
struct Map {
    /// The requirement's axis point and direction, in the target's frame.
    p: V,
    d: V,
    /// The stack's, placed.
    pc: V,
    dc: V,
    /// The stack's station per requirement station.
    k: f64,
    base: f64,
}

impl Map {
    fn new((p, d): (V, V), (pc, dc): (V, V)) -> Self {
        Self {
            p,
            d,
            pc,
            dc,
            k: dot(d, dc),
            base: dot(sub(p, pc), dc),
        }
    }

    /// The stack's station at requirement station `s`.
    fn cand(&self, s: f64) -> f64 {
        self.base + s * self.k
    }

    /// The requirement's station at stack station `c`.
    fn req(&self, c: f64) -> f64 {
        (c - self.base) / self.k
    }

    /// The distance of the requirement's axis at station `s` from the
    /// stack's.
    fn offset(&self, s: f64) -> f64 {
        let q = sub(add(self.p, scale(self.d, s)), self.pc);
        norm(sub(q, scale(self.dc, dot(q, self.dc))))
    }

    fn sine(&self) -> f64 {
        norm(cross(self.d, self.dc))
    }
}

fn axial(
    ctx: &Ctx<'_>,
    requirement: &Requirement,
    (hole, axis): (bool, &Axis),
    spans: &[Span],
    open: Option<Open>,
    budget: &mut Budget,
) -> Evidence {
    if let Some(e) = no_room(ctx, requirement, hole, spans) {
        return e;
    }
    let line = (axis.point_mm.to_array(), axis.direction.to_array());
    let (low, high) = spans
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), s| {
            (a.min(s.from.min(s.to)), b.max(s.from.max(s.to)))
        });
    let (low, high) = if low <= high { (low, high) } else { (0.0, 0.0) };
    let mut placed: Vec<(&Stack, Map, f64)> = ctx
        .cand
        .stacks()
        .iter()
        .filter(|s| s.hole == hole)
        .map(|s| {
            let map = Map::new(line, (ctx.place.point(s.point), ctx.place.dir(s.dir)));
            let offset = map.offset(low).max(map.offset(high));
            (s, map, offset)
        })
        .collect();
    placed.sort_by(|a, b| a.2.total_cmp(&b.2));
    let mut first: Option<Evidence> = None;
    for (stack, map, offset) in &placed {
        if !(map.sine() < ctx.sine && *offset < ctx.eps) {
            continue;
        }
        let e = one(ctx, requirement, stack, map, *offset, spans, open, budget);
        if e.passes() {
            return e;
        }
        first.get_or_insert(e);
    }
    first.unwrap_or_else(|| missing(ctx, hole, line, (low, high), &placed))
}

/// VR4's no-room branch: a clearance span whose limits from its neighbours
/// leave the original no room fails every candidate (VD6).
fn no_room(
    ctx: &Ctx<'_>,
    requirement: &Requirement,
    hole: bool,
    spans: &[Span],
) -> Option<Evidence> {
    spans.iter().find_map(|s| {
        let c = s.clear.filter(|_| !s.pinned)?;
        let g = 2.0 * (ctx.tau(requirement, s.from, s.to) + ctx.u);
        let room = if hole { c + g < s.d } else { s.d + g < c };
        (!room).then(|| {
            Evidence::failed(
                super::FailureCode::FloorAboveTolerance,
                format!(
                    "over {}..{} the clear diameter {c} mm and the original's {} mm leave no room for a margin of {} mm",
                    s.from,
                    s.to,
                    s.d,
                    g / 2.0
                ),
            )
        })
    })
}

/// No stack on the axis: an unknown cylinder there, the nearest stack off
/// it, or nothing.
fn missing(
    ctx: &Ctx<'_>,
    hole: bool,
    (p, d): (V, V),
    (low, high): (f64, f64),
    placed: &[(&Stack, Map, f64)],
) -> Evidence {
    let sense = if hole { Sense::Hole } else { Sense::Shaft };
    let unknown = ctx.cand.part().cylinders.iter().find(|c| {
        c.sense == sense && !c.class.is_full() && {
            let map = Map::new(
                (p, d),
                (
                    ctx.place.point(c.axis.point_mm.to_array()),
                    ctx.place.dir(c.axis.direction.to_array()),
                ),
            );
            map.sine() < ctx.sine && map.offset(low).max(map.offset(high)) < ctx.eps
        }
    });
    if let Some(c) = unknown {
        return Evidence::new(
            vec![c.id],
            Vec::new(),
            vec![Failure::new(
                FailureCode::CandidateFeatureUnknown,
                At::whole(),
                format!(
                    "only {}, which M2 left undecided or partial, lies on the axis",
                    c.id
                ),
            )],
        );
    }
    let near = placed
        .iter()
        .find(|(_, _, offset)| *offset < NEAR_MM.max(10.0 * ctx.eps));
    let Some((stack, map, offset)) = near else {
        return Evidence::failed(
            FailureCode::NoFeature,
            format!(
                "no candidate {} on the axis",
                if hole { "hole" } else { "shaft" }
            ),
        );
    };
    let measures = axis_measures(ctx, map.sine(), *offset).to_vec();
    let code = if map.sine() < ctx.sine {
        FailureCode::AxisOffset
    } else {
        FailureCode::AxisAngle
    };
    Evidence::new(
        vec![stack.id],
        measures,
        vec![Failure::new(
            code,
            At::whole(),
            format!(
                "{} lies {offset} mm off the axis at sine {}",
                stack.id,
                map.sine()
            ),
        )],
    )
}

fn axis_measures(ctx: &Ctx<'_>, sine: f64, offset: f64) -> [Measure; 2] {
    [
        Measure::number(
            Quantity::AxisSine,
            At::whole(),
            (sine, Some(0.0)),
            (Band::Angle, ctx.angle),
            (None, Some(ctx.sine)),
        ),
        Measure::number(
            Quantity::AxisOffsetMm,
            At::whole(),
            (offset, Some(0.0)),
            (Band::Epsilon, ctx.eps),
            (None, Some(ctx.eps)),
        ),
    ]
}

/// What one check gathers.
#[derive(Default)]
struct Found {
    measures: Vec<Measure>,
    failures: Vec<Failure>,
    /// A clearance passed only because D6 is functional.
    departs: bool,
}

impl Found {
    fn measure(&mut self, m: Measure, code: FailureCode, at: &At, detail: impl FnOnce() -> String) {
        if !m.passed() {
            self.failures.push(Failure::new(code, at.clone(), detail()));
        }
        self.measures.push(m);
    }

    fn fail(&mut self, code: FailureCode, at: &At, detail: impl Into<String>) {
        self.failures.push(Failure::new(code, at.clone(), detail));
    }
}

/// One stack on the axis, every span and end judged.
#[allow(clippy::too_many_arguments)]
fn one(
    ctx: &Ctx<'_>,
    requirement: &Requirement,
    stack: &Stack,
    map: &Map,
    offset: f64,
    spans: &[Span],
    open: Option<Open>,
    budget: &mut Budget,
) -> Evidence {
    let mut found = Found {
        measures: axis_measures(ctx, map.sine(), offset).to_vec(),
        failures: Vec::new(),
        departs: false,
    };
    for span in spans {
        span_check(ctx, requirement, stack, map, span, budget, &mut found);
    }
    if let Some(open) = open {
        for (side, required) in [(EndSide::Start, open.start), (EndSide::Finish, open.finish)] {
            if required {
                end_open(
                    ctx,
                    requirement,
                    stack,
                    map,
                    spans,
                    side,
                    budget,
                    &mut found,
                );
            }
        }
    }
    Evidence::new(vec![stack.id], found.measures, found.failures).departing(found.departs)
}

/// The run of `pieces` holding the middle of stations `lo` to `hi`: where
/// it starts and ends, and its pieces that meet the stations by more than
/// `touch` (a piece that only touches an end is not met: a counterbore's
/// step is no part of the bore below it), or hold them all when they are
/// shorter than that.
fn run(pieces: &[Piece], (lo, hi): (f64, f64), touch: f64) -> Option<(f64, f64, Vec<Piece>)> {
    let m = f64::midpoint(lo, hi);
    let hit = pieces.iter().position(|p| p.lo <= m && m <= p.hi)?;
    let id = pieces.get(hit)?.run;
    let joined = |a: &Piece, b: &Piece| {
        a.run == id && b.run == id && (b.lo - a.hi).abs() <= 1e-9 * (1.0 + a.hi.abs())
    };
    let mut start = hit;
    while start > 0
        && pieces
            .get(start - 1)
            .zip(pieces.get(start))
            .is_some_and(|(a, b)| joined(a, b))
    {
        start -= 1;
    }
    let mut end = hit;
    while pieces
        .get(end)
        .zip(pieces.get(end + 1))
        .is_some_and(|(a, b)| joined(a, b))
    {
        end += 1;
    }
    let chain = pieces.get(start..=end)?;
    let over = chain
        .iter()
        .filter(|p| {
            if hi - lo > touch {
                p.hi.min(hi) - p.lo.max(lo) > touch
            } else {
                p.lo <= m && m <= p.hi
            }
        })
        .copied()
        .collect();
    Some((chain.first()?.lo, chain.last()?.hi, over))
}

/// One span: its wall, its diameters, its wall openings and its core.
fn span_check(
    ctx: &Ctx<'_>,
    requirement: &Requirement,
    stack: &Stack,
    map: &Map,
    span: &Span,
    budget: &mut Budget,
    found: &mut Found,
) {
    let at = At::span(span.from, span.to);
    let eps = ctx.eps;
    let (a, b) = (map.cand(span.from), map.cand(span.to));
    let (lo, hi) = (a.min(b), a.max(b));
    let delta = map.offset(span.from).max(map.offset(span.to));
    let ran = run(&stack.pieces, (lo, hi), TOUCH_MM);
    let (wall_lo, wall_hi) = ran.as_ref().map_or((f64::NAN, f64::NAN), |(clo, chi, _)| {
        let (x, y) = (map.req(*clo), map.req(*chi));
        (x.min(y), x.max(y))
    });
    let short = || {
        if wall_lo.is_nan() {
            "no usable wall holds the middle of the span".to_owned()
        } else {
            format!("the usable wall runs {wall_lo}..{wall_hi}, short of the span")
        }
    };
    found.measure(
        Measure::number(
            Quantity::WallFromMm,
            at.clone(),
            (wall_lo, Some(span.from)),
            (Band::Epsilon, eps),
            (None, Some(span.from + eps)),
        ),
        FailureCode::WallMissing,
        &at,
        short,
    );
    found.measure(
        Measure::number(
            Quantity::WallToMm,
            at.clone(),
            (wall_hi, Some(span.to)),
            (Band::Epsilon, eps),
            (Some(span.to - eps), None),
        ),
        FailureCode::WallMissing,
        &at,
        short,
    );
    let over = ran.map(|r| r.2).unwrap_or_default();
    if span.pinned {
        pinned(ctx, span, &over, (lo, hi), &at, found);
    } else if let Some(c) = span.clear {
        let tau = ctx.tau(requirement, span.from, span.to);
        clearance(ctx, stack.hole, span, c, (tau, delta), &over, &at, found);
    } else if let Some(profile) = span.profile {
        zone_profile(ctx, span, profile, map, &over, (lo, hi), &at, found);
    }
    for (olo, ohi) in &stack.openings {
        if *olo <= hi + eps && *ohi >= lo - eps {
            found.fail(
                FailureCode::WallOpening,
                &at,
                format!(
                    "the span meets a wall opening in {} over {olo}..{ohi}",
                    stack.id
                ),
            );
        }
    }
    core(ctx, requirement, stack, map, span, budget, found);
}

/// A pinned span: on a cylinder, one cylinder piece covering it within ε
/// at both ends, its diameter within ε of the original's; on a zone, every
/// piece there at the original's diameter.
fn pinned(
    ctx: &Ctx<'_>,
    span: &Span,
    over: &[Piece],
    (lo, hi): (f64, f64),
    at: &At,
    found: &mut Found,
) {
    let eps = ctx.eps;
    let piece = if span.basis == SpanBasis::Zone {
        over.iter().max_by(|a, b| {
            (2.0 * a.r - span.d)
                .abs()
                .total_cmp(&(2.0 * b.r - span.d).abs())
        })
    } else {
        let one = over
            .iter()
            .find(|p| p.cylinder.is_some() && p.lo - lo < eps && hi - p.hi < eps);
        if one.is_none() {
            found.fail(
                FailureCode::WallMissing,
                at,
                "no single cylinder covers the pinned span",
            );
        }
        one.or_else(|| over.iter().find(|p| p.cylinder.is_some()))
    };
    let diameter = piece.map_or(f64::NAN, |p| 2.0 * p.r);
    let code = if diameter < span.d {
        FailureCode::DiameterMin
    } else {
        FailureCode::DiameterMax
    };
    found.measure(
        Measure::number(
            Quantity::DiameterMm,
            at.clone(),
            (diameter, Some(span.d)),
            (Band::Epsilon, eps),
            (Some(span.d - eps), Some(span.d + eps)),
        ),
        code,
        at,
        || format!("Ø{diameter} where Ø{} is pinned", span.d),
    );
}

/// A clearance span (VR4): the radial margin beyond the clear diameter
/// above τ + u, the candidate axis's offset counted; under D6 strict, no
/// radius past the original's by ε.
#[allow(clippy::too_many_arguments)]
fn clearance(
    ctx: &Ctx<'_>,
    hole: bool,
    span: &Span,
    c: f64,
    (tau, delta): (f64, f64),
    over: &[Piece],
    at: &At,
    found: &mut Found,
) {
    let eps = ctx.eps;
    let (least, most) = over
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
            (a.min(p.r), b.max(p.r))
        });
    let (least, most) = if over.is_empty() {
        (f64::NAN, f64::NAN)
    } else {
        (least, most)
    };
    let guard = tau + ctx.u;
    let (margin, code) = if hole {
        (least - delta - c / 2.0, FailureCode::DiameterMin)
    } else {
        (c / 2.0 - most - delta, FailureCode::DiameterMax)
    };
    found.measure(
        Measure::number(
            Quantity::ClearMarginMm,
            at.clone(),
            (margin, None),
            (Band::TauGuard, guard),
            (Some(guard), None),
        ),
        code,
        at,
        || format!("a radial margin of {margin} mm beyond the clear Ø{c}, not above {guard} mm"),
    );
    if !ctx.strict {
        // What strict would have asked: past the original's by ε.
        let past = if hole {
            2.0 * most >= span.d + eps
        } else {
            2.0 * least <= span.d - eps
        };
        found.departs |= past;
        return;
    }
    let (measure, code) = if hole {
        (
            Measure::number(
                Quantity::DiameterMm,
                at.clone(),
                (2.0 * most, Some(span.d)),
                (Band::Epsilon, eps),
                (None, Some(span.d + eps)),
            ),
            FailureCode::DiameterMax,
        )
    } else {
        (
            Measure::number(
                Quantity::DiameterMm,
                at.clone(),
                (2.0 * least, Some(span.d)),
                (Band::Epsilon, eps),
                (Some(span.d - eps), None),
            ),
            FailureCode::DiameterMin,
        )
    };
    found.measure(measure, code, at, || {
        format!("past the original's Ø{} under D6 strict", span.d)
    });
}

/// A zone piece with the original's profile (X8): the candidate's radius
/// within ε of the original's at every station. Both are straight within
/// each piece, so the ends of the pieces bound the difference; a zone that
/// is no single cone has no radius known there.
#[allow(clippy::too_many_arguments)]
fn zone_profile(
    ctx: &Ctx<'_>,
    span: &Span,
    [d1, d2]: [f64; 2],
    map: &Map,
    over: &[Piece],
    (lo, hi): (f64, f64),
    at: &At,
    found: &mut Found,
) {
    let original = |s: f64| {
        let t = if span.to > span.from {
            ((s - span.from) / (span.to - span.from)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        d1 / 2.0 + (d2 - d1) * t / 2.0
    };
    let radius = |p: &Piece, x: f64| match (p.cylinder, p.taper) {
        (Some(_), _) => p.r,
        (None, Some(((s1, r1), (s2, r2)))) if (s2 - s1).abs() > 0.0 => {
            r1 + (r2 - r1) * ((x - s1) / (s2 - s1)).clamp(0.0, 1.0)
        }
        _ => f64::NAN,
    };
    let mut worst: f64 = if over.is_empty() { f64::NAN } else { 0.0 };
    for p in over {
        for x in [p.lo.max(lo), p.hi.min(hi)] {
            let d = (radius(p, x) - original(map.req(x))).abs();
            // A radius not known anywhere leaves the profile unknown: NaN
            // stays, whatever follows (`f64::max` would drop it).
            worst = if d.is_nan() || worst.is_nan() {
                f64::NAN
            } else {
                worst.max(d)
            };
        }
    }
    found.measure(
        Measure::number(
            Quantity::ProfileOffsetMm,
            at.clone(),
            (worst, Some(0.0)),
            (Band::Epsilon, ctx.eps),
            (None, Some(ctx.eps)),
        ),
        FailureCode::DiameterProfile,
        at,
        || format!("the radius departs {worst} mm from the original's profile"),
    );
}

/// How many pieces a zone's core is cut into along its profile.
const PROFILE_PIECES: u32 = 16;

/// VR22: the span's core clear of every face of the candidate but the
/// stack's own walls and the zones VR8 took. A bore's core is the solid
/// cylinder of radius c/2 + τ + u (clearance) or d/2 − τ (pinned); a
/// shaft's, all outside c/2 − τ − u or d/2 + τ; a zone piece's follows the
/// original's profile, less τ (a bore) or plus it (a shaft), cut into pieces
/// each with the radius that makes it largest, so the core only grows. Over
/// the span widened by ε, stopping short of the stack's own root or bottom
/// within ε of an end, by u.
fn core(
    ctx: &Ctx<'_>,
    requirement: &Requirement,
    stack: &Stack,
    map: &Map,
    span: &Span,
    budget: &mut Budget,
    found: &mut Found,
) {
    let tau = ctx.tau(requirement, span.from, span.to);
    let (lo, hi) = (span.from.min(span.to), span.from.max(span.to));
    let (mut from, mut to) = ((lo - ctx.eps, false), (hi + ctx.eps, false));
    for end in &stack.ends {
        let EndAt::Closed { at } = end else {
            continue;
        };
        let s = map.req(*at);
        if (s - lo).abs() <= ctx.eps {
            from = (s + ctx.u, true);
        }
        if (s - hi).abs() <= ctx.eps {
            to = (s - ctx.u, true);
        }
    }
    let profile = span
        .profile
        .filter(|_| !span.pinned && span.clear.is_none());
    let pieces: Vec<Core> = if let Some([d1, d2]) = profile {
        {
            // The original's radius at station x, constant past the span.
            let radius = |x: f64| {
                let t = if hi > lo {
                    ((x - span.from) / (span.to - span.from)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                d1 / 2.0 + (d2 - d1) * t / 2.0
            };
            let step = (to.0 - from.0) / f64::from(PROFILE_PIECES);
            (0..PROFILE_PIECES)
                .map(|k| {
                    let a = from.0 + step * f64::from(k);
                    let b = if k + 1 == PROFILE_PIECES {
                        to.0
                    } else {
                        a + step
                    };
                    let (ra, rb) = (radius(a), radius(b));
                    Core {
                        rho: if stack.hole {
                            ra.max(rb) - tau
                        } else {
                            ra.min(rb) + tau
                        },
                        hole: stack.hole,
                        from: if k == 0 { from } else { (a, false) },
                        to: if k + 1 == PROFILE_PIECES {
                            to
                        } else {
                            (b, false)
                        },
                    }
                })
                .collect()
        }
    } else {
        {
            let rho = match (stack.hole, span.clear.filter(|_| !span.pinned)) {
                (true, Some(c)) => c / 2.0 + tau + ctx.u,
                (true, None) => span.d / 2.0 - tau,
                (false, Some(c)) => c / 2.0 - tau - ctx.u,
                (false, None) => span.d / 2.0 + tau,
            };
            vec![Core {
                rho,
                hole: stack.hole,
                from,
                to,
            }]
        }
    };
    let frame = Frame::new(ctx.back.point(map.p), ctx.back.dir(map.d));
    let at = At::span(span.from, span.to);
    for piece in &pieces {
        if intruded(ctx, stack, (&frame, piece), tau, &at, budget, found) {
            return;
        }
    }
}

/// One piece of a core against the candidate's faces: true if a failure
/// was recorded.
fn intruded(
    ctx: &Ctx<'_>,
    stack: &Stack,
    (frame, piece): (&Frame, &Core),
    tau: f64,
    at: &At,
    budget: &mut Budget,
    found: &mut Found,
) -> bool {
    let rho = piece.rho;
    match ctx.cand.own().intrusion(
        frame,
        piece,
        &stack.exempt,
        (&mut budget.used, budget.limit),
    ) {
        Err(_) => {
            found.fail(
                FailureCode::CoverageExhausted,
                at,
                "the core test reached its bound",
            );
            true
        }
        Ok(None) => false,
        Ok(Some((face, distance))) => {
            let limits = if stack.hole {
                (Some(rho), None)
            } else {
                (None, Some(rho))
            };
            found.measures.push(Measure::number(
                Quantity::CoreDistanceMm,
                at.clone(),
                (distance, Some(rho)),
                (Band::Tau, tau),
                limits,
            ));
            found.fail(
                FailureCode::CoreIntruded,
                at,
                format!(
                    "face #{face} may lie {distance} mm from the axis, inside the core's {rho} mm"
                ),
            );
            true
        }
    }
}

/// VR7's openings: at an end the bore requires open, the candidate's end
/// facing that way is an opening within ε of the span's end there, or, when
/// the end span states a clear diameter, an opening farther out reached
/// through a profile that clears it, crossing no wall opening, its core
/// clear (RS5, RS13, RS14).
#[allow(clippy::too_many_arguments)]
fn end_open(
    ctx: &Ctx<'_>,
    requirement: &Requirement,
    stack: &Stack,
    map: &Map,
    spans: &[Span],
    side: EndSide,
    budget: &mut Budget,
    found: &mut Found,
) {
    let start = side == EndSide::Start;
    let end_span = if start {
        spans
            .iter()
            .min_by(|a, b| a.from.min(a.to).total_cmp(&b.from.min(b.to)))
    } else {
        spans
            .iter()
            .max_by(|a, b| a.from.max(a.to).total_cmp(&b.from.max(b.to)))
    };
    let edge = end_span.map_or(0.0, |s| {
        if start {
            s.from.min(s.to)
        } else {
            s.from.max(s.to)
        }
    });
    let facing = if start == (map.k > 0.0) {
        stack.ends[0]
    } else {
        stack.ends[1]
    };
    let at = At {
        end: Some(side),
        ..At::default()
    };
    let ok = match facing {
        EndAt::Open { wall, rim } => {
            let (wall, rim) = (map.req(wall), rim.map(|x| map.req(x)));
            let near =
                (wall - edge).abs() < ctx.eps || rim.is_some_and(|x| (x - edge).abs() < ctx.eps);
            let outer = rim.unwrap_or(wall);
            let outward = if start { outer < edge } else { outer > edge };
            near || match end_span.and_then(|s| s.clear.map(|c| (s, c))) {
                Some((last, clear)) if outward => {
                    let extension = Span {
                        from: edge.min(outer),
                        to: edge.max(outer),
                        clear: Some(clear),
                        pinned: false,
                        basis: SpanBasis::Cylinder,
                        profile: None,
                        ..*last
                    };
                    extend(
                        ctx,
                        requirement,
                        stack,
                        map,
                        (&extension, last),
                        budget,
                        found,
                    )
                }
                _ => false,
            }
        }
        _ => false,
    };
    let quantity = if start {
        Quantity::OpenStart
    } else {
        Quantity::OpenFinish
    };
    found
        .measures
        .push(Measure::flag(quantity, at.clone(), ok, true));
    if !ok {
        found.fail(
            FailureCode::EndNotOpen,
            &at,
            format!(
                "{} is not open within the tolerance of the span's end at {edge}",
                stack.id
            ),
        );
    }
}

/// RS14: the extension from an end span out to the candidate's opening, as
/// a clearance span of its own at the end span's clear diameter and τ: its
/// profile clears it all the way, it meets no wall opening, its core is
/// clear. True if it passes; its failures are recorded.
fn extend(
    ctx: &Ctx<'_>,
    requirement: &Requirement,
    stack: &Stack,
    map: &Map,
    (extension, end_span): (&Span, &Span),
    budget: &mut Budget,
    found: &mut Found,
) -> bool {
    let mut own = Found::default();
    let (a, b) = (map.cand(extension.from), map.cand(extension.to));
    let (lo, hi) = (a.min(b), a.max(b));
    let at = At::span(extension.from, extension.to);
    let covered = run(&stack.pieces, (lo, hi), TOUCH_MM).filter(|(clo, chi, _)| {
        clo - lo < 1e-9 * (1.0 + lo.abs()) && hi - chi < 1e-9 * (1.0 + hi.abs())
    });
    match covered {
        Some((_, _, over)) => {
            let tau = ctx.tau(requirement, end_span.from, end_span.to);
            let delta = map.offset(extension.from).max(map.offset(extension.to));
            let c = extension.clear.unwrap_or(f64::NAN);
            let lax = Ctx {
                strict: false,
                ..ctx.clone()
            };
            clearance(&lax, true, extension, c, (tau, delta), &over, &at, &mut own);
        }
        None => own.fail(
            FailureCode::EndNotOpen,
            &at,
            "no usable profile out to the opening",
        ),
    }
    for (olo, ohi) in &stack.openings {
        if *olo <= hi + ctx.eps && *ohi >= lo - ctx.eps {
            own.fail(
                FailureCode::WallOpening,
                &at,
                "the extension meets a wall opening",
            );
        }
    }
    core(ctx, requirement, stack, map, extension, budget, &mut own);
    let ok = own.failures.is_empty() && own.measures.iter().all(Measure::passed);
    found.measures.extend(own.measures);
    found.failures.extend(own.failures);
    ok
}

#[cfg(test)]
mod tests {
    use super::{Found, Map, Span, zone_profile};
    use crate::geom::Rigid;
    use crate::matching::candidate::{CandidateView, Piece};
    use crate::matching::evidence::At;
    use crate::matching::verify::Ctx;
    use crate::matching::{MatchPolicy, Placement, SocketView};
    use crate::socket::{SocketDocument, SpanBasis};

    /// A zone of the profile whose radius is unknown (no single cone)
    /// fails the profile, whichever piece comes before it.
    #[test]
    fn a_zone_of_unknown_radius_fails_the_profile_in_any_order() {
        let bytes = include_bytes!("../../tests/assemblies/s03_dowels.stp");
        let Ok(document) = SocketDocument::extract(bytes, Some("PLATE"), None) else {
            unreachable!()
        };
        let Some(view) = SocketView::new(&document, "socket-1") else {
            unreachable!()
        };
        let read = crate::features::FeaturesDocument::read(include_bytes!(
            "../../tests/data/p01_through.stp"
        ));
        let Some(part) = read.part() else {
            unreachable!()
        };
        let cand = CandidateView::of(part);
        let at = Placement::given(Rigid::IDENTITY);
        let ctx = Ctx::new(&view, &cand, &at, &MatchPolicy::default());
        let span = Span {
            from: 0.0,
            to: 10.0,
            d: 6.0,
            clear: None,
            pinned: false,
            basis: SpanBasis::Zone,
            profile: Some([6.0, 6.0]),
        };
        let map = Map::new(([0.0; 3], [0.0, 0.0, 1.0]), ([0.0; 3], [0.0, 0.0, 1.0]));
        let unknown = Piece {
            lo: 0.0,
            hi: 5.0,
            r: 3.0,
            cylinder: None,
            taper: None,
            run: 0,
        };
        let wall = Piece {
            lo: 5.0,
            hi: 10.0,
            r: 3.0,
            cylinder: Some(
                part.cylinders
                    .first()
                    .map_or_else(|| unreachable!(), |c| c.id),
            ),
            taper: None,
            run: 0,
        };
        for pieces in [vec![unknown], vec![unknown, wall], vec![wall, unknown]] {
            let mut found = Found::default();
            zone_profile(
                &ctx,
                &span,
                [6.0, 6.0],
                &map,
                &pieces,
                (0.0, 10.0),
                &At::whole(),
                &mut found,
            );
            assert_eq!(found.failures.len(), 1, "{pieces:?}");
        }
        let mut found = Found::default();
        zone_profile(
            &ctx,
            &span,
            [6.0, 6.0],
            &map,
            &[wall],
            (5.0, 10.0),
            &At::whole(),
            &mut found,
        );
        assert_eq!(found.failures.len(), 0);
    }
}
