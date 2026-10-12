//! Rule F: whether a cylinder group wraps all the way around its axis
//! (`docs/research/m2-features.md`, section 8.3).

use std::f64::consts::TAU;

use libm::atan2;

use super::cylinder::{CylinderClass, End, UndecidedReason};
use super::group::{self, IDENTITY_MM, LEVER_MIN_MM};
use super::topo::{Curve, Loop, P, Topo, Travel, cross, dot, length, neg, radial, scale, sub};

/// Which side of the stations a vertex lies on, in mm (F4).
pub(crate) const VERTEX_SIDE_MM: f64 = 1e-3;

/// How close a station's summed sweep must be to 2π, in rad (F2).
pub(crate) const SWEEP_RAD: f64 = 1e-9;

/// What rule F finds of one group.
pub(crate) struct Classified {
    pub(crate) class: CylinderClass,
    /// The axis point: at the lowest station, else at the start of the
    /// seam's span, else the reference surface's location.
    pub(crate) point: P,
    /// The axis direction, with the canonical sign.
    pub(crate) z: P,
    /// The stations, as distances along `z` from `point`.
    pub(crate) stations: Vec<f64>,
    pub(crate) wall_openings: u32,
}

/// A chain of end arcs that sweeps a multiple of 2π other than ±2π
/// (check 29), at the `#id` of one of its edges.
pub(crate) struct Contradiction(pub(crate) u64);

/// `z` with its canonical sign (section 9.4): the largest absolute
/// component positive, ties broken in the order x, y, z.
pub(crate) fn canonical(z: P) -> P {
    let mut largest = 0;
    for (i, c) in z.iter().enumerate() {
        if z.get(largest).is_some_and(|l| c.abs() > l.abs()) {
            largest = i;
        }
    }
    if z.get(largest).is_some_and(|c| *c < 0.0) {
        neg(z)
    } else {
        z
    }
}

/// A unit vector perpendicular to unit `z`: the world axis least along
/// it, projected off it.
fn perpendicular(z: P) -> P {
    let mut least = 0;
    for (i, c) in z.iter().enumerate() {
        if z.get(least).is_some_and(|l| c.abs() < l.abs()) {
            least = i;
        }
    }
    let mut e = [0.0; 3];
    if let Some(slot) = e.get_mut(least) {
        *slot = 1.0;
    }
    let x = sub(e, scale(z, dot(e, z)));
    scale(x, 1.0 / length(x))
}

/// The group's axis and the frame its angles are measured in.
struct Frame {
    origin: P,
    z: P,
    x: P,
    y: P,
    radius: f64,
}

impl Frame {
    fn station(&self, p: P) -> f64 {
        dot(sub(p, self.origin), self.z)
    }

    fn angle(&self, p: P, centre: P) -> f64 {
        let d = sub(p, centre);
        atan2(dot(d, self.y), dot(d, self.x))
    }

    /// True if `travel` runs along a line on the cylinder: parallel to the
    /// axis at its lever arm, and at the radius.
    fn axial(&self, travel: &Travel<'_>) -> bool {
        let Curve::Line { dir, .. } = travel.curve else {
            return false;
        };
        let lever = length(sub(travel.to, travel.from)).max(LEVER_MIN_MM);
        let middle = scale(
            [
                travel.from[0] + travel.to[0],
                travel.from[1] + travel.to[1],
                travel.from[2] + travel.to[2],
            ],
            0.5,
        );
        length(cross(*dir, self.z)) * lever <= IDENTITY_MM
            && (radial(middle, self.origin, self.z).0 - self.radius).abs() <= IDENTITY_MM
    }
}

/// An end arc: a circle on the cylinder, used by one face of the group.
struct Arc {
    edge: u64,
    station: f64,
    /// The vertices it runs from and to, in its loop's direction.
    first: usize,
    last: usize,
    /// The signed angle it sweeps about the group's axis.
    sweep: f64,
    closed: bool,
    /// The counterclockwise start of the angle it covers, and its width.
    start: f64,
}

/// A boundary edge use of the group, as section 8.3 sorts it.
enum Bounding {
    Arc(Arc),
    Axial,
    /// Anything else, with its two vertices.
    Other(usize, usize),
}

