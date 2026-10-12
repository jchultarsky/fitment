//! Rule F: whether a cylinder group wraps all the way around its axis
//! (`docs/research/m2-features.md`, section 8.3).
//!
//! Only end circles prove a wall wraps around: a station is full when its
//! arcs close one chain about the axis that sums to 2π and covers it. A
//! seam proves nothing without the curves on the surface, which stepq does
//! not read: a slit along the axis is used twice by one loop too, so rule
//! F3 was withdrawn in review of PR 5.

use std::f64::consts::TAU;

use libm::atan2;

use super::FeatureId;
use super::cylinder::{CylinderClass, End, UndecidedReason};
use super::group::{self, IDENTITY_MM, LEVER_MIN_MM};
use super::topo::{Curve, P, Topo, Travel, cross, dot, length, neg, radial, scale, sub};

/// Which side of the stations a vertex lies on, in mm (F4).
pub(crate) const VERTEX_SIDE_MM: f64 = 1e-3;

/// How close a station's summed sweep must be to 2π, in rad (F2).
pub(crate) const SWEEP_RAD: f64 = 1e-9;

/// What rule F finds of one group.
pub(crate) struct Classified {
    pub(crate) class: CylinderClass,
    /// The axis point: at the lowest station, else the reference surface's
    /// location.
    pub(crate) point: P,
    /// The axis direction, with the canonical sign.
    pub(crate) z: P,
    /// The stations, as distances along `z` from `point`.
    pub(crate) stations: Vec<f64>,
    /// The end arcs of each station, as indices into [`Topo::edges`]: where
    /// the end walk of section 9.1 starts.
    pub(crate) arcs: Vec<Vec<usize>>,
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
    /// Its index into [`Topo::edges`].
    index: usize,
    station: f64,
    /// The vertices it runs from and to, in its loop's direction.
    first: usize,
    last: usize,
    /// The signed angle it sweeps about the group's axis.
    sweep: f64,
    closed: bool,
    /// An open arc whose two vertices lie within ε of each other: whether
    /// it sweeps nothing or a whole turn is not stated.
    ambiguous: bool,
    /// The counterclockwise start of the angle it covers.
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
                index: travel.edge,
                station: frame.station(*centre),
                first: travel.first,
                last: travel.last,
                sweep,
                closed: travel.closed,
                ambiguous: !travel.closed && length(sub(travel.from, travel.to)) <= IDENTITY_MM,
                start: frame.angle(from, *centre),
            })
        }
        _ if frame.axial(travel) => Bounding::Axial,
        _ => Bounding::Other(travel.first, travel.last),
    }
}

/// What F2 finds of one station.
struct Station {
    at: f64,
    /// Its arcs' edges.
    arcs: Vec<usize>,
    full: bool,
    /// The angle its arcs cover.
    cover: f64,
    /// True if its arcs run counterclockwise about the axis in all.
    ccw: bool,
}

