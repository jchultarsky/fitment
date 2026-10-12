//! J3: does material lie across the line between two holes? A plane face
//! blocks it where the axis crosses the face strictly inside its known
//! outline, with a disc of the possible fastener's radius clear of the
//! outline's edges. Exact: lines and arcs as they are, never sampled.
// a, b, p, t, n, e and the like are the design's own names for targets,
// neighbours, pairs and stations.
#![allow(clippy::many_single_char_names)]

use super::geo::{V, add, cross, dot, norm, scale, sub};
use crate::features::{Outline, Segment};

/// The signed angle `a` to `b` turns about unit `n`, in (−π, π]; 0 if
/// either is zero.
fn turn(a: V, b: V, n: V) -> f64 {
    let (sin, cos) = (dot(cross(a, b), n), dot(a, b));
    if sin == 0.0 && cos == 0.0 {
        0.0
    } else {
        libm::atan2(sin, cos)
    }
}

/// `v` turned by `angle` about unit `n` (Rodrigues).
fn rotate(v: V, n: V, angle: f64) -> V {
    let (s, c) = (libm::sin(angle), libm::cos(angle));
    add(
        add(scale(v, c), scale(cross(n, v), s)),
        scale(n, dot(n, v) * (1.0 - c)),
    )
}

/// How far `x` turns around as one segment runs, about `n`.
fn winding_of(segment: &Segment, x: V, n: V) -> f64 {
    match segment {
        Segment::Line { from_mm, to_mm, .. } => {
            turn(sub(from_mm.to_array(), x), sub(to_mm.to_array(), x), n)
        }
        Segment::Arc {
            from_mm,
            to_mm,
            center_mm,
            radius_mm,
            sweep_rad,
            ..
        } => {
            let (a, b, c) = (from_mm.to_array(), to_mm.to_array(), center_mm.to_array());
            let chord = turn(sub(a, x), sub(b, x), n);
            // Inside the region between the chord and the arc, the arc
            // turns once more about x than its chord does.
            let mid = add(c, rotate(sub(a, c), n, sweep_rad / 2.0));
            let side = |p: V| dot(cross(sub(b, a), sub(p, a)), n);
            let whole = (sweep_rad.abs() - std::f64::consts::TAU).abs() < 1e-9;
            let inside =
                norm(sub(x, c)) < *radius_mm && (whole || side(x).signum() == side(mid).signum());
            if inside {
                chord + std::f64::consts::TAU.copysign(*sweep_rad)
            } else {
                chord
            }
        }
    }
}

/// The distance from `x` (on the plane) to one segment.
fn distance_to(segment: &Segment, x: V, n: V) -> f64 {
    match segment {
        Segment::Line { from_mm, to_mm, .. } => {
            let (a, b) = (from_mm.to_array(), to_mm.to_array());
            let d = sub(b, a);
            let len = dot(d, d);
            let t = if len > 0.0 {
                (dot(sub(x, a), d) / len).clamp(0.0, 1.0)
            } else {
                0.0
            };
            norm(sub(x, add(a, scale(d, t))))
        }
        Segment::Arc {
            from_mm,
            to_mm,
            center_mm,
            radius_mm,
            sweep_rad,
            ..
        } => {
            let (a, b, c) = (from_mm.to_array(), to_mm.to_array(), center_mm.to_array());
            let along = turn(sub(a, c), sub(x, c), n);
            // The angle from the start to x, measured in the arc's sense.
            let angle = if *sweep_rad >= 0.0 {
                if along < 0.0 {
                    along + std::f64::consts::TAU
                } else {
                    along
                }
            } else if along > 0.0 {
                along - std::f64::consts::TAU
            } else {
                along
            };
            let within = angle.abs() <= sweep_rad.abs();
            if within {
                (norm(sub(x, c)) - radius_mm).abs()
            } else {
                norm(sub(x, a)).min(norm(sub(x, b)))
            }
        }
    }
}

/// True if the disc of `radius` about `x`, a point of the face's plane
/// with outward normal `n`, lies inside the face's known outline: x inside
/// the outer loop, outside every inner loop, and at least `radius` from
/// every edge. An unknown outline proves nothing.
pub(crate) fn disc_inside(outline: &Outline, x: V, n: V, radius: f64) -> bool {
    let Outline::Known { loops } = outline else {
        return false;
    };
    if loops.is_empty() {
        return false;
    }
    for lp in loops {
        let winding: f64 = lp.segments.iter().map(|s| winding_of(s, x, n)).sum();
        let around = winding.abs() > std::f64::consts::PI;
        if lp.outer != around {
            return false;
        }
        if lp.segments.iter().any(|s| distance_to(s, x, n) < radius) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{FeatureId, FeatureKind, OutlineLoop};
    use crate::geom::Point3;

    fn p(x: f64, y: f64) -> Point3 {
        Point3::new([x, y, 0.0]).unwrap_or(Point3::ORIGIN)
    }

    fn line(a: (f64, f64), b: (f64, f64)) -> Segment {
        Segment::Line {
            edge: 1,
            from_mm: p(a.0, a.1),
            to_mm: p(b.0, b.1),
            across: FeatureId::new(FeatureKind::Plane, 1),
        }
    }

    #[test]
    fn a_square_with_a_round_hole() {
        let outer = OutlineLoop {
            bound: 1,
            outer: true,
            area_mm2: 400.0,
            segments: vec![
                line((0.0, 0.0), (20.0, 0.0)),
                line((20.0, 0.0), (20.0, 20.0)),
                line((20.0, 20.0), (0.0, 20.0)),
                line((0.0, 20.0), (0.0, 0.0)),
            ],
        };
        let hole = OutlineLoop {
            bound: 2,
            outer: false,
            area_mm2: -std::f64::consts::PI * 4.0,
            segments: vec![Segment::Arc {
                edge: 2,
                from_mm: p(12.0, 10.0),
                to_mm: p(12.0, 10.0),
                center_mm: p(10.0, 10.0),
                radius_mm: 2.0,
                sweep_rad: -std::f64::consts::TAU,
                across: FeatureId::new(FeatureKind::Cylinder, 2),
            }],
        };
        let outline = Outline::Known {
            loops: vec![outer, hole],
        };
        let n = [0.0, 0.0, 1.0];
        assert!(disc_inside(&outline, [5.0, 5.0, 0.0], n, 1.0));
        assert!(
            !disc_inside(&outline, [5.0, 5.0, 0.0], n, 6.0),
            "the disc crosses an edge"
        );
        assert!(
            !disc_inside(&outline, [10.0, 10.0, 0.0], n, 0.5),
            "in the hole"
        );
        assert!(
            !disc_inside(&outline, [10.0, 13.0, 0.0], n, 1.5),
            "the disc reaches the hole"
        );
        assert!(!disc_inside(&outline, [25.0, 5.0, 0.0], n, 1.0), "outside");
    }
}
