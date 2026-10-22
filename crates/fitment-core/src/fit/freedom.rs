//! A placement's freedom (`docs/research/m7-fit.md`, FT10, FC4): the
//! motions the interface leaves free, found from the placement itself,
//! never from how alignment seeded it. The chosen placement is moved by
//! each motion of a set built from the socket's own geometry, and the
//! interface is checked again there; a motion after which every active
//! requirement still passes is free. A free turn about an axis the
//! candidate is axisymmetric about is excused; any other free motion makes
//! the fit `placement_not_unique` (D1's rule).

use crate::features::{Axis, Part};
use crate::geom::{Dir3, Point3, Rigid};
use crate::matching::{CandidateView, MatchPolicy, Passing, Placement, SocketView, passing};
use crate::socket::Requires;
use crate::socket::geo::{V, across, cross, dot, norm, perpendiculars, scale, sub};

/// The turn tried either way about each axis and normal, in degrees.
pub const FREE_TURN_DEG: f64 = 1.0;

/// The slide tried either way along each axis, normal and in-plane
/// direction, in mm.
pub const FREE_SLIDE_MM: f64 = 0.1;

/// Lines this close are one: in sine, and in mm.
const SAME_LINE_SINE: f64 = 1e-9;
const SAME_LINE_MM: f64 = 1e-6;

/// A free motion's kind (FM4).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MotionKind {
    /// A turn about a line.
    Turn,
    /// A slide along a direction.
    Slide,
}

/// One motion the interface leaves free (FT10, FM4).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FreeMotion {
    /// A turn or a slide.
    pub kind: MotionKind,
    /// The line it turns about, or a line along which it slides, in the
    /// socket's frame.
    pub about: Axis,
    /// For a turn, true if the candidate is axisymmetric about the line
    /// (from its own topology): the turn is excused.
    pub axisymmetric: bool,
}

/// A placement's free motions (FC4): none for a unique placement.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct Freedom {
    /// The free motions, each once, turns about requirement axes first.
    pub motions: Vec<FreeMotion>,
}

impl Freedom {
    /// True if no motion is free.
    pub fn unique(&self) -> bool {
        self.motions.is_empty()
    }

    /// True if every free motion is a turn about an axis the candidate is
    /// axisymmetric about: a fit may be asked (FT10).
    pub fn excused(&self) -> bool {
        self.motions
            .iter()
            .all(|m| m.kind == MotionKind::Turn && m.axisymmetric)
    }

    /// The free motions FT10 does not excuse, for people.
    pub fn describe(&self) -> String {
        let named: Vec<String> = self
            .motions
            .iter()
            .filter(|m| !(m.kind == MotionKind::Turn && m.axisymmetric))
            .map(|m| {
                let [p, d] = [m.about.point_mm.to_array(), m.about.direction.to_array()];
                let kind = match m.kind {
                    MotionKind::Turn => "a turn about",
                    MotionKind::Slide => "a slide along",
                };
                format!("{kind} {p:?} {d:?}")
            })
            .collect();
        named.join("; ")
    }
}

/// One motion to try: its kind, its line in the socket's frame, and the
/// two motions either way.
struct Trial {
    kind: MotionKind,
    point: V,
    direction: V,
    either: [Rigid; 2],
}