fn bounding(topo: &Topo, travel: &Travel<'_>, frame: &Frame) -> Bounding {
    let instance = topo.edges.get(travel.edge).map_or(0, |e| e.instance);
    match travel.curve {
        Curve::Circle {
            centre,
            z: n,
            radius,
            ..
        } if length(cross(*n, frame.z)) * frame.radius <= IDENTITY_MM
            && radial(*centre, frame.origin, frame.z).0 <= IDENTITY_MM
            && (radius - frame.radius).abs() <= IDENTITY_MM =>
        {
            // Counterclockwise about z iff along a circle whose axis agrees.
            let about_n = travel.sweep(*centre, *n);
            let sweep = if dot(*n, frame.z) > 0.0 {
                about_n
            } else {
                -about_n
            };
            let from = if sweep >= 0.0 { travel.from } else { travel.to };
            Bounding::Arc(Arc {
                edge: instance,
                station: frame.station(*centre),
                first: travel.first,
                last: travel.last,
                sweep,
                closed: travel.closed,
                start: frame.angle(from, *centre),
            })
        }
        _ if frame.axial(travel) => Bounding::Axial,
        _ => Bounding::Other(travel.first, travel.last),
    }
}

/// F2: whether one station's arcs are a full circle, and the angle they
/// cover. A chain whose sweeps sum to a multiple of 2π other than ±2π is a
/// contradiction (check 29).
fn full_station(arcs: &[&Arc], radius: f64) -> Result<(bool, f64), Contradiction> {
    let coverage = cover(arcs, IDENTITY_MM / radius);
    if let [arc] = arcs {
        if arc.closed {
            return Ok((true, TAU));
        }
    }
    if arcs.iter().any(|a| a.closed) {
        return Ok((false, coverage));
    }
    // Each vertex begins exactly one arc and ends exactly one, and
    // following them from the first visits every arc once.
    let mut firsts: Vec<usize> = arcs.iter().map(|a| a.first).collect();
    let mut lasts: Vec<usize> = arcs.iter().map(|a| a.last).collect();
    firsts.sort_unstable();
    lasts.sort_unstable();
    let distinct = firsts.windows(2).all(|w| w[0] != w[1]);
    if firsts != lasts || !distinct {
        return Ok((false, coverage));
    }
    let Some(first) = arcs.first() else {
        return Ok((false, coverage));
    };
    let mut at = first.last;
    let mut steps = 1;
    while at != first.first && steps <= arcs.len() {
        let Some(next) = arcs.iter().find(|a| a.first == at) else {
            return Ok((false, coverage));
        };
        at = next.last;
        steps += 1;
    }
    if steps != arcs.len() || at != first.first {
        return Ok((false, coverage));
    }
    let sum: f64 = arcs.iter().map(|a| a.sweep).sum();
    let turns = (sum / TAU).round();
    if (sum - turns * TAU).abs() > SWEEP_RAD {
        return Ok((false, coverage));
    }
    if turns.abs() >= 2.0 {
        return Err(Contradiction(first.edge));
    }
    Ok(((turns.abs() - 1.0).abs() < 0.5 && coverage >= TAU, coverage))
}

/// The angle the arcs' intervals cover, counting gaps no wider than `gap`
/// as covered, at most 2π.
fn cover(arcs: &[&Arc], gap: f64) -> f64 {
    let mut intervals: Vec<(f64, f64)> = arcs
        .iter()
        .map(|a| (a.start.rem_euclid(TAU), a.sweep.abs().min(TAU)))
        .collect();
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let Some(&(first, _)) = intervals.first() else {
        return 0.0;
    };
    let mut reach = first;
    let mut missing = 0.0;
    for &(start, width) in &intervals {
        if start - reach > gap {
            missing += start - reach;
        }
        reach = reach.max(start + width);
    }
    let wrap = first + TAU - reach;
    if wrap > gap {
        missing += wrap;
    }
    (TAU - missing).clamp(0.0, TAU)
}