/// F2: whether one station's arcs are a full circle, the angle they
/// cover, and which way they run. A chain whose sweeps sum to a multiple
/// of 2π other than ±2π is a contradiction (check 29).
fn full_station(arcs: &[&Arc], radius: f64) -> Result<Station, Contradiction> {
    let at = arcs.first().map_or(0.0, |a| a.station);
    let sum: f64 = arcs.iter().map(|a| a.sweep).sum();
    let mut station = Station {
        at,
        arcs: arcs.iter().map(|a| a.index).collect(),
        full: false,
        cover: cover(arcs, IDENTITY_MM / radius),
        ccw: sum > 0.0,
    };
    if let [arc] = arcs {
        if arc.closed {
            station.full = true;
            station.cover = TAU;
            return Ok(station);
        }
    }
    if arcs.iter().any(|a| a.closed || a.ambiguous) {
        return Ok(station);
    }
    // Each vertex begins exactly one arc and ends exactly one, and
    // following them from the first visits every arc once.
    let mut firsts: Vec<(usize, usize)> =
        arcs.iter().enumerate().map(|(i, a)| (a.first, i)).collect();
    firsts.sort_unstable();
    let mut lasts: Vec<usize> = arcs.iter().map(|a| a.last).collect();
    lasts.sort_unstable();
    let distinct = firsts.windows(2).all(|w| w[0].0 != w[1].0);
    if !distinct || !firsts.iter().map(|f| f.0).eq(lasts.iter().copied()) {
        return Ok(station);
    }
    let Some(first) = arcs.first() else {
        return Ok(station);
    };
    let mut at = first.last;
    let mut steps = 1;
    while at != first.first && steps <= arcs.len() {
        let next = firsts
            .binary_search_by_key(&at, |f| f.0)
            .ok()
            .and_then(|i| firsts.get(i))
            .and_then(|&(_, i)| arcs.get(i));
        let Some(next) = next else {
            return Ok(station);
        };
        at = next.last;
        steps += 1;
    }
    if steps != arcs.len() || at != first.first {
        return Ok(station);
    }
    let turns = (sum / TAU).round();
    if (sum - turns * TAU).abs() > SWEEP_RAD {
        return Ok(station);
    }
    if turns.abs() >= 2.0 {
        return Err(Contradiction(first.edge));
    }
    station.full = (turns.abs() - 1.0).abs() < 0.5 && station.cover >= TAU;
    Ok(station)
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

/// The cylinder group `id`, whose faces are those `owner` gives `id`, as
/// rule F sees it: its reference surface is the cylinder of `radius` about
/// the line through `origin` along unit `axis`, and `shaft` is its sense.
pub(crate) struct Group<'a> {
    pub(crate) id: FeatureId,
    pub(crate) faces: &'a [usize],
    pub(crate) owner: &'a [FeatureId],
    pub(crate) origin: P,
    pub(crate) axis: P,
    pub(crate) radius: f64,
    pub(crate) shaft: bool,
}

/// Rule F: classifies one cylinder group.
pub(crate) fn classify(topo: &Topo, group: &Group<'_>) -> Result<Classified, Contradiction> {
    let z = canonical(group.axis);
    let x = perpendicular(z);
    let frame = Frame {
        origin: group.origin,
        z,
        x,
        y: cross(z, x),
        radius: group.radius,
    };
    let mut uses = Vec::new();
    for &index in group.faces {
        let Some(face) = topo.faces.get(index) else {
            continue;
        };
        for travel in topo.travels(face).iter().flatten() {
            let Some(edge) = topo.edges.get(travel.edge) else {
                continue;
            };
            let inside = edge
                .faces
                .iter()
                .all(|&f| group.owner.get(f) == Some(&group.id));
            if !inside {
                uses.push(bounding(topo, travel, &frame));
            }
        }
    }
    let point = |v: usize| topo.vertices.get(v).map_or(group.origin, |v| v.point);
    let stations = stations(&uses, group.radius)?;
    let others: Vec<f64> = uses
        .iter()
        .filter_map(|b| match b {
            Bounding::Other(a, b) => Some([*a, *b]),
            _ => None,
        })
        .flatten()
        .map(|v| frame.station(point(v)))
        .collect();
    let axial = uses.iter().any(|b| matches!(b, Bounding::Axial));
    let class = decide(&stations, axial, &others, group.shaft);
    let at: Vec<f64> = stations.iter().map(|s| s.at).collect();
    let base = at.first().copied().unwrap_or(0.0);
    let wall_openings = match (&class, at.first(), at.last()) {
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
            group.origin[0] + base * z[0],
            group.origin[1] + base * z[1],
            group.origin[2] + base * z[2],
        ],
        z,
        stations: at.iter().map(|s| s - base).collect(),
        arcs: stations.into_iter().map(|s| s.arcs).collect(),
        wall_openings,
    })
}

/// F1 and F2: the stations, where sorted end arcs leave a gap wider than
/// ε, each with what F2 finds of it.
fn stations(uses: &[Bounding], radius: f64) -> Result<Vec<Station>, Contradiction> {
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
    clusters
        .iter()
        .map(|cluster| full_station(cluster, radius))
        .collect()
}