/// FC4: the motions free at placement `at` of the candidate `cand` in
/// socket `view`, under `policy`. Each motion is applied after `at` (in
/// the socket's frame) and every active requirement checked again, as
/// [`crate::Matcher::verify_at`] checks them; a motion after which every
/// requirement that passes at `at` still passes is free. Where the fit is
/// asked every active requirement passes at `at`; a requirement that fails
/// everywhere (an unverifiable one) constrains nothing. A check that stops
/// at the coverage bound could not be judged and counts as passing, so a
/// bound never makes a placement look unique.
pub fn freedom(
    view: &SocketView<'_>,
    cand: &CandidateView<'_>,
    at: &Placement,
    policy: &MatchPolicy,
) -> Freedom {
    let back = at.transform.inverse();
    let base = passing(view, cand, at, policy);
    let mut out = Freedom::default();
    for trial in trials(view) {
        let free = trial.either.iter().any(|motion| {
            at.transform.then(motion).is_some_and(|moved| {
                let there = passing(view, cand, &Placement::given(moved), policy);
                base.iter()
                    .zip(&there)
                    .all(|(a, b)| *a != Passing::Passes || *b != Passing::Fails)
            })
        });
        if !free {
            continue;
        }
        let axisymmetric = trial.kind == MotionKind::Turn
            && back.is_some_and(|back| {
                let p = Point3::new(trial.point).and_then(|p| back.apply_point(p));
                let d = Dir3::from_unit(trial.direction).and_then(|d| back.apply_dir(d));
                p.zip(d)
                    .is_some_and(|(p, d)| axisymmetric(cand.part(), p.to_array(), d.to_array()))
            });
        let (Some(point_mm), Some(direction)) =
            (Point3::new(trial.point), Dir3::from_unit(trial.direction))
        else {
            continue;
        };
        out.motions.push(FreeMotion {
            kind: trial.kind,
            about: Axis {
                point_mm,
                direction,
            },
            axisymmetric,
        });
    }
    out
}