/// Rule F: classifies the group of faces `group`, whose reference surface
/// is the cylinder of `radius` about the line through `origin` along unit
/// `axis`.
pub(crate) fn classify(
    topo: &Topo,
    group: &[usize],
    (origin, axis, radius): (P, P, f64),
) -> Result<Classified, Contradiction> {
    let z = canonical(axis);
    let x = perpendicular(z);
    let frame = Frame {
        origin,
        z,
        x,
        y: cross(z, x),
        radius,
    };
    let mut uses = Vec::new();
    let mut seams = Vec::new();
    for &index in group {
        let Some(face) = topo.faces.get(index) else {
            continue;
        };
        for travel in topo.travels(face).iter().flatten() {
            let Some(edge) = topo.edges.get(travel.edge) else {
                continue;
            };
            if edge.faces.iter().all(|f| group.contains(f)) {
                // F3: a seam is used twice by one loop of one face, and
                // runs along the axis on the cylinder. A bridge or slit
                // edge, also used twice by one loop, does not run along it.
                let in_one_loop = face.bounds.iter().any(|bound| match &bound.lp {
                    Loop::Edges(uses) => uses.iter().filter(|u| u.edge == travel.edge).count() == 2,
                    Loop::Vertex(_) => false,
                });
                if in_one_loop && frame.axial(travel) {
                    seams.push((travel.edge, edge.start, edge.end));
                }
            } else {
                uses.push(bounding(topo, travel, &frame));
            }
        }
    }
    seams.sort_unstable();
    seams.dedup();
    let point = |v: usize| topo.vertices.get(v).map_or(origin, |v| v.point);

    let (stations, all_full, widest) = stations(&uses, radius)?;
    let others: Vec<usize> = uses
        .iter()
        .filter_map(|b| match b {
            Bounding::Other(a, b) => Some([*a, *b]),
            _ => None,
        })
        .flatten()
        .collect();
    let (class, base) = decide(&uses, &stations, all_full, widest, |past| {
        others.iter().any(|&v| past(frame.station(point(v))))
    })
    .or_else(|| {
        seam_span(&seams, &point, &frame).map(|(lo, hi)| {
            (
                CylinderClass::FullBySeam {
                    proven_extent_mm: [0.0, hi - lo],
                },
                lo,
            )
        })
    })
    .unwrap_or((
        CylinderClass::Undecided {
            reason: UndecidedReason::NoEndCircle,
        },
        0.0,
    ));
    let wall_openings = match (&class, stations.first(), stations.last()) {
        (
            CylinderClass::Full {} | CylinderClass::FullOneEndIrregular { .. },
            Some(&lo),
            Some(&hi),
        ) => openings(&uses, |v| frame.station(point(v)), (lo, hi)),
        _ => 0,
    };
    Ok(Classified {
        class,
        point: [
            origin[0] + base * z[0],
            origin[1] + base * z[1],
            origin[2] + base * z[2],
        ],
        z,
        stations: stations.iter().map(|s| s - base).collect(),
        wall_openings,
    })
}

/// F1 and F2: the stations, where sorted end arcs leave a gap wider than
/// ε; whether every one is full; and the widest angle one covers.
fn stations(
    uses: &[Bounding],
    radius: f64,
) -> Result<(Vec<f64>, bool, Option<f64>), Contradiction> {
    let mut arcs: Vec<&Arc> = uses
        .iter()
        .filter_map(|b| match b {
            Bounding::Arc(arc) => Some(arc),
            _ => None,
        })
        .collect();
    arcs.sort_by(|a, b| a.station.total_cmp(&b.station));
    let mut clusters: Vec<Vec<&Arc>> = Vec::new();
    for arc in arcs {
        match clusters.last_mut() {
            Some(cluster)
                if cluster
                    .last()
                    .is_some_and(|last| arc.station - last.station <= IDENTITY_MM) =>
            {
                cluster.push(arc);
            }
            _ => clusters.push(vec![arc]),
        }
    }
    let mut stations = Vec::new();
    let mut all_full = true;
    let mut widest: Option<f64> = None;
    for cluster in &clusters {
        let (full, coverage) = full_station(cluster, radius)?;
        all_full &= full;
        widest = Some(widest.map_or(coverage, |w: f64| w.max(coverage)));
        stations.push(cluster.first().map_or(0.0, |a| a.station));
    }
    Ok((stations, all_full, widest))
}

/// F4: the class of a group with the given stations, or `None` if it has
/// none and is not partial, for F3 to decide. `past` tells whether a
/// vertex of an "other" edge lies at a station that satisfies its test.
fn decide(
    uses: &[Bounding],
    stations: &[f64],
    all_full: bool,
    widest: Option<f64>,
    past: impl Fn(&dyn Fn(f64) -> bool) -> bool,
) -> Option<(CylinderClass, f64)> {
    if uses.iter().any(|b| matches!(b, Bounding::Axial)) || !all_full {
        return Some((
            CylinderClass::Partial { sweep_rad: widest },
            stations.first().copied().unwrap_or(0.0),
        ));
    }
    let (&lo, &hi) = (stations.first()?, stations.last()?);
    let below = past(&|t| t < lo - VERTEX_SIDE_MM);
    let above = past(&|t| t > hi + VERTEX_SIDE_MM);
    let class = match (below, above) {
        (false, false) => CylinderClass::Full {},
        (true, false) => CylinderClass::FullOneEndIrregular {
            irregular: End::Start,
        },
        (false, true) => CylinderClass::FullOneEndIrregular {
            irregular: End::Finish,
        },
        (true, true) => CylinderClass::Undecided {
            reason: UndecidedReason::IrregularAtBothEnds,
        },
    };
    Some((class, lo))
}