/// F4: the class of a group with these stations, axial lines or not, and
/// these stations of its "other" edges' vertices.
///
/// The wall lies above a full station (along z) exactly when its arcs run
/// counterclockwise for a shaft, or clockwise for a hole, as check 25 has
/// it. The lowest station must face up and the highest down; a single
/// station's wall runs past it on the side it faces, which a closed curve
/// with one vertex on the station cannot show (review of PR 5).
fn decide(stations: &[Station], axial: bool, others: &[f64], shaft: bool) -> CylinderClass {
    let widest = stations.iter().map(|s| s.cover).reduce(f64::max);
    if axial || !stations.iter().all(|s| s.full) {
        return CylinderClass::Partial { sweep_rad: widest };
    }
    let (Some(lo), Some(hi)) = (stations.first(), stations.last()) else {
        return CylinderClass::Undecided {
            reason: UndecidedReason::NoEndCircle,
        };
    };
    let up = |s: &Station| s.ccw == shaft;
    let below = others.iter().any(|&t| t < lo.at - VERTEX_SIDE_MM);
    let above = others.iter().any(|&t| t > hi.at + VERTEX_SIDE_MM);
    let (below, above) = if stations.len() == 1 {
        if up(lo) { (below, true) } else { (true, above) }
    } else if up(lo) && !up(hi) {
        (below, above)
    } else {
        return CylinderClass::Undecided {
            reason: UndecidedReason::EndsDisagree,
        };
    };
    match (below, above) {
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
    }
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

    fn arc(first: usize, last: usize, start: f64, sweep: f64) -> Arc {
        Arc {
            edge: 7,
            index: 0,
            station: 0.0,
            first,
            last,
            sweep,
            closed: false,
            ambiguous: false,
            start,
        }
    }

    #[test]
    fn coverage_counts_gaps_and_wraps() {
        let (a, b) = (arc(0, 0, 0.0, PI), arc(0, 0, PI, PI));
        assert_eq!(cover(&[&a, &b], 1e-9), TAU);
        let half = arc(0, 0, -PI / 2.0, PI);
        assert!((cover(&[&half], 1e-9) - PI).abs() < 1e-12);
        // The same half twice covers a half, whatever the sum says.
        assert!((cover(&[&half, &half], 1e-9) - PI).abs() < 1e-12);
    }

    #[test]
    fn two_arcs_around_the_circle_once_are_a_full_station() {
        let (a, b) = (arc(0, 1, 0.0, PI), arc(1, 0, PI, PI));
        let station = full_station(&[&a, &b], 3.0).ok().unwrap();
        assert!(station.full && station.ccw);
    }

    #[test]
    fn a_chain_of_arcs_around_the_circle_twice_is_a_contradiction() {
        // Check 29: the arcs chain and close, but sweep 4π.
        let (a, b) = (arc(0, 1, 0.0, 3.0 * PI), arc(1, 0, PI, PI));
        assert!(matches!(
            full_station(&[&a, &b], 3.0),
            Err(Contradiction(7))
        ));
    }

    #[test]
    fn a_chain_that_sweeps_nothing_is_not_full() {
        // Out over a half circle and back over the same half.
        let (a, b) = (arc(0, 1, 0.0, PI), arc(1, 0, 0.0, -PI));
        assert!(!full_station(&[&a, &b], 3.0).ok().unwrap().full);
    }

    #[test]
    fn a_zero_length_arc_leaves_the_station_not_full_rather_than_a_contradiction() {
        // A whole turn and an arc between two vertices at one point, which
        // the angle maps to a whole turn too (review of PR 5).
        let whole = arc(0, 1, 0.0, TAU);
        let mut zero = arc(1, 0, 0.0, TAU);
        zero.ambiguous = true;
        assert!(!full_station(&[&whole, &zero], 3.0).ok().unwrap().full);
    }

    fn full(at: f64, ccw: bool) -> Station {
        Station {
            at,
            arcs: Vec::new(),
            full: true,
            cover: TAU,
            ccw,
        }
    }

    #[test]
    fn one_station_faces_the_way_its_arcs_run() {
        // A hole whose wall rises from a full circle at 0 to an ellipse
        // whose one vertex touches the circle: the end is irregular above.
        let class = decide(&[full(0.0, false)], false, &[0.0], false);
        assert_eq!(
            class,
            CylinderClass::FullOneEndIrregular {
                irregular: End::Finish
            }
        );
        let class = decide(&[full(0.0, true)], false, &[0.0], false);
        assert_eq!(
            class,
            CylinderClass::FullOneEndIrregular {
                irregular: End::Start
            }
        );
    }

    #[test]
    fn end_stations_facing_away_from_the_wall_are_undecided() {
        let class = decide(&[full(0.0, true), full(5.0, false)], false, &[], false);
        assert!(matches!(class, CylinderClass::Undecided { .. }));
        let class = decide(&[full(0.0, false), full(5.0, true)], false, &[], false);
        assert_eq!(class, CylinderClass::Full {});
    }
}
