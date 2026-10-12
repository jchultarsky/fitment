//! C2: may two plane faces' regions share interior? Each region is
//! over-approximated by a polygon, so the test can only err toward "may":
//! an arc whose centre lies inside the region is replaced by its
//! circumscribing polyline, one whose centre lies outside by its chord.
//! A face's holes are under-approximated, by inscribed polylines, so a
//! region found inside one surely lies in the hole. Never sampled.

// a, b, c, p, q, r: points and polygons, as in any segment test.
#![allow(clippy::many_single_char_names)]

use super::geo::{V, add, cross, dot, norm, scale, sub};
use crate::features::{Outline, Segment};

/// A point in a plane's own 2D frame.
pub(crate) type P2 = [f64; 2];

/// `v` turned by `angle` about unit `n`.
fn rotate(v: V, n: V, angle: f64) -> V {
    let (s, c) = (libm::sin(angle), libm::cos(angle));
    add(
        add(scale(v, c), scale(cross(n, v), s)),
        scale(n, dot(n, v) * (1.0 - c)),
    )
}

/// The outer loop of a known outline as a polygon over-approximating the
/// face's region, in 3D, the face's outward normal being `n`; `None` for
/// an unknown outline.
pub(crate) fn outer_polygon(outline: &Outline, n: V) -> Option<Vec<V>> {
    let Outline::Known { loops } = outline else {
        return None;
    };
    let outer = loops.iter().find(|l| l.outer)?;
    let mut points = Vec::new();
    for segment in &outer.segments {
        match segment {
            Segment::Line { from_mm, .. } => points.push(from_mm.to_array()),
            Segment::Arc {
                from_mm,
                center_mm,
                radius_mm,
                sweep_rad,
                ..
            } => {
                let (a, c) = (from_mm.to_array(), center_mm.to_array());
                points.push(a);
                // The outer loop runs counterclockwise about the normal:
                // an arc turning the same way has the region on its centre's
                // side, so its circumscribing polyline lies outside it.
                if *sweep_rad > 0.0 {
                    let pieces = (sweep_rad.abs() / STEP).ceil().max(1.0);
                    let step = sweep_rad / pieces;
                    let reach = radius_mm / libm::cos(step / 2.0);
                    let start = sub(a, c);
                    let unit = scale(start, 1.0 / norm(start).max(f64::MIN_POSITIVE));
                    let mut k = 0.0;
                    while k < pieces {
                        let turned = rotate(unit, n, step * (k + 0.5));
                        points.push(add(c, scale(turned, reach)));
                        k += 1.0;
                    }
                }
            }
        }
    }
    (points.len() >= 3).then_some(points)
}

/// The widest turn one piece of a polyline replaces (π/8: a circumscribed
/// piece reaches 2% past its arc, an inscribed one falls 2% short).
const STEP: f64 = std::f64::consts::FRAC_PI_8;

/// The holes of a known outline (its inner loops), each as a polygon in
/// 3D lying inside the hole: an arc turning with its loop (clockwise about
/// the normal, so the hole lies on its centre's side) is replaced by an
/// inscribed polyline. A loop with an arc turning against it is skipped,
/// as if it had no hole.
pub(crate) fn hole_polygons(outline: &Outline, n: V) -> Vec<Vec<V>> {
    let Outline::Known { loops } = outline else {
        return Vec::new();
    };
    let mut out = Vec::new();
    'loops: for inner in loops.iter().filter(|l| !l.outer) {
        let mut points = Vec::new();
        for segment in &inner.segments {
            match segment {
                Segment::Line { from_mm, .. } => points.push(from_mm.to_array()),
                Segment::Arc {
                    from_mm,
                    center_mm,
                    sweep_rad,
                    ..
                } => {
                    if *sweep_rad > 0.0 {
                        continue 'loops;
                    }
                    let (a, c) = (from_mm.to_array(), center_mm.to_array());
                    let pieces = (sweep_rad.abs() / STEP).ceil().max(1.0);
                    let step = sweep_rad / pieces;
                    let mut k = 0.0;
                    while k < pieces {
                        points.push(add(c, rotate(sub(a, c), n, step * k)));
                        k += 1.0;
                    }
                }
            }
        }
        if points.len() >= 3 {
            out.push(points);
        }
    }
    out
}

/// A circle of `radius` about `center` in the plane of unit normal `n`, as
/// a polygon around it (circumscribed).
pub(crate) fn disc(center: V, n: V, radius: f64) -> Vec<V> {
    let (u, _) = super::geo::perpendiculars(n);
    let reach = radius / libm::cos(STEP / 2.0);
    let mut out = Vec::new();
    let mut k = 0.0;
    while k < 16.0 {
        out.push(add(center, scale(rotate(u, n, STEP * k), reach)));
        k += 1.0;
    }
    out
}

/// True if polygon `inner` lies inside polygon `outer`, at least `tau`
/// from its edges. `pairs` counts the edge pairs examined (check 39).
pub(crate) fn contained(inner: &[P2], outer: &[P2], tau: f64, pairs: &mut u64) -> bool {
    if outer.len() < 3 || inner.is_empty() {
        return false;
    }
    for (j, r) in outer.iter().enumerate() {
        let q = outer[(j + 1) % outer.len()];
        for (i, p) in inner.iter().enumerate() {
            *pairs += 1;
            if distance_to_segment(*p, *r, q) < tau
                || segments_meet(*p, inner[(i + 1) % inner.len()], *r, q)
            {
                return false;
            }
        }
    }
    inner.iter().all(|p| inside(*p, outer))
}