/// F3: the span of the group's seams along the axis, if it has one.
fn seam_span(
    seams: &[(usize, usize, usize)],
    point: &dyn Fn(usize) -> P,
    frame: &Frame,
) -> Option<(f64, f64)> {
    let stations: Vec<f64> = seams
        .iter()
        .flat_map(|&(_, a, b)| [a, b])
        .map(|v| frame.station(point(v)))
        .collect();
    let lo = stations.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = stations.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (hi > lo).then_some((lo, hi))
}

/// F4: the wall openings, as connected chains of "other" edges with a
/// vertex strictly between the stations.
fn openings(uses: &[Bounding], station: impl Fn(usize) -> f64, (lo, hi): (f64, f64)) -> u32 {
    let edges: Vec<(usize, usize)> = uses
        .iter()
        .filter_map(|b| match b {
            Bounding::Other(a, b) => Some((*a, *b)),
            _ => None,
        })
        .collect();
    let mut vertices: Vec<usize> = edges.iter().flat_map(|&(a, b)| [a, b]).collect();
    vertices.sort_unstable();
    vertices.dedup();
    // Union-find over the chains' vertices.
    let index = |v: usize| vertices.binary_search(&v).unwrap_or(0);
    let mut root: Vec<usize> = (0..vertices.len()).collect();
    for &(a, b) in &edges {
        let (ra, rb) = (
            group::find(&mut root, index(a)),
            group::find(&mut root, index(b)),
        );
        if let Some(slot) = root.get_mut(ra.max(rb)) {
            *slot = ra.min(rb);
        }
    }
    let mut inside: Vec<usize> = Vec::new();
    for (i, &v) in vertices.iter().enumerate() {
        let t = station(v);
        if t > lo + VERTEX_SIDE_MM && t < hi - VERTEX_SIDE_MM {
            inside.push(group::find(&mut root, i));
        }
    }
    inside.sort_unstable();
    inside.dedup();
    u32::try_from(inside.len()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::*;

    #[test]
    fn the_canonical_sign_makes_the_largest_component_positive() {
        assert_eq!(canonical([0.0, 0.0, -1.0]), [0.0, 0.0, 1.0]);
        assert_eq!(canonical([-0.6, 0.8, 0.0]), [-0.6, 0.8, 0.0]);
        assert_eq!(canonical([-0.6, -0.6, 0.5]), [0.6, 0.6, -0.5]);
    }

    #[test]
    fn coverage_counts_gaps_and_wraps() {
        let arc = |start: f64, sweep: f64| Arc {
            edge: 0,
            station: 0.0,
            first: 0,
            last: 0,
            sweep,
            closed: false,
            start,
        };
        let (a, b) = (arc(0.0, PI), arc(PI, PI));
        assert_eq!(cover(&[&a, &b], 1e-9), TAU);
        let half = arc(-PI / 2.0, PI);
        assert!((cover(&[&half], 1e-9) - PI).abs() < 1e-12);
        // The same half twice covers a half, whatever the sum says.
        assert!((cover(&[&half, &half], 1e-9) - PI).abs() < 1e-12);
    }

    fn arc(first: usize, last: usize, sweep: f64) -> Arc {
        Arc {
            edge: 7,
            station: 0.0,
            first,
            last,
            sweep,
            closed: false,
            start: 0.0,
        }
    }

    #[test]
    fn two_arcs_around_the_circle_once_are_a_full_station() {
        let (a, b) = (arc(0, 1, PI), arc(1, 0, PI));
        let mut b = b;
        b.start = PI;
        assert!(matches!(full_station(&[&a, &b], 3.0), Ok((true, _))));
    }

    #[test]
    fn a_chain_of_arcs_around_the_circle_twice_is_a_contradiction() {
        // Check 29: the arcs chain and close, but sweep 4π.
        let (a, b) = (arc(0, 1, 3.0 * PI), arc(1, 0, PI));
        assert!(matches!(
            full_station(&[&a, &b], 3.0),
            Err(Contradiction(7))
        ));
    }

    #[test]
    fn a_chain_that_sweeps_nothing_is_not_full() {
        // Out over a half circle and back over the same half.
        let (a, b) = (arc(0, 1, PI), arc(1, 0, -PI));
        assert!(matches!(full_station(&[&a, &b], 3.0), Ok((false, _))));
    }
}