/// FT10's motions for `view`'s socket: a turn either way about every
/// requirement axis and every seat normal (through the seat's plane), a
/// slide either way along each, and along two directions in every seat's
/// plane, each line once.
fn trials(view: &SocketView<'_>) -> Vec<Trial> {
    // Axes first, then seat normals, each line once among both.
    let mut lines: Vec<(V, V)> = Vec::new();
    let mut normals: Vec<(V, V)> = Vec::new();
    for requirement in &view.socket().requirements {
        match &requirement.requires {
            Requires::Bore { axis, .. }
            | Requires::Shaft { axis, .. }
            | Requires::ConicalSeat { axis, .. }
            | Requires::PartialBore { axis, .. }
            | Requires::PartialShaft { axis, .. } => {
                push_line(
                    &mut lines,
                    axis.point_mm.to_array(),
                    axis.direction.to_array(),
                );
            }
            Requires::Seat {
                normal, offset_mm, ..
            } => {
                let n = normal.to_array();
                push_line(&mut normals, scale(n, *offset_mm), n);
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    let mut turns = lines.clone();
    for (p, n) in &normals {
        push_line(&mut turns, *p, *n);
    }
    for (p, d) in &turns {
        out.extend(trial(MotionKind::Turn, *p, *d));
    }
    let mut slides: Vec<(V, V)> = Vec::new();
    for (p, d) in &lines {
        push_slide(&mut slides, *p, *d);
    }
    for (p, n) in &normals {
        let (a, b) = perpendiculars(*n);
        for d in [*n, a, b] {
            push_slide(&mut slides, *p, d);
        }
    }
    for (p, d) in slides {
        out.extend(trial(MotionKind::Slide, p, d));
    }
    out
}

/// Adds the line `(p, d)` unless it is one already listed.
fn push_line(lines: &mut Vec<(V, V)>, p: V, d: V) {
    let same = |(q, e): &(V, V)| {
        norm(cross(*e, d)) <= SAME_LINE_SINE && norm(across(sub(p, *q), *e)) <= SAME_LINE_MM
    };
    if !lines.iter().any(same) {
        lines.push((p, d));
    }
}

/// Adds a slide along `d` unless one along it is listed: a slide's line
/// does not matter, only its direction.
fn push_slide(slides: &mut Vec<(V, V)>, p: V, d: V) {
    if !slides
        .iter()
        .any(|(_, e)| norm(cross(*e, d)) <= SAME_LINE_SINE)
    {
        slides.push((p, d));
    }
}

fn trial(kind: MotionKind, p: V, d: V) -> Option<Trial> {
    let either = match kind {
        MotionKind::Turn => [turn(p, d, FREE_TURN_DEG)?, turn(p, d, -FREE_TURN_DEG)?],
        MotionKind::Slide => [slide(d, FREE_SLIDE_MM)?, slide(d, -FREE_SLIDE_MM)?],
    };
    Some(Trial {
        kind,
        point: p,
        direction: d,
        either,
    })
}

/// A turn by `degrees` about the line through `p` along unit `d`.
fn turn(point: V, d: V, degrees: f64) -> Option<Rigid> {
    let (sin, cos) = libm::sincos(degrees.to_radians());
    let [x, y, z] = d;
    let versine = 1.0 - cos;
    let rotation = [
        [
            versine * x * x + cos,
            versine * x * y - sin * z,
            versine * x * z + sin * y,
        ],
        [
            versine * x * y + sin * z,
            versine * y * y + cos,
            versine * y * z - sin * x,
        ],
        [
            versine * x * z - sin * y,
            versine * y * z + sin * x,
            versine * z * z + cos,
        ],
    ];
    let moved = rotation.map(|row| dot(row, point));
    Rigid::new(rotation, sub(point, moved))
}

/// A slide by `mm` along unit `d`.
fn slide(d: V, mm: f64) -> Option<Rigid> {
    Rigid::new(Rigid::IDENTITY.rotation(), scale(d, mm))
}

/// FT10: true if every face of `part` is a plane square to the line
/// `(p, d)` (in the part's frame) or a cylinder, cone or torus on it, and
/// none is a face no rule reads.
fn axisymmetric(part: &Part, p: V, d: V) -> bool {
    let parallel = |e: V| norm(cross(e, d)) <= SAME_LINE_SINE;
    let on = |axis: &Axis| {
        parallel(axis.direction.to_array())
            && norm(across(sub(axis.point_mm.to_array(), p), d)) <= SAME_LINE_MM
    };
    part.other_faces.is_empty()
        && part.planes.iter().all(|pl| parallel(pl.normal.to_array()))
        && part.cylinders.iter().all(|c| on(&c.axis))
        && part.cones.iter().all(|c| on(&c.axis))
        && part.tori.iter().all(|t| on(&t.axis))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A turn moves a point off its axis by its angle, and none on it; a
    /// slide moves every point by its length.
    #[test]
    fn turns_and_slides_move_as_stated() {
        let r = turn([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], 90.0).unwrap();
        let on = r
            .apply_point(Point3::new([1.0, 0.0, 5.0]).unwrap())
            .unwrap();
        assert!(norm(sub(on.to_array(), [1.0, 0.0, 5.0])) < 1e-12);
        let off = r
            .apply_point(Point3::new([2.0, 0.0, 0.0]).unwrap())
            .unwrap();
        assert!(norm(sub(off.to_array(), [1.0, 1.0, 0.0])) < 1e-12);
        let s = slide([0.0, 1.0, 0.0], 0.1).unwrap();
        assert_eq!(s.translation(), [0.0, 0.1, 0.0]);
    }

    /// Coaxial lines are one, parallel ones two; slides along one
    /// direction are one whatever their line.
    #[test]
    fn lines_and_slides_are_listed_once() {
        let mut lines = Vec::new();
        push_line(&mut lines, [0.0; 3], [0.0, 0.0, 1.0]);
        push_line(&mut lines, [0.0, 0.0, 7.0], [0.0, 0.0, 1.0]);
        push_line(&mut lines, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        assert_eq!(lines.len(), 2);
        let mut slides = Vec::new();
        push_slide(&mut slides, [0.0; 3], [0.0, 0.0, 1.0]);
        push_slide(&mut slides, [5.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        push_slide(&mut slides, [5.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        assert_eq!(slides.len(), 2);
    }

    /// A placement free only in an axisymmetric turn is excused; a slide,
    /// or a turn the shape sees, is not.
    #[test]
    fn only_axisymmetric_turns_are_excused() {
        let axis = Axis {
            point_mm: Point3::ORIGIN,
            direction: Dir3::from_unit([0.0, 0.0, 1.0]).unwrap(),
        };
        let motion = |kind, axisymmetric| FreeMotion {
            kind,
            about: axis,
            axisymmetric,
        };
        let unique = Freedom::default();
        assert!(unique.unique() && unique.excused());
        let round = Freedom {
            motions: vec![motion(MotionKind::Turn, true)],
        };
        assert!(!round.unique() && round.excused());
        assert_eq!(round.describe(), "");
        let block = Freedom {
            motions: vec![
                motion(MotionKind::Turn, true),
                motion(MotionKind::Turn, false),
            ],
        };
        assert!(!block.excused());
        assert!(block.describe().starts_with("a turn about"));
        let pin = Freedom {
            motions: vec![motion(MotionKind::Slide, false)],
        };
        assert!(!pin.excused());
    }
}