fn distance_to_segment(p: P2, a: P2, b: P2) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (x, y) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    (x * x + y * y).sqrt()
}

/// `points` in the 2D frame (`u`, `w`) of a plane through `origin`.
pub(crate) fn flatten(points: &[V], origin: V, u: V, w: V) -> Vec<P2> {
    points
        .iter()
        .map(|p| {
            let d = sub(*p, origin);
            [dot(d, u), dot(d, w)]
        })
        .collect()
}

fn bounds(polygon: &[P2]) -> (P2, P2) {
    polygon.iter().fold(
        ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]),
        |(lo, hi), p| {
            (
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            )
        },
    )
}

fn orient(a: P2, b: P2, c: P2) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// True if segments `ab` and `cd` meet, touching included.
fn segments_meet(a: P2, b: P2, c: P2, d: P2) -> bool {
    let (o1, o2, o3, o4) = (
        orient(a, b, c),
        orient(a, b, d),
        orient(c, d, a),
        orient(c, d, b),
    );
    let within = |p: P2, q: P2, r: P2| {
        r[0] >= p[0].min(q[0])
            && r[0] <= p[0].max(q[0])
            && r[1] >= p[1].min(q[1])
            && r[1] <= p[1].max(q[1])
    };
    if o1 * o2 < 0.0 && o3 * o4 < 0.0 {
        return true;
    }
    (o1 == 0.0 && within(a, b, c))
        || (o2 == 0.0 && within(a, b, d))
        || (o3 == 0.0 && within(c, d, a))
        || (o4 == 0.0 && within(c, d, b))
}

/// True if `p` lies inside `polygon` (winding).
fn inside(p: P2, polygon: &[P2]) -> bool {
    let mut winding = 0i64;
    for (i, a) in polygon.iter().enumerate() {
        let b = polygon[(i + 1) % polygon.len()];
        if a[1] <= p[1] {
            if b[1] > p[1] && orient(*a, b, p) > 0.0 {
                winding += 1;
            }
        } else if b[1] <= p[1] && orient(*a, b, p) < 0.0 {
            winding -= 1;
        }
    }
    winding != 0
}

/// True if two polygons may share interior: their boxes (grown by `tau`)
/// meet, and an edge of one meets an edge of the other or one holds a
/// vertex of the other. Touching counts, the safe side. `pairs` counts the
/// edge pairs examined (check 39).
pub(crate) fn may_overlap(a: &[P2], b: &[P2], tau: f64, pairs: &mut u64) -> bool {
    let ((alo, ahi), (blo, bhi)) = (bounds(a), bounds(b));
    if (0..2).any(|i| alo[i] > bhi[i] + tau || blo[i] > ahi[i] + tau) {
        return false;
    }
    if a.first().is_some_and(|p| inside(*p, b)) || b.first().is_some_and(|p| inside(*p, a)) {
        return true;
    }
    for (i, p) in a.iter().enumerate() {
        let q = a[(i + 1) % a.len()];
        for (j, r) in b.iter().enumerate() {
            *pairs += 1;
            if segments_meet(*p, q, *r, b[(j + 1) % b.len()]) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squares_that_overlap_touch_or_stand_apart() {
        let square = |x: f64| vec![[x, 0.0], [x + 10.0, 0.0], [x + 10.0, 10.0], [x, 10.0]];
        let mut pairs = 0;
        assert!(may_overlap(&square(0.0), &square(5.0), 0.01, &mut pairs));
        assert!(
            may_overlap(&square(0.0), &square(10.0), 0.01, &mut pairs),
            "touching counts"
        );
        assert!(!may_overlap(&square(0.0), &square(20.0), 0.01, &mut pairs));
        let small = vec![[2.0, 2.0], [3.0, 2.0], [3.0, 3.0], [2.0, 3.0]];
        assert!(
            may_overlap(&square(0.0), &small, 0.01, &mut pairs),
            "one inside the other"
        );
    }

    #[test]
    fn a_disc_lies_in_a_wider_hole_only() {
        let n = [0.0, 0.0, 1.0];
        let flat = |points: &[V]| flatten(points, [0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        // A dia 6.6 hole's inscribed polygon holds a dia 6 disc's
        // circumscribed one, but not a dia 6.5 disc's, nor one off centre.
        let hole: Vec<V> = (0..32)
            .map(|k| {
                let a = f64::from(k) * std::f64::consts::PI / 16.0;
                [3.3 * libm::cos(a), 3.3 * libm::sin(a), 0.0]
            })
            .collect();
        let mut pairs = 0;
        assert!(contained(
            &flat(&disc([0.0; 3], n, 3.0)),
            &flat(&hole),
            0.01,
            &mut pairs
        ));
        assert!(!contained(
            &flat(&disc([0.0; 3], n, 3.25)),
            &flat(&hole),
            0.01,
            &mut pairs
        ));
        assert!(!contained(
            &flat(&disc([0.3, 0.0, 0.0], n, 3.0)),
            &flat(&hole),
            0.01,
            &mut pairs
        ));
    }
}
