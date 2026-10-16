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

/// A face's box in the plane: lowest and highest corner.
type Box2 = (P2, P2);

impl S2 {
    /// A box holding the edge: a line's ends, an arc's whole circle.
    fn bounds(&self) -> Box2 {
        match *self {
            Self::Line { a, b } => (
                [a[0].min(b[0]), a[1].min(b[1])],
                [a[0].max(b[0]), a[1].max(b[1])],
            ),
            Self::Arc { c, r, .. } => ([c[0] - r, c[1] - r], [c[0] + r, c[1] + r]),
        }
    }
}

/// The cover's faces by a grid of their boxes, so containment asks only
/// the faces whose boxes hold the point.
struct Grid {
    boxes: Vec<Box2>,
    origin: P2,
    cell: P2,
    side: usize,
    cells: Vec<Vec<usize>>,
}

impl Grid {
    /// The grid of `cover`; each face's edges and each cell it is filed in
    /// count toward `budget`.
    fn new(cover: &[Face2], budget: &mut Budget) -> Result<Self, Uncovered> {
        let boxes: Vec<Box2> = cover
            .iter()
            .map(|f| {
                budget.used = budget.used.saturating_add(f.segs.len() as u64);
                f.segs.iter().map(S2::bounds).fold(
                    ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]),
                    |(lo, hi), (a, b)| {
                        (
                            [lo[0].min(a[0]), lo[1].min(a[1])],
                            [hi[0].max(b[0]), hi[1].max(b[1])],
                        )
                    },
                )
            })
            .collect();
        let (lo, hi) = boxes.iter().fold(
            ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]),
            |(lo, hi), (a, b)| {
                (
                    [lo[0].min(a[0]), lo[1].min(a[1])],
                    [hi[0].max(b[0]), hi[1].max(b[1])],
                )
            },
        );
        // About one face per cell, at most 256 a side.
        let side: u32 = (1..256)
            .find(|s| (s * s) as usize >= boxes.len())
            .unwrap_or(256);
        let span = |k: usize| ((hi[k] - lo[k]) / f64::from(side)).max(f64::MIN_POSITIVE);
        let side = side as usize;
        let cell = [span(0), span(1)];
        let finite = lo.iter().chain(&hi).all(|x| x.is_finite());
        let mut grid = Self {
            boxes,
            origin: lo,
            cell,
            side,
            cells: vec![Vec::new(); side * side],
        };
        for (i, (a, b)) in grid.boxes.clone().iter().enumerate() {
            let range = |k: usize| {
                if finite && a[k].is_finite() && b[k].is_finite() {
                    (grid.index(a[k], k), grid.index(b[k], k))
                } else {
                    (0, side - 1)
                }
            };
            let ((x0, x1), (y0, y1)) = (range(0), range(1));
            budget.step((x1 - x0 + 1) * (y1 - y0 + 1))?;
            for x in x0..=x1 {
                for y in y0..=y1 {
                    if let Some(c) = grid.cells.get_mut(x * side + y) {
                        c.push(i);
                    }
                }
            }
        }
        Ok(grid)
    }

    /// The cell index along `k` of coordinate `v`, clamped to the grid.
    fn index(&self, v: f64, k: usize) -> usize {
        let at = ((v - self.origin[k]) / self.cell[k]).floor();
        // Steps of one cell up to the grid's side: no cast of a float.
        (0..self.side)
            .take_while(|i| {
                at.is_finite() && f64::from(u32::try_from(*i).unwrap_or(u32::MAX)) <= at
            })
            .last()
            .unwrap_or(0)
    }

    /// The faces whose boxes hold `q`.
    fn holding(&self, q: P2) -> impl Iterator<Item = usize> + '_ {
        let cell = self.index(q[0], 0) * self.side + self.index(q[1], 1);
        self.cells
            .get(cell)
            .into_iter()
            .flatten()
            .copied()
            .filter(move |&i| {
                self.boxes.get(i).is_some_and(|(a, b)| {
                    q[0] >= a[0] && q[0] <= b[0] && q[1] >= a[1] && q[1] <= b[1]
                })
            })
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
    /// One step that evaluates `work` edges; `Err` past the bound. A step
    /// costs what it reads, so a region of many edges spends the bound as
    /// fast as it spends time (AL11).
    fn step(&mut self, work: usize) -> Result<(), Uncovered> {
        self.used = self
            .used
            .saturating_add(u64::try_from(work).unwrap_or(u64::MAX).max(1));
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
    let depth = region.segs.len();
    let grid = Grid::new(cover, budget)?;
    // Each edge's start first: most wrong placements end here.
    for s in boundary() {
        budget.step(depth)?;
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
            budget.step(depth)?;
            let q = s.at(x.min(l));
            let h = region.depth(q) - eps / 2.0;
            if h > 0.0 {
                // A face whose box does not hold q does not hold it.
                let mut inside = false;
                for f in grid.holding(q) {
                    let Some(face) = cover.get(f) else {
                        continue;
                    };
                    budget.step(face.segs.len())?;
                    if face.contains(q) {
                        inside = true;
                        break;
                    }
                }
                if !inside {
                    return Err(Uncovered::NotCovered { at: q });
                }
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
        budget.step(region.segs.len())?;
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
        Budget::of(10_000_000)
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
        let mut tight = Budget::of(10);
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
