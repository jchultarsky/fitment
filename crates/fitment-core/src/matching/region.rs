//! VR11: a seat's region covered by the candidate's faces, certified and
//! never sampled without a bound. Regions are bounded by lines and circular
//! arcs, exactly as M2 states them, in the seat's plane.

use std::f64::consts::PI;

use super::verify::Budget;

/// A point in the seat's plane.
pub(crate) type P2 = [f64; 2];

/// One edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum S2 {
    /// A straight edge from `a` to `b`.
    Line { a: P2, b: P2 },
    /// An arc about `c` of radius `r` from angle `a0`, sweeping `sw`
    /// (counterclockwise positive).
    Arc { c: P2, r: f64, a0: f64, sw: f64 },
}

fn sub2(a: P2, b: P2) -> P2 {
    [a[0] - b[0], a[1] - b[1]]
}

fn len2(a: P2) -> f64 {
    (a[0] * a[0] + a[1] * a[1]).sqrt()
}

/// True for an arc that is a whole circle.
fn whole(sw: f64) -> bool {
    sw.abs() >= 2.0 * PI - 1e-9
}

impl S2 {
    fn length(&self) -> f64 {
        match *self {
            Self::Line { a, b } => len2(sub2(b, a)),
            Self::Arc { r, sw, .. } => r * sw.abs(),
        }
    }

    /// The point at arc length `x` from its start.
    fn at(&self, x: f64) -> P2 {
        match *self {
            Self::Line { a, b } => {
                let t = x / len2(sub2(b, a)).max(f64::MIN_POSITIVE);
                [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
            }
            Self::Arc { c, r, a0, sw } => {
                let angle = a0 + sw.signum() * x / r;
                [c[0] + r * libm::cos(angle), c[1] + r * libm::sin(angle)]
            }
        }
    }

    fn ends(&self) -> (P2, P2) {
        (self.at(0.0), self.at(self.length()))
    }

    /// The angle this edge sweeps seen from `q`: summed over a closed loop,
    /// 2π times its winding number about `q`.
    fn view(&self, q: P2) -> f64 {
        let (first, last) = self.ends();
        let (to_first, to_last) = (sub2(first, q), sub2(last, q));
        let principal = libm::atan2(
            to_first[0] * to_last[1] - to_first[1] * to_last[0],
            to_first[0] * to_last[0] + to_first[1] * to_last[1],
        );
        match *self {
            Self::Line { .. } => principal,
            Self::Arc { c, r, sw, .. } => {
                let inside = len2(sub2(q, c)) < r;
                if whole(sw) {
                    // Its ends coincide, so the principal angle is
                    // rounding: from inside it winds once.
                    if inside { 2.0 * PI * sw.signum() } else { 0.0 }
                } else if !inside {
                    principal
                } else if sw > 0.0 && principal <= 0.0 {
                    principal + 2.0 * PI
                } else if sw < 0.0 && principal >= 0.0 {
                    principal - 2.0 * PI
                } else {
                    principal
                }
            }
        }
    }

    /// The distance from `q`.
    fn dist(&self, q: P2) -> f64 {
        match *self {
            Self::Line { a, b } => {
                let d = sub2(b, a);
                let l2 = d[0] * d[0] + d[1] * d[1];
                let t = if l2 > 0.0 {
                    (((q[0] - a[0]) * d[0] + (q[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                len2(sub2(q, [a[0] + d[0] * t, a[1] + d[1] * t]))
            }
            Self::Arc { c, r, a0, sw } => {
                let w = sub2(q, c);
                let phi = libm::atan2(w[1], w[0]);
                let turned = if sw > 0.0 {
                    (phi - a0).rem_euclid(2.0 * PI)
                } else {
                    (a0 - phi).rem_euclid(2.0 * PI)
                };
                if turned <= sw.abs() || whole(sw) {
                    (len2(w) - r).abs()
                } else {
                    let (a, b) = self.ends();
                    len2(sub2(q, a)).min(len2(sub2(q, b)))
                }
            }
        }
    }

    /// The curve at distance `e` on its left, the material side of an M2
    /// loop; `None` where an arc shrinks to nothing.
    fn left_offset(&self, e: f64) -> Option<Self> {
        match *self {
            Self::Line { a, b } => {
                let d = sub2(b, a);
                let l = len2(d);
                if l <= 0.0 {
                    return None;
                }
                let n = [-d[1] / l * e, d[0] / l * e];
                Some(Self::Line {
                    a: [a[0] + n[0], a[1] + n[1]],
                    b: [b[0] + n[0], b[1] + n[1]],
                })
            }
            Self::Arc { c, r, a0, sw } => {
                let r2 = if sw > 0.0 { r - e } else { r + e };
                (r2 > 0.0).then_some(Self::Arc { c, r: r2, a0, sw })
            }
        }
    }
}

/// A face in the seat's plane: its loops' edges together (outer
/// counterclockwise, holes clockwise, about the seat's normal), each marked
/// if both faces it joins are in the union, so no boundary of it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Face2 {
    /// Its edges.
    pub(crate) segs: Vec<S2>,
    /// Whether each joins two faces of the union.
    pub(crate) internal: Vec<bool>,
}

impl Face2 {
    fn winding(&self, q: P2) -> f64 {
        self.segs.iter().map(|s| s.view(q)).sum::<f64>() / (2.0 * PI)
    }

    /// True if `q` lies inside.
    pub(crate) fn contains(&self, q: P2) -> bool {
        self.winding(q) > 0.5
    }

    fn dist(&self, q: P2) -> f64 {
        self.segs
            .iter()
            .map(|s| s.dist(q))
            .fold(f64::INFINITY, f64::min)
    }

    /// The signed depth of `q`: positive inside.
    fn depth(&self, q: P2) -> f64 {
        let d = self.dist(q);
        if self.contains(q) { d } else { -d }
    }
}

/// Why a region is not covered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Uncovered {
    /// A candidate edge lies this deep inside, here.
    EdgeInside { at: P2, depth: f64 },
    /// A point at depth ε is not covered.
    NotCovered { at: P2 },
    /// No tolerance left to cover within.
    NoTolerance,
    /// The work bound was reached.
    Exhausted,
}

impl Budget {
    /// One step; `Err` past the bound.
    fn step(&mut self) -> Result<(), Uncovered> {
        self.used += 1;
        if self.used > self.limit {
            Err(Uncovered::Exhausted)
        } else {
            Ok(())
        }
    }
}

/// VR11: does the union of `cover` hold every point of `region` deeper
/// than `eps`? (i) No boundary edge of the union lies ε/2 or deeper inside
/// the region, marched with the 1-Lipschitz depth f in steps of ε/2 − f;
/// (ii) every point deeper than ε/2 on the curves at depth ε (each edge
/// offset inward, a circle of radius ε about each vertex) is in the union.
/// (i) keeps membership constant on each component of the points deeper
/// than ε/2, and (ii) visits every component holding a point at depth ε;
/// the rest lies within ε of the boundary.
pub(crate) fn covers(
    region: &Face2,
    cover: &[Face2],
    eps: f64,
    budget: &mut Budget,
) -> Result<(), Uncovered> {
    if eps.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return Err(Uncovered::NoTolerance);
    }
    let boundary = || {
        cover.iter().flat_map(|f| {
            f.segs
                .iter()
                .zip(&f.internal)
                .filter(|(_, internal)| !**internal)
                .map(|(s, _)| s)
        })
    };
    // Each edge's start first: most wrong placements end here.
    for s in boundary() {
        budget.step()?;
        let q = s.at(0.0);
        let f = region.depth(q);
        if f >= eps / 2.0 {
            return Err(Uncovered::EdgeInside { at: q, depth: f });
        }
    }
    for s in boundary() {
        march_edge(region, s, eps, budget)?;
    }
    let mut curves: Vec<S2> = Vec::new();
    for s in &region.segs {
        if let Some(o) = s.left_offset(eps) {
            curves.push(o);
        }
        curves.push(S2::Arc {
            c: s.at(0.0),
            r: eps,
            a0: 0.0,
            sw: 2.0 * PI,
        });
    }
    for s in &curves {
        let l = s.length();
        let mut x: f64 = 0.0;
        loop {
            budget.step()?;
            let q = s.at(x.min(l));
            let h = region.depth(q) - eps / 2.0;
            if h > 0.0 && !cover.iter().any(|f| f.contains(q)) {
                return Err(Uncovered::NotCovered { at: q });
            }
            if x >= l {
                break;
            }
            x += h.abs().max(eps * 1e-4);
        }
    }
    Ok(())
}

/// (i) on one edge of the union.
fn march_edge(region: &Face2, edge: &S2, eps: f64, budget: &mut Budget) -> Result<(), Uncovered> {
    let length = edge.length();
    let mut along: f64 = 0.0;
    loop {
        budget.step()?;
        let point = edge.at(along.min(length));
        let depth = region.depth(point);
        let step = eps / 2.0 - depth;
        // At ε/2 or deeper, or too close to it to march past.
        if step < eps * 1e-4 {
            return Err(Uncovered::EdgeInside { at: point, depth });
        }
        if along >= length {
            return Ok(());
        }
        along += step;
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::{Face2, S2, Uncovered, covers};
    use crate::matching::verify::Budget;

    fn square(lo: f64, hi: f64) -> Face2 {
        let p = [[lo, lo], [hi, lo], [hi, hi], [lo, hi]];
        let segs: Vec<S2> = (0..4)
            .map(|i| S2::Line {
                a: p[i],
                b: p[(i + 1) % 4],
            })
            .collect();
        Face2 {
            internal: vec![false; segs.len()],
            segs,
        }
    }

    fn disc(r: f64) -> Face2 {
        Face2 {
            segs: vec![S2::Arc {
                c: [0.0, 0.0],
                r,
                a0: 0.0,
                sw: 2.0 * PI,
            }],
            internal: vec![false],
        }
    }

    fn budget() -> Budget {
        Budget {
            used: 0,
            limit: 10_000_000,
        }
    }

    /// The region covers itself; a smaller cover fails; a hole 0.6ε inside
    /// fails; an edge between ε/2 and ε inside rejects (VR11's false
    /// negative), one under ε/2 passes.
    #[test]
    fn a_region_is_covered_to_within_the_tolerance() {
        let eps = 0.01;
        let region = square(0.0, 10.0);
        assert_eq!(
            covers(&region, std::slice::from_ref(&region), eps, &mut budget()),
            Ok(())
        );
        assert!(covers(&region, &[square(0.0, 9.0)], eps, &mut budget()).is_err());
        assert_eq!(
            covers(&region, &[square(0.004, 9.996)], eps, &mut budget()),
            Ok(())
        );
        assert!(matches!(
            covers(&region, &[square(0.007, 9.993)], eps, &mut budget()),
            Err(Uncovered::EdgeInside { .. })
        ));
        assert!(covers(&region, &[], eps, &mut budget()).is_err());
        // Two halves sharing an edge cover it when the edge is internal,
        // and not when it is a boundary (a face of unknown outline beside).
        let half = |lo: f64, hi: f64, shared: usize| {
            let p = [[lo, 0.0], [hi, 0.0], [hi, 10.0], [lo, 10.0]];
            let segs: Vec<S2> = (0..4)
                .map(|i| S2::Line {
                    a: p[i],
                    b: p[(i + 1) % 4],
                })
                .collect();
            let internal = (0..4).map(|i| i == shared).collect();
            Face2 { segs, internal }
        };
        let joined = [half(0.0, 5.0, 1), half(5.0, 10.0, 3)];
        assert_eq!(covers(&region, &joined, eps, &mut budget()), Ok(()));
        let one = [half(0.0, 5.0, 4)];
        assert!(covers(&region, &one, eps, &mut budget()).is_err());
    }

    /// A whole circle winds once from inside, never from outside; a disc
    /// region is covered by itself and not by a smaller disc.
    #[test]
    fn a_whole_circle_winds_once_from_inside() {
        let d = disc(5.0);
        assert!(d.contains([0.0, 0.0]) && d.contains([4.99, 0.0]));
        assert!(!d.contains([5.01, 0.0]));
        assert_eq!(
            covers(&d, std::slice::from_ref(&d), 0.01, &mut budget()),
            Ok(())
        );
        assert!(covers(&d, &[disc(4.9)], 0.01, &mut budget()).is_err());
        // An annulus: the disc with a clockwise hole.
        let mut ring = disc(5.0);
        ring.segs.push(S2::Arc {
            c: [0.0, 0.0],
            r: 2.0,
            a0: 0.0,
            sw: -2.0 * PI,
        });
        ring.internal.push(false);
        assert!(!ring.contains([0.0, 0.0]) && ring.contains([3.0, 0.0]));
        assert_eq!(
            covers(&ring, std::slice::from_ref(&d), 0.01, &mut budget()),
            Ok(())
        );
        assert!(covers(&d, &[ring.clone()], 0.01, &mut budget()).is_err());
    }

    /// The bound ends the work as a failure, never a pass.
    #[test]
    fn coverage_stops_at_its_bound() {
        let region = square(0.0, 1000.0);
        let mut tight = Budget { used: 0, limit: 10 };
        assert_eq!(
            covers(&region, std::slice::from_ref(&region), 0.01, &mut tight),
            Err(Uncovered::Exhausted)
        );
        assert_eq!(
            covers(&region, std::slice::from_ref(&region), 0.0, &mut budget()),
            Err(Uncovered::NoTolerance)
        );
    }
}
